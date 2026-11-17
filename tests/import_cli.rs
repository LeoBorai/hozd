use std::path::PathBuf;

use hozd::{ImportArgs, ImportKind, read_document};

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn parses_subcommand_path_and_name() {
    let a = ImportArgs::parse(&args(&["openapi", "api.yaml", "-n", "Pets"])).unwrap();
    assert_eq!(a.kind, ImportKind::OpenApi);
    assert_eq!(a.path, PathBuf::from("api.yaml"));
    assert_eq!(a.name.as_deref(), Some("Pets"));
    let a = ImportArgs::parse(&args(&["postman", "--name", "X", "c.json"])).unwrap();
    assert_eq!((a.kind, a.name), (ImportKind::Postman, Some("X".into())));
    let a = ImportArgs::parse(&args(&["openapi", "--name=Y", "a.json"])).unwrap();
    assert_eq!(a.name.as_deref(), Some("Y"));
}

#[test]
fn rejects_bad_arguments() {
    assert!(ImportArgs::parse(&args(&[])).is_err());
    assert!(ImportArgs::parse(&args(&["a.json"])).is_err());
    assert!(ImportArgs::parse(&args(&["--openapi", "a.json"])).is_err());
    assert!(ImportArgs::parse(&args(&["openapi"])).is_err());
    assert!(ImportArgs::parse(&args(&["openapi", "a.json", "-n"])).is_err());
    assert!(ImportArgs::parse(&args(&["openapi", "a.json", "-n", " "])).is_err());
    assert!(ImportArgs::parse(&args(&["openapi", "a.json", "b.json"])).is_err());
    assert!(ImportArgs::parse(&args(&["openapi", "a.json", "--nope"])).is_err());
}

#[test]
fn reads_yaml_and_json_to_the_same_project() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = dir.path().join("api.yaml");
    std::fs::write(
        &yaml,
        "openapi: 3.0.0\ninfo:\n  title: Pets\n  version: '1'\npaths:\n  /pets:\n    get:\n      responses:\n        '200':\n          description: ok\n",
    )
    .unwrap();
    let json = dir.path().join("api.json");
    std::fs::write(&json, serde_json::to_string(&read_document(&yaml).unwrap()).unwrap()).unwrap();
    let a = hozd::model::import(&read_document(&yaml).unwrap()).unwrap();
    let b = hozd::model::import(&read_document(&json).unwrap()).unwrap();
    assert_eq!(a.name, "Pets");
    assert_eq!(a.all_requests().len(), 1);
    assert_eq!(a.name, b.name);
    // YAML in a .json file still reads
    let odd = dir.path().join("odd.json");
    std::fs::copy(&yaml, &odd).unwrap();
    assert!(read_document(&odd).is_ok());
}
