mod support;

use std::path::PathBuf;

use hozd::client::{CallKind, GrpcError, load_schema_file};
use support::fixtures;

fn proto() -> PathBuf {
    fixtures().join("proto/demo.proto")
}

fn schema_error(err: GrpcError) -> String {
    let GrpcError::Schema(message) = err else {
        panic!("expected a schema error, got {err:?}");
    };
    message
}

// ---- descriptor sets ---------------------------------------------------------

#[test]
fn descriptor_set_lists_services_and_methods() {
    let schema = load_schema_file(&fixtures().join("descriptor.bin"), &[]).unwrap();
    let names: Vec<_> = schema.methods().iter().map(|m| m.full_name()).collect();
    assert_eq!(
        names,
        [
            "demo.v1.Demo/Chat",
            "demo.v1.Demo/Collect",
            "demo.v1.Demo/Create",
            "demo.v1.Demo/GetUser",
            "demo.v1.Demo/ListUsers",
            "demo.v1.Demo/Tree",
        ]
    );
    let kind = |n: &str| {
        schema
            .methods()
            .iter()
            .find(|m| m.name == n)
            .map(|m| m.kind)
            .unwrap()
    };
    assert_eq!(kind("GetUser"), CallKind::Unary);
    assert_eq!(kind("ListUsers"), CallKind::ServerStreaming);
    assert_eq!(kind("Collect"), CallKind::ClientStreaming);
    assert_eq!(kind("Chat"), CallKind::Bidirectional);
}

#[test]
fn pb_and_desc_extensions_are_descriptor_sets_too() {
    let tmp = tempfile::tempdir().unwrap();
    for ext in ["pb", "desc", "BIN"] {
        let path = tmp.path().join(format!("schema.{ext}"));
        std::fs::copy(fixtures().join("descriptor.bin"), &path).unwrap();
        assert_eq!(
            load_schema_file(&path, &[]).unwrap().service_count(),
            1,
            "{ext}"
        );
    }
}

#[test]
fn missing_descriptor_file_names_the_path() {
    let err = load_schema_file(&fixtures().join("nope.bin"), &[]).unwrap_err();
    assert!(schema_error(err).contains("nope.bin"));
}

#[test]
fn invalid_descriptor_file_names_the_path_and_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bad.bin");
    std::fs::write(&path, b"this is not a descriptor set").unwrap();
    let message = schema_error(load_schema_file(&path, &[]).unwrap_err());
    assert!(
        message.contains("bad.bin") && message.contains("FileDescriptorSet"),
        "{message}"
    );
}

#[test]
fn unsupported_extension_lists_the_accepted_ones() {
    let message = schema_error(load_schema_file(&PathBuf::from("schema.json"), &[]).unwrap_err());
    for accepted in [".proto", ".bin", ".pb", ".desc"] {
        assert!(message.contains(accepted), "{message}");
    }
}

// ---- proto sources -----------------------------------------------------------

#[test]
fn proto_resolves_sibling_and_well_known_imports() {
    let schema = load_schema_file(&proto(), &[]).unwrap();
    assert_eq!(schema.methods().len(), 6);
    // `common.proto` is a sibling, `timestamp.proto` is bundled.
    assert!(schema.pool().get_enum_by_name("demo.v1.Role").is_some());
    assert!(
        schema
            .pool()
            .get_message_by_name("google.protobuf.Timestamp")
            .is_some()
    );
}

#[test]
fn proto_import_is_found_under_an_extra_root() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("shared")).unwrap();
    std::fs::create_dir_all(tmp.path().join("svc")).unwrap();
    std::fs::write(
        tmp.path().join("shared/types.proto"),
        "syntax = \"proto3\";\npackage s;\nmessage Id { int64 v = 1; }\n",
    )
    .unwrap();
    let service = tmp.path().join("svc/api.proto");
    std::fs::write(
        &service,
        "syntax = \"proto3\";\npackage s;\nimport \"types.proto\";\n\
         service A { rpc Get(Id) returns (Id); }\n",
    )
    .unwrap();

    let without = schema_error(load_schema_file(&service, &[]).unwrap_err());
    assert!(without.contains("types.proto"), "{without}");

    let schema = load_schema_file(&service, &[tmp.path().join("shared")]).unwrap();
    assert_eq!(schema.methods()[0].full_name(), "s.A/Get");
}

#[test]
fn proto_syntax_error_reports_file_and_line() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("broken.proto");
    std::fs::write(
        &path,
        "syntax = \"proto3\";\n\nmessage M {\n  int32 a = ;\n}\n",
    )
    .unwrap();
    let message = schema_error(load_schema_file(&path, &[]).unwrap_err());
    assert!(message.contains("broken.proto:4"), "{message}");
}

#[test]
fn proto_with_unresolved_import_names_the_import() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.proto");
    std::fs::write(&path, "syntax = \"proto3\";\nimport \"missing.proto\";\n").unwrap();
    let message = schema_error(load_schema_file(&path, &[]).unwrap_err());
    assert!(message.contains("missing.proto"), "{message}");
}

// ---- skeletons ---------------------------------------------------------------

fn skeleton(method: &str) -> serde_json::Value {
    let schema = load_schema_file(&fixtures().join("descriptor.bin"), &[]).unwrap();
    serde_json::from_str(&schema.skeleton(method).unwrap()).unwrap()
}

#[test]
fn unary_skeleton_lists_every_field_with_defaults() {
    assert_eq!(
        skeleton("demo.v1.Demo/ListUsers"),
        serde_json::json!({"name": "", "pageSize": 0, "delayMs": 0})
    );
}

#[test]
fn skeleton_uses_json_forms_for_enums_timestamps_and_nested_messages() {
    assert_eq!(
        skeleton("demo.v1.Demo/Create"),
        serde_json::json!({
            "id": 0,
            "name": "",
            "role": "ROLE_UNSPECIFIED",
            "createdAt": "1970-01-01T00:00:00Z",
            "address": {"city": ""},
        })
    );
}

#[test]
fn streaming_skeleton_is_an_array_of_one() {
    assert_eq!(
        skeleton("demo.v1.Demo/Collect"),
        serde_json::json!([{"id": 0}])
    );
}

#[test]
fn self_referencing_message_nests_one_level() {
    assert_eq!(
        skeleton("demo.v1.Demo/Tree"),
        serde_json::json!({"name": "", "child": {"name": "", "child": null}})
    );
}

#[test]
fn skeleton_of_unknown_method_is_none() {
    let schema = load_schema_file(&fixtures().join("descriptor.bin"), &[]).unwrap();
    assert!(schema.skeleton("demo.v1.Demo/Nope").is_none());
}

#[test]
fn skeleton_round_trips_through_the_input_message() {
    // Every skeleton must be accepted by the schema it was made from.
    let schema = load_schema_file(&fixtures().join("descriptor.bin"), &[]).unwrap();
    for m in schema.methods() {
        let text = schema.skeleton(&m.full_name()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let input = schema.method(&m.full_name()).unwrap().input();
        let one = match value {
            serde_json::Value::Array(mut items) => items.remove(0),
            other => other,
        };
        prost_reflect::DynamicMessage::deserialize(input, one)
            .unwrap_or_else(|e| panic!("{}: {e}", m.full_name()));
    }
}
