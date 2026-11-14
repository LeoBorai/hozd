use hozd::model::*;
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/model/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn first_diff(path: &str, a: &Value, b: &Value) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in x {
                match y.get(k) {
                    Some(w) => {
                        if let Some(d) = first_diff(&format!("{path}/{k}"), v, w) {
                            return Some(d);
                        }
                    }
                    None => return Some(format!("{path}/{k}: missing in actual (expected {v})")),
                }
            }
            y.keys()
                .find(|k| !x.contains_key(*k))
                .map(|k| format!("{path}/{k}: unexpected in actual ({})", y[k]))
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
            .iter()
            .zip(y)
            .enumerate()
            .find_map(|(i, (v, w))| first_diff(&format!("{path}/{i}"), v, w)),
        _ if a == b => None,
        _ => Some(format!("{path}: expected {a}, actual {b}")),
    }
}

#[track_caller]
fn assert_same(expected: &Value, actual: &Value) {
    if let Some(d) = first_diff("", expected, actual) {
        panic!("documents differ at {d}");
    }
}

fn rest(r: &Request) -> &RestRequest {
    r.api.as_rest().unwrap()
}

fn find<'a>(p: &'a Project, name: &str) -> &'a Request {
    p.all_requests()
        .into_iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("no request {name}"))
}

// ------------------------------------------------------------------ Postman

#[test]
fn postman_round_trip() {
    let doc = fixture("postman.json");
    let project = import_postman(&doc).unwrap();
    let out = export_postman(&project).unwrap();
    assert_same(&doc, &out.document);
}

#[test]
fn postman_structure() {
    let p = import_postman(&fixture("postman.json")).unwrap();
    assert_eq!(p.name, "Pet Store");
    assert_eq!(p.source.format, SourceFormat::Postman);
    assert_eq!(p.directories.len(), 1);
    let users = &p.directories[0];
    assert_eq!(users.directories[0].name, "Admin");
    assert_eq!(users.directories[0].auth, Some(Auth::NoAuth));
    assert_eq!(
        users.directories[0].scripts[0].source,
        "console.log('admin');\npm.variables.set('a', 1);"
    );
    assert_eq!(p.servers[0].url, "https://api.example.com");

    let del = find(&p, "Delete user");
    assert_eq!(rest(del).path, "/users/{id}");
    assert_eq!(rest(del).server.as_deref(), Some("{{baseUrl}}"));
    assert_eq!(rest(find(&p, "Lock")).method, Method::Custom("LOCK".into()));
    assert_eq!(
        find(&p, "Get user").description.as_deref(),
        Some("Fetch one")
    );
    assert_eq!(rest(find(&p, "Get user")).responses[0].code, "200");

    // `None` inherits, explicit NoAuth does not
    assert_eq!(rest(find(&p, "Get user")).auth, None);
    assert!(matches!(
        rest(find(&p, "Create user")).auth,
        Some(Auth::Bearer { .. })
    ));
    assert!(matches!(p.auth, Some(Auth::Other { .. })));

    // unknown keys survive in extensions
    assert_eq!(find(&p, "Create user").extensions["x-owner"], "team-a");
    assert_eq!(p.extensions["x-unknown-root"], json!({ "a": 1 }));
    assert_eq!(
        rest(find(&p, "Create user")).settings.extensions["disableBodyPruning"],
        true
    );
}

#[test]
fn postman_every_body_mode() {
    let p = import_postman(&fixture("postman.json")).unwrap();
    let body = |n: &str| rest(find(&p, n)).body.clone().unwrap().content;
    assert!(
        matches!(body("Create user"), BodyContent::Raw { language: Some(l), .. } if l == "json")
    );
    assert!(matches!(body("Login"), BodyContent::FormUrlEncoded { fields } if fields.len() == 2));
    assert!(
        matches!(body("Upload"), BodyContent::Multipart { fields } if fields[1].kind == FieldKind::File)
    );
    assert!(
        matches!(body("Binary"), BodyContent::Binary { file: Some(f) } if f == "/tmp/data.bin")
    );
}

#[test]
fn postman_rejects_v2_0() {
    let doc = json!({ "info": { "name": "x", "schema": "https://schema.getpostman.com/json/collection/v2.0.0/collection.json" }, "item": [] });
    assert!(matches!(
        import_postman(&doc),
        Err(ConvertError::Unsupported(_))
    ));
}

// ------------------------------------------------------------------ OpenAPI
