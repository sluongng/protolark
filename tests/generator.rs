use prost_types::{
    DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    field_descriptor_proto::{Label, Type},
};
use protolark::generate;

fn message_file(name: &str, message: &str) -> FileDescriptorProto {
    FileDescriptorProto {
        name: Some(name.into()),
        syntax: Some("proto3".into()),
        message_type: vec![DescriptorProto {
            name: Some(message.into()),
            ..Default::default()
        }],
        ..Default::default()
    }
}
#[test]
fn deterministic_dependency_bundle_and_recursive_schema() {
    let dep = message_file("dep.proto", "Dep");
    let mut root = message_file("root.proto", "Root");
    root.dependency.push("dep.proto".into());
    root.message_type[0].field.push(FieldDescriptorProto {
        name: Some("next".into()),
        number: Some(1),
        label: Some(Label::Optional as i32),
        r#type: Some(Type::Message as i32),
        type_name: Some(".Root".into()),
        ..Default::default()
    });
    let a = FileDescriptorSet {
        file: vec![root.clone(), dep.clone()],
    };
    let b = FileDescriptorSet {
        file: vec![dep, root],
    };
    let code = generate(&a, &["root.proto".into()]).unwrap();
    assert_eq!(code, generate(&b, &[]).unwrap());
    assert!(code.contains("root_pb2 = struct("));
    assert!(code.contains("dep_pb2 = struct("));
    assert!(code.contains("\"next\": (\"message\""));
}
#[test]
fn rejects_missing_dependencies_ambiguous_exports_and_malformed_fields() {
    let mut file = message_file("foo.proto", "Foo");
    file.dependency.push("missing.proto".into());
    assert!(
        generate(&FileDescriptorSet { file: vec![file] }, &[])
            .unwrap_err()
            .to_string()
            .contains("missing descriptor")
    );
    let set = FileDescriptorSet {
        file: vec![
            message_file("a/foo.proto", "Foo"),
            message_file("b/foo.proto", "Bar"),
        ],
    };
    assert!(
        generate(&set, &[])
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
    let mut file = message_file("foo.proto", "Foo");
    file.message_type[0].field.push(FieldDescriptorProto {
        name: Some("bad".into()),
        ..Default::default()
    });
    assert!(generate(&FileDescriptorSet { file: vec![file] }, &[]).is_err());
}
#[test]
fn rejects_constructor_namespace_collision_and_bad_oneof_index() {
    let mut file = message_file("foo.proto", "Foo");
    file.message_type[0].nested_type.push(DescriptorProto {
        name: Some("create".into()),
        ..Default::default()
    });
    assert!(
        generate(
            &FileDescriptorSet {
                file: vec![file.clone()]
            },
            &[]
        )
        .unwrap_err()
        .to_string()
        .contains("collision")
    );
    file.message_type[0].nested_type.clear();
    file.message_type[0].field.push(FieldDescriptorProto {
        name: Some("value".into()),
        number: Some(1),
        label: Some(Label::Optional as i32),
        r#type: Some(Type::Int32 as i32),
        oneof_index: Some(-1),
        ..Default::default()
    });
    assert!(generate(&FileDescriptorSet { file: vec![file] }, &[]).is_err());
}

#[test]
fn generated_integer_validation_accepts_boundaries_and_rejects_overflow() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let set = protox::compile([fixture.join("config.proto")], [&fixture]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("types.scl"), generate(&set, &[]).unwrap()).unwrap();
    let config = dir.path().join("config.scl");
    for (values, valid) in [
        ("[-2147483648, 0, 1, 2147483647]", true),
        ("[-2147483649]", false),
        ("[2147483648]", false),
        ("[True]", false),
    ] {
        std::fs::write(&config, format!("load('types.scl', 'config_pb2')\nproject = config_pb2.Config.create(name = 'test', counts = {values})\n")).unwrap();
        assert_eq!(
            protolark::runtime::evaluate(&config, "project", None).is_ok(),
            valid,
            "counts={values}"
        );
    }
}
