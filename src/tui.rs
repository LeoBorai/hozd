use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use futures_util::StreamExt;
use ratatui::layout::Rect;
use tokio::sync::mpsc::UnboundedReceiver;

pub use app::{
    App, Focus, GrpcView, HISTORY_LIMIT, LeftTab, LoadedSchema, Msg, Picker, PickerItem,
    PickerOrigin, PickerTag, RequestTab, RequestView, SchemaKey, SchemaMode, StoredTemplate, Tab,
    View,
};
pub use terminal::{RestoreGuard, osc52};

pub mod auth;
pub mod commands;
pub mod complete;
pub mod headers;
pub mod json_editor;
pub mod log;
pub mod modal;
pub mod syntax_highlight;
pub mod ui;

mod app;
mod terminal;

/// Runs the TUI until the user quits. State changes only happen in this loop; requests
/// run in spawned tasks and report back through `rx`, so the UI never blocks on them.
pub async fn run(mut app: App, mut rx: UnboundedReceiver<Msg>) -> std::io::Result<()> {
    let mut terminal = ratatui::try_init()?;
    crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
    // Lets terminals that speak the kitty protocol report Ctrl+Enter distinctly.
    let enhanced = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        crossterm::execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }

    // `ratatui::init` restores the terminal on panic but knows nothing about the mouse.
    let panic_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        panic_hook(info);
    }));

    let _guard = RestoreGuard::new(|| {
        if enhanced {
            let _ = crossterm::execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        ratatui::restore();
    });

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(140));

    while !app.should_quit {
        if let Some(text) = app.take_clipboard() {
            use std::io::Write;
            let mut out = std::io::stdout();
            out.write_all(osc52(&text, std::env::var_os("TMUX").is_some()).as_bytes())?;
            out.flush()?;
        }
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
            _ = ticker.tick(), if app.sending => app.tick(),
            else => break,
        }
    }
    Ok(())
}
