use std::sync::Arc;

use anyhow::Context;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::client::{ReqwestClient, TonicGrpc};
use crate::config::Config;
use crate::history::History;
use crate::paths::Home;
use crate::project::load_projects;
use crate::tui::{App, Msg};

pub mod client;
pub mod config;
pub mod history;
pub mod model;
pub mod paths;
pub mod project;
pub mod tui;
pub mod vars;

/// Starts hozd: resolves the home directory, opens history and runs the TUI.
pub async fn run() -> anyhow::Result<()> {
    let home = Home::resolve()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some("import") = args.first().map(String::as_str) {
        let import = ImportArgs::parse(&args[1..])?;
        return import_project(&home, &import).await;
    }
    let (app, rx) = startup(&home).await?;

    tui::run(app, rx)
        .await
        .context("cannot start the terminal UI; hozd needs an interactive terminal")
}

/// Everything that must succeed before the UI starts: home directory, config,
/// database (created and migrated) and templates.
pub async fn startup(home: &Home) -> anyhow::Result<(App, UnboundedReceiver<Msg>)> {
    home.ensure()?;

    let config = Config::load(&home.config_path())?;
    let history = History::open(&home.db_path())
        .await
        .with_context(|| format!("cannot open history at {}", home.db_path().display()))?;
    let projects = load_projects(&home.projects_dir());
    let (tx, rx) = unbounded_channel();
    let grpc = Arc::new(TonicGrpc::new(config.proto_include_dirs(home.root())));
    let app = App::new(Arc::new(ReqwestClient::new()), grpc, history, projects, tx);

    Ok((app, rx))
}

/// What `hozd import` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    OpenApi,
    Postman,
}

/// Arguments of `hozd import <openapi|postman>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportArgs {
    pub kind: ImportKind,
    pub path: std::path::PathBuf,
    /// Overrides the name stored in the document.
    pub name: Option<String>,
}

const IMPORT_USAGE: &str = "usage: hozd import <openapi|postman> <file> [-n|--name <name>]\n  openapi: OpenAPI 3.x or Swagger 2.0, JSON or YAML\n  postman: Postman collection v2.1, JSON";

impl ImportArgs {
    /// Parses what follows `hozd import`: a subcommand, a file and an optional
    /// `-n`/`--name <name>`.
    pub fn parse(args: &[String]) -> anyhow::Result<Self> {
        let (kind, rest) = match args.split_first() {
            Some((k, rest)) if k == "openapi" => (ImportKind::OpenApi, rest),
            Some((k, rest)) if k == "postman" => (ImportKind::Postman, rest),
            Some((k, _)) => anyhow::bail!("unknown import source `{k}`\n{IMPORT_USAGE}"),
            None => anyhow::bail!("{IMPORT_USAGE}"),
        };
        let mut path = None;
        let mut name = None;
        let mut it = rest.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "-n" | "--name" => {
                    let value = it
                        .next()
                        .with_context(|| format!("{arg} needs a value\n{IMPORT_USAGE}"))?;
                    name = Some(value.clone());
                }
                other => {
                    if let Some(value) = other.strip_prefix("--name=") {
                        name = Some(value.to_owned());
                    } else if other.starts_with('-') {
                        anyhow::bail!("unknown option `{other}`\n{IMPORT_USAGE}");
                    } else {
                        anyhow::ensure!(
                            path.replace(other.to_owned()).is_none(),
                            "only one file can be imported\n{IMPORT_USAGE}"
                        );
                    }
                }
            }
        }
        let path = path.with_context(|| format!("missing file\n{IMPORT_USAGE}"))?;
        if let Some(name) = &name {
            anyhow::ensure!(!name.trim().is_empty(), "the project name cannot be empty");
        }
        Ok(Self { kind, path: path.into(), name })
    }
}

/// Reads a JSON or YAML document; the extension decides, falling back to the other
/// format when it does not parse.
pub fn read_document(path: &std::path::Path) -> anyhow::Result<serde_json::Value> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let yaml_first = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"));
    let json = || serde_json::from_str(&text).ok();
    let yaml = || serde_yaml_ng::from_str::<serde_json::Value>(&text).ok();
    let doc = if yaml_first { yaml().or_else(json) } else { json().or_else(yaml) };
    doc.with_context(|| format!("{} is neither valid JSON nor YAML", path.display()))
}

/// `hozd import`: stores the Postman collection or OpenAPI/Swagger document as a
/// project, shown in the Projects tab.
async fn import_project(home: &Home, args: &ImportArgs) -> anyhow::Result<()> {
    home.ensure()?;
    let doc = read_document(&args.path)?;
    let is_openapi = doc.get("openapi").is_some() || doc.get("swagger").is_some();
    match args.kind {
        ImportKind::OpenApi if !is_openapi => anyhow::bail!(
            "{} is not an OpenAPI or Swagger document (no `openapi` or `swagger` field)",
            args.path.display()
        ),
        ImportKind::Postman if is_openapi => anyhow::bail!(
            "{} is an OpenAPI document; use `hozd import openapi`",
            args.path.display()
        ),
        _ => {}
    }
    let mut project = model::import(&doc).map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(name) = &args.name {
        project.name = name.trim().to_owned();
    }
    let history = History::open(&home.db_path())
        .await
        .with_context(|| format!("cannot open history at {}", home.db_path().display()))?;
    history.save_project(&project).await?;
    println!(
        "imported project \"{}\" ({} requests)",
        project.name,
        project.all_requests().len()
    );
    Ok(())
}
