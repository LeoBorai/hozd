mod app;
pub mod json;
mod terminal;
pub mod ui;

use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedReceiver;

pub use app::{App, Focus, HISTORY_LIMIT, Msg, RequestView, StoredTemplate, Tab, View};
pub use terminal::RestoreGuard;

/// Runs the TUI until the user quits. State changes only happen in this loop; requests
/// run in spawned tasks and report back through `rx`, so the UI never blocks on them.
pub async fn run(mut app: App, mut rx: UnboundedReceiver<Msg>) -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let _guard = RestoreGuard::new(ratatui::restore);
    let mut events = EventStream::new();

    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &app))?;
        tokio::select! {
            Some(event) = events.next() => {
                if let Event::Key(key) = event? {
                    app.handle_key(key);
                }
            }
            Some(msg) = rx.recv() => app.update(msg),
            else => break,
        }
    }
    Ok(())
}
