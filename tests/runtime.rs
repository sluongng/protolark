use prost::Message;
use protolark::{codec, runtime};
use serde_json::json;
use std::{fs, path::PathBuf};

fn fixture() -> (tempfile::TempDir, Vec<PathBuf>) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("test.proto"),
        r#"syntax = "proto2";
package test;
message Child { required string name = 1; }
message Config {
 required string name = 1;
 optional uint64 count = 2;
 repeated Child children = 3;
 map<int32, string> labels = 4;
 optional bytes data = 5;
 oneof choice { string first = 6; int32 second = 7; }
 optional bool enabled = 8;
 optional Child child = 9;
}"#,
    )
    .unwrap();
    let set = protox::compile([dir.path().join("test.proto")], [dir.path()]).unwrap();
    fs::write(
        dir.path().join("types.scl"),
        protolark::generate(&set, &[]).unwrap(),
    )
    .unwrap();
    let descriptor = dir.path().join("schema.pb");
    fs::write(&descriptor, set.encode_to_vec()).unwrap();
    (dir, vec![descriptor])
}

#[test]
fn generated_config_binary_and_text_roundtrip() {
    let (dir, sets) = fixture();
    let config = dir.path().join("PROJECT.scl");
    fs::write(
        &config,
        r#"load("//:types.scl", "test_pb2")
project = test_pb2.Config.create(
    name = "example",
    count = 18446744073709551615,
    children = [test_pb2.Child.create(name = n) for n in ["one", "two"]],
    labels = {1: "a", 2: "b"},
    data = "AAEC",
    enabled = True,
)
"#,
    )
    .unwrap();
    let value = runtime::evaluate(&config, "project", None).unwrap();
    assert_eq!(value["count"], json!(u64::MAX));
    let bytes = codec::encode(&sets, "test.Config", &value).unwrap();
    let decoded = codec::decode(&sets, "test.Config", &bytes).unwrap();
    assert_eq!(decoded["count"], json!(u64::MAX.to_string()));
    assert_eq!(decoded["children"][1]["name"], "two");
    assert_eq!(decoded["labels"]["1"], "a");
    assert_eq!(decoded["data"], "AAEC");
    let text = codec::encode_text(&sets, "test.Config", &value).unwrap();
    assert_eq!(
        codec::decode_text(&sets, "test.Config", &text).unwrap(),
        decoded
    );
}

#[test]
fn invalid_fields_types_required_and_oneof_are_rejected() {
    let (dir, sets) = fixture();
    for invalid in [
        json!({"name":"ok","unknown":1}),
        json!({"name":1}),
        json!({}),
        json!({"name":"ok","child":{}}),
        json!({"name":"ok","first":"a","second":1}),
    ] {
        assert!(
            codec::encode(&sets, "test.Config", &invalid).is_err(),
            "accepted {invalid}"
        );
    }
    let config = dir.path().join("bad.scl");
    for args in [
        "",
        "name = 1",
        "name = 'ok', first = 'a', second = 1",
        "name = 'ok', children = [1]",
        "name = 'ok', count = True",
        "name = 'ok', labels = {'bad': 'a'}",
    ] {
        fs::write(
            &config,
            format!("load('types.scl', 'test_pb2')\nproject = test_pb2.Config.create({args})\n"),
        )
        .unwrap();
        assert!(
            runtime::evaluate(&config, "project", None).is_err(),
            "accepted {args}"
        );
    }
    assert!(codec::decode(&sets, "test.Missing", &[]).is_err());
    assert!(codec::decode(&sets, "test.Config", &[]).is_err());
}

#[test]
fn loader_cycles_missing_files_and_escape_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    let config = root.join("a.scl");
    fs::write(root.join("b.scl"), "load('a.scl', 'project')\n").unwrap();
    fs::write(&config, "load('b.scl', 'project')\n").unwrap();
    assert!(
        runtime::evaluate(&config, "project", None)
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
    fs::write(
        dir.path().join("outside.scl"),
        "project = struct(name = 'outside')\n",
    )
    .unwrap();
    fs::write(&config, "load('../outside.scl', 'project')\n").unwrap();
    assert!(
        runtime::evaluate(&config, "project", None)
            .unwrap_err()
            .to_string()
            .contains("escapes")
    );
    fs::write(&config, "load('missing.scl', 'project')\n").unwrap();
    assert!(runtime::evaluate(&config, "project", None).is_err());
    for label in [
        "@other//protolark:runtime.scl",
        "@protolark//protolark:other.scl",
    ] {
        fs::write(&config, format!("load({label:?}, 'project')\n")).unwrap();
        assert!(runtime::evaluate(&config, "project", None).is_err());
    }
    fs::write(&config, "project = open('outside.scl')\n").unwrap();
    assert!(runtime::evaluate(&config, "project", None).is_err());
}

#[test]
fn protobuf_numbers_and_map_keys_do_not_lose_precision() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.scl");
    fs::write(&config,"project = struct(low = -9223372036854775808, high = 18446744073709551615, flags = {True: 1, False: 2})\n").unwrap();
    let value = runtime::evaluate(&config, "project", None).unwrap();
    assert_eq!(value["low"], json!(i64::MIN));
    assert_eq!(value["high"], json!(u64::MAX));
    assert_eq!(value["flags"]["true"], 1);
    fs::write(&config, "project = struct(value = 18446744073709551616)\n").unwrap();
    assert!(runtime::evaluate(&config, "project", None).is_err());
}

#[test]
fn costly_evaluation_and_exponential_json_expansion_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.scl");
    fs::write(&config, "project = [i for i in range(1000010)]\n").unwrap();
    assert!(runtime::evaluate(&config, "project", None).is_err());
    fs::write(&config, "def expand():\n    value = None\n    for i in range(24):\n        value = [value, value]\n    return value\nproject = expand()\n").unwrap();
    let error = runtime::evaluate(&config, "project", None).unwrap_err();
    assert!(format!("{error:#}").contains("one million JSON values"));
}

#[cfg(unix)]
#[test]
fn symlink_load_cannot_escape_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(dir.path().join("outside.scl"), "value = 'outside'\n").unwrap();
    std::os::unix::fs::symlink(dir.path().join("outside.scl"), root.join("link.scl")).unwrap();
    let config = root.join("config.scl");
    fs::write(&config, "load('link.scl', 'value')\nproject = value\n").unwrap();
    assert!(
        runtime::evaluate(&config, "project", None)
            .unwrap_err()
            .to_string()
            .contains("escapes")
    );
}

#[test]
fn structural_well_known_types_and_protojson_both_encode() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("well_known.proto"),
        r#"syntax = "proto3";
package test;
import "google/protobuf/timestamp.proto";
import "google/protobuf/any.proto";
import "google/protobuf/struct.proto";
message Config {
 google.protobuf.Timestamp time = 1;
 google.protobuf.Any any = 2;
 google.protobuf.Struct structure = 3;
 double number = 4;
 map<string, google.protobuf.Timestamp> times = 5;
}"#,
    )
    .unwrap();
    let set = protox::compile([dir.path().join("well_known.proto")], [dir.path()]).unwrap();
    let descriptor = dir.path().join("schema.pb");
    fs::write(&descriptor, set.encode_to_vec()).unwrap();
    let sets = vec![descriptor];
    fs::write(
        dir.path().join("types.scl"),
        protolark::generate(&set, &[]).unwrap(),
    )
    .unwrap();
    let config = dir.path().join("config.scl");
    fs::write(&config,r#"load("types.scl", "well_known_pb2", "timestamp_pb2", "any_pb2", "struct_pb2")
project = well_known_pb2.Config.create(
    time = timestamp_pb2.Timestamp.create(seconds = 0),
    any = any_pb2.Any.create(type_url = "type.googleapis.com/google.protobuf.Timestamp", value = ""),
    structure = struct_pb2.Struct.create(fields = {"hello": struct_pb2.Value.create(string_value = "world")}),
    number = float("nan"),
    times = {"first": timestamp_pb2.Timestamp.create(seconds = 1)},
)
"#).unwrap();
    let value = runtime::evaluate(&config, "project", None).unwrap();
    assert_eq!(value["number"], "NaN");
    let bytes = codec::encode_config(&sets, "test.Config", &value).unwrap();
    let decoded = codec::decode(&sets, "test.Config", &bytes).unwrap();
    assert_eq!(decoded["time"], "1970-01-01T00:00:00Z");
    assert_eq!(decoded["structure"], json!({"hello":"world"}));
    assert_eq!(
        decoded["any"]["@type"],
        "type.googleapis.com/google.protobuf.Timestamp"
    );
    assert_eq!(decoded["times"]["first"], "1970-01-01T00:00:01Z");
    assert_eq!(decoded["number"], "NaN");
    let text = codec::encode_config_text(&sets, "test.Config", &value).unwrap();
    assert_eq!(
        codec::decode_text(&sets, "test.Config", &text).unwrap(),
        decoded
    );
    assert_eq!(
        codec::decode(
            &sets,
            "test.Config",
            &codec::encode(&sets, "test.Config", &decoded).unwrap()
        )
        .unwrap(),
        decoded
    );
    let scalar = json!({"seconds": 0});
    assert_eq!(
        codec::decode(
            &sets,
            "google.protobuf.Timestamp",
            &codec::encode_config(&sets, "google.protobuf.Timestamp", &scalar).unwrap()
        )
        .unwrap(),
        "1970-01-01T00:00:00Z"
    );
}

#[test]
fn maps_have_deterministic_wire_and_text_encoding() {
    let (_dir, sets) = fixture();
    let value = json!({"name":"ok","labels":{"1":"a","0":"zero","-1":"negative","32":"b","4":"c"}});
    let expected = codec::encode(&sets, "test.Config", &value).unwrap();
    let expected_text = codec::encode_text(&sets, "test.Config", &value).unwrap();
    for _ in 0..20 {
        assert_eq!(
            codec::encode(&sets, "test.Config", &value).unwrap(),
            expected
        );
        assert_eq!(
            codec::encode_config(&sets, "test.Config", &value).unwrap(),
            expected
        );
        assert_eq!(
            codec::encode_text(&sets, "test.Config", &value).unwrap(),
            expected_text
        );
        assert_eq!(
            codec::encode_config_text(&sets, "test.Config", &value).unwrap(),
            expected_text
        );
    }
}

#[test]
fn protojson_any_payload_maps_are_deterministic_but_structural_bytes_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("any_maps.proto"),
        r#"syntax = "proto3";
package test;
import "google/protobuf/any.proto";
message Payload { map<string, int32> values = 1; google.protobuf.Any nested = 2; }
message Config { google.protobuf.Any payload = 1; }
"#,
    )
    .unwrap();
    let set = protox::compile([dir.path().join("any_maps.proto")], [dir.path()]).unwrap();
    let descriptor = dir.path().join("schema.pb");
    fs::write(&descriptor, set.encode_to_vec()).unwrap();
    let sets = vec![descriptor];
    let value = json!({"payload":{"@type":"type.googleapis.com/test.Payload", "values":{"a":1,"b":2,"c":3,"d":4}, "nested":{"@type":"type.googleapis.com/test.Payload","values":{"z":1,"y":2,"x":3,"w":4}}}});
    let expected = codec::encode(&sets, "test.Config", &value).unwrap();
    let expected_text = codec::encode_text(&sets, "test.Config", &value).unwrap();
    for _ in 0..20 {
        assert_eq!(
            codec::encode(&sets, "test.Config", &value).unwrap(),
            expected
        );
        assert_eq!(
            codec::encode_text(&sets, "test.Config", &value).unwrap(),
            expected_text
        );
    }
    assert_eq!(
        codec::decode(&sets, "test.Config", &expected).unwrap(),
        value
    );
    // Empty Payload with an unknown wire field: supplied bytes must not be
    // canonicalized, discarded or require that the type_url is resolvable.
    let structural = json!({"type_url":"type.googleapis.com/unknown.Payload","value":"GAc="});
    let encoded = codec::encode_config(&sets, "google.protobuf.Any", &structural).unwrap();
    let pool =
        prost_reflect::DescriptorPool::decode(fs::read(&sets[0]).unwrap().as_slice()).unwrap();
    let message = prost_reflect::DynamicMessage::decode(
        pool.get_message_by_name("google.protobuf.Any").unwrap(),
        encoded.as_slice(),
    )
    .unwrap();
    assert_eq!(
        message
            .get_field_by_name("value")
            .unwrap()
            .as_bytes()
            .unwrap()
            .as_ref(),
        &[0x18, 0x07]
    );
}

#[test]
fn structural_maps_reject_null_scalar_and_message_values() {
    let (dir, sets) = fixture();
    assert!(
        codec::encode_config(
            &sets,
            "test.Config",
            &json!({"name":"ok","labels":{"1":null}})
        )
        .is_err()
    );
    fs::write(
        dir.path().join("map_message.proto"),
        r#"syntax = "proto2";
package maptest;
message Child { required string name = 1; }
message Config { map<string, Child> children = 1; }
"#,
    )
    .unwrap();
    let set = protox::compile([dir.path().join("map_message.proto")], [dir.path()]).unwrap();
    let descriptor = dir.path().join("map_schema.pb");
    fs::write(&descriptor, set.encode_to_vec()).unwrap();
    let sets = vec![descriptor];
    assert!(
        codec::encode_config(&sets, "maptest.Config", &json!({"children":{"a":null}})).is_err()
    );
    assert!(codec::encode_config(&sets, "maptest.Config", &json!({"children":{"a":{}}})).is_err());
    let value = json!({"children":{"a":{"name":"ok"}}});
    assert_eq!(
        codec::decode(
            &sets,
            "maptest.Config",
            &codec::encode_config(&sets, "maptest.Config", &value).unwrap()
        )
        .unwrap(),
        value
    );
}

#[test]
fn structural_fields_are_not_confused_with_json_aliases() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("aliases.proto"),
        r#"syntax = "proto3";
package test;
message Config {
 string first = 1 [json_name = "second"];
 string second = 2 [json_name = "other"];
 Config child = 3;
 map<string, Config> children = 4;
}"#,
    )
    .unwrap();
    let set = protox::compile([dir.path().join("aliases.proto")], [dir.path()]).unwrap();
    let descriptor = dir.path().join("schema.pb");
    fs::write(&descriptor, set.encode_to_vec()).unwrap();
    let sets = vec![descriptor];
    fs::write(
        dir.path().join("types.scl"),
        protolark::generate(&set, &[]).unwrap(),
    )
    .unwrap();
    let config = dir.path().join("config.scl");
    fs::write(&config, "load('types.scl', 'aliases_pb2')\nchild = aliases_pb2.Config.create(first = 'one', second = 'two')\nproject = aliases_pb2.Config.create(first = 'one', second = 'two', child = child, children = {'key': child})\n").unwrap();
    let value = runtime::evaluate(&config, "project", None).unwrap();
    let binary = codec::encode_config(&sets, "test.Config", &value).unwrap();
    assert_eq!(codec::decode(&sets, "test.Config", &binary).unwrap(), value);
    let text = codec::encode_config_text(&sets, "test.Config", &value).unwrap();
    assert_eq!(
        codec::decode_text(&sets, "test.Config", &text).unwrap(),
        value
    );
    assert!(
        codec::encode_config(
            &sets,
            "test.Config",
            &json!({"second":"one", "other":"two"})
        )
        .is_err()
    );
}

#[test]
fn extensions_preserve_names_validate_children_and_sort_maps() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("extensions.proto"),
        r#"syntax = "proto2";
package test;
import "google/protobuf/any.proto";
message Child { required string name = 1; map<string, int32> counts = 2; }
message Config { extensions 100 to max; }
extend Config { optional Child extra = 100; optional google.protobuf.Any payload = 101; }
"#,
    )
    .unwrap();
    let set = protox::compile([dir.path().join("extensions.proto")], [dir.path()]).unwrap();
    let descriptor = dir.path().join("schema.pb");
    fs::write(&descriptor, set.encode_to_vec()).unwrap();
    let sets = vec![descriptor];
    let value = json!({"[test.extra]":{"name":"ok","counts":{"a":1,"b":2,"c":3,"d":4}}, "[test.payload]":{"@type":"type.googleapis.com/test.Child","name":"any","counts":{"a":1,"b":2,"c":3,"d":4}}});
    let bytes = codec::encode(&sets, "test.Config", &value).unwrap();
    let text = codec::encode_text(&sets, "test.Config", &value).unwrap();
    assert_eq!(codec::decode(&sets, "test.Config", &bytes).unwrap(), value);
    assert_eq!(
        codec::decode_text(&sets, "test.Config", &text).unwrap(),
        value
    );
    for _ in 0..20 {
        assert_eq!(codec::encode(&sets, "test.Config", &value).unwrap(), bytes);
        assert_eq!(
            codec::encode_text(&sets, "test.Config", &value).unwrap(),
            text
        );
    }
    assert!(codec::encode(&sets, "test.Config", &json!({"[test.extra]":{}})).is_err());
    assert!(codec::decode_text(&sets, "test.Config", "[test.extra] {}").is_err());
    // Wire field 100 is an empty embedded Child, missing its required name.
    assert!(codec::decode(&sets, "test.Config", &[0xa2, 0x06, 0x00]).is_err());
}

#[test]
fn declared_load_files_evaluate_generated_constructors_with_root_and_relative_loads() {
    let (dir, _) = fixture();
    let config = dir.path().join("input.scl");
    let helper = dir.path().join("helper.scl");
    fs::write(&config,"load('//generated:types.scl', 'test_pb2')\nload('helper.scl', 'name')\nproject = test_pb2.Config.create(name = name)\n").unwrap();
    fs::write(&helper, "name = 'declared-files'\n").unwrap();
    let files = vec![
        (PathBuf::from("app/PROJECT.scl"), config),
        (PathBuf::from("app/helper.scl"), helper),
        (
            PathBuf::from("generated/types.scl"),
            dir.path().join("types.scl"),
        ),
    ];
    let value = runtime::evaluate_files(std::path::Path::new("app/PROJECT.scl"), "project", &files)
        .unwrap();
    assert_eq!(value, json!({"name":"declared-files"}));
}

#[test]
fn declared_load_files_reject_unsafe_duplicates_missing_config_and_undeclared_loads() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("input.scl");
    fs::write(&source, "project = 'ok'\n").unwrap();
    let config = PathBuf::from("PROJECT.scl");
    for logical in [
        "",
        "../escape.scl",
        "/absolute.scl",
        "./dot.scl",
        "app/../escape.scl",
        "app/./dot.scl",
        "app//double.scl",
    ] {
        let files = vec![
            (config.clone(), source.clone()),
            (PathBuf::from(logical), source.clone()),
        ];
        assert!(
            runtime::evaluate_files(&config, "project", &files).is_err(),
            "accepted {logical:?}"
        );
        assert!(runtime::evaluate_files(std::path::Path::new(logical), "project", &[]).is_err());
    }
    let duplicate = vec![
        (config.clone(), source.clone()),
        (config.clone(), source.clone()),
    ];
    assert!(
        runtime::evaluate_files(&config, "project", &duplicate)
            .unwrap_err()
            .to_string()
            .contains("duplicate")
    );
    assert!(
        runtime::evaluate_files(&config, "project", &[])
            .unwrap_err()
            .to_string()
            .contains("not declared")
    );
    fs::write(dir.path().join("undeclared.scl"), "name = 'outside'\n").unwrap();
    fs::write(&source, "load('undeclared.scl', 'name')\nproject = name\n").unwrap();
    assert!(runtime::evaluate_files(&config, "project", &[(config.clone(), source)]).is_err());
}

#[cfg(unix)]
#[test]
fn declared_physical_symlinks_are_copied_inside_the_load_root() {
    let (external, _) = fixture();
    let sandbox = tempfile::tempdir().unwrap();
    let config = sandbox.path().join("source.scl");
    fs::write(&config,"load('generated.scl', 'test_pb2')\nproject = test_pb2.Config.create(name = 'external-generated-input')\n").unwrap();
    let symlink = sandbox.path().join("generated-input.scl");
    std::os::unix::fs::symlink(external.path().join("types.scl"), &symlink).unwrap();
    let files = vec![
        (PathBuf::from("PROJECT.scl"), config),
        (PathBuf::from("generated.scl"), symlink),
    ];
    assert_eq!(
        runtime::evaluate_files(std::path::Path::new("PROJECT.scl"), "project", &files).unwrap(),
        json!({"name":"external-generated-input"})
    );
}
