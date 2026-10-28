use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui_textarea::{CursorMove, Input, TextArea};
use tokio::sync::mpsc::UnboundedSender;

use super::headers;
use super::json::pretty;
use super::modal::Modal;
use super::ui;
use crate::client::{Header, Method, Request, SharedClient};
use crate::history::{History, HistoryEntry, Outcome, Summary};
use crate::project::{Loaded, Template};

/// Two clicks on the response body closer together than this open it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// How many history rows the left panel keeps.
pub const HISTORY_LIMIT: u32 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Stored,
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Url,
    Body,
    Headers,
    /// The response body box; `Enter` opens it in the reading panel.
    Response,
}

/// Which editor the request's lower box shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestTab {
    Body,
    Headers,
}

/// Results of background work, applied to the state by [`App::update`].
#[derive(Debug)]
pub enum Msg {
    Sent(Outcome),
    History(Vec<Summary>),
    Preview(Box<HistoryEntry>),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct StoredTemplate {
    pub project: String,
    pub path: PathBuf,
    pub template: Template,
}

/// What the center and right panels show right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestView {
    pub method: Method,
    pub url: String,
    pub headers: Vec<Header>,
    pub body: String,
}

impl From<&Request> for RequestView {
    fn from(r: &Request) -> Self {
        Self {
            method: r.method,
            url: r.url.clone(),
            headers: r.headers.clone(),
            body: r.body.clone().unwrap_or_default(),
        }
    }
}

pub struct View<'a> {
    pub request: RequestView,
    pub outcome: Option<&'a Outcome>,
    /// True while the panels mirror a hovered entry instead of the editor.
    pub previewing: bool,
}

pub struct App {
    pub tab: Tab,
    pub focus: Focus,
    pub templates: Vec<StoredTemplate>,
    pub stored_sel: usize,
    pub history: Vec<Summary>,
    pub history_sel: usize,
    pub method: Method,
    pub url: TextArea<'static>,
    pub body: TextArea<'static>,
    /// One `Name: value` per line, see [`headers`].
    pub headers: TextArea<'static>,
    pub request_tab: RequestTab,
    pub live: Option<Outcome>,
    pub preview: Option<HistoryEntry>,
    pub sending: bool,
    pub status: Option<String>,
    /// A non-error message for the status bar, cleared by the next key press.
    pub notice: Option<String>,
    clipboard: Option<String>,
    /// The response body open in the big reading panel.
    pub modal: Option<Modal>,
    /// Set when the response box got focus while previewing a hovered entry, so the
    /// panels keep showing that entry.
    inspect_preview: bool,
    last_response_click: Option<Instant>,
    pub scroll: u16,
    pub should_quit: bool,
    /// The list item the last click landed on; clicking it again loads it.
    last_click: Option<(Tab, usize)>,

    client: SharedClient,
    store: History,
    tx: UnboundedSender<Msg>,
}

impl App {
    pub fn new(
        client: SharedClient,
        store: History,
        projects: Loaded,
        tx: UnboundedSender<Msg>,
    ) -> Self {
        let status = (!projects.issues.is_empty()).then(|| {
            let first = &projects.issues[0];
            format!(
                "{} template(s) failed to load, e.g. {first}",
                projects.issues.len()
            )
        });
        let templates = projects
            .projects
            .into_iter()
            .flat_map(|p| {
                let name = p.name;
                p.templates.into_iter().map(move |t| StoredTemplate {
                    project: name.clone(),
                    path: t.path,
                    template: t.template,
                })
            })
            .collect();

        let app = Self {
            tab: Tab::Stored,
            focus: Focus::List,
            templates,
            stored_sel: 0,
            history: Vec::new(),
            history_sel: 0,
            method: Method::Get,
            url: single_line(""),
            body: TextArea::default(),
            headers: TextArea::default(),
            request_tab: RequestTab::Body,
            live: None,
            preview: None,
            sending: false,
            status,
            notice: None,
            clipboard: None,
            modal: None,
            inspect_preview: false,
            last_response_click: None,
            scroll: 0,
            should_quit: false,
            last_click: None,
            client,
            store,
            tx,
        };
        app.refresh_history();
        app
    }

    // ---- derived state -------------------------------------------------

    pub fn url_text(&self) -> String {
        self.url.lines().join("")
    }

    pub fn body_text(&self) -> String {
        self.body.lines().join("\n")
    }

    /// The request as edited; header lines that are not `Name: value` are skipped.
    pub fn editor_request(&self) -> Request {
        self.build_request(headers::parse_lenient(self.headers.lines()))
    }

    /// Like [`editor_request`](Self::editor_request) but fails on an invalid header line.
    pub fn try_request(&self) -> Result<Request, String> {
        Ok(self.build_request(headers::parse(self.headers.lines())?))
    }

    fn build_request(&self, headers: Vec<Header>) -> Request {
        let body = self.body_text();
        Request {
            method: self.method,
            url: self.url_text().trim().to_owned(),
            headers,
            body: (!body.is_empty()).then_some(body),
        }
    }

    /// The focus value of the editor the request's lower box currently shows.
    pub fn editor_focus(&self) -> Focus {
        match self.request_tab {
            RequestTab::Body => Focus::Body,
            RequestTab::Headers => Focus::Headers,
        }
    }

    pub fn set_request_tab(&mut self, tab: RequestTab) {
        self.request_tab = tab;
        if matches!(self.focus, Focus::Body | Focus::Headers) {
            self.focus = self.editor_focus();
        }
    }

    pub fn selected_summary(&self) -> Option<&Summary> {
        self.history.get(self.history_sel)
    }

    pub fn selected_template(&self) -> Option<&StoredTemplate> {
        self.templates.get(self.stored_sel)
    }

    pub fn view(&self) -> View<'_> {
        if self.focus == Focus::List || (self.focus == Focus::Response && self.inspect_preview) {
            match self.tab {
                Tab::History => {
                    if let (Some(sum), Some(entry)) = (self.selected_summary(), &self.preview)
                        && entry.id == sum.id
                    {
                        return View {
                            request: (&entry.request).into(),
                            outcome: Some(&entry.outcome),
                            previewing: true,
                        };
                    }
                }
                Tab::Stored => {
                    if let Some(t) = self.selected_template() {
                        return View {
                            request: (&t.template.to_request()).into(),
                            outcome: self.live.as_ref(),
                            previewing: true,
                        };
                    }
                }
            }
        }
        View {
            request: (&self.editor_request()).into(),
            outcome: self.live.as_ref(),
            previewing: false,
        }
    }

    // ---- background results -------------------------------------------

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Sent(outcome) => {
                self.sending = false;
                self.scroll = 0;
                self.live = Some(outcome);
            }
            Msg::History(list) => {
                self.history = list;
                self.history_sel = self.history_sel.min(self.history.len().saturating_sub(1));
                self.request_preview();
            }
            Msg::Preview(entry) => {
                if self.selected_summary().is_some_and(|s| s.id == entry.id) {
                    self.scroll = 0;
                    self.preview = Some(*entry);
                }
            }
            Msg::Failed(message) => {
                self.sending = false;
                self.status = Some(message);
            }
        }
    }

    // ---- input ---------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        self.notice = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.modal.is_some() {
            return self.modal_key(key.code, ctrl);
        }

        match (key.code, ctrl) {
            (KeyCode::Char('c'), true) => return self.should_quit = true,
            (KeyCode::Char('s'), true) => return self.send(),
            (KeyCode::Char('n'), true) => return self.method = self.method.next(),
            (KeyCode::Char('p'), true) => return self.method = self.method.prev(),
            (KeyCode::Char('t'), true) => return self.toggle_request_tab(),
            (KeyCode::PageDown, _) => return self.scroll = self.scroll.saturating_add(5),
            (KeyCode::PageUp, _) => return self.scroll = self.scroll.saturating_sub(5),
            (KeyCode::Tab, _) => {
                return self.set_focus(next_focus(self.focus, self.editor_focus()));
            }
            (KeyCode::BackTab, _) => {
                return self.set_focus(prev_focus(self.focus, self.editor_focus()));
            }
            (KeyCode::Esc, _) => return self.set_focus(Focus::List),
            _ => {}
        }

        match self.focus {
            Focus::List => self.list_key(key.code),
            Focus::Url => match key.code {
                KeyCode::Enter => self.send(),
                _ => {
                    self.url.input(Input::from(key));
                }
            },
            Focus::Body => {
                self.body.input(Input::from(key));
            }
            Focus::Headers => {
                self.headers.input(Input::from(key));
            }
            Focus::Response => self.response_key(key.code),
        }
    }

    fn response_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => self.open_modal(),
            KeyCode::Char('y') => self.copy_response(),
            KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            _ => {}
        }
    }

    // ---- reading panel ---------------------------------------------------

    /// Opens the response body shown in the right panel in the big reading panel.
    pub fn open_modal(&mut self) {
        let text = match self.view().outcome {
            Some(Outcome::Response(r)) => r.body.clone(),
            Some(Outcome::Error(message)) => message.clone(),
            None => {
                self.notice = Some("no response to open".into());
                return;
            }
        };
        if text.is_empty() {
            self.notice = Some("the response body is empty".into());
            return;
        }
        self.modal = Some(Modal::new(text));
    }

    fn modal_key(&mut self, code: KeyCode, ctrl: bool) {
        let Some(modal) = self.modal.as_mut() else {
            return;
        };
        match (code, ctrl) {
            (KeyCode::Char('c'), true) => self.should_quit = true,
            (KeyCode::Esc | KeyCode::Char('q'), _) => self.modal = None,
            (KeyCode::Char('f' | 'F'), _) => {
                if let Err(message) = modal.toggle_format() {
                    self.notice = Some(message);
                }
            }
            (KeyCode::Char('y'), _) => {
                let text = modal.displayed().to_owned();
                self.notice = Some(format!("copied {} bytes to the clipboard", text.len()));
                self.clipboard = Some(text);
            }
            (KeyCode::Down | KeyCode::Char('j'), _) => modal.scroll_by(1),
            (KeyCode::Up | KeyCode::Char('k'), _) => modal.scroll_by(-1),
            (KeyCode::PageDown | KeyCode::Char(' '), _) => modal.scroll_page(1),
            (KeyCode::PageUp | KeyCode::Char('b'), _) => modal.scroll_page(-1),
            (KeyCode::Home | KeyCode::Char('g'), _) => modal.scroll = 0,
            (KeyCode::End | KeyCode::Char('G'), _) => modal.scroll_to_end(),
            _ => {}
        }
    }

    fn toggle_request_tab(&mut self) {
        self.set_request_tab(match self.request_tab {
            RequestTab::Body => RequestTab::Headers,
            RequestTab::Headers => RequestTab::Body,
        });
    }

    fn list_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('y') => self.copy_response(),
            KeyCode::Char('[') | KeyCode::Left | KeyCode::Char('h') => self.set_tab(Tab::Stored),
            KeyCode::Char(']') | KeyCode::Right | KeyCode::Char('l') => self.set_tab(Tab::History),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Enter => self.activate(),
            _ => {}
        }
    }

    // ---- clipboard -----------------------------------------------------

    /// Queues the response body shown in the right panel (pretty-printed when it is JSON)
    /// for the clipboard; the event loop writes it to the terminal.
    pub fn copy_response(&mut self) {
        let text = match self.view().outcome {
            Some(Outcome::Response(r)) => pretty(&r.body),
            Some(Outcome::Error(message)) => message.clone(),
            None => String::new(),
        };
        if text.is_empty() {
            self.notice = Some("nothing to copy".into());
        } else {
            self.notice = Some(format!("copied {} bytes to the clipboard", text.len()));
            self.clipboard = Some(text);
        }
    }

    /// Text waiting to be put on the clipboard, if a copy was requested.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard.take()
    }

    // ---- mouse ----------------------------------------------------------

    /// `screen` is the full terminal area the UI was last drawn into.
    pub fn handle_mouse(&mut self, event: MouseEvent, screen: Rect) {
        let areas = ui::areas(screen);
        let at = Position::new(event.column, event.row);
        if let Some(modal) = self.modal.as_mut() {
            match event.kind {
                MouseEventKind::ScrollDown => modal.scroll_by(3),
                MouseEventKind::ScrollUp => modal.scroll_by(-3),
                MouseEventKind::Down(MouseButton::Left) if !ui::modal_area(screen).contains(at) => {
                    self.modal = None;
                }
                _ => {}
            }
            return;
        }
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => self.click(at, &areas),
            MouseEventKind::Down(MouseButton::Right) if areas.method.contains(at) => {
                self.focus_editor(Focus::Url);
                self.method = self.method.prev();
            }
            MouseEventKind::Down(MouseButton::Right) if areas.response_body.contains(at) => {
                self.copy_response()
            }
            MouseEventKind::ScrollDown => self.wheel(at, &areas, 1),
            MouseEventKind::ScrollUp => self.wheel(at, &areas, -1),
            _ => {}
        }
    }

    fn click(&mut self, at: Position, areas: &ui::Areas) {
        if areas.tabs.contains(at) {
            if let Some(tab) = ui::tab_at(areas.tabs, at.x) {
                self.focus = Focus::List;
                self.set_tab(tab);
            }
        } else if areas.list.contains(at) {
            self.click_list(at.y - areas.list.y, areas.list.height);
        } else if areas.method.contains(at) {
            self.focus_editor(Focus::Url);
            self.method = self.method.next();
        } else if areas.url.contains(at) {
            self.focus_editor(Focus::Url);
            let inner = ui::inner(areas.url);
            if inner.contains(at) {
                let (_, col_scroll) = ui::editor_scroll(cursor_of(&self.url), inner);
                self.url
                    .move_cursor(CursorMove::Jump(0, (at.x - inner.x) + col_scroll as u16));
            }
        } else if areas.response_body.contains(at) {
            self.set_focus(Focus::Response);
            let now = Instant::now();
            if self
                .last_response_click
                .is_some_and(|t| now.duration_since(t) <= DOUBLE_CLICK)
            {
                self.last_response_click = None;
                self.open_modal();
            } else {
                self.last_response_click = Some(now);
            }
        } else if areas.body.contains(at) {
            if let Some(tab) = ui::request_tab_at(areas.body, at, self.view().request.headers.len())
            {
                self.focus_editor(self.editor_focus());
                self.set_request_tab(tab);
                self.focus = self.editor_focus();
                return;
            }
            self.focus_editor(self.editor_focus());
            let inner = ui::inner(areas.body);
            if inner.contains(at) {
                let ta = match self.request_tab {
                    RequestTab::Body => &mut self.body,
                    RequestTab::Headers => &mut self.headers,
                };
                let (row_scroll, col_scroll) = ui::editor_scroll(cursor_of(ta), inner);
                ta.move_cursor(CursorMove::Jump(
                    (at.y - inner.y) + row_scroll as u16,
                    (at.x - inner.x) + col_scroll as u16,
                ));
            }
        }
    }

    /// Clicking an item selects (and previews) it; clicking it again loads it.
    fn click_list(&mut self, y_in_list: u16, height: u16) {
        let item = match self.tab {
            Tab::Stored => {
                let rows = ui::stored_row_map(self);
                let selected_row = rows.iter().position(|r| *r == Some(self.stored_sel));
                let offset = ui::list_offset(selected_row, height);
                rows.get(y_in_list as usize + offset).copied().flatten()
            }
            Tab::History => {
                let selected_row = (!self.history.is_empty()).then_some(self.history_sel);
                let row = y_in_list as usize + ui::list_offset(selected_row, height);
                (row < self.history.len()).then_some(row)
            }
        };
        let Some(item) = item else {
            return;
        };

        self.focus = Focus::List;
        if self.last_click == Some((self.tab, item)) {
            self.last_click = None;
            self.select(item);
            self.activate();
        } else {
            self.select(item);
            self.last_click = Some((self.tab, item));
        }
    }

    fn select(&mut self, item: usize) {
        match self.tab {
            Tab::Stored => self.stored_sel = item,
            Tab::History => self.history_sel = item,
        }
        self.scroll = 0;
        self.request_preview();
    }

    fn wheel(&mut self, at: Position, areas: &ui::Areas, direction: i16) {
        if areas.list.contains(at) {
            self.move_selection(direction as isize);
        } else if areas.response_body.contains(at) {
            self.scroll = self.scroll.saturating_add_signed(direction * 3);
        }
    }

    /// Moves focus into the editor. If the panels were previewing a hovered entry, that
    /// entry is loaded first, so clicking into a preview edits what you were looking at.
    fn focus_editor(&mut self, focus: Focus) {
        if matches!(self.focus, Focus::List | Focus::Response) && self.view().previewing {
            self.activate();
        }
        self.set_focus(focus);
    }

    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.last_click = None;
        self.scroll = 0;
        self.request_preview();
    }

    fn set_focus(&mut self, focus: Focus) {
        if focus == Focus::Response && self.focus != Focus::Response {
            self.inspect_preview = self.view().previewing;
        }
        self.focus = focus;
        self.scroll = 0;
        self.request_preview();
    }

    fn move_selection(&mut self, delta: isize) {
        let (sel, len) = match self.tab {
            Tab::Stored => (&mut self.stored_sel, self.templates.len()),
            Tab::History => (&mut self.history_sel, self.history.len()),
        };
        if len == 0 {
            return;
        }
        *sel = sel.saturating_add_signed(delta).min(len - 1);
        self.last_click = None;
        self.scroll = 0;
        self.request_preview();
    }

    /// Copies the selected entry into the editor so it can be modified and sent.
    fn activate(&mut self) {
        let request = match self.tab {
            Tab::Stored => self.selected_template().map(|t| t.template.to_request()),
            Tab::History => match (self.selected_summary(), &self.preview) {
                (Some(s), Some(e)) if e.id == s.id => Some(e.request.clone()),
                _ => None,
            },
        };
        if let Some(request) = request {
            self.load_request(&request);
            self.set_focus(Focus::Url);
        }
    }

    pub fn load_request(&mut self, request: &Request) {
        self.method = request.method;
        self.url = single_line(&request.url);
        self.body = request
            .body
            .as_deref()
            .map(|b| TextArea::new(b.lines().map(str::to_owned).collect()))
            .unwrap_or_default();
        self.headers = match headers::to_lines(&request.headers) {
            lines if lines.is_empty() => TextArea::default(),
            lines => TextArea::new(lines),
        };
    }

    // ---- background work ------------------------------------------------

    pub fn send(&mut self) {
        if self.sending {
            return;
        }
        let request = match self.try_request() {
            Ok(request) => request,
            Err(message) => {
                // show the offending line in red
                self.status = Some(message);
                self.request_tab = RequestTab::Headers;
                self.set_focus(Focus::Headers);
                return;
            }
        };
        if request.url.is_empty() {
            self.status = Some("enter a URL first".into());
            return;
        }
        self.sending = true;
        self.status = None;

        let (client, store, tx) = (self.client.clone(), self.store.clone(), self.tx.clone());
        tokio::spawn(async move {
            let outcome: Outcome = client.send(&request).await.into();
            if let Err(e) = store.record(&request, &outcome).await {
                let _ = tx.send(Msg::Failed(format!("could not save history: {e}")));
            }
            let _ = tx.send(Msg::Sent(outcome));
            match store.list(HISTORY_LIMIT).await {
                Ok(list) => {
                    let _ = tx.send(Msg::History(list));
                }
                Err(e) => {
                    let _ = tx.send(Msg::Failed(format!("could not read history: {e}")));
                }
            }
        });
    }

    pub fn refresh_history(&self) {
        let (store, tx) = (self.store.clone(), self.tx.clone());
        tokio::spawn(async move {
            let msg = match store.list(HISTORY_LIMIT).await {
                Ok(list) => Msg::History(list),
                Err(e) => Msg::Failed(format!("could not read history: {e}")),
            };
            let _ = tx.send(msg);
        });
    }

    /// Fetches the full entry under the history selection so the panels can show it.
    /// Never sends anything or writes history.
    pub fn request_preview(&self) {
        if self.focus != Focus::List || self.tab != Tab::History {
            return;
        }
        let Some(id) = self.selected_summary().map(|s| s.id) else {
            return;
        };
        if self.preview.as_ref().is_some_and(|e| e.id == id) {
            return;
        }
        let (store, tx) = (self.store.clone(), self.tx.clone());
        tokio::spawn(async move {
            let msg = match store.get(id).await {
                Ok(Some(entry)) => Msg::Preview(Box::new(entry)),
                Ok(None) => return,
                Err(e) => Msg::Failed(format!("could not read history entry: {e}")),
            };
            let _ = tx.send(msg);
        });
    }
}

fn next_focus(f: Focus, editor: Focus) -> Focus {
    match f {
        Focus::List => Focus::Url,
        Focus::Url => editor,
        Focus::Body | Focus::Headers => Focus::Response,
        Focus::Response => Focus::List,
    }
}

fn prev_focus(f: Focus, editor: Focus) -> Focus {
    match f {
        Focus::List => Focus::Response,
        Focus::Url => Focus::List,
        Focus::Body | Focus::Headers => Focus::Url,
        Focus::Response => editor,
    }
}

fn single_line(text: &str) -> TextArea<'static> {
    let mut ta = TextArea::new(vec![text.to_owned()]);
    ta.move_cursor(ratatui_textarea::CursorMove::End);
    ta
}

fn cursor_of(ta: &TextArea<'static>) -> (usize, usize) {
    let c = ta.cursor();
    (c.0, c.1)
}
