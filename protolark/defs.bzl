"""Generate Starlark constructors backed by Protolark's shared runtime."""

load("@protobuf//bazel/common:proto_info.bzl", "ProtoInfo")

def _protolark_library_impl(ctx):
    descriptors = depset(transitive = [dep[ProtoInfo].transitive_descriptor_sets for dep in ctx.attr.deps])
    out = ctx.outputs.out
    if not out:
        out = ctx.actions.declare_file(ctx.label.name + ".scl")
    args = ctx.actions.args()
    args.add("generate")
    if ctx.attr.runtime_load:
        args.add("--runtime-load", ctx.attr.runtime_load)
    args.add_all(descriptors, before_each = "--descriptor-set")
    args.add("--out", out)
    ctx.actions.run(
        executable = ctx.executable.generator,
        arguments = [args],
        inputs = descriptors,
        outputs = [out],
        mnemonic = "ProtolarkGenerate",
        progress_message = "Generating Starlark constructors %{label}",
    )
    return [DefaultInfo(files = depset([out]))]

protolark_library = rule(
    implementation = _protolark_library_impl,
    doc = """Generates one .scl or .bzl file from proto_library dependencies.

Exports the schemas supplied by `deps`, including their transitive imports.
Generation uses the exec-configured CLI and declared descriptor inputs only.
Generated constructors load a shared runtime rather than copying its source.

Bazel cannot load action outputs during the same build's loading phase. To load
the generated constructors from BUILD/.bzl/PROJECT.scl, first generate and copy
the output into your source tree, then load that checked-in file on a later run.
""",
    attrs = {
        "deps": attr.label_list(
            mandatory = True,
            allow_empty = False,
            providers = [ProtoInfo],
            doc = "proto_library targets supplying schemas and their imports.",
        ),
        "out": attr.output(
            doc = "Output file; defaults to <name>.scl.",
        ),
        "runtime_load": attr.string(
            doc = "Override the shared runtime load label; inferred from the output extension by default. Bazel-native .scl files require a local runtime copy.",
        ),
        "generator": attr.label(
            default = Label("//:protolark"),
            executable = True,
            cfg = "exec",
            doc = "Exec-platform protolark binary, built from source by default.",
        ),
    },
)

def _protolark_config_impl(ctx):
    descriptors = depset(transitive = [dep[ProtoInfo].transitive_descriptor_sets for dep in ctx.attr.deps])
    out = ctx.outputs.out
    if not out:
        out = ctx.actions.declare_file(ctx.label.name + (".txtpb" if ctx.attr.format == "text" else ".pb"))
    sources = depset([ctx.file.src] + ctx.files.data)
    args = ctx.actions.args()
    args.add("encode")
    paths = {}
    config_path = None
    for source in sources.to_list():
        path = source.short_path
        if source.owner.repo_name != ctx.label.repo_name:
            fail("src and data must belong to the rule's repository; copy external inputs with a local rule first: " + path)
        if path.startswith("../"):
            path = "/".join(path.split("/")[2:])
        if path in paths:
            fail("multiple load inputs have the same path: " + path)
        paths[path] = True
        args.add("--load-file")
        args.add(path)
        args.add(source)
        if source == ctx.file.src:
            config_path = path

    args.add_all(descriptors, before_each = "--descriptor-set")
    args.add("--message", ctx.attr.message)
    args.add("--config", config_path)
    args.add("--symbol", ctx.attr.symbol)
    args.add("--format", ctx.attr.format)
    args.add("--out", out)

    ctx.actions.run(
        executable = ctx.executable.generator,
        arguments = [args],
        inputs = depset(transitive = [sources, descriptors]),
        outputs = [out],
        mnemonic = "ProtolarkEncode",
        progress_message = "Evaluating protobuf configuration %{label}",
    )
    return [DefaultInfo(files = depset([out]))]

protolark_config = rule(
    implementation = _protolark_config_impl,
    doc = """Evaluates a Starlark config and encodes its exported value as protobuf.

Unlike BUILD-file loading, embedded evaluation occurs during execution and can
load generated protolark_library outputs listed in data in the same build.
Loads resolve relative to the configuration or via //package:file from the
repository root. All loaded files must be declared in data. The CLI stages
declared inputs under their repository-relative paths and cleans them up.
External-repository input files must be copied to local outputs first.
""",
    attrs = {
        "src": attr.label(
            mandatory = True,
            allow_single_file = [".scl", ".bzl", ".star"],
            doc = "Starlark configuration source.",
        ),
        "data": attr.label_list(
            allow_files = True,
            doc = "Declared load dependencies, including generated protolark_library targets.",
        ),
        "deps": attr.label_list(
            mandatory = True,
            allow_empty = False,
            providers = [ProtoInfo],
            doc = "proto_library targets supplying the message descriptors.",
        ),
        "message": attr.string(
            mandatory = True,
            doc = "Fully-qualified protobuf message name.",
        ),
        "symbol": attr.string(
            default = "project",
            doc = "Global variable to encode after evaluation.",
        ),
        "format": attr.string(
            default = "binary",
            values = ["binary", "text"],
            doc = "Protobuf output format.",
        ),
        "out": attr.output(
            doc = "Output file; defaults to <name>.pb or <name>.txtpb.",
        ),
        "generator": attr.label(
            default = Label("//:protolark"),
            executable = True,
            cfg = "exec",
            doc = "Exec-platform protolark binary.",
        ),
    },
)
