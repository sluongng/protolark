# protolark

Protobuf-backed Starlark configuration: generate typed constructors, evaluate
configs, and encode or decode binary and text protobuf. Inspired by Bazel's
`PROJECT.scl` pattern; independent of Google's Protolark implementation.

## Install

Build with Rust 1.98.1 (edition 2024) or newer. The installed CLI needs neither
Rust nor `protoc`:

```sh
cargo install --path . --locked
protolark --help
```

## Quick start

Define `project.proto`:

```protobuf
syntax = "proto3";
package demo;
message Project {
  string name = 1;
  repeated string targets = 2;
}
```

Generate constructors:

```sh
protolark generate -I . project.proto --out project_generated.bzl
```

Write `project.star`:

```python
load(":project_generated.bzl", "project_pb2")

project = project_pb2.Project.create(
    name = "demo",
    targets = ["//app:all", "//tests:all"],
)
```

Evaluate to JSON, or encode using a standard descriptor set:

```sh
protolark eval --config project.star
protoc -I . --include_imports --descriptor_set_out=schema.pb project.proto
protolark encode --descriptor-set schema.pb --message demo.Project \
  --config project.star --out project.pb
protolark decode --descriptor-set schema.pb --message demo.Project --input project.pb
```

Use `--format text --out project.txtpb` for text protobuf, or `encode --input`
for protobuf JSON input. All commands support `--out` and `--json`; JSON-mode
encoding requires `--out`. See `protolark <command> --help` for options.

## Bazel

Use the local checkout from a consuming workspace:

```python
# MODULE.bazel
bazel_dep(name = "protolark", version = "0.1.0")
bazel_dep(name = "protobuf", version = "36.2")
local_path_override(module_name = "protolark", path = "/path/to/protolark")
```

```python
# BUILD.bazel
load("@protobuf//bazel:proto_library.bzl", "proto_library")
load("@protolark//protolark:defs.bzl", "protolark_config", "protolark_library")

proto_library(name = "schema", srcs = ["project.proto"])

protolark_library(
    name = "constructors",
    deps = [":schema"],
    out = "project_generated.bzl",
)

protolark_config(
    name = "config",
    src = "project.star",
    data = [":constructors"],
    deps = [":schema"],
    message = "demo.Project",
)
```

`bazel build :config` produces `config.pb`; `format = "text"` produces
`config.txtpb`. Declare loaded files in `data`. Schemas come from `ProtoInfo`;
the rules build the CLI using **rules_rs and hermetic LLVM**, without shell actions.
See [examples](examples/) and the [downstream fixture](e2e/bzlmod/).

### Shared runtime and loading

Generated `.bzl` files load `@protolark//protolark:runtime.bzl`; `.scl` files load
`//protolark:runtime.scl`. Both use one implementation. The CLI bundles the runtime,
so `eval` and `protolark_config` need no local runtime copy.

Bazel-native `.scl` files, including `PROJECT.scl`, only accept same-repository
`//...` loads of other `.scl` files. Copy [runtime.scl](protolark/runtime.scl) into
the consuming repository for that use case. Customize its location with
`--runtime-load //config:runtime.scl` or the rule's `runtime_load` attribute.
Keep the runtime and generator versions aligned.

Bazel cannot load action outputs during its loading phase: check in generated
constructors for BUILD/`.bzl`/`PROJECT.scl` loads. `protolark_config` evaluates
during execution and can load generated constructors in the same build.

## Semantics

- Proto2/proto3 messages, nested types, enums, maps, repeated fields, required
  fields, and oneofs are supported. The generator rejects Editions and extensions/groups.
- Constructors validate fields and return structs; omitted fields and `None` stay absent.
- Bytes are base64 strings. `encode --config` uses structural fields, including
  for well-known types; `encode --input` uses standard ProtoJSON mappings.
- Decoding emits ProtoJSON and omits unknown wire fields; it is not a lossless wire editor.
- Local loads stay under `--load-root` (default: config directory). Apart from the
  bundled runtime, external repository loads are unsupported. Resource limits are
  best-effort, not an OS sandbox.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
bazel test //...
(cd e2e/bzlmod && bazel test //...)
```

After changing generation, refresh the native Starlark test fixture:

```sh
bazel build //examples:project_starlark
cp bazel-bin/examples/project_generated.bzl tests/project_generated.golden.bzl
```
