use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui_textarea::{CursorMove, Input, TextArea};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use super::headers;
use super::json::pretty;
use super::log::{Level, Log};
use super::modal::Modal;
use super::ui;
use crate::client::{
    CallEvent, CallKind, GrpcOutcome, GrpcRequest, GrpcStatus, Header, HttpRequest, Kind, Method,
    Request, Schema, SchemaSource, SharedGrpc, SharedHttp, Trace,
};
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
    /// gRPC editor: the schema source box.
    Schema,
    /// gRPC editor: the method box; `Enter` opens the picker.
    Method,
    /// The response body box; `Enter` opens it in the reading panel.
    Response,
}

/// Which editor the request's lower box shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestTab {
    Body,
    Headers,
}

/// What a loaded schema was loaded for; it is dropped when the editor stops matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaKey {
    pub address: String,
    pub source: SchemaSource,
    pub insecure: bool,
}

#[derive(Debug, Clone)]
pub struct LoadedSchema {
    pub key: SchemaKey,
    pub schema: Schema,
}

/// The open method picker: a filter over the loaded schema's methods.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Picker {
    pub filter: String,
    /// Index into [`App::picker_matches`].
    pub selected: usize,
}

/// Where the gRPC editor takes the schema from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaMode {
    Reflection,
    File,
}

/// Results of background work, applied to the state by [`App::update`].
#[derive(Debug)]
pub enum Msg {
    Sent(Outcome),
    /// A gRPC call made progress; the last event of a call is `Done`.
    GrpcEvent(CallEvent),
    /// The result of a schema load started for `SchemaKey`.
    SchemaLoaded(SchemaKey, Result<Schema, String>),
    History(Vec<Summary>),
    Preview(Box<HistoryEntry>),
    /// A line of the connection trace of the request in flight.
    Trace(String),
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
    pub kind: Kind,
    /// The HTTP method; unused for gRPC.
    pub method: Method,
    /// The URL, or the server address for gRPC.
    pub url: String,
    /// The headers, or the metadata for gRPC.
    pub headers: Vec<Header>,
    pub body: String,
    pub grpc: GrpcView,
}

/// The gRPC-only part of a [`RequestView`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GrpcView {
    /// `package.Service/Method`, empty when none is selected.
    pub method: String,
    pub schema: SchemaSource,
    pub insecure: bool,
}

impl From<&HttpRequest> for RequestView {
    fn from(r: &HttpRequest) -> Self {
        Self {
            kind: Kind::Http,
            method: r.method,
            url: r.url.clone(),
            headers: r.headers.clone(),
            body: r.body.clone().unwrap_or_default(),
            grpc: GrpcView::default(),
        }
    }
}

impl From<&Request> for RequestView {
    fn from(r: &Request) -> Self {
        match r {
            Request::Http(r) => r.into(),
            Request::Grpc(r) => Self {
                kind: Kind::Grpc,
                method: Method::default(),
                url: r.address.clone(),
                headers: r.metadata.clone(),
                body: r.body.clone().unwrap_or_default(),
                grpc: GrpcView {
                    method: r.method.clone(),
                    schema: r.schema.clone(),
                    insecure: r.insecure,
                },
            },
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
    /// Whether the editor builds an HTTP or a gRPC request.
    pub kind: Kind,
    pub method: Method,
    /// The URL; the server address in gRPC mode.
    pub url: TextArea<'static>,
    pub body: TextArea<'static>,
    /// One `Name: value` per line, see [`headers`].
    pub headers: TextArea<'static>,
    pub request_tab: RequestTab,
    /// gRPC editor: whether the schema comes from reflection or from `schema_path`.
    pub schema_mode: SchemaMode,
    /// gRPC editor: the `.proto` or descriptor set file used when `schema_mode` is `File`.
    pub schema_path: TextArea<'static>,
    /// gRPC editor: skip TLS certificate verification.
    pub insecure: bool,
    /// gRPC editor: the `package.Service/Method` to call.
    pub selected_method: Option<String>,
    /// The schema loaded for the gRPC editor, if any.
    pub schema: Option<LoadedSchema>,
    pub schema_loading: bool,
    /// The method picker, when open.
    pub picker: Option<Picker>,
    pub live: Option<Outcome>,
    pub preview: Option<HistoryEntry>,
    /// A request (or its schema load) is in flight.
    pub sending: bool,
    /// Cancels the in-flight gRPC call; `None` when nothing cancellable is running.
    cancel: Option<CancellationToken>,
    pub status: Option<String>,
    /// What happened, newest last; shown under the request list.
    pub log: Log,
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

    client: SharedHttp,
    grpc: SharedGrpc,
    store: History,
    tx: UnboundedSender<Msg>,
}

impl App {
    pub fn new(
        client: SharedHttp,
        grpc: SharedGrpc,
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
            kind: Kind::Http,
            method: Method::Get,
            url: single_line(""),
            body: TextArea::default(),
            headers: TextArea::default(),
            request_tab: RequestTab::Body,
            schema_mode: SchemaMode::Reflection,
            schema_path: single_line(""),
            insecure: false,
            selected_method: None,
            schema: None,
            schema_loading: false,
            picker: None,
            live: None,
            preview: None,
            sending: false,
            cancel: None,
            status,
            log: Log::default(),
            notice: None,
            clipboard: None,
            modal: None,
            inspect_preview: false,
            last_response_click: None,
            scroll: 0,
            should_quit: false,
            last_click: None,
            client,
            grpc,
            store,
            tx,
        };
        let mut app = app;
        if let Some(issue) = app.status.clone() {
            app.log.push(Level::Error, issue);
        }
        app.log.push(Level::Info, "ready");
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
    pub fn editor_request(&self) -> HttpRequest {
        self.build_request(headers::parse_lenient(self.headers.lines()))
    }

    /// Like [`editor_request`](Self::editor_request) but fails on an invalid header line.
    pub fn try_request(&self) -> Result<HttpRequest, String> {
        Ok(self.build_request(headers::parse(self.headers.lines())?))
    }

    fn build_request(&self, headers: Vec<Header>) -> HttpRequest {
        let body = self.body_text();
        HttpRequest {
            method: self.method,
            url: self.url_text().trim().to_owned(),
            headers,
            body: (!body.is_empty()).then_some(body),
        }
    }

    pub fn schema_path_text(&self) -> String {
        self.schema_path.lines().join("")
    }

    /// The schema source the gRPC editor currently describes.
    pub fn schema_source(&self) -> SchemaSource {
        match self.schema_mode {
            SchemaMode::Reflection => SchemaSource::Reflection,
            SchemaMode::File => SchemaSource::File(PathBuf::from(self.schema_path_text().trim())),
        }
    }

    /// One line about the loaded schema for the Schema box.
    pub fn schema_status(&self) -> String {
        if self.schema_loading {
            return "loading...".into();
        }
        match &self.schema {
            Some(loaded) if loaded.key == self.schema_key() => counts(&loaded.schema),
            _ => "not loaded".into(),
        }
    }

    /// What a schema loaded now would be for.
    pub fn schema_key(&self) -> SchemaKey {
        SchemaKey {
            address: self.url_text().trim().to_owned(),
            source: self.schema_source(),
            insecure: self.insecure,
        }
    }

    /// The call kind of `method` (`package.Service/Method`) in the loaded schema.
    pub fn method_kind(&self, method: &str) -> Option<CallKind> {
        self.schema
            .as_ref()?
            .schema
            .methods()
            .iter()
            .find(|m| m.full_name() == method)
            .map(|m| m.kind)
    }

    /// The gRPC request as edited; metadata lines that are not `Name: value` are skipped.
    pub fn editor_grpc_request(&self) -> GrpcRequest {
        self.build_grpc_request(headers::parse_lenient(self.headers.lines()))
    }

    /// Like [`editor_grpc_request`](Self::editor_grpc_request) but fails on an invalid line.
    pub fn try_grpc_request(&self) -> Result<GrpcRequest, String> {
        Ok(self.build_grpc_request(headers::parse(self.headers.lines())?))
    }

    fn build_grpc_request(&self, metadata: Vec<Header>) -> GrpcRequest {
        let body = self.body_text();
        GrpcRequest {
            address: self.url_text().trim().to_owned(),
            method: self.selected_method.clone().unwrap_or_default(),
            schema: self.schema_source(),
            insecure: self.insecure,
            metadata,
            body: (!body.is_empty()).then_some(body),
        }
    }

    /// The editor's request, of the kind the editor is in.
    pub fn editor_view(&self) -> RequestView {
        match self.kind {
            Kind::Http => (&self.editor_request()).into(),
            Kind::Grpc => (&Request::Grpc(self.editor_grpc_request())).into(),
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
            request: self.editor_view(),
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
                self.log_outcome(&outcome);
                self.live = Some(outcome);
            }
            Msg::GrpcEvent(event) => self.apply_event(event),
            Msg::SchemaLoaded(key, result) => {
                self.schema_loading = false;
                match result {
                    Ok(schema) => {
                        let loaded = format!("loaded {}", counts(&schema));
                        self.log.push(Level::Info, loaded.clone());
                        self.notice = Some(loaded);
                        self.schema = Some(LoadedSchema { key, schema });
                        // A call waiting for this schema learns whether its response streams.
                        let streams = self
                            .selected_method
                            .as_deref()
                            .and_then(|m| self.method_kind(m))
                            .is_some_and(CallKind::streams_response);
                        if let (true, Some(Outcome::Grpc(live))) = (self.sending, &mut self.live) {
                            live.response_stream = streams;
                        }
                    }
                    Err(message) => {
                        self.log.push(Level::Error, format!("schema: {message}"));
                        self.status = Some(message);
                    }
                }
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
            Msg::Trace(line) => self.log.push(Level::Detail, line),
            Msg::Failed(message) => {
                self.sending = false;
                self.log.push(Level::Error, message.clone());
                self.status = Some(message);
            }
        }
    }

    fn log_outcome(&mut self, outcome: &Outcome) {
        match outcome {
            Outcome::Response(r) => self.log.push(
                if r.status < 400 {
                    Level::Info
                } else {
                    Level::Error
                },
                format!("<- {} in {} ms", r.status, r.elapsed.as_millis()),
            ),
            Outcome::Grpc(g) => match (&g.status, &g.error) {
                (Some(s), _) => self.log.push(
                    if s.code == 0 {
                        Level::Info
                    } else {
                        Level::Error
                    },
                    format!("<- {} in {} ms", s.name(), g.elapsed.as_millis()),
                ),
                (None, Some(error)) => self.log.push(Level::Error, format!("failed: {error}")),
                (None, None) => self.log.push(Level::Error, "failed"),
            },
            Outcome::Error(message) => self.log.push(Level::Error, format!("failed: {message}")),
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
        if self.picker.is_some() {
            return self.picker_key(key.code, ctrl);
        }

        match (key.code, ctrl) {
            (KeyCode::Char('c'), true) => return self.should_quit = true,
            (KeyCode::Char('s'), true) => return self.send(),
            (KeyCode::Char('n') | KeyCode::Char('p'), true) if self.kind == Kind::Grpc => {
                return self.open_picker();
            }
            (KeyCode::Char('n'), true) => return self.method = self.method.next(),
            (KeyCode::Char('p'), true) => return self.method = self.method.prev(),
            (KeyCode::Char('t'), true) => return self.toggle_request_tab(),
            (KeyCode::Char('g'), true) => return self.toggle_kind(),
            (KeyCode::Char('l'), true) if self.kind == Kind::Grpc => return self.load_schema(),
            (KeyCode::Char('x'), true) => return self.cancel_call(),
            (KeyCode::Char('e'), true) => return self.open_log(),
            (KeyCode::Char('o'), true) if self.kind == Kind::Grpc => {
                return self.toggle_schema_mode();
            }
            (KeyCode::Char('k'), true) if self.kind == Kind::Grpc => {
                return self.insecure = !self.insecure;
            }
            (KeyCode::PageDown, _) => return self.scroll = self.scroll.saturating_add(5),
            (KeyCode::PageUp, _) => return self.scroll = self.scroll.saturating_sub(5),
            (KeyCode::Tab, _) => {
                return self.set_focus(next_focus(self.focus, self.editor_focus(), self.kind));
            }
            (KeyCode::BackTab, _) => {
                return self.set_focus(prev_focus(self.focus, self.editor_focus(), self.kind));
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
            Focus::Schema => self.schema_input(key),
            Focus::Method => self.method_key(key.code),
            Focus::Response => self.response_key(key.code),
        }
    }

    fn schema_input(&mut self, key: KeyEvent) {
        match (key.code, self.schema_mode) {
            (KeyCode::Enter, _) => self.load_schema(),
            (_, SchemaMode::File) => {
                self.schema_path.input(Input::from(key));
            }
            (_, SchemaMode::Reflection) => {}
        }
    }

    fn method_key(&mut self, code: KeyCode) {
        if code == KeyCode::Enter {
            self.open_picker();
        }
    }

    /// Loads the schema for the current address and source in the background; the result
    /// replaces the loaded schema, and a failure leaves it as it was.
    pub fn load_schema(&mut self) {
        if self.kind != Kind::Grpc || self.schema_loading {
            return;
        }
        if let Err(message) = self.schema_ready_to_load() {
            self.status = Some(message);
            return;
        }
        self.status = None;
        self.log.push(Level::Info, "loading schema");
        self.schema_loading = true;
        let key = self.schema_key();
        let (grpc, tx) = (self.grpc.clone(), self.tx.clone());
        tokio::spawn(async move {
            let result = grpc
                .load_schema(&key.address, &key.source, key.insecure)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::SchemaLoaded(key, result));
        });
    }

    /// What is missing before a schema can be loaded or a call made.
    fn schema_ready_to_load(&self) -> Result<(), String> {
        match self.schema_mode {
            SchemaMode::Reflection if self.url_text().trim().is_empty() => {
                Err("enter the server address first".into())
            }
            SchemaMode::File if self.schema_path_text().trim().is_empty() => {
                Err("enter the path of a schema file first".into())
            }
            _ => Ok(()),
        }
    }

    /// The loaded schema, if it is the one the editor currently describes.
    pub fn current_schema(&self) -> Option<&Schema> {
        let loaded = self.schema.as_ref()?;
        (loaded.key == self.schema_key()).then_some(&loaded.schema)
    }

    /// Opens the method picker over the loaded schema.
    pub fn open_picker(&mut self) {
        if self.kind != Kind::Grpc {
            return;
        }
        if self.current_schema().is_none() {
            self.status = Some("load the schema first (Ctrl+L)".into());
            return;
        }
        self.status = None;
        self.picker = Some(Picker::default());
    }

    /// The methods the picker lists: those whose `service/method` contains the filter,
    /// ignoring case, in schema order (grouped by service).
    pub fn picker_matches(&self) -> Vec<&crate::client::MethodInfo> {
        let (Some(picker), Some(schema)) = (&self.picker, self.current_schema()) else {
            return Vec::new();
        };
        let filter = picker.filter.to_lowercase();
        schema
            .methods()
            .iter()
            .filter(|m| m.full_name().to_lowercase().contains(&filter))
            .collect()
    }

    fn picker_key(&mut self, code: KeyCode, ctrl: bool) {
        match (code, ctrl) {
            (KeyCode::Char('c'), true) => self.should_quit = true,
            (KeyCode::Esc, _) => self.picker = None,
            (KeyCode::Enter, _) => self.pick_selected(),
            (KeyCode::Down | KeyCode::Char('n'), _) if ctrl || code == KeyCode::Down => {
                self.move_picker(1)
            }
            (KeyCode::Up | KeyCode::Char('p'), _) if ctrl || code == KeyCode::Up => {
                self.move_picker(-1)
            }
            (KeyCode::PageDown, _) => self.move_picker(8),
            (KeyCode::PageUp, _) => self.move_picker(-8),
            (KeyCode::Backspace, _) => {
                if let Some(picker) = &mut self.picker {
                    picker.filter.pop();
                    picker.selected = 0;
                }
            }
            (KeyCode::Char(c), false) => {
                if let Some(picker) = &mut self.picker {
                    picker.filter.push(c);
                    picker.selected = 0;
                }
            }
            _ => {}
        }
    }

    pub fn move_picker(&mut self, delta: isize) {
        let len = self.picker_matches().len();
        if let Some(picker) = &mut self.picker {
            picker.selected = picker
                .selected
                .saturating_add_signed(delta)
                .min(len.saturating_sub(1));
        }
    }

    /// Selects the highlighted method; an empty body is filled with the input message's
    /// skeleton, a written one is left alone.
    pub fn pick_selected(&mut self) {
        let Some(selected) = self.picker.as_ref().map(|p| p.selected) else {
            return;
        };
        self.pick(selected);
    }

    pub fn pick(&mut self, index: usize) {
        let Some(full_name) = self.picker_matches().get(index).map(|m| m.full_name()) else {
            return;
        };
        self.picker = None;
        if self.body_text().trim().is_empty()
            && let Some(skeleton) = self
                .current_schema()
                .and_then(|schema| schema.skeleton(&full_name))
        {
            self.body = TextArea::new(skeleton.lines().map(str::to_owned).collect());
        }
        self.selected_method = Some(full_name);
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
            Some(Outcome::Grpc(g)) => g.body_text(),
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

    /// Opens the whole log in the big reading panel.
    pub fn open_log(&mut self) {
        if self.log.is_empty() {
            self.notice = Some("the log is empty".into());
            return;
        }
        let mut modal = Modal::log("Logs", self.log.to_text());
        // opens on the newest entries; the renderer clamps this to the end
        modal.scroll = usize::MAX;
        self.modal = Some(modal);
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
                let text = modal
                    .selected_text()
                    .unwrap_or_else(|| modal.displayed().to_owned());
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

    /// Switches the editor between REST and gRPC. The address/URL, body and
    /// headers/metadata texts are kept.
    pub fn toggle_kind(&mut self) {
        self.set_kind(match self.kind {
            Kind::Http => Kind::Grpc,
            Kind::Grpc => Kind::Http,
        });
    }

    pub fn set_kind(&mut self, kind: Kind) {
        if self.kind == kind {
            return;
        }
        self.kind = kind;
        // The editors that only exist in one mode cannot keep the focus.
        if kind == Kind::Http && matches!(self.focus, Focus::Schema | Focus::Method) {
            self.focus = Focus::Url;
        }
    }

    /// gRPC editor: reflection or a schema file.
    pub fn toggle_schema_mode(&mut self) {
        self.schema_mode = match self.schema_mode {
            SchemaMode::Reflection => SchemaMode::File,
            SchemaMode::File => SchemaMode::Reflection,
        };
        if self.schema_mode == SchemaMode::File {
            self.focus = Focus::Schema;
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
            Some(Outcome::Grpc(g)) => pretty(&g.body_text()),
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
        let areas = ui::areas_for(screen, self.view().request.kind);
        let at = Position::new(event.column, event.row);
        if self.picker.is_some() {
            return self.picker_mouse(event, screen);
        }
        if self.modal.is_some() {
            return self.modal_mouse(event, screen);
        }
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => self.click(at, &areas),
            MouseEventKind::Down(MouseButton::Right)
                if areas.method.contains(at) && self.kind == Kind::Http =>
            {
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

    /// Wheel scrolls; dragging with the left button selects text and copies it on release.
    fn modal_mouse(&mut self, event: MouseEvent, screen: Rect) {
        let Some(modal) = self.modal.as_mut() else {
            return;
        };
        let area = ui::modal_area(screen);
        let view = ui::inner(area);
        // Cell of the viewport under the pointer, clamped to it.
        let cell = |event: &MouseEvent| {
            let x = event.column.clamp(view.x, view.right().saturating_sub(1)) - view.x;
            let y = event.row.clamp(view.y, view.bottom().saturating_sub(1)) - view.y;
            (x as usize, y as usize)
        };
        match event.kind {
            MouseEventKind::ScrollDown => modal.scroll_by(3),
            MouseEventKind::ScrollUp => modal.scroll_by(-3),
            MouseEventKind::Down(MouseButton::Left)
                if !area.contains(Position::new(event.column, event.row)) =>
            {
                self.modal = None;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let (x, y) = cell(&event);
                modal.clear_selection();
                if view.contains(Position::new(event.column, event.row)) {
                    modal.select_start(x, y);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // dragging past an edge scrolls
                if event.row < view.y {
                    modal.scroll_by(-1);
                } else if event.row >= view.bottom() {
                    modal.scroll_by(1);
                }
                let (x, y) = cell(&event);
                modal.select_to(x, y);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(text) = modal.selected_text().filter(|_| modal.has_range()) {
                    self.notice = Some(format!("copied {} bytes to the clipboard", text.len()));
                    self.clipboard = Some(text);
                } else {
                    modal.clear_selection();
                }
            }
            _ => {}
        }
    }

    fn picker_mouse(&mut self, event: MouseEvent, screen: Rect) {
        let at = Position::new(event.column, event.row);
        let popup = ui::picker_area(screen);
        match event.kind {
            MouseEventKind::ScrollDown => self.move_picker(1),
            MouseEventKind::ScrollUp => self.move_picker(-1),
            MouseEventKind::Down(MouseButton::Left) if !popup.contains(at) => self.picker = None,
            MouseEventKind::Down(MouseButton::Left) => {
                let list = ui::picker_list_area(popup);
                if !list.contains(at) {
                    return;
                }
                let map = ui::picker_row_map(self);
                let selected = self.picker.as_ref().map(|p| p.selected);
                let selected_row = map.iter().position(|r| *r == selected);
                let row = (at.y - list.y) as usize + ui::list_offset(selected_row, list.height);
                if let Some(Some(index)) = map.get(row) {
                    self.pick(*index);
                }
            }
            _ => {}
        }
    }

    fn click(&mut self, at: Position, areas: &ui::Areas) {
        if areas.logs.contains(at) {
            return self.open_log();
        }
        if areas.tabs.contains(at) {
            if let Some(tab) = ui::tab_at(areas.tabs, at.x) {
                self.focus = Focus::List;
                self.set_tab(tab);
            }
        } else if areas.list.contains(at) {
            self.click_list(at.y - areas.list.y, areas.list.height);
        } else if areas.method.contains(at) {
            self.focus_editor(Focus::Url);
            match self.kind {
                Kind::Http => self.method = self.method.next(),
                Kind::Grpc => self.insecure = !self.insecure,
            }
        } else if areas.schema.contains(at) {
            self.focus_editor(Focus::Schema);
            let inner = ui::inner(areas.schema);
            if self.schema_mode == SchemaMode::File && inner.contains(at) {
                let (_, col_scroll) = ui::editor_scroll(cursor_of(&self.schema_path), inner);
                self.schema_path
                    .move_cursor(CursorMove::Jump(0, (at.x - inner.x) + col_scroll as u16));
            }
        } else if areas.rpc.contains(at) {
            self.focus_editor(Focus::Method);
            self.open_picker();
        } else if let Some(kind) = ui::kind_at(areas.url, at) {
            self.set_kind(kind);
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
            let view = self.view().request;
            if let Some(tab) = ui::request_tab_at(areas.body, at, view.headers.len(), view.kind) {
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
        let request = match request {
            Request::Http(request) => request,
            Request::Grpc(request) => return self.load_grpc_request(request),
        };
        self.set_kind(Kind::Http);
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

    fn load_grpc_request(&mut self, request: &GrpcRequest) {
        self.set_kind(Kind::Grpc);
        self.url = single_line(&request.address);
        self.selected_method = (!request.method.is_empty()).then(|| request.method.clone());
        match &request.schema {
            SchemaSource::Reflection => self.schema_mode = SchemaMode::Reflection,
            SchemaSource::File(path) => {
                self.schema_mode = SchemaMode::File;
                self.schema_path = single_line(&path.to_string_lossy());
            }
        }
        self.insecure = request.insecure;
        self.body = request
            .body
            .as_deref()
            .map(|b| TextArea::new(b.lines().map(str::to_owned).collect()))
            .unwrap_or_default();
        self.headers = match headers::to_lines(&request.metadata) {
            lines if lines.is_empty() => TextArea::default(),
            lines => TextArea::new(lines),
        };
    }

    // ---- background work ------------------------------------------------

    pub fn send(&mut self) {
        if self.sending {
            self.status = Some("a request is already in flight".into());
            return;
        }
        match self.kind {
            Kind::Http => self.send_http(),
            Kind::Grpc => self.send_grpc(),
        }
    }

    fn send_http(&mut self) {
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
        self.log.push(
            Level::Info,
            format!("-> {} {}", request.method.as_str(), request.url),
        );

        let (client, store, tx) = (self.client.clone(), self.store.clone(), self.tx.clone());
        tokio::spawn(async move {
            let trace = Trace::new({
                let tx = tx.clone();
                move |line| {
                    let _ = tx.send(Msg::Trace(line));
                }
            });
            let outcome: Outcome = client.send_traced(&request, &trace).await.into();
            if let Err(e) = store.record(&Request::Http(request), &outcome).await {
                let _ = tx.send(Msg::Failed(format!("could not save history: {e}")));
            }
            let _ = tx.send(Msg::Sent(outcome));
            record_done(&store, &tx).await;
        });
    }

    fn send_grpc(&mut self) {
        let request = match self.try_grpc_request() {
            Ok(request) => request,
            Err(message) => {
                self.status = Some(message);
                self.request_tab = RequestTab::Headers;
                self.set_focus(Focus::Headers);
                return;
            }
        };
        if request.address.is_empty() {
            self.status = Some("enter the server address first".into());
            return;
        }
        if request.method.is_empty() {
            self.status = Some("pick a method first (Ctrl+N)".into());
            return;
        }
        if let Err(message) = self.schema_ready_to_load() {
            self.status = Some(message);
            return;
        }

        let key = self.schema_key();
        let cached = self.current_schema().cloned();
        self.log.push(
            Level::Info,
            format!("-> gRPC {} {}", request.address, request.method),
        );
        self.sending = true;
        self.status = None;
        self.scroll = 0;
        self.live = Some(Outcome::Grpc(GrpcOutcome {
            response_stream: self
                .method_kind(&request.method)
                .is_some_and(CallKind::streams_response),
            ..Default::default()
        }));

        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        let (grpc, store, tx) = (self.grpc.clone(), self.store.clone(), self.tx.clone());
        tokio::spawn(async move {
            let schema = match cached {
                Some(schema) => Ok(schema),
                None => {
                    let loaded = tokio::select! {
                        biased;
                        _ = cancel.cancelled() => Err(cancelled()),
                        loaded = grpc.load_schema(&key.address, &key.source, key.insecure) => {
                            loaded.map_err(|e| e.to_string())
                        }
                    };
                    if let Ok(schema) = &loaded {
                        let _ = tx.send(Msg::SchemaLoaded(key, Ok(schema.clone())));
                    }
                    loaded
                }
            };

            let outcome = match schema {
                Ok(schema) => run_call(&*grpc, request.clone(), schema, &cancel, &tx).await,
                Err(message) => {
                    let outcome = failed_outcome(message);
                    let _ = tx.send(Msg::GrpcEvent(CallEvent::Done(outcome.clone())));
                    outcome
                }
            };
            if let Err(e) = store
                .record(&Request::Grpc(request), &Outcome::Grpc(outcome))
                .await
            {
                let _ = tx.send(Msg::Failed(format!("could not save history: {e}")));
            }
            record_done(&store, &tx).await;
        });
    }

    /// Cancels the in-flight gRPC call; it ends with status `CANCELLED`.
    pub fn cancel_call(&mut self) {
        match &self.cancel {
            Some(token) => token.cancel(),
            None if self.sending => {
                self.notice = Some("this request cannot be cancelled".into());
            }
            None => self.notice = Some("nothing to cancel".into()),
        }
    }

    fn apply_event(&mut self, event: CallEvent) {
        match event {
            CallEvent::Headers(headers) => {
                if let Some(Outcome::Grpc(live)) = &mut self.live {
                    live.metadata = headers;
                }
            }
            CallEvent::Log(line) => self.log.push(Level::Detail, line),
            CallEvent::Message(message) => {
                if let Some(Outcome::Grpc(live)) = &mut self.live {
                    live.messages.push(message);
                }
            }
            CallEvent::Done(outcome) => {
                self.log_outcome(&Outcome::Grpc(outcome.clone()));
                self.sending = false;
                self.cancel = None;
                self.scroll = 0;
                self.live = Some(Outcome::Grpc(outcome));
            }
        }
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

/// The focus ring: list, URL, [schema, method (gRPC only)], the request editor, response.
fn next_focus(f: Focus, editor: Focus, kind: Kind) -> Focus {
    match (f, kind) {
        (Focus::List, _) => Focus::Url,
        (Focus::Url, Kind::Http) => editor,
        (Focus::Url, Kind::Grpc) => Focus::Schema,
        (Focus::Schema, _) => Focus::Method,
        (Focus::Method, _) => editor,
        (Focus::Body | Focus::Headers, _) => Focus::Response,
        (Focus::Response, _) => Focus::List,
    }
}

fn prev_focus(f: Focus, editor: Focus, kind: Kind) -> Focus {
    match (f, kind) {
        (Focus::List, _) => Focus::Response,
        (Focus::Url, _) => Focus::List,
        (Focus::Schema, _) => Focus::Url,
        (Focus::Method, _) => Focus::Schema,
        (Focus::Body | Focus::Headers, Kind::Http) => Focus::Url,
        (Focus::Body | Focus::Headers, Kind::Grpc) => Focus::Method,
        (Focus::Response, _) => editor,
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

/// Forwards the events of a call and returns how it ended.
async fn run_call(
    grpc: &dyn crate::client::GrpcClient,
    request: GrpcRequest,
    schema: Schema,
    cancel: &CancellationToken,
    tx: &UnboundedSender<Msg>,
) -> GrpcOutcome {
    let mut call = grpc.call(request, schema);
    let canceller = call.canceller();
    let token = cancel.clone();
    let watcher = tokio::spawn(async move {
        token.cancelled().await;
        canceller.cancel();
    });

    let mut last = None;
    while let Some(event) = call.events.recv().await {
        if let CallEvent::Done(outcome) = &event {
            last = Some(outcome.clone());
        }
        let _ = tx.send(Msg::GrpcEvent(event));
    }
    watcher.abort();
    last.unwrap_or_else(|| failed_outcome("the call ended without a result".into()))
}

/// `3 services, 11 methods`.
fn counts(schema: &Schema) -> String {
    let (services, methods) = (schema.service_count(), schema.methods().len());
    format!(
        "{services} service{}, {methods} method{}",
        if services == 1 { "" } else { "s" },
        if methods == 1 { "" } else { "s" },
    )
}

fn cancelled() -> String {
    "cancelled by the user".into()
}

/// The outcome of a call that failed before reaching the server.
fn failed_outcome(message: String) -> GrpcOutcome {
    if message == cancelled() {
        return GrpcOutcome {
            status: Some(GrpcStatus {
                code: 1,
                message,
                trailers: Vec::new(),
            }),
            ..Default::default()
        };
    }
    GrpcOutcome {
        error: Some(message),
        ..Default::default()
    }
}

/// Posts the refreshed history list after a request was recorded.
async fn record_done(store: &History, tx: &UnboundedSender<Msg>) {
    match store.list(HISTORY_LIMIT).await {
        Ok(list) => {
            let _ = tx.send(Msg::History(list));
        }
        Err(e) => {
            let _ = tx.send(Msg::Failed(format!("could not read history: {e}")));
        }
    }
}
