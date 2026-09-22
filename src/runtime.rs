//! Local Starlark configuration evaluation with confined loads and best-effort resource limits.
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value as Json};
use starlark::environment::LibraryExtension;
use starlark::environment::{FrozenModule, Globals, Module};
use starlark::eval::{Evaluator, ReturnFileLoader};
use starlark::syntax::{AstModule, Dialect};
use starlark::values::{Value, dict::DictRef, list::ListRef, structs::StructRef, tuple::TupleRef};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Component, Path, PathBuf},
};

pub const SCL_RUNTIME_LABEL: &str = "//protolark:runtime.scl";
pub const BZL_RUNTIME_LABEL: &str = "@protolark//protolark:runtime.bzl";
pub(crate) const RUNTIME_SOURCE: &str = include_str!("../protolark/runtime.scl");

/// Evaluate `symbol` with loads confined to `load_root` (default: config directory).
/// Supports relative loads and workspace-root `//package:file.scl` labels.
/// Applies best-effort per-module limits of 64 MiB heap, one million evaluator ticks, 128 stack frames;
/// at most 128 modules and 64 levels of loading/JSON nesting are accepted.
pub fn evaluate(config: &Path, symbol: &str, load_root: Option<&Path>) -> Result<Json> {
    let config = config
        .canonicalize()
        .with_context(|| format!("read config {}", config.display()))?;
    let root = load_root
        .unwrap_or_else(|| config.parent().unwrap())
        .canonicalize()
        .context("resolve load root")?;
    let mut loader = Loader {
        root,
        cache: HashMap::new(),
        active: HashSet::new(),
        runtime: None,
        globals: Globals::extended_by(&[LibraryExtension::StructType]),
    };
    let module = loader.evaluate(&config)?;
    let value = module
        .get(symbol)
        .with_context(|| format!("configuration has no exported symbol {symbol:?}"))?;
    to_json(value.value(), 0, &mut 1_000_000).context("convert Starlark configuration to JSON")
}

/// Evaluate explicitly declared files in an isolated temporary load tree.
/// Each pair is `(logical repository-relative path, physical input path)`.
/// Physical inputs may be symlinks (as in a Bazel sandbox); their contents are
/// copied, so evaluation cannot follow them outside the temporary load root.
/// `config` is a logical path and must appear in `files`. No shell is invoked.
pub fn evaluate_files(config: &Path, symbol: &str, files: &[(PathBuf, PathBuf)]) -> Result<Json> {
    fn validate(path: &Path) -> Result<()> {
        // Comparing the reconstructed path also rejects interior `.` and empty
        // components that Path::components() would otherwise silently normalize.
        let normalized: PathBuf = path.components().collect();
        if path.as_os_str().is_empty()
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || normalized.as_os_str() != path.as_os_str()
        {
            bail!(
                "logical load path {} must be a nonempty normalized relative path without . or .. components",
                path.display()
            );
        }
        Ok(())
    }
    validate(config)?;
    let mut declared = HashSet::new();
    for (logical, _) in files {
        validate(logical)?;
        if !declared.insert(logical) {
            bail!("duplicate logical load path {}", logical.display());
        }
    }
    if !declared.contains(&config.to_path_buf()) {
        bail!(
            "configuration {} is not declared in the load files",
            config.display()
        );
    }
    let root = tempfile::tempdir().context("create isolated Starlark load root")?;
    for (logical, physical) in files {
        let destination = root.path().join(logical);
        std::fs::create_dir_all(destination.parent().unwrap())
            .with_context(|| format!("create load directory for {}", logical.display()))?;
        std::fs::copy(physical, &destination).with_context(|| {
            format!(
                "copy declared load file {} from {}",
                logical.display(),
                physical.display()
            )
        })?;
    }
    // TempDir's destructor removes the staged files on success and on error.
    evaluate(&root.path().join(config), symbol, Some(root.path()))
}

struct Loader {
    root: PathBuf,
    cache: HashMap<PathBuf, FrozenModule>,
    active: HashSet<PathBuf>,
    globals: Globals,
    runtime: Option<FrozenModule>,
}
impl Loader {
    fn runtime_module(&mut self) -> Result<FrozenModule> {
        if let Some(module) = &self.runtime {
            return Ok(module.clone());
        }
        let ast = AstModule::parse(
            SCL_RUNTIME_LABEL,
            RUNTIME_SOURCE.to_owned(),
            &Dialect::Standard,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        let module = self.evaluate_ast(ast, HashMap::new())?;
        self.runtime = Some(module.clone());
        Ok(module)
    }
    fn resolve(&self, parent: &Path, label: &str) -> Result<PathBuf> {
        let path = if let Some(label) = label.strip_prefix("//") {
            self.root
                .join(label.strip_prefix(':').unwrap_or(label).replace(':', "/"))
        } else {
            if label.starts_with('@') || Path::new(label).is_absolute() {
                bail!("unsupported load {label:?}: use a relative path or //package:file label");
            }
            parent.join(label.strip_prefix(':').unwrap_or(label))
        };
        let path = path
            .canonicalize()
            .with_context(|| format!("resolve load {label:?}"))?;
        if !path.starts_with(&self.root) {
            bail!("load {label:?} escapes load root {}", self.root.display());
        }
        Ok(path)
    }
    fn evaluate(&mut self, path: &Path) -> Result<FrozenModule> {
        if !path.starts_with(&self.root) {
            bail!(
                "config {} is outside load root {}",
                path.display(),
                self.root.display()
            );
        }
        if let Some(module) = self.cache.get(path) {
            return Ok(module.clone());
        }
        if self.active.contains(path) {
            bail!("Starlark load cycle at {}", path.display());
        }
        if self.active.len() >= 64 || self.active.len() + self.cache.len() >= 128 {
            bail!("Starlark module loading limit exceeded");
        }
        self.active.insert(path.to_owned());
        let mut source = String::new();
        std::fs::File::open(path)
            .with_context(|| format!("read {}", path.display()))?
            .take(4 * 1024 * 1024 + 1)
            .read_to_string(&mut source)
            .with_context(|| format!("read UTF-8 Starlark {}", path.display()))?;
        if source.len() > 4 * 1024 * 1024 {
            bail!("Starlark module exceeds 4 MiB source limit");
        }
        let ast = AstModule::parse(&path.to_string_lossy(), source, &Dialect::Standard)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut dependencies = HashMap::new();
        for load in ast.loads() {
            let module = if matches!(load.module_id, SCL_RUNTIME_LABEL | BZL_RUNTIME_LABEL) {
                self.runtime_module()?
            } else {
                let resolved = self.resolve(path.parent().unwrap(), load.module_id)?;
                self.evaluate(&resolved)?
            };
            dependencies.insert(load.module_id.to_owned(), module);
        }
        let frozen = self.evaluate_ast(ast, dependencies)?;
        self.active.remove(path);
        self.cache.insert(path.to_owned(), frozen.clone());
        Ok(frozen)
    }

    fn evaluate_ast(
        &self,
        ast: AstModule,
        dependencies: HashMap<String, FrozenModule>,
    ) -> Result<FrozenModule> {
        let modules = dependencies.iter().map(|(k, v)| (k.as_str(), v)).collect();
        let file_loader = ReturnFileLoader { modules: &modules };
        Module::with_temp_heap(|module| -> Result<FrozenModule> {
            {
                let mut eval = Evaluator::new(&module);
                eval.set_loader(&file_loader);
                eval.set_max_callstack_size(128)?;
                eval.set_max_heap_size(64 * 1024 * 1024)?;
                eval.set_max_tick_count(1_000_000)?;
                eval.eval_module(ast, &self.globals)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            module.freeze().map_err(|e| anyhow::anyhow!("{e:?}"))
        })
    }
}

fn to_json(value: Value<'_>, depth: usize, remaining: &mut usize) -> Result<Json> {
    if *remaining == 0 {
        bail!("configuration exceeds one million JSON values");
    }
    *remaining -= 1;
    if depth > 64 {
        bail!("configuration nesting exceeds 64 levels (or contains a cycle)");
    }
    if let Some(s) = StructRef::from_value(value) {
        return s
            .iter()
            .map(|(k, v)| Ok((k.as_str().to_owned(), to_json(v, depth + 1, remaining)?)))
            .collect::<Result<Map<_, _>>>()
            .map(Json::Object);
    }
    if let Some(d) = DictRef::from_value(value) {
        let mut result = Map::new();
        for (key, v) in d.iter() {
            let key = if let Some(s) = key.unpack_str() {
                s.to_owned()
            } else if key.get_type() == "int" {
                key.to_repr()
            } else if let Some(b) = key.unpack_bool() {
                b.to_string()
            } else {
                bail!("protobuf map keys must be strings, integers, or booleans");
            };
            if result
                .insert(key.clone(), to_json(v, depth + 1, remaining)?)
                .is_some()
            {
                bail!("map keys collide after JSON conversion: {key:?}");
            }
        }
        return Ok(Json::Object(result));
    }
    if let Some(list) = ListRef::from_value(value) {
        return list
            .iter()
            .map(|v| to_json(v, depth + 1, remaining))
            .collect::<Result<Vec<_>>>()
            .map(Json::Array);
    }
    if let Some(tuple) = TupleRef::from_value(value) {
        return tuple
            .iter()
            .map(|v| to_json(v, depth + 1, remaining))
            .collect::<Result<Vec<_>>>()
            .map(Json::Array);
    }
    if value.get_type() == "int" {
        let repr = value.to_repr();
        if let Ok(n) = repr.parse::<i64>() {
            return Ok(Json::from(n));
        }
        if let Ok(n) = repr.parse::<u64>() {
            return Ok(Json::from(n));
        }
        bail!("integer {repr} is outside protobuf signed/unsigned 64-bit range");
    }
    if value.get_type() == "float" {
        let number = value
            .to_repr()
            .parse::<f64>()
            .context("parse Starlark float")?;
        if !number.is_finite() {
            return Ok(Json::String(
                if number.is_nan() {
                    "NaN"
                } else if number.is_sign_positive() {
                    "Infinity"
                } else {
                    "-Infinity"
                }
                .to_owned(),
            ));
        }
    }
    if value.is_none()
        || value.unpack_str().is_some()
        || value.unpack_bool().is_some()
        || value.get_type() == "float"
    {
        return value.to_json_value();
    }
    bail!("unsupported configuration value type {}", value.get_type())
}
