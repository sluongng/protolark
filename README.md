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
load(":project_generated.bzl", "project_proto")

project = project_proto.Project.create(
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
- Constructors validate fields and return ordinary structs; protobuf message
  identity is not retained or checked. Omitted fields and `None` stay absent.
- Bytes are base64 strings. `encode --config` uses structural fields, including
  for well-known types; `encode --input` uses standard ProtoJSON mappings.
- Decoding emits ProtoJSON and omits unknown wire fields; it is not a lossless wire editor.
- Local loads stay under `--load-root` (default: config directory). Apart from the
  bundled runtime, external repository loads are unsupported. Resource limits are
  best-effort, not an OS sandbox.

## Well-known types

Generated constructors support the structural fields of `google.protobuf`
messages. Constructors check Timestamp bounds, Duration bounds/signs, and Value's
selected kind and finite number requirements. The codec also enforces these
constraints when encoding or decoding, traversing nested messages, repeated
fields, and maps even when values bypass generated constructors.

Load portable helpers from the shared runtime to avoid manually building
`Struct`, `Value`, and `ListValue` trees:

```python
load("//protolark:runtime.scl", "wkt")
load(":service_generated.scl", "service_proto")

project = service_proto.Config.create(
    updated_at = wkt.timestamp(seconds = 1700000000, nanos = 123000000),
    timeout = wkt.duration(seconds = 5),
    metadata = wkt.struct({"enabled": True, "labels": ["stable"], "owner": None}),
)
```

The example assumes corresponding Timestamp, Duration, and Struct fields in
`service.proto`. In `.bzl`, load `wkt` from
`@protolark//protolark:runtime.bzl`; native `.scl` consumers need the same local
runtime copy described above. Standalone evaluation bundles both load labels.

- `wkt.timestamp(seconds = 0, nanos = 0)` and `wkt.duration(...)` accept exact
  integer components; they validate rather than normalize invalid components.
- `wkt.value(value)` converts JSON-like data to a Value. `wkt.struct(dict)` and
  `wkt.list_value(list_or_tuple)` return Struct and ListValue respectively.
- Conversion accepts string-keyed dictionaries, lists/tuples, strings, booleans,
  finite numbers, and `None`. Here `None` means explicit JSON null; it still
  means omission in ordinary generated constructor arguments.
- Helpers copy nested containers and restrict integer Value inputs to
  `[-9007199254740991, 9007199254740991]` to avoid double-precision rounding.
  Use strings for larger identifiers. Embedded evaluation also accepts finite
  floats; native Bazel Starlark has no float values.
- Conversion is bounded to 20 nested containers and 100,000 expanded values.
  The evaluator's existing 64-level structural JSON limit also counts enclosing
  configuration fields and the extra WKT wrapper layers.

For example, `wkt.value(None)` selects Value's `null_value` variant, and
`wkt.list_value([True, "stable", None])` constructs a ListValue containing a
boolean, a string, and null. An empty `Value.create()` is invalid; use explicit
null instead. Invalid Timestamp nanos and inconsistent Duration signs are also
rejected rather than silently normalized.

Structural config values remain distinct from ProtoJSON: `encode --input` and
`decode` use the standard WKT JSON forms (for example, timestamp strings and
plain objects for Struct). Raw `Any.type_url`/`Any.value` construction remains
available and preserves supplied payload bytes; these helpers do not pack Any
payloads or validate FieldMask paths against a target schema.

## Comparison with Skycfg

[Skycfg](https://github.com/stripe/skycfg) also uses Starlark to construct
protobuf configuration, but the projects use different execution models:

| Area | Protolark | Skycfg |
| --- | --- | --- |
| Schema access | Generates `.scl`/`.bzl` constructors from schemas or descriptor sets | Exposes registered protobuf types through `proto.package()` |
| Values | Ordinary structs with constructor validation; no message identity | Protobuf-aware objects with typed message assignments |
| Execution | Standalone Rust CLI, exported config values, and Bazel generation/encoding rules | Go embedding API with `main(ctx)`, caller-supplied variables, and multiple returned messages |
| Authoring | Portable WKT helpers and binary/text codecs | Message clone/merge, Any packing/unpacking, and config-native tests |

Choose Protolark when generated constructors must also run in native Bazel
Starlark, including `PROJECT.scl` with the loading restrictions above. Skycfg
fits applications embedding a richer protobuf-aware Starlark environment in Go;
its host-provided APIs are not native Bazel builtins. Protolark is not a
drop-in implementation of Skycfg's API. See Skycfg's
[protobuf model](https://github.com/stripe/skycfg/blob/trunk/docs/protobuf.asciidoc)
and [module reference](https://github.com/stripe/skycfg/blob/trunk/docs/modules.asciidoc).

## Development

Use Bazel for development and validation:

```sh
bazel build //:protolark
bazel test //...
(cd e2e/bzlmod && bazel test //...)
```

After changing generation, refresh the native Starlark test fixture:

```sh
bazel build //examples:project_starlark
cp bazel-bin/examples/project_generated.bzl tests/project_generated.golden.bzl
```
