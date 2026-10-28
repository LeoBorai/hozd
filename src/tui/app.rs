use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui_textarea::{Input, TextArea};
use tokio::sync::mpsc::UnboundedSender;

use crate::client::{Header, Method, Request, SharedClient};
use crate::history::{History, HistoryEntry, Outcome, Summary};
use crate::project::{Loaded, Template};

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
    /// Headers of the loaded template/history entry; carried along, not editable yet.
    pub headers: Vec<Header>,
    pub live: Option<Outcome>,
    pub preview: Option<HistoryEntry>,
    pub sending: bool,
    pub status: Option<String>,
    pub scroll: u16,
    pub should_quit: bool,

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
            headers: Vec::new(),
            live: None,
            preview: None,
            sending: false,
            status,
            scroll: 0,
            should_quit: false,
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

    pub fn editor_request(&self) -> Request {
        let body = self.body_text();
        Request {
            method: self.method,
            url: self.url_text().trim().to_owned(),
            headers: self.headers.clone(),
            body: (!body.is_empty()).then_some(body),
        }
    }

    pub fn selected_summary(&self) -> Option<&Summary> {
        self.history.get(self.history_sel)
    }

    pub fn selected_template(&self) -> Option<&StoredTemplate> {
        self.templates.get(self.stored_sel)
    }

    pub fn view(&self) -> View<'_> {
        if self.focus == Focus::List {
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
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match (key.code, ctrl) {
            (KeyCode::Char('c'), true) => return self.should_quit = true,
            (KeyCode::Char('s'), true) => return self.send(),
            (KeyCode::Char('n'), true) => return self.method = self.method.next(),
            (KeyCode::Char('p'), true) => return self.method = self.method.prev(),
            (KeyCode::PageDown, _) => return self.scroll = self.scroll.saturating_add(5),
            (KeyCode::PageUp, _) => return self.scroll = self.scroll.saturating_sub(5),
            (KeyCode::Tab, _) => return self.set_focus(next_focus(self.focus)),
            (KeyCode::BackTab, _) => return self.set_focus(prev_focus(self.focus)),
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
        }
    }

    fn list_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('[') | KeyCode::Left | KeyCode::Char('h') => self.set_tab(Tab::Stored),
            KeyCode::Char(']') | KeyCode::Right | KeyCode::Char('l') => self.set_tab(Tab::History),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Enter => self.activate(),
            _ => {}
        }
    }

    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.scroll = 0;
        self.request_preview();
    }

    fn set_focus(&mut self, focus: Focus) {
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
        self.headers = request.headers.clone();
    }

    // ---- background work ------------------------------------------------

    pub fn send(&mut self) {
        if self.sending {
            return;
        }
        let request = self.editor_request();
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

fn next_focus(f: Focus) -> Focus {
    match f {
        Focus::List => Focus::Url,
        Focus::Url => Focus::Body,
        Focus::Body => Focus::List,
    }
}

fn prev_focus(f: Focus) -> Focus {
    match f {
        Focus::List => Focus::Body,
        Focus::Url => Focus::List,
        Focus::Body => Focus::Url,
    }
}

fn single_line(text: &str) -> TextArea<'static> {
    let mut ta = TextArea::new(vec![text.to_owned()]);
    ta.move_cursor(ratatui_textarea::CursorMove::End);
    ta
}
