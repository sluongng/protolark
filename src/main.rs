use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use prost::Message;
use prost_types::FileDescriptorSet;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    version,
    about = "Protobuf-backed Starlark configuration",
    long_about = "Generate Starlark constructors from protobuf schemas, evaluate local configurations, and encode/decode protobuf messages. All commands run offline without credentials."
)]
struct Cli {
    /// Emit a stable JSON success/error envelope on stdout
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate a .scl or .bzl constructor module using the shared runtime
    Generate(Generate),
    /// Evaluate a local Starlark configuration and export its value as JSON
    Eval(Eval),
    /// Encode a Starlark configuration or protobuf JSON into a protobuf message
    Encode(Encode),
    /// Decode a protobuf message into protobuf JSON
    Decode(Decode),
}

#[derive(Args)]
struct Generate {
    /// Serialized FileDescriptorSet; repeat to combine proto_library descriptors
    #[arg(long, conflicts_with = "sources")]
    descriptor_set: Vec<PathBuf>,
    /// .proto sources (alternative to --descriptor-set)
    #[arg(required_unless_present = "descriptor_set")]
    sources: Vec<PathBuf>,
    /// .proto import search directory; defaults to the current directory
    #[arg(short = 'I', long = "proto-path", default_value = ".")]
    proto_path: Vec<PathBuf>,
    /// Root descriptor filename to export, including its transitive imports
    #[arg(long)]
    file: Vec<String>,
    /// Override the runtime load label (default inferred from --out: .bzl or .scl)
    #[arg(long)]
    runtime_load: Option<String>,
    /// Output file; otherwise write generated source to stdout
    #[arg(long)]
    out: Option<PathBuf>,
    /// Check that --out already matches generated output without changing it
    #[arg(long, requires = "out")]
    check: bool,
}

#[derive(Args)]
struct Config {
    /// Configuration file to evaluate
    #[arg(long)]
    config: PathBuf,
    /// Global variable to export
    #[arg(long, default_value = "project")]
    symbol: String,
    #[command(flatten)]
    loads: LoadOptions,
}

#[derive(Args)]
struct LoadOptions {
    /// Root allowed for local loads; defaults to the configuration's directory
    #[arg(long, requires = "config", conflicts_with = "load_file")]
    load_root: Option<PathBuf>,
    /// Stage a declared file at a logical load path; repeat for all config/load files
    #[arg(long, requires = "config", num_args = 2, value_names = ["LOGICAL", "SOURCE"])]
    load_file: Vec<PathBuf>,
}

#[derive(Args)]
struct Eval {
    #[command(flatten)]
    config: Config,
    /// Output JSON file; otherwise stdout
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Args)]
struct Schema {
    /// Serialized FileDescriptorSet, including imports; repeat to combine sets
    #[arg(long, required = true)]
    descriptor_set: Vec<PathBuf>,
    /// Fully qualified protobuf message name, e.g. example.Project
    #[arg(long)]
    message: String,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Binary,
    Text,
}

#[derive(Args)]
struct Encode {
    #[command(flatten)]
    schema: Schema,
    /// Evaluate this Starlark file instead of reading protobuf JSON
    #[arg(long, conflicts_with = "input")]
    config: Option<PathBuf>,
    /// Configuration global to export
    #[arg(long, default_value = "project")]
    symbol: String,
    #[command(flatten)]
    loads: LoadOptions,
    /// Protobuf JSON input file, or - for stdin (default without --config)
    #[arg(long)]
    input: Option<PathBuf>,
    /// Output protobuf representation
    #[arg(long, value_enum, default_value = "binary")]
    format: Format,
    /// Output file; required with --json; otherwise raw protobuf to stdout
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Args)]
struct Decode {
    #[command(flatten)]
    schema: Schema,
    /// Protobuf input file, or - for stdin
    #[arg(long, default_value = "-")]
    input: PathBuf,
    /// Input protobuf representation
    #[arg(long, value_enum, default_value = "binary")]
    format: Format,
    /// Output JSON file; otherwise stdout
    #[arg(long)]
    out: Option<PathBuf>,
}

fn read_input(path: &Path) -> Result<Vec<u8>> {
    if path == Path::new("-") {
        let mut bytes = Vec::new();
        std::io::stdin()
            .read_to_end(&mut bytes)
            .context("reading stdin")?;
        Ok(bytes)
    } else {
        std::fs::read(path).with_context(|| format!("reading {}", path.display()))
    }
}

fn write_output(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating output in {}", parent.display()))?;
    temp.write_all(bytes)?;
    temp.persist(path)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn emit(
    command: &str,
    bytes: &[u8],
    out: Option<&Path>,
    payload: Value,
    json_mode: bool,
) -> Result<()> {
    if let Some(path) = out {
        write_output(path, bytes)?;
    }
    if json_mode {
        let data = match out {
            Some(path) => json!({"output": path, "bytes": bytes.len()}),
            None => payload,
        };
        println!("{}", json!({"ok": true, "command": command, "data": data}));
    } else if out.is_none() {
        std::io::stdout().write_all(bytes)?;
    }
    Ok(())
}

fn load_descriptors(paths: &[PathBuf]) -> Result<FileDescriptorSet> {
    let mut files = BTreeMap::new();
    for path in paths {
        let set = FileDescriptorSet::decode(read_input(path)?.as_slice())
            .with_context(|| format!("decoding descriptor set {}", path.display()))?;
        for mut file in set.file {
            // Source locations are not part of the schema and may vary between sets.
            file.source_code_info = None;
            let name = file
                .name
                .clone()
                .context("descriptor is missing its filename")?;
            if let Some(old) = files.insert(name.clone(), file.clone())
                && old != file
            {
                bail!("conflicting descriptors for {name}");
            }
        }
    }
    Ok(FileDescriptorSet {
        file: files.into_values().collect(),
    })
}

fn evaluate_config(config: &Path, symbol: &str, loads: &LoadOptions) -> Result<Value> {
    if loads.load_file.is_empty() {
        protolark::runtime::evaluate(config, symbol, loads.load_root.as_deref())
    } else {
        let files = loads
            .load_file
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| (pair[0].clone(), pair[1].clone()))
            .collect::<Vec<_>>();
        protolark::runtime::evaluate_files(config, symbol, &files)
    }
}

fn run(cli: Cli) -> Result<()> {
    let json_mode = cli.json;
    match cli.command {
        Command::Generate(args) => {
            let descriptors = if args.descriptor_set.is_empty() {
                protox::compile(&args.sources, &args.proto_path)
                    .context("compiling .proto sources")?
            } else {
                load_descriptors(&args.descriptor_set)?
            };
            let runtime_load = args.runtime_load.as_deref().unwrap_or_else(|| {
                if args
                    .out
                    .as_ref()
                    .and_then(|path| path.extension())
                    .is_some_and(|ext| ext == "bzl")
                {
                    protolark::runtime::BZL_RUNTIME_LABEL
                } else {
                    protolark::runtime::SCL_RUNTIME_LABEL
                }
            });
            let source = protolark::generate_with_runtime(&descriptors, &args.file, runtime_load)?;
            if args.check {
                let out = args.out.as_deref().context("--check requires --out")?;
                if read_input(out)? != source.as_bytes() {
                    bail!(
                        "{} is not up to date; regenerate without --check",
                        out.display()
                    );
                }
                if json_mode {
                    println!(
                        "{}",
                        json!({"ok":true,"command":"generate","data":{"output":out,"matches":true}})
                    );
                }
                return Ok(());
            }
            emit(
                "generate",
                source.as_bytes(),
                args.out.as_deref(),
                json!({"source":source}),
                json_mode,
            )
        }
        Command::Eval(args) => {
            let value =
                evaluate_config(&args.config.config, &args.config.symbol, &args.config.loads)?;
            let bytes = format!("{}\n", serde_json::to_string_pretty(&value)?);
            emit(
                "eval",
                bytes.as_bytes(),
                args.out.as_deref(),
                json!({"value":value}),
                json_mode,
            )
        }
        Command::Encode(args) => {
            if json_mode && args.out.is_none() {
                bail!(
                    "encode --json requires --out to keep protobuf bytes separate from JSON status"
                );
            }
            let is_config = args.config.is_some();
            let value = match args.config {
                Some(path) => evaluate_config(&path, &args.symbol, &args.loads)?,
                None => serde_json::from_slice(&read_input(
                    args.input.as_deref().unwrap_or(Path::new("-")),
                )?)
                .context("parsing protobuf JSON input")?,
            };
            let bytes = match (args.format, is_config) {
                (Format::Binary, true) => protolark::codec::encode_config(
                    &args.schema.descriptor_set,
                    &args.schema.message,
                    &value,
                )?,
                (Format::Text, true) => protolark::codec::encode_config_text(
                    &args.schema.descriptor_set,
                    &args.schema.message,
                    &value,
                )?
                .into_bytes(),
                (Format::Binary, false) => protolark::codec::encode(
                    &args.schema.descriptor_set,
                    &args.schema.message,
                    &value,
                )?,
                (Format::Text, false) => protolark::codec::encode_text(
                    &args.schema.descriptor_set,
                    &args.schema.message,
                    &value,
                )?
                .into_bytes(),
            };
            emit(
                "encode",
                &bytes,
                args.out.as_deref(),
                Value::Null,
                json_mode,
            )
        }
        Command::Decode(args) => {
            let input = read_input(&args.input)?;
            let value = match args.format {
                Format::Binary => protolark::codec::decode(
                    &args.schema.descriptor_set,
                    &args.schema.message,
                    &input,
                )?,
                Format::Text => protolark::codec::decode_text(
                    &args.schema.descriptor_set,
                    &args.schema.message,
                    std::str::from_utf8(&input).context("text protobuf must be UTF-8")?,
                )?,
            };
            let bytes = format!("{}\n", serde_json::to_string_pretty(&value)?);
            emit(
                "decode",
                bytes.as_bytes(),
                args.out.as_deref(),
                json!({"value":value}),
                json_mode,
            )
        }
    }
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let json_mode = args.iter().any(|arg| arg == "--json");
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                let _ = error.print();
                return;
            }
            if json_mode {
                println!(
                    "{}",
                    json!({"ok":false,"error":{"code":"usage","message":error.to_string()}})
                );
            } else {
                let _ = error.print();
            }
            std::process::exit(2);
        }
    };
    if let Err(error) = run(cli) {
        if json_mode {
            println!(
                "{}",
                json!({"ok":false,"error":{"code":"operation_failed","message":format!("{error:#}")}})
            );
        } else {
            eprintln!("protolark: {error:#}");
        }
        std::process::exit(1);
    }
}
