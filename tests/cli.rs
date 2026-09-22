use prost::Message;
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Output};

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_protolark"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("demo.proto"),
        r#"syntax = "proto3";
package example;
message Project { string name = 1; repeated string targets = 2; bytes payload = 3; }
"#,
    )
    .unwrap();
    let set = protox::compile([root.path().join("demo.proto")], [root.path()]).unwrap();
    std::fs::write(root.path().join("schema.pb"), set.encode_to_vec()).unwrap();
    root
}

#[test]
fn installed_style_workflow_roundtrips_binary_and_text() {
    let root = fixture();
    let result = success(cli(
        root.path(),
        &["--json", "generate", "demo.proto", "--out", "demo.scl"],
    ));
    assert_eq!(result["data"]["output"], "demo.scl");
    std::fs::write(root.path().join("PROJECT.scl"), "load(\":demo.scl\", \"demo_pb2\")\nproject = demo_pb2.Project.create(name = \"demo\", targets = [\"//:test\"], payload = \"aGk=\")\n").unwrap();
    let value = success(cli(
        root.path(),
        &["--json", "eval", "--config", "PROJECT.scl"],
    ));
    assert_eq!(
        value["data"]["value"],
        json!({"name":"demo", "targets":["//:test"], "payload":"aGk="})
    );
    for format in ["binary", "text"] {
        success(cli(
            root.path(),
            &[
                "--json",
                "encode",
                "--descriptor-set",
                "schema.pb",
                "--message",
                "example.Project",
                "--config",
                "PROJECT.scl",
                "--format",
                format,
                "--out",
                "message.pb",
            ],
        ));
        let decoded = success(cli(
            root.path(),
            &[
                "--json",
                "decode",
                "--descriptor-set",
                "schema.pb",
                "--message",
                "example.Project",
                "--format",
                format,
                "--input",
                "message.pb",
            ],
        ));
        assert_eq!(decoded["data"]["value"], value["data"]["value"]);
    }
}

#[test]
fn generates_from_merged_descriptor_sets_and_checks_without_writing() {
    let root = fixture();
    let generated = success(cli(
        root.path(),
        &[
            "--json",
            "generate",
            "--descriptor-set",
            "schema.pb",
            "--descriptor-set",
            "schema.pb",
            "--file",
            "demo.proto",
            "--out",
            "demo.bzl",
        ],
    ));
    assert!(generated["data"]["bytes"].as_u64().unwrap() > 0);
    let checked = success(cli(
        root.path(),
        &[
            "--json",
            "generate",
            "demo.proto",
            "--out",
            "demo.bzl",
            "--check",
        ],
    ));
    assert_eq!(checked["data"]["matches"], true);
    for (filename, runtime_label) in [
        ("shared.bzl", "@protolark//protolark:runtime.bzl"),
        ("shared.scl", "//protolark:runtime.scl"),
    ] {
        success(cli(
            root.path(),
            &["--json", "generate", "demo.proto", "--out", filename],
        ));
        let source = std::fs::read_to_string(root.path().join(filename)).unwrap();
        assert!(source.contains(&format!("load({runtime_label:?},")));
        std::fs::write(root.path().join("shared_config.scl"), format!("load({filename:?}, 'demo_pb2')\nproject = demo_pb2.Project.create(name = 'shared')\n")).unwrap();
        let evaluated = success(cli(
            root.path(),
            &["--json", "eval", "--config", "shared_config.scl"],
        ));
        assert_eq!(evaluated["data"]["value"], json!({"name":"shared"}));
    }
    std::fs::write(
        root.path().join("local_runtime.scl"),
        include_str!("../protolark/runtime.scl"),
    )
    .unwrap();
    success(cli(
        root.path(),
        &[
            "--json",
            "generate",
            "demo.proto",
            "--runtime-load",
            "//:local_runtime.scl",
            "--out",
            "shared.scl",
        ],
    ));
    let evaluated = success(cli(
        root.path(),
        &["--json", "eval", "--config", "shared_config.scl"],
    ));
    assert_eq!(evaluated["data"]["value"], json!({"name":"shared"}));
    std::fs::write(root.path().join("demo.bzl"), "stale").unwrap();
    let mismatch = cli(
        root.path(),
        &[
            "--json",
            "generate",
            "demo.proto",
            "--out",
            "demo.bzl",
            "--check",
        ],
    );
    assert_eq!(mismatch.status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(root.path().join("demo.bzl")).unwrap(),
        "stale"
    );
}

#[test]
fn decodes_independent_wire_fixture() {
    let root = fixture();
    // Field 1, length-delimited, four bytes "demo". Not generated by our encoder.
    std::fs::write(
        root.path().join("wire.pb"),
        [0x0a, 4, b'd', b'e', b'm', b'o'],
    )
    .unwrap();
    let result = success(cli(
        root.path(),
        &[
            "decode",
            "--json",
            "--descriptor-set",
            "schema.pb",
            "--message",
            "example.Project",
            "--input",
            "wire.pb",
        ],
    ));
    assert_eq!(result["data"]["value"], json!({"name":"demo"}));
}

#[test]
fn invalid_schema_value_preserves_existing_output_and_returns_json() {
    let root = fixture();
    std::fs::write(root.path().join("input.json"), r#"{"not_a_field": true}"#).unwrap();
    std::fs::write(root.path().join("existing.pb"), "keep").unwrap();
    let result = cli(
        root.path(),
        &[
            "--json",
            "encode",
            "--descriptor-set",
            "schema.pb",
            "--message",
            "example.Project",
            "--input",
            "input.json",
            "--out",
            "existing.pb",
        ],
    );
    assert_eq!(result.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(error["ok"], false);
    assert_eq!(error["error"]["code"], "operation_failed");
    assert_eq!(
        std::fs::read_to_string(root.path().join("existing.pb")).unwrap(),
        "keep"
    );
}

#[test]
fn json_usage_errors_and_binary_output_are_unambiguous() {
    let root = fixture();
    let usage = cli(root.path(), &["--json", "no-such-command"]);
    assert_eq!(usage.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&usage.stdout).unwrap();
    assert_eq!(error["error"]["code"], "usage");
    let no_output = cli(
        root.path(),
        &[
            "--json",
            "encode",
            "--descriptor-set",
            "schema.pb",
            "--message",
            "example.Project",
        ],
    );
    assert_eq!(no_output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&no_output.stdout).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("requires --out")
    );
}

#[test]
fn structural_timestamp_and_maps_are_stable_across_processes() {
    let root = fixture();
    std::fs::write(
        root.path().join("demo.proto"),
        r#"syntax = "proto3";
package example;
import "google/protobuf/timestamp.proto";
message Project { google.protobuf.Timestamp time = 1; map<string, int32> counts = 2; }
"#,
    )
    .unwrap();
    let set = protox::compile([root.path().join("demo.proto")], [root.path()]).unwrap();
    std::fs::write(root.path().join("schema.pb"), set.encode_to_vec()).unwrap();
    success(cli(
        root.path(),
        &["--json", "generate", "demo.proto", "--out", "demo.scl"],
    ));
    std::fs::write(root.path().join("PROJECT.scl"), "load(\":demo.scl\", \"demo_pb2\", \"timestamp_pb2\")\nproject = demo_pb2.Project.create(time = timestamp_pb2.Timestamp.create(seconds = 0), counts = {\"a\": 1, \"b\": 2, \"c\": 3, \"d\": 4})\n").unwrap();
    for format in ["binary", "text"] {
        let mut previous = None;
        for _ in 0..8 {
            success(cli(
                root.path(),
                &[
                    "--json",
                    "encode",
                    "--descriptor-set",
                    "schema.pb",
                    "--message",
                    "example.Project",
                    "--config",
                    "PROJECT.scl",
                    "--format",
                    format,
                    "--out",
                    "message.pb",
                ],
            ));
            let bytes = std::fs::read(root.path().join("message.pb")).unwrap();
            if let Some(previous) = &previous {
                assert_eq!(previous, &bytes);
            }
            previous = Some(bytes);
        }
        let value = success(cli(
            root.path(),
            &[
                "--json",
                "decode",
                "--descriptor-set",
                "schema.pb",
                "--message",
                "example.Project",
                "--format",
                format,
                "--input",
                "message.pb",
            ],
        ));
        assert_eq!(value["data"]["value"]["time"], "1970-01-01T00:00:00Z");
        assert_eq!(
            value["data"]["value"]["counts"],
            json!({"a":1,"b":2,"c":3,"d":4})
        );
    }
}

#[test]
fn declared_load_files_work_without_a_physical_config_tree() {
    let root = fixture();
    success(cli(
        root.path(),
        &[
            "--json",
            "generate",
            "demo.proto",
            "--out",
            "generated types.scl",
        ],
    ));
    std::fs::write(
        root.path().join("source config.scl"),
        "load(\":types.scl\", \"demo_pb2\")\nproject = demo_pb2.Project.create(name = \"mapped\")\n",
    )
    .unwrap();
    let mapped = [
        "--config",
        "app/PROJECT.scl",
        "--load-file",
        "app/PROJECT.scl",
        "source config.scl",
        "--load-file",
        "app/types.scl",
        "generated types.scl",
    ];
    let mut args = vec!["--json", "eval"];
    args.extend(mapped);
    let value = success(cli(root.path(), &args));
    assert_eq!(value["data"]["value"], json!({"name":"mapped"}));

    let mut args = vec![
        "--json",
        "encode",
        "--descriptor-set",
        "schema.pb",
        "--message",
        "example.Project",
        "--out",
        "mapped.pb",
    ];
    args.extend(mapped);
    success(cli(root.path(), &args));
    let decoded = success(cli(
        root.path(),
        &[
            "--json",
            "decode",
            "--descriptor-set",
            "schema.pb",
            "--message",
            "example.Project",
            "--input",
            "mapped.pb",
        ],
    ));
    assert_eq!(decoded["data"]["value"], value["data"]["value"]);
    assert!(!root.path().join("app").exists());

    args.extend(["--load-file", "app/types.scl", "generated types.scl"]);
    let error = cli(root.path(), &args);
    assert_eq!(error.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&error.stdout).unwrap();
    assert_eq!(error["ok"], false);

    let conflicting = cli(
        root.path(),
        &[
            "--json",
            "eval",
            "--config",
            "app/PROJECT.scl",
            "--load-root",
            ".",
            "--load-file",
            "app/PROJECT.scl",
            "source config.scl",
        ],
    );
    assert_eq!(conflicting.status.code(), Some(2));
}
