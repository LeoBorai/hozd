mod app;
pub mod json;
mod terminal;
pub mod ui;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream};
use futures_util::StreamExt;
use ratatui::layout::Rect;
use tokio::sync::mpsc::UnboundedReceiver;

pub use app::{App, Focus, HISTORY_LIMIT, Msg, RequestView, StoredTemplate, Tab, View};
pub use terminal::RestoreGuard;

/// Runs the TUI until the user quits. State changes only happen in this loop; requests
/// run in spawned tasks and report back through `rx`, so the UI never blocks on them.
pub async fn run(mut app: App, mut rx: UnboundedReceiver<Msg>) -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
    // `ratatui::init` restores the terminal on panic but knows nothing about the mouse.
    let panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        panic_hook(info);
    }));
    let _guard = RestoreGuard::new(|| {
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        ratatui::restore();
    });
    let mut events = EventStream::new();

    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &app))?;
        tokio::select! {
            Some(event) = events.next() => {
                match event? {
                    Event::Key(key) => app.handle_key(key),
                    Event::Mouse(mouse) => {
                        let size = terminal.size()?;
                        app.handle_mouse(mouse, Rect::new(0, 0, size.width, size.height));
                    }
                    _ => {}
                }
            }
            Some(msg) = rx.recv() => app.update(msg),
            else => break,
        }
    }
    Ok(())
}
