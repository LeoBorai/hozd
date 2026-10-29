//! The TOML examples in the README must be valid templates or settings.

use hozd::config::Config;
use hozd::project::{Template, TemplateKind};

fn toml_blocks() -> Vec<String> {
    let readme =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in readme.lines() {
        match (&mut current, line.trim_end()) {
            (None, "```toml") => current = Some(String::new()),
            (Some(_), "```") => blocks.extend(current.take()),
            (Some(block), text) => {
                block.push_str(text);
                block.push('\n');
            }
            _ => {}
        }
    }
    blocks
}

#[test]
fn every_documented_toml_example_loads() {
    let blocks = toml_blocks();
    let (mut http, mut grpc, mut settings) = (0, 0, 0);
    for block in &blocks {
        if block.contains("proto_include") {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, block).unwrap();
            let config = Config::load(&path).unwrap_or_else(|e| panic!("{e}\n{block}"));
            assert!(!config.proto_include.is_empty());
            settings += 1;
        } else {
            let template = Template::parse(block).unwrap_or_else(|e| panic!("{e}\n{block}"));
            match template.kind {
                TemplateKind::Http(_) => http += 1,
                TemplateKind::Grpc(_) => grpc += 1,
            }
        }
    }
    assert_eq!((http, grpc, settings), (1, 1, 1), "{} blocks", blocks.len());
}
