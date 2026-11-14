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
        let [_, path] = args.as_slice() else {
            anyhow::bail!("usage: hozd import <postman|openapi|swagger file>");
        };
        return import_project(&home, std::path::Path::new(path)).await;
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

/// `hozd import <file>`: stores the Postman collection or OpenAPI/Swagger document at
/// `path` as a project, shown in the Projects tab.
async fn import_project(home: &Home, path: &std::path::Path) -> anyhow::Result<()> {
    home.ensure()?;
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let doc: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("{} is not JSON", path.display()))?;
    let project = model::import(&doc).map_err(|e| anyhow::anyhow!("{e}"))?;
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
