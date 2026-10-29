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
pub mod paths;
pub mod project;
pub mod tui;

/// Starts hozd: resolves the home directory, opens history and runs the TUI.
pub async fn run() -> anyhow::Result<()> {
    let home = Home::resolve()?;
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
