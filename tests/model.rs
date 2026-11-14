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
fn postman_to_openapi_is_valid_shape() {
    let p = import_postman(&fixture("postman.json")).unwrap();
    let out = export_openapi(&p, OpenApiVersion::V3_0).unwrap();
    let d = &out.document;
    assert_eq!(d["openapi"], "3.0.3");
    assert_eq!(d["servers"][0]["url"], "https://api.example.com");
    let op = &d["paths"]["/users/{id}"]["get"];
    let id = op["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "id")
        .unwrap();
    assert_eq!(id["in"], "path");
    assert_eq!(id["required"], true);
    assert!(
        d["paths"]["/users/{id}"]["delete"]["tags"][0]
            .as_str()
            .unwrap()
            .contains("Users/Admin")
    );
    assert!(out.warnings.iter().any(|w| w.contains("LOCK")));
    // scripts survive as vendor extensions
    assert!(d["paths"]["/users"]["post"]["x-hozd-scripts"].is_array());
    // Postman-only keys are not leaked
    assert!(d.get("x-unknown-root").is_some());
    assert!(d["paths"]["/users"]["post"].get("request.proxy").is_none());
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

#[test]
fn openapi_30_round_trip() {
    let doc = fixture("openapi30.json");
    let out = export_openapi(&import_openapi(&doc).unwrap(), OpenApiVersion::V3_0).unwrap();
    assert_same(&doc, &out.document);
}

#[test]
fn openapi_31_round_trip() {
    let doc = fixture("openapi31.json");
    let out = export_openapi(&import_openapi(&doc).unwrap(), OpenApiVersion::V3_1).unwrap();
    assert_same(&doc, &out.document);
}

#[test]
fn openapi_structure() {
    let p = import_openapi(&fixture("openapi30.json")).unwrap();
    assert_eq!(p.source.version.as_deref(), Some("3.0.3"));
    assert_eq!(p.version.as_deref(), Some("1.2.0"));
    assert_eq!(p.servers[0].variables[0].allowed, ["api", "staging"]);
    assert_eq!(
        p.auth,
        Some(Auth::Reference {
            scheme: "bearerAuth".into(),
            scopes: vec![]
        })
    );
    assert_eq!(p.security_schemes.len(), 3);
    assert_eq!(
        p.directories
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>(),
        ["pets", "store"]
    );
    assert_eq!(
        p.directories[0].description.as_deref(),
        Some("Pet operations")
    );
    assert_eq!(p.extensions["x-api-id"], "pets-1");

    let list = find(&p, "List pets");
    let r = rest(list);
    assert_eq!(r.meta.tags, ["pets", "public"]);
    assert_eq!(r.meta.operation_id.as_deref(), Some("listPets"));
    assert_eq!(r.params.len(), 3);
    assert_eq!(r.params[0].pair.value, "10");
    assert_eq!(r.params[2].location, ParamLocation::Cookie);
    assert_eq!(r.responses.len(), 2);
    assert_eq!(list.extensions["x-codegen"], json!({ "name": "list" }));

    assert!(rest(find(&p, "Create pet")).meta.deprecated);
    assert_eq!(rest(find(&p, "Get pet")).auth, Some(Auth::NoAuth));
    assert_eq!(
        rest(find(&p, "Create pet")).auth,
        Some(Auth::Reference {
            scheme: "apiKey".into(),
            scopes: vec![]
        })
    );
    // untagged operations live at the root
    assert!(p.requests.iter().any(|r| r.name == "upload"));
    assert!(matches!(
        rest(find(&p, "placeOrder")).body.as_ref().unwrap().content,
        BodyContent::FormUrlEncoded { .. }
    ));
}

#[test]
fn openapi_to_postman_converts_path_params() {
    let p = import_openapi(&fixture("openapi30.json")).unwrap();
    let out = export_postman(&p).unwrap();
    let text = serde_json::to_string(&out.document).unwrap();
    assert!(text.contains("\"/") || text.contains(":petId"));
    assert!(text.contains(":petId"));
    assert!(!text.contains("{petId}"));
    assert_eq!(out.document["info"]["name"], "Pets");
    assert_eq!(out.document["variable"][0]["key"], "baseUrl");
    // and Postman output imports again
    let again = import_postman(&out.document).unwrap();
    assert_eq!(again.all_requests().len(), p.all_requests().len());
    assert_eq!(rest(find(&again, "Get pet")).path, "/pets/{petId}");
}

// ------------------------------------------------------------------ Swagger

#[test]
fn swagger_imports_to_the_same_model() {
    let p = import_openapi(&fixture("swagger2.json")).unwrap();
    assert_eq!(p.source.format, SourceFormat::Swagger);
    assert_eq!(
        p.servers.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(),
        [
            "https://legacy.example.com/api",
            "http://legacy.example.com/api"
        ]
    );
    assert_eq!(p.extensions["x-vendor"], 1);
    assert_eq!(
        p.extensions["components"]["schemas"]["Item"]["type"],
        "object"
    );
    assert_eq!(
        p.auth,
        Some(Auth::Reference {
            scheme: "key".into(),
            scopes: vec![]
        })
    );
    assert!(matches!(p.security_schemes[0].auth, Auth::Basic { .. }));

    let get = rest(find(&p, "Get item"));
    assert_eq!(get.path, "/items/{id}");
    assert_eq!(get.params[1].schema.as_ref().unwrap()["type"], "array");
    assert_eq!(
        get.responses[0].schema.as_ref().unwrap()["$ref"],
        "#/components/schemas/Item"
    );
    assert_eq!(get.responses[0].headers[0].pair.name, "X-Rate");

    let put = rest(find(&p, "putItem"));
    assert_eq!(
        put.body.as_ref().unwrap().content_type.as_deref(),
        Some("application/json")
    );
    assert_eq!(
        put.auth,
        Some(Auth::Reference {
            scheme: "basic".into(),
            scopes: vec![]
        })
    );

    let upload = rest(find(&p, "upload"));
    assert!(
        matches!(&upload.body.as_ref().unwrap().content, BodyContent::Multipart { fields } if fields.iter().any(|f| f.kind == FieldKind::File))
    );
}

#[test]
fn swagger_exports_as_openapi_3() {
    let p = import_openapi(&fixture("swagger2.json")).unwrap();
    let out = export_openapi(&p, OpenApiVersion::V3_0).unwrap();
    let d = &out.document;
    assert_eq!(d["openapi"], "3.0.3");
    assert!(d.get("swagger").is_none());
    assert_eq!(d["components"]["schemas"]["Item"]["type"], "object");
    assert_eq!(
        d["components"]["securitySchemes"]["basic"]["scheme"],
        "basic"
    );
    assert_eq!(
        d["paths"]["/items/{id}"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
            ["$ref"],
        "#/components/schemas/Item"
    );
    // importing the export gives the same model modulo the source
    let again = import_openapi(d).unwrap();
    assert_eq!(again.all_requests().len(), p.all_requests().len());
}

// ------------------------------------------------------------------ model

#[test]
fn unknown_field_round_trips_through_extensions() {
    let mut doc = fixture("openapi30.json");
    doc["paths"]["/pets"]["get"]["x-brand-new"] = json!({ "k": [1, 2] });
    doc["x-root-new"] = json!("v");
    let p = import_openapi(&doc).unwrap();
    assert_eq!(
        find(&p, "List pets").extensions["x-brand-new"],
        json!({ "k": [1, 2] })
    );
    let out = export_openapi(&p, OpenApiVersion::V3_0).unwrap();
    assert_same(&doc, &out.document);

    let mut pm = fixture("postman.json");
    pm["item"][1]["unknownKey"] = json!(true);
    pm["item"][1]["request"]["unknownReqKey"] = json!([1]);
    let p = import_postman(&pm).unwrap();
    assert_same(&pm, &export_postman(&p).unwrap().document);
}

#[test]
fn native_project_exports_valid_documents() {
    let mut p = Project::new("Native");
    p.servers.push(Server {
        url: "https://x.test".into(),
        ..Server::default()
    });
    let mut dir = Directory::new("Users");
    dir.directories.push(Directory::new("Admin"));
    let mut req = RestRequest::new(Method::Post, "/users/{id}");
    req.params
        .push(RestParam::new(ParamLocation::Query, "q", "1"));
    req.auth = Some(Auth::Bearer { token: "t".into() });
    req.body = Some(RestBody {
        content: BodyContent::Raw {
            text: "{\"a\":1}".into(),
            language: Some("json".into()),
        },
        ..RestBody::default()
    });
    dir.directories[0]
        .requests
        .push(Request::rest("Make admin", req));
    p.directories.push(dir);
    p.requests.push(Request::rest(
        "Ping",
        RestRequest::new(Method::Get, "/ping"),
    ));

    let oa = export_openapi(&p, OpenApiVersion::V3_1).unwrap().document;
    assert_eq!(oa["openapi"], "3.1.0");
    assert_eq!(oa["info"]["version"], "1.0.0");
    let op = &oa["paths"]["/users/{id}"]["post"];
    assert_eq!(op["summary"], "Make admin");
    assert_eq!(op["tags"][0], "Users/Admin");
    assert_eq!(op["parameters"][1]["in"], "path");
    assert_eq!(
        op["requestBody"]["content"]["application/json"]["example"],
        json!({ "a": 1 })
    );
    assert_eq!(op["security"][0]["bearerAuth"], json!([]));
    assert_eq!(
        oa["components"]["securitySchemes"]["bearerAuth"]["scheme"],
        "bearer"
    );
    assert!(oa["paths"]["/ping"]["get"]["responses"].is_object());

    // nested directories survive an OpenAPI round trip
    let back = import_openapi(&oa).unwrap();
    assert_eq!(
        back.directories[0].directories[0].requests[0].name,
        "Make admin"
    );

    let pm = export_postman(&p).unwrap().document;
    assert!(
        pm["info"]["schema"]
            .as_str()
            .unwrap()
            .ends_with("v2.1.0/collection.json")
    );
    assert_eq!(
        pm["item"][0]["item"][0]["item"][0]["request"]["url"]["path"],
        json!(["users", ":id"])
    );
    assert_eq!(
        pm["item"][0]["item"][0]["item"][0]["request"]["url"]["raw"],
        "{{baseUrl}}/users/:id?q=1"
    );
}

#[test]
fn ids_are_kept_and_generated() {
    let p = import_postman(&fixture("postman.json")).unwrap();
    assert_eq!(p.id, "11111111-2222-4333-8444-555555555555");
    assert_eq!(find(&p, "Get user").id, "req-get");
    let a = import_openapi(&fixture("openapi30.json")).unwrap();
    let b = import_openapi(&fixture("openapi30.json")).unwrap();
    assert_eq!(find(&a, "List pets").id, find(&b, "List pets").id);
    assert!(is_derived(&find(&a, "List pets").id));
    assert_ne!(new_id(), new_id());
}

#[test]
fn project_serializes_with_serde() {
    for name in ["postman.json", "openapi30.json", "swagger2.json"] {
        let p = import(&fixture(name)).unwrap();
        let text = p.to_json().unwrap();
        assert_eq!(Project::from_json(&text).unwrap(), p, "{name}");
    }
    let r = Request::rest("x", RestRequest::new(Method::Get, "/"));
    assert_eq!(serde_json::to_value(&r).unwrap()["api"]["kind"], "rest");
    assert_eq!(r.kind(), ApiKind::Rest);
}
