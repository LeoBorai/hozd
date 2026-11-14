use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui_image::picker::Picker as ImagePicker;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_textarea::{CursorMove, TextArea};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::client::{
    Body, BodyKind, CallEvent, CallKind, Field, GraphqlOperation, GraphqlRequest, GraphqlSchema,
    GrpcOutcome, GrpcRequest, GrpcStatus, Header, HttpRequest, JsonValue, Kind, Method,
    OperationKind, Part, PartValue, Request, Schema, SchemaSource, SharedGrpc, SharedHttp,
    SharedWs, Trace, TungsteniteWs, WsEvent, WsOutcome, WsRequest, introspect, split_query,
    with_query,
};
use crate::history::{History, HistoryEntry, Outcome, Summary};
use crate::model::{Directory, Project, from_client_request, to_client_request};
use crate::project::{Loaded, Template};
use crate::vars::{self, Vars};

use super::auth::{self, AuthEditor, Scheme};
use super::commands::{Action, Command, commands_for};
use super::complete::{self, Suggestion};
use super::headers::{Col, HeaderEditor};
use super::json_editor::{JsonCol, JsonEditor};
use super::log::{Level, Log};
use super::modal::Modal;
use super::syntax_highlight::json::pretty;
use super::ui;

/// Two clicks on the response body closer together than this open it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// How many history rows the left panel keeps.
pub const HISTORY_LIMIT: u32 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Projects stored in the database: project > directories > requests.
    Projects,
    Stored,
    History,
}

/// The tabs over the center panel while the Projects tab is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CenterTab {
    /// The request editor.
    #[default]
    Request,
    /// Title and description of the selected project, directory or request.
    Metadata,
}

/// A yes/no question over the screen: Ok runs the pending deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    pub message: String,
    /// Whether Ok (rather than Cancel) is the active option.
    pub ok: bool,
    /// The directory or request to delete.
    node: String,
}

/// One visible line of the Projects tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    pub depth: usize,
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: ProjectRowKind,
    /// Index into [`App::projects`].
    pub project: usize,
    /// The directory holding this line; `None` at the project root.
    pub parent: Option<String>,
}

/// One line of the Projects list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelRow {
    /// Outside a project: starts naming a new project.
    CreateProject,
    Node(ProjectRow),
    /// Inside a project only.
    CreateDirectory,
    /// Inside a project only.
    CreateRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectRowKind {
    Project {
        open: bool,
    },
    Directory {
        open: bool,
    },
    /// The HTTP verb.
    Request {
        method: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    /// Metadata tab: the title box.
    MetaTitle,
    /// Metadata tab: the description box.
    MetaDescription,
    Url,
    Body,
    /// GraphQL editor: the variables JSON.
    Variables,
    Headers,
    /// REST editor: the URL query parameters.
    Params,
    /// REST editor: the `Authorization` header.
    Auth,
    /// gRPC editor: the schema source box.
    Schema,
    /// gRPC editor: the method box; `Enter` opens the picker.
    Method,
    /// The response body box; `Enter` opens it in the reading panel.
    Response,
    /// The Variables panel: the captures, one row each (variable name, path into the
    /// response).
    Captures,
}

/// What the left column's lower panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LeftTab {
    #[default]
    Logs,
    Variables,
}

/// Which editor the request's lower box shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestTab {
    Body,
    /// GraphQL only.
    Variables,
    Headers,
    /// REST only.
    Params,
    /// REST only.
    Auth,
}

/// What a loaded schema was loaded for; it is dropped when the editor stops matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaKey {
    pub address: String,
    pub source: SchemaSource,
    pub insecure: bool,
}

/// The schema introspected from a GraphQL endpoint.
#[derive(Debug, Clone)]
pub struct LoadedGraphql {
    /// The endpoint it came from.
    pub url: String,
    pub schema: GraphqlSchema,
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
    /// The GraphQL editors as they were when the picker opened, kept while the highlighted
    /// operation is previewed in them; `None` when nothing is previewed (gRPC, or a query
    /// already written).
    pub origin: Option<PickerOrigin>,
}

/// The query and variables texts to put back when the picker closes without a pick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PickerOrigin {
    pub query: String,
    pub variables: String,
}

/// What a picker row is: a gRPC method with its call kind, or a GraphQL operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerTag {
    Call(CallKind),
    Query,
    Mutation,
}

impl PickerTag {
    pub fn label(self) -> &'static str {
        match self {
            PickerTag::Call(kind) => kind.label(),
            PickerTag::Query => "query",
            PickerTag::Mutation => "mutation",
        }
    }
}

/// One row of the picker: a method or operation, listed under its `group`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    /// The gRPC service, or `Query`/`Mutation`.
    pub group: String,
    pub name: String,
    pub tag: PickerTag,
}

impl PickerItem {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.group, self.name)
    }
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
    /// A WebSocket session made progress; the last event of a session is `Done`.
    WsEvent(WsEvent),
    /// The result of a schema load started for `SchemaKey`.
    SchemaLoaded(SchemaKey, Result<Schema, String>),
    /// The result of introspecting the GraphQL endpoint at the URL.
    GraphqlLoaded(String, Result<GraphqlSchema, String>),
    History(Vec<Summary>),
    Projects(Vec<Project>),
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
    /// The body as text, see [`Body::preview`].
    pub body: String,
    /// What the body is; only HTTP requests have anything but raw JSON.
    pub body_kind: BodyKind,
    /// The GraphQL variables; empty for other kinds.
    pub variables: String,
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
            body: r.body.as_ref().map(Body::preview).unwrap_or_default(),
            body_kind: r.body.as_ref().map_or_else(BodyKind::default, Body::kind),
            variables: String::new(),
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
                body_kind: BodyKind::default(),
                variables: String::new(),
                grpc: GrpcView {
                    method: r.method.clone(),
                    schema: r.schema.clone(),
                    insecure: r.insecure,
                },
            },
            Request::Graphql(r) => Self {
                kind: Kind::Graphql,
                method: Method::Post,
                url: r.url.clone(),
                headers: r.headers.clone(),
                body: r.query.clone(),
                body_kind: BodyKind::default(),
                variables: r.variables.clone().unwrap_or_default(),
                grpc: GrpcView::default(),
            },
            Request::Ws(r) => Self {
                kind: Kind::Ws,
                method: Method::default(),
                url: r.url.clone(),
                headers: r.headers.clone(),
                body: r.body.clone().unwrap_or_default(),
                body_kind: BodyKind::default(),
                variables: String::new(),
                grpc: GrpcView::default(),
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

/// The path suggestions of the schema file input.
#[derive(Debug, Default)]
pub struct Suggestions {
    /// Which input the suggestions complete.
    pub target: SuggestTarget,
    pub items: Vec<Suggestion>,
    /// The highlighted suggestion, once the user moved onto the list.
    pub sel: Option<usize>,
}

/// The input a list of suggestions completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SuggestTarget {
    /// The gRPC schema file.
    #[default]
    Schema,
    /// An `@file` of the request body.
    Body,
}

/// Image bytes and their drawable form, `None` when they do not decode.
type CachedImage = (Arc<[u8]>, Option<StatefulProtocol>);

pub struct App {
    pub tab: Tab,
    pub focus: Focus,
    pub templates: Vec<StoredTemplate>,
    pub stored_sel: usize,
    /// Projects of the Projects tab, as stored in the database.
    pub projects: Vec<Project>,
    /// Ids of the open projects and directories.
    pub open_nodes: HashSet<String>,
    /// Selected row of [`App::project_rows`].
    pub projects_sel: usize,
    /// The project request the editor is bound to: its edits are copied into the project
    /// as they are made, and saved when the request is sent or the editor loses focus.
    bound: Option<String>,
    /// The editor's request as last copied into the project.
    bound_last: Option<HttpRequest>,
    /// Edits copied into the project but not yet written to the database.
    bound_dirty: bool,
    /// The deletion waiting for an answer.
    pub confirm: Option<Confirm>,
    /// Id of the project whose context is open in the Projects list.
    pub project_ctx: Option<String>,
    /// The name being typed for a new project, while the Projects list asks for one.
    pub naming: Option<TextArea<'static>>,
    /// Metadata tab editors, showing the tree line `meta_for`.
    pub meta_title: TextArea<'static>,
    pub meta_desc: TextArea<'static>,
    meta_for: Option<String>,
    /// The tree line last selected in the Projects list.
    last_node: Option<String>,
    /// Queue of projects to write to the database, in order.
    saver: UnboundedSender<Project>,
    /// The selected tab over the center panel; only shown in the Projects tab.
    pub center_tab: CenterTab,
    pub history: Vec<Summary>,
    pub history_sel: usize,
    /// Whether the editor builds an HTTP or a gRPC request.
    pub kind: Kind,
    pub method: Method,
    /// The URL; the server address in gRPC mode.
    pub url: TextArea<'static>,
    /// The body text: free text or raw JSON in REST, the message in the other modes.
    pub body: TextArea<'static>,
    /// What the REST body is.
    pub body_kind: BodyKind,
    /// The REST body when it is built field by field.
    pub json: JsonEditor,
    /// The REST body when it is a URL encoded form.
    pub form: HeaderEditor,
    /// The REST body when it is multipart; a value `@path` is a file.
    pub multipart: HeaderEditor,
    /// The REST body when it is a file: `@path`.
    pub file: TextArea<'static>,
    /// One `Name: value` per line, see [`headers`].
    pub headers: HeaderEditor,
    /// REST URL query parameters, encoded into the URL when the request is built.
    pub params: HeaderEditor,
    /// REST `Authorization` header, put together when the request is built.
    pub auth: AuthEditor,
    pub request_tab: RequestTab,
    /// Which panel the left column's lower half shows.
    pub left_tab: LeftTab,
    /// Variables to capture from the next response: name and a jq-style path into its
    /// JSON body.
    pub captures: HeaderEditor,
    /// The captured values, substituted for `$name` in REST bodies.
    pub vars: Vars,
    /// Whether `$name` in the REST body is replaced by the variable's value.
    pub vars_body: bool,
    /// Same for the REST header values.
    pub vars_headers: bool,
    /// Same for the REST URL param values.
    pub vars_params: bool,
    /// GraphQL editor: the variables, a JSON object.
    pub variables: TextArea<'static>,
    /// GraphQL editor: the schema introspected from the endpoint, if any.
    pub graphql: Option<LoadedGraphql>,
    /// gRPC editor: whether the schema comes from reflection or from `schema_path`.
    pub schema_mode: SchemaMode,
    /// gRPC editor: the `.proto` or descriptor set file used when `schema_mode` is `File`.
    pub schema_path: TextArea<'static>,
    /// Completions for `schema_path`, shown under the Schema box while it has focus.
    pub suggestions: Suggestions,
    /// The path text `suggestions` were computed for.
    suggested_for: Option<String>,
    /// gRPC editor: skip TLS certificate verification.
    pub insecure: bool,
    /// gRPC editor: the `package.Service/Method` to call.
    pub selected_method: Option<String>,
    /// The schema loaded for the gRPC editor, if any.
    pub schema: Option<LoadedSchema>,
    pub schema_loading: bool,
    /// The method picker, when open.
    pub picker: Option<Picker>,
    /// The keybindings menu, when open: the selected row of [`App::commands`].
    pub help: Option<usize>,
    pub live: Option<Outcome>,
    pub preview: Option<HistoryEntry>,
    /// A request (or its schema load) is in flight.
    pub sending: bool,
    /// Animation frame counter, advanced by [`App::tick`] while a request is in flight.
    pub frame: usize,
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
    /// Draws image response bodies. Halfblocks until [`App::set_image_picker`] gives it
    /// the protocol the terminal speaks.
    image_picker: ImagePicker,
    /// The image body drawn last, decoded for [`App::image_picker`]; `None` inside when
    /// its bytes are not an image the decoder knows.
    image_cache: RefCell<Option<CachedImage>>,
    pub should_quit: bool,
    /// The list item the last click landed on; clicking it again loads it.
    last_click: Option<(Tab, usize)>,

    client: SharedHttp,
    grpc: SharedGrpc,
    ws: SharedWs,
    /// Sends messages into the open WebSocket session; `None` when there is none.
    ws_out: Option<UnboundedSender<String>>,
    /// Opens a URL in the browser; replaced in tests.
    opener: fn(&str),
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

        let (saver, mut saved) = tokio::sync::mpsc::unbounded_channel::<Project>();
        {
            let (store, tx) = (store.clone(), tx.clone());
            tokio::spawn(async move {
                while let Some(project) = saved.recv().await {
                    if let Err(e) = store.save_project(&project).await {
                        let _ = tx.send(Msg::Failed(format!("could not save project: {e}")));
                    }
                }
            });
        }

        let app = Self {
            tab: Tab::Projects,
            focus: Focus::List,
            templates,
            stored_sel: 0,
            projects: Vec::new(),
            open_nodes: HashSet::new(),
            projects_sel: 0,
            project_ctx: None,
            confirm: None,
            bound: None,
            bound_last: None,
            bound_dirty: false,
            naming: None,
            meta_title: single_line(""),
            meta_desc: TextArea::default(),
            meta_for: None,
            last_node: None,
            saver,
            center_tab: CenterTab::default(),
            history: Vec::new(),
            history_sel: 0,
            kind: Kind::Http,
            method: Method::Get,
            url: single_line(""),
            body: TextArea::default(),
            body_kind: BodyKind::default(),
            json: JsonEditor::default(),
            form: HeaderEditor::default(),
            multipart: HeaderEditor::default(),
            file: single_line(""),
            headers: HeaderEditor::default(),
            params: HeaderEditor::default(),
            auth: AuthEditor::default(),
            request_tab: RequestTab::Body,
            left_tab: LeftTab::default(),
            captures: HeaderEditor::default(),
            vars: Vars::default(),
            vars_body: true,
            vars_headers: true,
            vars_params: true,
            variables: TextArea::default(),
            graphql: None,
            schema_mode: SchemaMode::Reflection,
            schema_path: single_line(""),
            suggestions: Suggestions::default(),
            suggested_for: None,
            insecure: false,
            selected_method: None,
            schema: None,
            schema_loading: false,
            picker: None,
            help: None,
            live: None,
            preview: None,
            sending: false,
            frame: 0,
            cancel: None,
            status,
            log: Log::default(),
            notice: None,
            clipboard: None,
            modal: None,
            inspect_preview: false,
            last_response_click: None,
            scroll: 0,
            image_picker: ImagePicker::halfblocks(),
            image_cache: RefCell::new(None),
            should_quit: false,
            last_click: None,
            client,
            grpc,
            ws: Arc::new(TungsteniteWs::new()),
            ws_out: None,
            opener: open_in_browser,
            store,
            tx,
        };
        let mut app = app;
        if let Some(issue) = app.status.clone() {
            app.log.push(Level::Error, issue);
        }
        app.log.push(Level::Info, "ready");
        app.refresh_history();
        app.refresh_projects();
        app
    }

    /// Replaces what opens links in the browser, for tests.
    pub fn with_opener(mut self, opener: fn(&str)) -> Self {
        self.opener = opener;
        self
    }

    /// Replaces the WebSocket client, for tests.
    pub fn with_ws(mut self, ws: SharedWs) -> Self {
        self.ws = ws;
        self
    }

    /// Whether a WebSocket session is open (or opening).
    pub fn ws_open(&self) -> bool {
        self.ws_out.is_some()
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
        self.build_request(self.headers.parse_lenient())
    }

    /// Like [`editor_request`](Self::editor_request) but fails on an invalid header line.
    pub fn try_request(&self) -> Result<HttpRequest, String> {
        let mut headers = self.headers.parse()?;
        self.add_authorization(&mut headers);
        let url = self.http_url(true).map_err(|e| format!("URL param {e}"))?;
        let body = self.build_body(true)?;
        if let Some(body) = &body {
            body.check().map_err(|e| e.to_string())?;
        }
        Ok(HttpRequest {
            method: self.method,
            url,
            headers,
            body,
        })
    }

    /// Query parameters are edited in the "URL Params" tab only: whatever follows a `?` in
    /// the address moves into the table.
    fn absorb_query(&mut self) {
        if self.kind != Kind::Http || !self.url_text().contains('?') {
            return;
        }
        let (url, found) = split_query(&self.url_text());
        let mut pairs = self.params.pairs_untrimmed(false).unwrap_or_default();
        pairs.extend(found);
        self.url = single_line(&url);
        self.params = HeaderEditor::from_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }

    /// The number shown next to the "URL Params" title: the table's rows while editing,
    /// the query pairs of the URL while previewing a stored request.
    pub fn params_count(&self, request: &RequestView, previewing: bool) -> usize {
        if previewing {
            let query = request.url.split('#').next().unwrap_or("");
            query
                .split_once('?')
                .map_or(0, |(_, q)| q.split('&').filter(|p| !p.is_empty()).count())
        } else {
            self.params.pairs_untrimmed(false).map_or(0, |p| p.len())
        }
    }

    /// The URL as sent: the address with the query parameters encoded onto it.
    fn http_url(&self, strict: bool) -> Result<String, String> {
        let params = self.params.pairs_untrimmed(strict)?;
        Ok(with_query(self.url_text().trim(), &params))
    }

    /// The URL as sent, with `$name` replaced in the param values.
    fn http_url_vars(&self) -> Result<String, String> {
        let mut params = self.params.pairs_untrimmed(true)?;
        for (_, v) in &mut params {
            *v = self.vars.substitute(v, false);
        }
        Ok(with_query(self.url_text().trim(), &params))
    }

    /// Adds the Auth tab's `Authorization` header unless the Headers tab has one.
    fn add_authorization(&self, headers: &mut Vec<Header>) {
        if self.kind == Kind::Http
            && let Some(value) = self.auth.header_value()
            && !headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("authorization"))
        {
            headers.push(Header::new("Authorization", value));
        }
    }

    /// The number shown next to the "Headers" title. The Auth tab's header is not counted
    /// while editing.
    pub fn headers_count(&self, request: &RequestView, previewing: bool) -> usize {
        if !previewing && request.kind == Kind::Http {
            self.headers.parse_lenient().len()
        } else {
            request.headers.len()
        }
    }

    /// The label next to "Auth" in the tab title: the scheme in use, empty for none.
    pub fn auth_label(&self, request: &RequestView, previewing: bool) -> &'static str {
        let scheme = if previewing {
            request
                .headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case("authorization"))
                .map_or(Scheme::None, |h| AuthEditor::from_header(&h.value).scheme)
        } else if self.auth.header_value().is_some() {
            self.auth.scheme
        } else {
            Scheme::None
        };
        match scheme {
            Scheme::None => "",
            Scheme::Raw => "Text",
            other => other.label(),
        }
    }

    fn build_request(&self, mut headers: Vec<Header>) -> HttpRequest {
        self.add_authorization(&mut headers);
        HttpRequest {
            method: self.method,
            url: self.http_url(false).unwrap_or_default(),
            headers,
            body: self.build_body(false).ok().flatten(),
        }
    }

    pub fn file_text(&self) -> String {
        self.file.lines().join("")
    }

    /// The REST body as edited; `None` when there is nothing to send. With `strict`, a row
    /// that cannot be sent is an error instead of being left out.
    pub fn build_body(&self, strict: bool) -> Result<Option<Body>, String> {
        let body = match self.body_kind {
            BodyKind::Text => Body::Text(self.body_text()),
            BodyKind::JsonRaw => Body::JsonRaw(self.body_text()),
            BodyKind::Json => Body::Json(self.json.to_value(strict)?),
            BodyKind::Form => Body::Form(
                self.form
                    .pairs(strict)?
                    .into_iter()
                    .map(|(name, value)| Field { name, value })
                    .collect(),
            ),
            BodyKind::Multipart => Body::Multipart(
                self.multipart
                    .pairs(strict)?
                    .into_iter()
                    .map(|(name, value)| Part {
                        name,
                        value: PartValue::parse(&value),
                    })
                    .collect(),
            ),
            BodyKind::File => {
                let text = self.file_text();
                Body::File(text.strip_prefix('@').unwrap_or(&text).to_owned())
            }
        };
        let empty = match &body {
            Body::Json(_) => self.json.is_empty(),
            other => other.is_empty(),
        };
        Ok((!empty).then_some(body))
    }

    /// Switches what the REST body is. Moving from the JSON fields to a text kind keeps
    /// the document as text; moving from raw JSON to the fields keeps it as fields when it
    /// is an object or an array.
    pub fn set_body_kind(&mut self, kind: BodyKind) {
        if kind == self.body_kind {
            return;
        }
        match (self.body_kind, kind) {
            (BodyKind::Json, BodyKind::JsonRaw | BodyKind::Text) if !self.json.is_empty() => {
                if let Ok(value) = self.json.to_value(false) {
                    let text = value.to_json_pretty();
                    self.body = TextArea::new(text.lines().map(str::to_owned).collect());
                }
            }
            (BodyKind::JsonRaw | BodyKind::Text, BodyKind::Json) => {
                let text = self.body_text();
                if let Some(json) = JsonValue::parse(&text)
                    .ok()
                    .and_then(|v| JsonEditor::from_value(&v))
                {
                    self.json = json;
                } else if !text.trim().is_empty() {
                    self.notice =
                        Some("the body is not a JSON object or array; kept as text".into());
                }
            }
            _ => {}
        }
        self.body_kind = kind;
        if self.focus == Focus::Body {
            self.refresh_suggestions();
        }
    }

    /// Selects the next body kind (REST only).
    pub fn next_body_kind(&mut self) {
        if self.kind != Kind::Http {
            self.notice = Some("body types are for REST requests".into());
            return;
        }
        self.set_body_kind(self.body_kind.next());
        self.notice = Some(format!("body: {}", self.body_kind));
        self.set_request_tab(RequestTab::Body);
    }

    pub fn schema_path_text(&self) -> String {
        self.schema_path.lines().join("")
    }

    /// The schema source the gRPC editor currently describes.
    pub fn schema_source(&self) -> SchemaSource {
        match self.schema_mode {
            SchemaMode::Reflection => SchemaSource::Reflection,
            SchemaMode::File => {
                SchemaSource::File(complete::expand_home(self.schema_path_text().trim()))
            }
        }
    }

    /// One line about the loaded schema for the Schema box.
    pub fn schema_status(&self) -> String {
        if self.schema_loading {
            return "loading...".into();
        }
        if self.kind == Kind::Graphql {
            return match self.current_graphql() {
                Some(schema) => format!("{} operations", schema.operations().len()),
                None => "not introspected".into(),
            };
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

    /// The WebSocket request as edited; header lines that are not `Name: value` are skipped.
    pub fn editor_ws_request(&self) -> WsRequest {
        self.build_ws_request(self.headers.parse_lenient())
    }

    /// Like [`editor_ws_request`](Self::editor_ws_request) but fails on an invalid header line.
    pub fn try_ws_request(&self) -> Result<WsRequest, String> {
        Ok(self.build_ws_request(self.headers.parse()?))
    }

    fn build_ws_request(&self, headers: Vec<Header>) -> WsRequest {
        let body = self.body_text();
        WsRequest {
            url: self.url_text().trim().to_owned(),
            headers,
            body: (!body.is_empty()).then_some(body),
        }
    }

    /// The GraphQL request as edited; header lines that are not `Name: value` are skipped.
    pub fn editor_graphql_request(&self) -> GraphqlRequest {
        self.build_graphql_request(self.headers.parse_lenient())
    }

    /// Like [`editor_graphql_request`](Self::editor_graphql_request) but fails on an invalid
    /// header line.
    pub fn try_graphql_request(&self) -> Result<GraphqlRequest, String> {
        Ok(self.build_graphql_request(self.headers.parse()?))
    }

    fn build_graphql_request(&self, headers: Vec<Header>) -> GraphqlRequest {
        let variables = self.variables_text();
        GraphqlRequest {
            url: self.url_text().trim().to_owned(),
            headers,
            query: self.body_text(),
            variables: (!variables.trim().is_empty()).then_some(variables),
        }
    }

    pub fn variables_text(&self) -> String {
        self.variables.lines().join("\n")
    }

    /// The gRPC request as edited; metadata lines that are not `Name: value` are skipped.
    pub fn editor_grpc_request(&self) -> GrpcRequest {
        self.build_grpc_request(self.headers.parse_lenient())
    }

    /// Like [`editor_grpc_request`](Self::editor_grpc_request) but fails on an invalid line.
    pub fn try_grpc_request(&self) -> Result<GrpcRequest, String> {
        Ok(self.build_grpc_request(self.headers.parse()?))
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
            Kind::Http => RequestView {
                body_kind: self.body_kind,
                ..(&self.editor_request()).into()
            },
            Kind::Grpc => (&Request::Grpc(self.editor_grpc_request())).into(),
            Kind::Ws => (&Request::Ws(self.editor_ws_request())).into(),
            Kind::Graphql => (&Request::Graphql(self.editor_graphql_request())).into(),
        }
    }

    /// The focus value of the editor the request's lower box currently shows.
    pub fn editor_focus(&self) -> Focus {
        match self.request_tab {
            RequestTab::Body => Focus::Body,
            RequestTab::Variables => Focus::Variables,
            RequestTab::Headers => Focus::Headers,
            RequestTab::Params => Focus::Params,
            RequestTab::Auth => Focus::Auth,
        }
    }

    /// Panels in display order; panel `n` of the UI is entry `n - 1`.
    pub fn panels(&self) -> Vec<Focus> {
        if self.center_tab() == Some(CenterTab::Metadata) {
            return vec![Focus::List, Focus::MetaTitle, Focus::MetaDescription];
        }
        let mut panels = vec![Focus::List];
        if self.left_tab == LeftTab::Variables {
            panels.push(Focus::Captures);
        }
        panels.push(Focus::Url);
        match self.kind {
            Kind::Grpc => panels.extend([Focus::Schema, Focus::Method]),
            Kind::Graphql => panels.push(Focus::Method),
            _ => {}
        }
        panels.extend([self.editor_focus(), Focus::Response]);
        panels
    }

    pub fn set_request_tab(&mut self, tab: RequestTab) {
        self.request_tab = tab;
        if matches!(
            self.focus,
            Focus::Body | Focus::Variables | Focus::Headers | Focus::Params | Focus::Auth
        ) {
            self.focus = self.editor_focus();
        }
    }

    pub fn selected_summary(&self) -> Option<&Summary> {
        self.history.get(self.history_sel)
    }

    /// Index into [`App::projects`] of the project whose context is open.
    fn project_ctx_index(&self) -> Option<usize> {
        let id = self.project_ctx.as_ref()?;
        self.projects.iter().position(|p| &p.id == id)
    }

    /// The tree lines of the Projects tab. Outside a project's context: one closed line
    /// per project. Inside one: that project (open), then its directories (nested) and
    /// requests.
    pub fn project_rows(&self) -> Vec<ProjectRow> {
        fn request_rows(
            out: &mut Vec<ProjectRow>,
            requests: &[crate::model::Request],
            depth: usize,
            project: usize,
            parent: Option<&str>,
        ) {
            for r in requests {
                let method = r
                    .api
                    .as_rest()
                    .map(|rest| rest.method.as_str().to_owned())
                    .unwrap_or_default();
                out.push(ProjectRow {
                    depth,
                    id: r.id.clone(),
                    name: r.name.clone(),
                    description: r.description.clone(),
                    kind: ProjectRowKind::Request { method },
                    project,
                    parent: parent.map(str::to_owned),
                });
            }
        }
        fn dir_rows(
            out: &mut Vec<ProjectRow>,
            open: &HashSet<String>,
            dirs: &[Directory],
            depth: usize,
            project: usize,
            parent: Option<&str>,
        ) {
            for d in dirs {
                let is_open = open.contains(&d.id);
                out.push(ProjectRow {
                    depth,
                    id: d.id.clone(),
                    name: d.name.clone(),
                    description: d.description.clone(),
                    kind: ProjectRowKind::Directory { open: is_open },
                    project,
                    parent: parent.map(str::to_owned),
                });
                if is_open {
                    dir_rows(out, open, &d.directories, depth + 1, project, Some(&d.id));
                    request_rows(out, &d.requests, depth + 1, project, Some(&d.id));
                }
            }
        }

        let ctx = self.project_ctx_index();
        let mut out = Vec::new();
        for (pi, p) in self.projects.iter().enumerate() {
            if ctx.is_some_and(|c| c != pi) {
                continue;
            }
            let is_open = ctx == Some(pi);
            out.push(ProjectRow {
                depth: 0,
                id: p.id.clone(),
                name: p.name.clone(),
                description: p.description.clone(),
                kind: ProjectRowKind::Project { open: is_open },
                project: pi,
                parent: None,
            });
            if is_open {
                dir_rows(&mut out, &self.open_nodes, &p.directories, 1, pi, None);
                request_rows(&mut out, &p.requests, 1, pi, None);
            }
        }
        out
    }

    /// Everything the Projects list shows. Outside a project: "Create project" and the
    /// projects. Inside one: its tree, then the create-directory and create-request
    /// rows; no other project can be created there.
    pub fn panel_rows(&self) -> Vec<PanelRow> {
        let nodes = self.project_rows().into_iter().map(PanelRow::Node);
        if self.project_ctx_index().is_some() {
            nodes
                .chain([PanelRow::CreateDirectory, PanelRow::CreateRequest])
                .collect()
        } else {
            std::iter::once(PanelRow::CreateProject)
                .chain(nodes)
                .collect()
        }
    }

    /// The client request of the selected Projects row, if it is a request.
    pub fn selected_project_request(&self) -> Option<Request> {
        let row = self.selected_project_row()?;
        if !matches!(row.kind, ProjectRowKind::Request { .. }) {
            return None;
        }
        let project = self.projects.get(row.project)?;
        let request = project
            .all_requests()
            .into_iter()
            .find(|r| r.id == row.id)?;
        Some(to_client_request(project, request))
    }

    /// The center tab to show: `Some` only while the Projects tab is showing.
    pub fn center_tab(&self) -> Option<CenterTab> {
        (self.tab == Tab::Projects).then_some(self.center_tab)
    }

    /// The selected tree line of the Projects list; `None` on a create row.
    pub fn selected_project_row(&self) -> Option<ProjectRow> {
        match self.panel_rows().into_iter().nth(self.projects_sel)? {
            PanelRow::Node(r) => Some(r),
            _ => None,
        }
    }

    /// Remembers the tree line under the selection: new directories and requests go
    /// next to it, or inside it when it is a directory.
    fn note_node(&mut self) {
        if self.tab == Tab::Projects
            && let Some(r) = self.selected_project_row()
        {
            self.last_node = Some(r.id);
        }
    }

    /// What clicking or pressing Enter on `row` of the Projects list does: starts naming
    /// a project, creates a directory or request, enters or leaves a project, or opens or
    /// closes a directory.
    fn open_panel_row(&mut self, row: usize) {
        match self.panel_rows().into_iter().nth(row) {
            Some(PanelRow::CreateProject) => self.naming = Some(single_line("")),
            Some(PanelRow::CreateDirectory) => self.create_node(true),
            Some(PanelRow::CreateRequest) => self.create_node(false),
            Some(PanelRow::Node(r)) => match r.kind {
                ProjectRowKind::Project { .. } => {
                    self.last_node = None;
                    if self.project_ctx.take().is_some() {
                        // the list gains the "Create project" row above the projects
                        self.projects_sel = r.project + 1;
                    } else {
                        self.project_ctx = Some(r.id);
                        self.projects_sel = 0;
                    }
                }
                ProjectRowKind::Directory { .. } => {
                    if !self.open_nodes.remove(&r.id) {
                        self.open_nodes.insert(r.id);
                    }
                }
                ProjectRowKind::Request { .. } => {}
            },
            None => {}
        }
    }

    /// Deletes the selected directory or request; a directory with anything in it asks
    /// first.
    fn delete_selected(&mut self) {
        let Some(row) = self.selected_project_row() else {
            return;
        };
        let Some(project) = self.projects.get_mut(row.project) else {
            return;
        };
        match row.kind {
            ProjectRowKind::Project { .. } => {}
            ProjectRowKind::Request { .. } => self.remove_node(&row.id),
            ProjectRowKind::Directory { .. } => {
                let (dirs, requests) = project
                    .directory_mut(&row.id)
                    .map_or((0, 0), |d| d.contents());
                if dirs + requests == 0 {
                    self.remove_node(&row.id);
                } else {
                    let plural = |n: usize, word: &str| {
                        format!("{n} {word}{}", if n == 1 { "" } else { "s" })
                    };
                    self.confirm = Some(Confirm {
                        message: format!(
                            "Delete \"{}\" with {} and {}?",
                            row.name,
                            plural(requests, "request"),
                            plural(dirs, "directory").replace("directorys", "directories"),
                        ),
                        ok: false,
                        node: row.id,
                    });
                }
            }
        }
    }

    fn remove_node(&mut self, id: &str) {
        if self.bound.as_deref() == Some(id) {
            self.bound = None;
            self.bound_dirty = false;
        }
        for pi in 0..self.projects.len() {
            if self.projects[pi].remove_node(id) {
                self.persist_project(pi);
                break;
            }
        }
        self.open_nodes.remove(id);
        self.last_node = None;
        self.last_click = None;
        self.projects_sel = self
            .projects_sel
            .min(self.panel_rows().len().saturating_sub(1));
        self.note_node();
    }

    /// Keys while the delete question is open: arrows pick Ok or Cancel, Enter answers.
    fn confirm_key(&mut self, code: KeyCode, ctrl: bool) {
        let Some(confirm) = &mut self.confirm else {
            return;
        };
        match code {
            KeyCode::Char('c') if ctrl => self.should_quit = true,
            KeyCode::Left | KeyCode::Char('h') => confirm.ok = true,
            KeyCode::Right | KeyCode::Char('l') => confirm.ok = false,
            KeyCode::Tab | KeyCode::BackTab => confirm.ok = !confirm.ok,
            KeyCode::Esc => self.confirm = None,
            KeyCode::Enter => {
                if let Some(c) = self.confirm.take()
                    && c.ok
                {
                    self.remove_node(&c.node);
                }
            }
            _ => {}
        }
    }

    /// Keys typed while a new project's name is being entered.
    fn naming_key(&mut self, key: KeyEvent) {
        let Some(name) = &mut self.naming else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.naming = None,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Enter => {
                let text = name.lines().join(" ").trim().to_owned();
                if !text.is_empty() {
                    self.create_project(&text);
                }
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {}
            _ => crate::tui::headers::edit(name, key),
        }
    }

    /// Stores a new empty project and enters its context.
    fn create_project(&mut self, name: &str) {
        let project = Project::new(name);
        self.project_ctx = Some(project.id.clone());
        self.projects.push(project);
        self.persist_project(self.projects.len() - 1);
        self.naming = None;
        self.projects_sel = 0;
        self.last_node = None;
        self.last_click = None;
        self.center_tab = CenterTab::Request;
        self.notice = Some(format!("created project {name}"));
    }

    /// Where a new directory or request goes: inside the directory last selected, next
    /// to the request last selected, else at the project root.
    fn target_parent(&self) -> Option<String> {
        let id = self.last_node.as_ref()?;
        let row = self.project_rows().into_iter().find(|r| &r.id == id)?;
        match row.kind {
            ProjectRowKind::Directory { .. } => Some(row.id),
            ProjectRowKind::Request { .. } => row.parent,
            ProjectRowKind::Project { .. } => None,
        }
    }

    /// Adds a directory or request to the open project and selects it.
    fn create_node(&mut self, directory: bool) {
        let Some(pi) = self.project_ctx_index() else {
            self.notice = Some("open a project first".into());
            return;
        };
        let parent = self.target_parent();
        let id = if directory {
            let dir = Directory::new("New directory");
            let id = dir.id.clone();
            self.projects[pi].add_directory(parent.as_deref(), dir);
            id
        } else {
            let request = crate::model::Request::rest(
                "New request",
                crate::model::RestRequest::new(crate::model::Method::Get, ""),
            );
            let id = request.id.clone();
            self.projects[pi].add_request(parent.as_deref(), request);
            id
        };
        if let Some(parent) = parent {
            self.open_nodes.insert(parent);
        }
        self.persist_project(pi);
        if let Some(i) = self
            .panel_rows()
            .iter()
            .position(|r| matches!(r, PanelRow::Node(n) if n.id == id))
        {
            self.projects_sel = i;
        }
        self.last_node = Some(id);
        self.last_click = None;
        self.scroll = 0;
        self.focus = Focus::List;
    }

    /// Queues project `index` to be written to the database.
    fn persist_project(&self, index: usize) {
        if let Some(p) = self.projects.get(index) {
            let _ = self.saver.send(p.clone());
        }
    }

    /// Makes the Metadata editors show the selected tree line.
    pub fn sync_meta(&mut self) {
        let row = if self.tab == Tab::Projects {
            self.selected_project_row()
        } else {
            None
        };
        let id = row.as_ref().map(|r| r.id.clone());
        if id == self.meta_for {
            return;
        }
        self.meta_for = id;
        let (name, description) = row.map_or_else(Default::default, |r| {
            (r.name, r.description.unwrap_or_default())
        });
        self.meta_title = single_line(&name);
        let lines: Vec<String> = description.lines().map(str::to_owned).collect();
        self.meta_desc = TextArea::new(if lines.is_empty() {
            vec![String::new()]
        } else {
            lines
        });
        self.meta_desc.move_cursor(CursorMove::Bottom);
        self.meta_desc.move_cursor(CursorMove::End);
    }

    /// Writes the Metadata editors into the project and queues it for saving, so the
    /// tree shows the new title right away.
    fn apply_meta(&mut self) {
        let Some(id) = self.meta_for.clone() else {
            return;
        };
        let name = self.meta_title.lines().join(" ");
        let description = self.meta_desc.lines().join("\n");
        let description = (!description.trim().is_empty()).then_some(description);
        for pi in 0..self.projects.len() {
            if self.projects[pi].set_meta(&id, &name, description.clone()) {
                self.persist_project(pi);
                break;
            }
        }
    }

    fn meta_key(&mut self, key: KeyEvent, title: bool) {
        if title {
            if key.code == KeyCode::Enter {
                return self.set_focus(Focus::MetaDescription);
            }
            crate::tui::headers::edit(&mut self.meta_title, key);
        } else {
            crate::tui::headers::edit(&mut self.meta_desc, key);
        }
        self.apply_meta();
    }

    /// Copies the editor's request into the project request it is bound to, when it
    /// changed, and saves once focus is somewhere other than an editor box.
    fn sync_bound(&mut self) {
        if let Some(id) = self.bound.clone()
            && self.kind == Kind::Http
        {
            let edited = self.editor_request();
            if self.bound_last.as_ref() != Some(&edited) {
                self.bound_last = Some(edited.clone());
                let found = self
                    .projects
                    .iter()
                    .position(|p| p.all_requests().iter().any(|r| r.id == id));
                match found {
                    None => self.bound = None,
                    Some(pi) => {
                        if let Some(request) = self.projects[pi].request_mut(&id) {
                            let crate::model::ApiRequest::Rest(old) = &request.api;
                            let new = from_client_request(old, &edited);
                            if *old != new {
                                request.api = crate::model::ApiRequest::Rest(new);
                                self.bound_dirty = true;
                            }
                        }
                    }
                }
            }
        }
        if !matches!(
            self.focus,
            Focus::Url
                | Focus::Body
                | Focus::Variables
                | Focus::Headers
                | Focus::Params
                | Focus::Auth
                | Focus::Schema
                | Focus::Method
        ) {
            self.flush_bound();
        }
    }

    /// Writes the bound request's project to the database if it has unsaved edits.
    fn flush_bound(&mut self) {
        if !self.bound_dirty {
            return;
        }
        self.bound_dirty = false;
        if let Some(id) = &self.bound
            && let Some(pi) = self
                .projects
                .iter()
                .position(|p| p.all_requests().iter().any(|r| &r.id == id))
        {
            self.persist_project(pi);
        }
    }

    /// Selects a center tab, moving focus off boxes the tab does not show.
    fn set_center_tab(&mut self, tab: CenterTab) {
        self.center_tab = tab;
        let meta = matches!(self.focus, Focus::MetaTitle | Focus::MetaDescription);
        if (tab == CenterTab::Metadata && !meta && self.focus != Focus::List)
            || (tab == CenterTab::Request && meta)
        {
            self.set_focus(Focus::List);
        }
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
                Tab::Projects => {
                    if let Some(request) = self.selected_project_request() {
                        return View {
                            request: (&request).into(),
                            outcome: self.live.as_ref(),
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

    /// Advances the loader animation.
    pub fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    // ---- background results -------------------------------------------

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Sent(outcome) => {
                self.sending = false;
                self.scroll = 0;
                self.log_outcome(&outcome);
                if let Outcome::Response(r) = &outcome {
                    self.capture(&r.body);
                }
                self.live = Some(outcome);
            }
            Msg::GrpcEvent(event) => self.apply_event(event),
            Msg::WsEvent(event) => self.apply_ws_event(event),
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
            Msg::GraphqlLoaded(url, result) => {
                self.schema_loading = false;
                match result {
                    Ok(schema) => {
                        let loaded = format!(
                            "introspected {} operations, {} types",
                            schema.operations().len(),
                            schema.type_count()
                        );
                        self.log.push(Level::Info, loaded.clone());
                        self.notice = Some(loaded);
                        self.graphql = Some(LoadedGraphql { url, schema });
                    }
                    Err(message) => {
                        self.log
                            .push(Level::Error, format!("introspection: {message}"));
                        self.status = Some(message);
                    }
                }
            }
            Msg::History(list) => {
                self.history = list;
                self.history_sel = self.history_sel.min(self.history.len().saturating_sub(1));
                self.request_preview();
            }
            Msg::Projects(list) => {
                self.projects = list;
                self.projects_sel = self
                    .projects_sel
                    .min(self.panel_rows().len().saturating_sub(1));
                self.sync_meta();
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
            Outcome::Ws(w) => match (&w.error, &w.close) {
                (Some(error), _) => self.log.push(Level::Error, format!("failed: {error}")),
                (None, Some(close)) => self.log.push(
                    Level::Info,
                    format!("<- {close} after {} ms", w.elapsed.as_millis()),
                ),
                (None, None) => self.log.push(
                    Level::Info,
                    format!("<- closed after {} ms", w.elapsed.as_millis()),
                ),
            },
            Outcome::Error(message) => self.log.push(Level::Error, format!("failed: {message}")),
        }
    }

    // ---- input ---------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.key_inner(key);
        self.sync_bound();
        self.sync_meta();
        // a query pasted or loaded into the address moves once the address loses focus
        if self.focus != Focus::Url {
            self.absorb_query();
        }
    }

    fn key_inner(&mut self, key: KeyEvent) {
        self.handle_key_event(key);
        self.refresh_suggestions();
    }

    fn handle_key_event(&mut self, key: KeyEvent) {
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
        if self.help.is_some() {
            return self.help_key(key.code, ctrl);
        }

        if self.confirm.is_some() {
            return self.confirm_key(key.code, ctrl);
        }
        if self.naming.is_some() && self.focus == Focus::List {
            return self.naming_key(key);
        }

        // In the Projects list: Ctrl+Shift+R / Ctrl+Shift+D add a request / directory to
        // the open project; Backspace deletes the selected directory or request.
        if ctrl
            && self.tab == Tab::Projects
            && self.focus == Focus::List
            && let KeyCode::Char(c) = key.code
        {
            let shifted = key.modifiers.contains(KeyModifiers::SHIFT) || c.is_ascii_uppercase();
            match (c.to_ascii_lowercase(), shifted) {
                ('r', true) => return self.create_node(false),
                ('d', true) => return self.create_node(true),
                _ => {}
            }
        }

        // `?` types a character in text fields, so F1 opens the menu from anywhere.
        let typing = matches!(
            self.focus,
            Focus::Url
                | Focus::Body
                | Focus::Variables
                | Focus::Headers
                | Focus::Params
                | Focus::Auth
                | Focus::Schema
                | Focus::Captures
                | Focus::MetaTitle
                | Focus::MetaDescription
        );
        if key.code == KeyCode::F(1) || (key.code == KeyCode::Char('?') && !typing && !ctrl) {
            return self.help = Some(0);
        }

        // Ctrl/Cmd+Enter sends from any panel; terminals without the kitty keyboard
        // protocol report Ctrl+Enter as a plain Enter or Ctrl+J, which the editors keep.
        if key.code == KeyCode::Enter
            && key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        {
            return self.send();
        }

        // Ctrl+<n> jumps to panel n from anywhere; a bare <n> does so where no text is typed.
        if let KeyCode::Char(c @ '1'..='9') = key.code {
            let ctrl_only = key.modifiers == KeyModifiers::CONTROL;
            if (ctrl_only || (!typing && key.modifiers.is_empty()))
                && let Some(&target) = self.panels().get(c as usize - '1' as usize)
            {
                return self.set_focus(target);
            }
        }

        if self.suggestions_open() && self.suggestion_key(key.code) {
            return;
        }

        let action = match (key.code, ctrl) {
            (KeyCode::Char('c'), true) => Some(Action::Quit),
            (KeyCode::Char('s'), true) => Some(Action::Send),
            (KeyCode::Char('n'), true) => Some(Action::NewRequest),
            (KeyCode::Char('p'), true) if matches!(self.kind, Kind::Grpc | Kind::Graphql) => {
                Some(Action::PickMethod)
            }
            (KeyCode::Char('p'), true) if self.kind == Kind::Http => Some(Action::PrevMethod),
            (KeyCode::Char('t'), true) => Some(Action::ToggleRequestTab),
            (KeyCode::Char('y'), true) if self.kind == Kind::Http => Some(Action::NextBodyType),
            (KeyCode::Char('g'), true) => Some(Action::ToggleKind),
            (KeyCode::Char('l'), true) if matches!(self.kind, Kind::Grpc | Kind::Graphql) => {
                Some(Action::LoadSchema)
            }
            (KeyCode::Char('x'), true) => Some(Action::Cancel),
            (KeyCode::Char('e'), true) => Some(Action::OpenLog),
            (KeyCode::Char('v'), true) => Some(Action::ToggleVariablesPanel),
            (KeyCode::Char('b'), true) => Some(Action::ToggleVars),
            (KeyCode::Char('o'), true) if self.kind == Kind::Grpc => {
                Some(Action::ToggleSchemaSource)
            }
            (KeyCode::Char('k'), true) if self.kind == Kind::Grpc => Some(Action::ToggleTls),
            (KeyCode::Tab, _) => Some(Action::NextPanel),
            (KeyCode::BackTab, _) => Some(Action::PrevPanel),
            (KeyCode::Esc, _) => Some(Action::BackToList),
            _ => None,
        };
        if let Some(action) = action {
            return self.run_action(action);
        }
        match key.code {
            KeyCode::PageDown => return self.scroll = self.scroll.saturating_add(5),
            KeyCode::PageUp => return self.scroll = self.scroll.saturating_sub(5),
            _ => {}
        }

        match self.focus {
            Focus::List => self.list_key(key.code),
            Focus::Url => match key.code {
                KeyCode::Enter => self.send(),
                // the query belongs to the "URL Params" tab: `?` jumps there
                KeyCode::Char('?')
                    if self.kind == Kind::Http
                        && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    self.request_tab = RequestTab::Params;
                    self.params.row = self.params.rows.len() - 1;
                    self.params.col = Col::Name;
                    self.set_focus(Focus::Params);
                }
                _ => {
                    crate::tui::headers::edit(&mut self.url, key);
                }
            },
            Focus::Body => self.body_key(key),
            Focus::Variables => {
                crate::tui::headers::edit(&mut self.variables, key);
            }
            Focus::Headers => {
                self.headers.key(key);
            }
            Focus::Params => {
                self.params.key(key);
            }
            Focus::Auth => self.auth.key(key),
            Focus::Schema => self.schema_input(key),
            Focus::Method => self.method_key(key.code),
            Focus::Response => self.response_key(key.code),
            Focus::Captures => self.captures_key(key),
            Focus::MetaTitle => self.meta_key(key, true),
            Focus::MetaDescription => self.meta_key(key, false),
        }
    }

    /// Keys typed in the Variables panel. `Ctrl+D` removes the row and its variable,
    /// `Ctrl+F` freezes or unfreezes the row's variable.
    fn captures_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let removing = key.code == KeyCode::Char('d') && ctrl;
        let name = self.captures.rows[self.captures.row].name_text();
        if key.code == KeyCode::Char('f') && ctrl {
            let name = name.trim().trim_start_matches('$');
            self.notice = Some(match self.vars.toggle_frozen(name) {
                Some(true) => format!("froze ${name}"),
                Some(false) => format!("unfroze ${name}"),
                None => format!("${name} has no value to freeze"),
            });
            return;
        }
        self.captures.key(key);
        if key.code == KeyCode::Enter {
            self.register_literals();
        }
        if removing {
            self.vars.remove(name.trim());
            self.notice = Some(format!("removed ${}", name.trim()));
        }
    }

    /// Sets the variables whose capture is a literal (a value not starting with `.`): they
    /// need no response, so they exist as soon as the row is entered or left.
    fn register_literals(&mut self) {
        for (name, expr) in self.captures.pairs(false).unwrap_or_default() {
            let name = name.trim().trim_start_matches('$');
            if !vars::valid_name(name) || expr.trim().is_empty() || expr.trim().starts_with('.') {
                continue;
            }
            if let Ok(value) = vars::capture("", &expr) {
                self.vars.set(name, value);
            }
        }
    }

    /// Runs the captures against the body of a response.
    fn capture(&mut self, body: &str) {
        for (name, expr) in self.captures.pairs(false).unwrap_or_default() {
            let name = name.trim().trim_start_matches('$').to_owned();
            if !vars::valid_name(&name) {
                self.log.push(
                    Level::Error,
                    format!("capture `{name}`: not a valid variable name"),
                );
                continue;
            }
            if expr.trim().is_empty() {
                continue;
            }
            if self.vars.is_frozen(&name) {
                self.log
                    .push(Level::Info, format!("${name} is frozen, kept"));
                continue;
            }
            match vars::capture(body, &expr) {
                Ok(value) => {
                    self.log.push(Level::Info, format!("captured ${name}"));
                    self.vars.set(&name, value);
                }
                Err(e) => self.log.push(Level::Error, format!("capture ${name}: {e}")),
            }
        }
    }

    fn toggle_variables_panel(&mut self) {
        match self.left_tab {
            LeftTab::Logs => {
                self.left_tab = LeftTab::Variables;
                self.set_focus(Focus::Captures);
            }
            LeftTab::Variables => {
                self.left_tab = LeftTab::Logs;
                if self.focus == Focus::Captures {
                    self.set_focus(Focus::List);
                }
            }
        }
    }

    /// Flips `$variables` for the section the request box shows (Auth goes out as a header).
    fn toggle_vars(&mut self) {
        if !ui::vars_tab(self.kind, self.request_tab) {
            return;
        }
        let (name, flag) = match self.request_tab {
            RequestTab::Params => ("URL params", &mut self.vars_params),
            RequestTab::Headers | RequestTab::Auth => ("headers", &mut self.vars_headers),
            _ => ("body", &mut self.vars_body),
        };
        *flag = !*flag;
        let on = *flag;
        self.notice = Some(format!(
            "variables in the {name}: {}",
            if on { "on" } else { "off" }
        ));
    }

    /// `view` as it would be sent: `$name` replaced where the section's switch is on.
    pub fn with_vars(&self, view: &RequestView) -> RequestView {
        let mut out = view.clone();
        if self.vars_headers {
            for h in &mut out.headers {
                h.value = self.vars.substitute(&h.value, false);
            }
        }
        if out.kind == Kind::Http {
            if self.vars_params
                && let Some((base, query)) = out.url.split_once('?')
            {
                // `$` is percent-encoded in the address
                let query = self.vars.substitute(&query.replace("%24", "$"), false);
                out.url = format!("{base}?{query}");
            }
            if self.vars_body {
                let json = matches!(out.body_kind, BodyKind::Json | BodyKind::JsonRaw);
                out.body = self.vars.substitute(&out.body, json);
            }
        }
        out
    }

    /// `headers` with `$name` replaced in the values, when the headers switch is on.
    fn with_header_vars(&self, headers: &[Header]) -> Vec<Header> {
        let mut out = headers.to_vec();
        if self.vars_headers {
            for h in &mut out {
                h.value = self.vars.substitute(&h.value, false);
            }
        }
        out
    }

    /// Whether `$variables` are on for the section `tab` shows.
    pub fn vars_on(&self, tab: RequestTab) -> bool {
        match tab {
            RequestTab::Params => self.vars_params,
            RequestTab::Headers | RequestTab::Auth => self.vars_headers,
            _ => self.vars_body,
        }
    }

    /// Keys typed in the body editor, which depends on the body kind.
    fn body_key(&mut self, key: KeyEvent) {
        if self.kind != Kind::Http {
            crate::tui::headers::edit(&mut self.body, key);
            return;
        }
        match self.body_kind {
            BodyKind::Text | BodyKind::JsonRaw => {
                crate::tui::headers::edit(&mut self.body, key);
            }
            BodyKind::Json => self.json.key(key),
            BodyKind::Form => self.form.key(key),
            BodyKind::Multipart => self.multipart.key(key),
            BodyKind::File => {
                if key.code != KeyCode::Enter {
                    crate::tui::headers::edit(&mut self.file, key);
                }
            }
        }
    }

    pub fn run_action(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::Send => self.send(),
            Action::PickMethod => self.open_picker(),
            Action::NewRequest => self.new_request(),
            Action::NextMethod => self.method = self.method.next(),
            Action::PrevMethod => self.method = self.method.prev(),
            Action::ToggleRequestTab => self.toggle_request_tab(),
            Action::NextBodyType => self.next_body_kind(),
            Action::ToggleKind => self.toggle_kind(),
            Action::LoadSchema => self.load_schema(),
            Action::Cancel => self.cancel_call(),
            Action::OpenLog => self.open_log(),
            Action::ToggleVariablesPanel => self.toggle_variables_panel(),
            Action::ToggleVars => self.toggle_vars(),
            Action::ToggleSchemaSource => self.toggle_schema_mode(),
            Action::ToggleTls => self.insecure = !self.insecure,
            Action::NextPanel if self.center_tab() == Some(CenterTab::Metadata) => {
                self.set_focus(match self.focus {
                    Focus::List => Focus::MetaTitle,
                    Focus::MetaTitle => Focus::MetaDescription,
                    _ => Focus::List,
                });
            }
            Action::PrevPanel if self.center_tab() == Some(CenterTab::Metadata) => {
                self.set_focus(match self.focus {
                    Focus::List => Focus::MetaDescription,
                    Focus::MetaDescription => Focus::MetaTitle,
                    _ => Focus::List,
                });
            }
            Action::NextPanel => {
                self.set_focus(next_focus(
                    self.focus,
                    self.editor_focus(),
                    self.kind,
                    self.left_tab == LeftTab::Variables,
                ));
            }
            Action::PrevPanel => {
                self.set_focus(prev_focus(
                    self.focus,
                    self.editor_focus(),
                    self.kind,
                    self.left_tab == LeftTab::Variables,
                ));
            }
            Action::BackToList => self.set_focus(Focus::List),
        }
    }

    /// The rows of the keybindings menu for the current mode.
    pub fn commands(&self) -> Vec<&'static Command> {
        commands_for(self.kind)
    }

    fn help_key(&mut self, code: KeyCode, ctrl: bool) {
        let len = self.commands().len();
        let Some(sel) = self.help else {
            return;
        };
        match (code, ctrl) {
            (KeyCode::Char('c'), true) => self.should_quit = true,
            (KeyCode::Esc | KeyCode::Char('q' | '?') | KeyCode::F(1), _) => self.help = None,
            (KeyCode::Down | KeyCode::Char('j'), _) => {
                self.help = Some((sel + 1).min(len.saturating_sub(1)));
            }
            (KeyCode::Up | KeyCode::Char('k'), _) => self.help = Some(sel.saturating_sub(1)),
            (KeyCode::PageDown, _) => self.help = Some((sel + 8).min(len.saturating_sub(1))),
            (KeyCode::PageUp, _) => self.help = Some(sel.saturating_sub(8)),
            (KeyCode::Home | KeyCode::Char('g'), _) => self.help = Some(0),
            (KeyCode::End | KeyCode::Char('G'), _) => self.help = Some(len.saturating_sub(1)),
            (KeyCode::Enter, _) => {
                self.help = None;
                if let Some(command) = self.commands().get(sel) {
                    self.run_action(command.action);
                }
            }
            _ => {}
        }
    }

    fn suggestions_open(&self) -> bool {
        !self.suggestions.items.is_empty()
    }

    /// Keys that work on the suggestions; `false` when the key is not one of them.
    fn suggestion_key(&mut self, code: KeyCode) -> bool {
        let count = self.suggestions.items.len();
        match code {
            KeyCode::Down => {
                self.suggestions.sel =
                    Some(self.suggestions.sel.map_or(0, |i| (i + 1).min(count - 1)));
            }
            KeyCode::Up => {
                self.suggestions.sel = match self.suggestions.sel {
                    None | Some(0) => None,
                    Some(i) => Some(i - 1),
                };
            }
            KeyCode::Tab => self.accept_suggestion(self.suggestions.sel.unwrap_or(0)),
            KeyCode::Enter if self.suggestions.sel.is_some() => {
                self.accept_suggestion(self.suggestions.sel.unwrap_or(0));
            }
            _ => return false,
        }
        true
    }

    /// Replaces the input with suggestion `index`; a directory keeps the suggestions open on
    /// its content.
    pub fn accept_suggestion(&mut self, index: usize) {
        let Some(suggestion) = self.suggestions.items.get(index) else {
            return;
        };
        let text = suggestion.text.clone();
        match self.suggestions.target {
            SuggestTarget::Schema => self.schema_path = single_line(&text),
            SuggestTarget::Body => match self.body_kind {
                BodyKind::Multipart => self.multipart.set_value(&text),
                _ => self.file = single_line(&text),
            },
        }
        self.suggestions = Suggestions::default();
        self.suggested_for = None;
    }

    /// The `@file` input under the cursor in the body editor, as typed.
    fn body_file_input(&self) -> Option<String> {
        if self.kind != Kind::Http
            || self.focus != Focus::Body
            || self.request_tab != RequestTab::Body
        {
            return None;
        }
        let text = match self.body_kind {
            BodyKind::Multipart if self.multipart.col == Col::Value => {
                self.multipart.cell().lines().join("")
            }
            BodyKind::File => self.file_text(),
            _ => return None,
        };
        (text.starts_with('@') && !text.starts_with("@@")).then_some(text)
    }

    /// Recomputes the suggestions when the input they complete changed; they only exist
    /// while that input has focus: the gRPC schema file, or an `@file` of the body.
    fn refresh_suggestions(&mut self) {
        let free = self.modal.is_none() && self.picker.is_none();
        let schema = self.kind == Kind::Grpc
            && self.schema_mode == SchemaMode::File
            && self.focus == Focus::Schema
            && free;
        let body = if free { self.body_file_input() } else { None };
        let (target, text) = match (schema, body) {
            (true, _) => (SuggestTarget::Schema, self.schema_path_text()),
            (false, Some(text)) => (SuggestTarget::Body, text),
            (false, None) => {
                self.suggestions = Suggestions::default();
                self.suggested_for = None;
                return;
            }
        };
        if self.suggested_for.as_deref() == Some(text.as_str()) && self.suggestions.target == target
        {
            return;
        }
        let mut items = match target {
            SuggestTarget::Schema => complete::complete(&text),
            SuggestTarget::Body => complete::complete_files(&text[1..])
                .into_iter()
                .map(|s| Suggestion {
                    text: format!("@{}", s.text),
                    dir: s.dir,
                })
                .collect(),
        };
        // the input already is that file: nothing left to suggest
        if items.len() == 1 && items[0].text == text {
            items.clear();
        }
        self.suggested_for = Some(text);
        self.suggestions = Suggestions {
            target,
            items,
            sel: None,
        };
    }

    fn schema_input(&mut self, key: KeyEvent) {
        match (key.code, self.schema_mode) {
            (KeyCode::Enter, _) => self.load_schema(),
            (_, SchemaMode::File) => {
                crate::tui::headers::edit(&mut self.schema_path, key);
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
        if self.kind == Kind::Graphql {
            return self.introspect();
        }
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

    /// GraphQL: asks the endpoint for its schema in the background.
    fn introspect(&mut self) {
        if self.schema_loading {
            return;
        }
        let request = match self.try_graphql_request() {
            Ok(request) => request,
            Err(message) => {
                self.status = Some(message);
                self.request_tab = RequestTab::Headers;
                self.set_focus(Focus::Headers);
                return;
            }
        };
        if request.url.is_empty() {
            self.status = Some("enter the endpoint URL first".into());
            return;
        }
        self.status = None;
        self.log
            .push(Level::Info, format!("introspecting {}", request.url));
        self.schema_loading = true;
        let (client, tx) = (self.client.clone(), self.tx.clone());
        tokio::spawn(async move {
            let result = introspect(&client, &request.url, &request.headers).await;
            let _ = tx.send(Msg::GraphqlLoaded(request.url, result));
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

    /// The introspected GraphQL schema, if it came from the endpoint the editor has now.
    pub fn current_graphql(&self) -> Option<&GraphqlSchema> {
        let loaded = self.graphql.as_ref()?;
        (loaded.url == self.url_text().trim()).then_some(&loaded.schema)
    }

    /// Opens the method picker over the loaded schema.
    pub fn open_picker(&mut self) {
        match self.kind {
            Kind::Grpc if self.current_schema().is_none() => {
                self.status = Some("load the schema first (Ctrl+L)".into());
                return;
            }
            Kind::Graphql if self.current_graphql().is_none() => {
                self.status = Some("introspect the endpoint first (Ctrl+L)".into());
                return;
            }
            Kind::Grpc | Kind::Graphql => {}
            _ => return,
        }
        self.status = None;
        let origin =
            (self.kind == Kind::Graphql && self.body_text().trim().is_empty()).then(|| {
                PickerOrigin {
                    query: self.body_text(),
                    variables: self.variables_text(),
                }
            });
        self.picker = Some(Picker {
            origin,
            ..Picker::default()
        });
        self.preview_selected();
    }

    /// Closes the picker without picking; a previewed operation is taken back out of the
    /// editors.
    pub fn close_picker(&mut self) {
        let Some(picker) = self.picker.take() else {
            return;
        };
        if let Some(origin) = picker.origin {
            self.set_graphql_editors(&origin.query, &origin.variables);
        }
    }

    /// Puts the highlighted GraphQL operation's skeleton in the editors (hover preview), or
    /// the original texts back when nothing is highlighted. Does nothing when the user had
    /// written a query before opening the picker.
    fn preview_selected(&mut self) {
        let Some(picker) = &self.picker else {
            return;
        };
        let Some(origin) = picker.origin.clone() else {
            return;
        };
        let selected = picker.selected;
        let skeleton = self
            .picker_matches()
            .get(selected)
            .and_then(|item| self.graphql_skeleton(&item.full_name()));
        match skeleton {
            Some((query, variables)) => {
                self.set_graphql_editors(&query, &variables.unwrap_or_default())
            }
            None => self.set_graphql_editors(&origin.query, &origin.variables),
        }
    }

    fn graphql_skeleton(&self, full_name: &str) -> Option<(String, Option<String>)> {
        let operation = GraphqlOperation::parse(full_name)?;
        self.current_graphql()?.skeleton(&operation)
    }

    fn set_graphql_editors(&mut self, query: &str, variables: &str) {
        self.body = TextArea::new(query.lines().map(str::to_owned).collect());
        self.variables = TextArea::new(variables.lines().map(str::to_owned).collect());
    }

    /// The rows the picker lists: those whose `group/name` contains the filter, ignoring
    /// case, in schema order (grouped by service, or queries before mutations).
    pub fn picker_matches(&self) -> Vec<PickerItem> {
        let Some(picker) = &self.picker else {
            return Vec::new();
        };
        let items: Vec<PickerItem> = if self.kind == Kind::Graphql {
            let operations = self.current_graphql().map(GraphqlSchema::operations);
            operations
                .unwrap_or_default()
                .iter()
                .map(|o| PickerItem {
                    group: o.kind.group().to_owned(),
                    name: o.name.clone(),
                    tag: match o.kind {
                        OperationKind::Query => PickerTag::Query,
                        OperationKind::Mutation => PickerTag::Mutation,
                    },
                })
                .collect()
        } else {
            let methods = self.current_schema().map(Schema::methods);
            methods
                .unwrap_or_default()
                .iter()
                .map(|m| PickerItem {
                    group: m.service.clone(),
                    name: m.name.clone(),
                    tag: PickerTag::Call(m.kind),
                })
                .collect()
        };
        let filter = picker.filter.to_lowercase();
        items
            .into_iter()
            .filter(|m| m.full_name().to_lowercase().contains(&filter))
            .collect()
    }

    fn picker_key(&mut self, code: KeyCode, ctrl: bool) {
        match (code, ctrl) {
            (KeyCode::Char('c'), true) => self.should_quit = true,
            (KeyCode::Esc, _) => self.close_picker(),
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
                self.preview_selected();
            }
            (KeyCode::Char(c), false) => {
                if let Some(picker) = &mut self.picker {
                    picker.filter.push(c);
                    picker.selected = 0;
                }
                self.preview_selected();
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
        self.preview_selected();
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
        if let Some(picker) = &mut self.picker {
            picker.selected = index;
        }
        // The hovered operation is already in the editors when the query was empty; a
        // written query is never touched.
        self.preview_selected();
        self.picker = None;
        if self.kind != Kind::Graphql
            && self.body_text().trim().is_empty()
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
            Some(Outcome::Ws(w)) => w.body_text(),
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

    /// Cycles the editor through REST, GraphQL, gRPC and WebSocket. The address/URL, body and
    /// headers/metadata texts are kept.
    pub fn toggle_kind(&mut self) {
        let kind = match self.kind {
            Kind::Http => Kind::Graphql,
            Kind::Graphql => Kind::Grpc,
            Kind::Grpc => Kind::Ws,
            Kind::Ws => Kind::Http,
        };
        self.set_kind(kind);
    }

    pub fn set_kind(&mut self, kind: Kind) {
        if self.kind == kind {
            return;
        }
        self.kind = kind;
        // The editors that only exist in one mode cannot keep the focus.
        let has_method = matches!(kind, Kind::Grpc | Kind::Graphql);
        if !has_method && matches!(self.focus, Focus::Schema | Focus::Method) {
            self.focus = Focus::Url;
        }
        if kind == Kind::Graphql && self.focus == Focus::Schema {
            self.focus = Focus::Method;
        }
        if kind != Kind::Graphql && self.request_tab == RequestTab::Variables {
            self.set_request_tab(RequestTab::Body);
        }
        if kind != Kind::Http && matches!(self.request_tab, RequestTab::Params | RequestTab::Auth) {
            self.set_request_tab(RequestTab::Body);
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
        self.set_request_tab(match (self.request_tab, self.kind) {
            (RequestTab::Body, Kind::Graphql) => RequestTab::Variables,
            (RequestTab::Body, _) => RequestTab::Headers,
            (RequestTab::Variables, _) => RequestTab::Headers,
            (RequestTab::Headers, Kind::Http) => RequestTab::Params,
            (RequestTab::Headers, _) => RequestTab::Body,
            (RequestTab::Params, _) => RequestTab::Auth,
            (RequestTab::Auth, _) => RequestTab::Body,
        });
    }

    fn list_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('y') => self.copy_response(),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Enter => self.activate(),
            KeyCode::Backspace if self.tab == Tab::Projects => self.delete_selected(),
            KeyCode::Char('m') if self.tab == Tab::Projects => {
                self.set_center_tab(CenterTab::Metadata);
            }
            KeyCode::Char('r') if self.tab == Tab::Projects => {
                self.set_center_tab(CenterTab::Request);
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Left | KeyCode::Char('h')
                if self.tab == Tab::Projects =>
            {
                let open = matches!(code, KeyCode::Right | KeyCode::Char('l'));
                let row = self.projects_sel;
                if let Some(r) = self.selected_project_row() {
                    let is_open = match r.kind {
                        ProjectRowKind::Project { open } | ProjectRowKind::Directory { open } => {
                            Some(open)
                        }
                        ProjectRowKind::Request { .. } => None,
                    };
                    if is_open.is_some_and(|o| o != open) {
                        self.open_panel_row(row);
                    }
                }
            }
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
            Some(Outcome::Ws(w)) => w.body_text(),
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
    /// Uses `picker` to draw image response bodies from now on.
    pub fn set_image_picker(&mut self, picker: ImagePicker) {
        self.image_picker = picker;
        self.image_cache.replace(None);
    }

    /// Runs `f` with the drawable form of the image `bytes`, decoding them only when they
    /// differ from the last image drawn; `None` when they cannot be decoded.
    pub(crate) fn with_image<R>(
        &self,
        bytes: &Arc<[u8]>,
        f: impl FnOnce(Option<&mut StatefulProtocol>) -> R,
    ) -> R {
        let mut cache = self.image_cache.borrow_mut();
        if !cache.as_ref().is_some_and(|(b, _)| Arc::ptr_eq(b, bytes)) {
            let protocol = image::load_from_memory(bytes)
                .ok()
                .map(|img| self.image_picker.new_resize_protocol(img));
            *cache = Some((Arc::clone(bytes), protocol));
        }
        f(cache.as_mut().and_then(|(_, p)| p.as_mut()))
    }

    pub fn take_clipboard(&mut self) -> Option<String> {
        crate::tui::headers::take_copied().or_else(|| self.clipboard.take())
    }

    // ---- mouse ----------------------------------------------------------

    /// `screen` is the full terminal area the UI was last drawn into.
    pub fn handle_mouse(&mut self, event: MouseEvent, screen: Rect) {
        self.absorb_query();
        self.handle_mouse_event(event, screen);
        self.sync_bound();
        self.sync_meta();
        self.refresh_suggestions();
    }

    fn handle_mouse_event(&mut self, event: MouseEvent, screen: Rect) {
        let areas = ui::areas_with(screen, self.view().request.kind, self.center_tab());
        let at = Position::new(event.column, event.row);
        if self.confirm.is_some() {
            return;
        }
        if self.help.is_some() {
            return;
        }
        if self.picker.is_some() {
            return self.picker_mouse(event, screen);
        }
        if self.modal.is_some() {
            return self.modal_mouse(event, screen);
        }
        if self.suggestions_open() {
            let anchor = ui::suggestions_anchor(self, areas.schema, areas.body);
            let popup = ui::suggestions_area(anchor, self.suggestions.items.len());
            if let (MouseEventKind::Down(MouseButton::Left), true) =
                (event.kind, popup.contains(at))
            {
                let row = at.y.saturating_sub(ui::inner(popup).y) as usize;
                let offset = ui::list_offset(self.suggestions.sel, ui::inner(popup).height);
                return self.accept_suggestion(row + offset);
            }
        }
        if event.kind == MouseEventKind::Down(MouseButton::Left)
            && ui::contribute_area(screen).contains(at)
        {
            (self.opener)(ui::REPO_URL);
            self.notice = Some(format!("opening {}", ui::REPO_URL));
            return;
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

    /// The index into [`picker_matches`](Self::picker_matches) under `at`, if it is a row.
    fn picker_row_at(&self, at: Position, popup: Rect) -> Option<usize> {
        let list = ui::picker_list_area(popup);
        if !list.contains(at) {
            return None;
        }
        let map = ui::picker_row_map(self);
        let selected = self.picker.as_ref().map(|p| p.selected);
        let selected_row = map.iter().position(|r| *r == selected);
        let row = (at.y - list.y) as usize + ui::list_offset(selected_row, list.height);
        map.get(row).copied().flatten()
    }

    fn picker_mouse(&mut self, event: MouseEvent, screen: Rect) {
        let at = Position::new(event.column, event.row);
        let popup = ui::picker_area(screen);
        match event.kind {
            MouseEventKind::ScrollDown => self.move_picker(1),
            MouseEventKind::ScrollUp => self.move_picker(-1),
            MouseEventKind::Down(MouseButton::Left) if !popup.contains(at) => self.close_picker(),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = self.picker_row_at(at, popup) {
                    self.pick(index);
                }
            }
            MouseEventKind::Moved => {
                if let Some(index) = self.picker_row_at(at, popup)
                    && let Some(picker) = &mut self.picker
                    && picker.selected != index
                {
                    picker.selected = index;
                    self.preview_selected();
                }
            }
            _ => {}
        }
    }

    fn click(&mut self, at: Position, areas: &ui::Areas) {
        if areas.logs.contains(at) {
            if let Some(tab) = ui::left_tab_at(areas.logs, at) {
                if tab != self.left_tab {
                    self.toggle_variables_panel();
                }
                return;
            }
            if self.left_tab == LeftTab::Variables {
                self.set_focus(Focus::Captures);
                let inner = ui::inner(areas.logs);
                if inner.contains(at) {
                    self.click_captures(at, inner);
                }
                return;
            }
            return self.open_log();
        }
        if areas.center_tabs.contains(at) {
            if let Some(tab) = ui::center_tab_at(areas.center_tabs, at.x) {
                self.set_center_tab(tab);
            }
        } else if areas.meta_title.contains(at) {
            self.set_focus(Focus::MetaTitle);
            let inner = ui::inner(areas.meta_title);
            if inner.contains(at) {
                let (_, col_scroll) = ui::editor_scroll(cursor_of(&self.meta_title), inner);
                self.meta_title
                    .move_cursor(CursorMove::Jump(0, (at.x - inner.x) + col_scroll as u16));
            }
        } else if areas.meta_desc.contains(at) {
            self.set_focus(Focus::MetaDescription);
            let inner = ui::inner(areas.meta_desc);
            if inner.contains(at) {
                let (row_scroll, col_scroll) = ui::editor_scroll(cursor_of(&self.meta_desc), inner);
                self.meta_desc.move_cursor(CursorMove::Jump(
                    (at.y - inner.y) + row_scroll as u16,
                    (at.x - inner.x) + col_scroll as u16,
                ));
            }
        } else if areas.tabs.contains(at) {
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
                Kind::Ws | Kind::Graphql => {}
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
            let current = self.view();
            let previewing = current.previewing;
            let view = current.request;
            if let Some(tab) = ui::request_tab_at(
                areas.body,
                at,
                self.headers_count(&view, previewing),
                self.params_count(&view, previewing),
                self.auth_label(&view, previewing),
                view.kind,
                self.panels().len() - 1,
            ) {
                self.focus_editor(self.editor_focus());
                self.set_request_tab(tab);
                self.focus = self.editor_focus();
                return;
            }
            if ui::vars_tab(view.kind, self.request_tab)
                && ui::vars_title_at(areas.body, at, self.vars_on(self.request_tab))
            {
                return self.toggle_vars();
            }
            if view.kind == Kind::Http
                && self.request_tab == RequestTab::Body
                && ui::body_kind_title_at(areas.body, at, view.body_kind)
            {
                self.focus_editor(self.editor_focus());
                return self.next_body_kind();
            }
            self.focus_editor(self.editor_focus());
            let inner = ui::inner(areas.body);
            if !inner.contains(at) {
                return;
            }
            match self.request_tab {
                RequestTab::Body if self.kind == Kind::Http => self.click_http_body(at, inner),
                RequestTab::Body => {
                    let (row_scroll, col_scroll) = ui::editor_scroll(cursor_of(&self.body), inner);
                    self.body.move_cursor(CursorMove::Jump(
                        (at.y - inner.y) + row_scroll as u16,
                        (at.x - inner.x) + col_scroll as u16,
                    ));
                }
                RequestTab::Variables => {
                    let (row_scroll, col_scroll) =
                        ui::editor_scroll(cursor_of(&self.variables), inner);
                    self.variables.move_cursor(CursorMove::Jump(
                        (at.y - inner.y) + row_scroll as u16,
                        (at.x - inner.x) + col_scroll as u16,
                    ));
                }
                RequestTab::Headers => self.click_headers(at, inner),
                RequestTab::Params => click_pairs(&mut self.params, at, inner),
                RequestTab::Auth => self.click_auth(at, inner),
            }
        }
    }

    /// A click in the Auth tab picks a scheme or puts the cursor in an input.
    fn click_auth(&mut self, at: Position, inner: Rect) {
        let layout = ui::auth_layout(inner, &self.auth);
        if let Some((scheme, _)) = layout.schemes.iter().find(|(_, r)| r.contains(at)) {
            return self.auth.set_scheme(*scheme);
        }
        for (field, rect) in &layout.fields {
            if rect.contains(at) {
                let x = match self.auth.input(*field) {
                    Some(ta) if *field == auth::Field::Token => {
                        ui::token_index(*rect, at, ta.cursor().1)
                    }
                    Some(ta) => {
                        let (_, scroll) = ui::editor_scroll(cursor_of(ta), *rect);
                        (at.x - rect.x) as usize + scroll
                    }
                    None => 0,
                };
                return self.auth.select(*field, x);
            }
        }
    }

    fn click_captures(&mut self, at: Position, inner: Rect) {
        let (rows, _, _) = ui::captures_areas(inner, self.captures.rows.len());
        if rows.contains(at) {
            click_pairs(&mut self.captures, at, rows);
        }
    }

    /// Clicking an input puts the cursor there; clicking the last line adds a header.
    fn click_headers(&mut self, at: Position, inner: Rect) {
        click_pairs(&mut self.headers, at, inner);
    }

    /// A click in the REST body editor, which depends on the body kind.
    fn click_http_body(&mut self, at: Position, inner: Rect) {
        match self.body_kind {
            BodyKind::Text | BodyKind::JsonRaw => {
                let (row_scroll, col_scroll) = ui::editor_scroll(cursor_of(&self.body), inner);
                self.body.move_cursor(CursorMove::Jump(
                    (at.y - inner.y) + row_scroll as u16,
                    (at.x - inner.x) + col_scroll as u16,
                ));
            }
            BodyKind::Form => click_pairs(&mut self.form, at, inner),
            BodyKind::Multipart => click_pairs(&mut self.multipart, at, inner),
            BodyKind::File => {
                let (_, col_scroll) = ui::editor_scroll(cursor_of(&self.file), inner);
                self.file
                    .move_cursor(CursorMove::Jump(0, (at.x - inner.x) + col_scroll as u16));
            }
            BodyKind::Json => {
                let depths: Vec<usize> = self.json.rows.iter().map(|r| r.depth).collect();
                let grid = ui::json_grid(inner, &depths, self.json.row);
                if grid.hint.is_some_and(|r| r.contains(at)) {
                    let last = self.json.rows.len() - 1;
                    self.json.select(last, JsonCol::Key);
                    return;
                }
                for cells in &grid.rows {
                    for (rect, col) in [(cells.key, JsonCol::Key), (cells.value, JsonCol::Value)] {
                        if rect.contains(at) {
                            self.json.select(cells.row, col);
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Clicking an item selects (and previews) it; clicking it again loads it.
    fn click_list(&mut self, y_in_list: u16, height: u16) {
        let item = match self.tab {
            Tab::Projects => {
                let rows = self.panel_rows().len();
                let selected_row = (rows > 0).then_some(self.projects_sel);
                let row = y_in_list as usize + ui::list_offset(selected_row, height);
                (row < rows).then_some(row)
            }
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
        if self.tab == Tab::Projects {
            let is_request = matches!(
                self.panel_rows().get(item),
                Some(PanelRow::Node(ProjectRow {
                    kind: ProjectRowKind::Request { .. },
                    ..
                }))
            );
            if !is_request {
                // One click opens or closes a project or directory, or runs a create row.
                self.select(item);
                self.open_panel_row(item);
                self.last_click = None;
                return;
            }
        }
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
            Tab::Projects => self.projects_sel = item,
            Tab::Stored => self.stored_sel = item,
            Tab::History => self.history_sel = item,
        }
        self.scroll = 0;
        self.note_node();
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
    /// Clears the editor for a fresh request of the current kind and focuses the URL.
    /// Refused while a request or WebSocket session is running.
    pub fn new_request(&mut self) {
        if self.sending || self.ws_open() {
            self.status = Some("a request is running; Ctrl+X cancels it first".into());
            return;
        }
        self.flush_bound();
        self.bound = None;
        self.bound_last = None;
        self.method = Method::Get;
        self.url = single_line("");
        self.body = TextArea::default();
        self.body_kind = BodyKind::default();
        self.json = JsonEditor::default();
        self.form = HeaderEditor::default();
        self.multipart = HeaderEditor::default();
        self.file = single_line("");
        self.variables = TextArea::default();
        self.headers = HeaderEditor::default();
        self.params = HeaderEditor::default();
        self.auth = AuthEditor::default();
        self.captures = HeaderEditor::default();
        self.schema_path = single_line("");
        self.schema_mode = SchemaMode::Reflection;
        self.suggestions = Suggestions::default();
        self.suggested_for = None;
        self.insecure = false;
        self.selected_method = None;
        self.picker = None;
        self.request_tab = RequestTab::Body;
        self.live = None;
        self.status = None;
        self.notice = Some("new request".into());
        self.set_focus(Focus::Url);
    }

    fn focus_editor(&mut self, focus: Focus) {
        if matches!(self.focus, Focus::List | Focus::Response) && self.view().previewing {
            self.activate();
        }
        self.set_focus(focus);
    }

    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.naming = None;
        self.sync_meta();
        self.last_click = None;
        self.scroll = 0;
        self.request_preview();
    }

    fn set_focus(&mut self, focus: Focus) {
        if focus == Focus::Response && self.focus != Focus::Response {
            self.inspect_preview = self.view().previewing;
        }
        if focus != self.focus {
            if self.focus == Focus::Captures {
                self.register_literals();
            }
            for ta in [
                &mut self.url,
                &mut self.body,
                &mut self.file,
                &mut self.variables,
                &mut self.schema_path,
            ] {
                ta.cancel_selection();
            }
        }
        self.focus = focus;
        self.scroll = 0;
        self.request_preview();
    }

    fn move_selection(&mut self, delta: isize) {
        let project_len = self.panel_rows().len();
        let (sel, len) = match self.tab {
            Tab::Projects => (&mut self.projects_sel, project_len),
            Tab::Stored => (&mut self.stored_sel, self.templates.len()),
            Tab::History => (&mut self.history_sel, self.history.len()),
        };
        if len == 0 {
            return;
        }
        *sel = sel.saturating_add_signed(delta).min(len - 1);
        self.last_click = None;
        self.scroll = 0;
        self.note_node();
        self.request_preview();
    }

    /// Copies the selected entry into the editor so it can be modified and sent.
    fn activate(&mut self) {
        let mut bind = None;
        let request = match self.tab {
            Tab::Projects => {
                let row = self.projects_sel;
                if self
                    .selected_project_row()
                    .is_none_or(|r| !matches!(r.kind, ProjectRowKind::Request { .. }))
                {
                    self.open_panel_row(row);
                    return;
                }
                bind = self.selected_project_row().map(|r| r.id);
                self.selected_project_request()
            }
            Tab::Stored => self.selected_template().map(|t| t.template.to_request()),
            Tab::History => match (self.selected_summary(), &self.preview) {
                (Some(s), Some(e)) if e.id == s.id => Some(e.request.clone()),
                _ => None,
            },
        };
        if let Some(request) = request {
            self.load_request(&request);
            if let (Some(id), Request::Http(_)) = (bind, &request) {
                // edits to the loaded request now flow into the project request
                self.bound = Some(id);
                self.bound_last = Some(self.editor_request());
            }
            self.set_focus(Focus::Url);
        }
    }

    pub fn load_request(&mut self, request: &Request) {
        self.flush_bound();
        self.bound = None;
        self.bound_last = None;
        let request = match request {
            Request::Http(request) => request,
            Request::Grpc(request) => return self.load_grpc_request(request),
            Request::Ws(request) => return self.load_ws_request(request),
            Request::Graphql(request) => return self.load_graphql_request(request),
        };
        self.set_kind(Kind::Http);
        self.method = request.method;
        self.url = single_line(&request.url);
        self.params = HeaderEditor::default();
        self.absorb_query();
        self.load_body(request.body.as_ref());
        // the Authorization header lives in the Auth tab
        let (auth, headers): (Vec<&Header>, Vec<&Header>) = request
            .headers
            .iter()
            .partition(|h| h.name.eq_ignore_ascii_case("authorization"));
        self.auth = auth
            .first()
            .map_or_else(AuthEditor::default, |h| AuthEditor::from_header(&h.value));
        let headers: Vec<Header> = headers.into_iter().cloned().collect();
        self.headers = HeaderEditor::from_headers(&headers);
    }

    /// Fills the REST body editors from `body`; the other kinds start empty.
    fn load_body(&mut self, body: Option<&Body>) {
        self.body = TextArea::default();
        self.json = JsonEditor::default();
        self.form = HeaderEditor::default();
        self.multipart = HeaderEditor::default();
        self.file = single_line("");
        self.body_kind = body.map_or_else(BodyKind::default, Body::kind);
        let text = |t: &str| TextArea::new(t.lines().map(str::to_owned).collect());
        match body {
            None => {}
            Some(Body::Text(t) | Body::JsonRaw(t)) => self.body = text(t),
            Some(Body::Json(value)) => match JsonEditor::from_value(value) {
                Some(json) => self.json = json,
                // a lone scalar has no fields to edit
                None => {
                    self.body = text(&value.to_json_pretty());
                    self.body_kind = BodyKind::JsonRaw;
                }
            },
            Some(Body::Form(fields)) => {
                self.form = HeaderEditor::from_pairs(
                    fields.iter().map(|f| (f.name.as_str(), f.value.as_str())),
                );
            }
            Some(Body::Multipart(parts)) => {
                let values: Vec<String> = parts.iter().map(|p| p.value.to_string()).collect();
                self.multipart = HeaderEditor::from_pairs(
                    parts
                        .iter()
                        .zip(&values)
                        .map(|(p, v)| (p.name.as_str(), v.as_str())),
                );
            }
            Some(Body::File(path)) => {
                self.file = single_line(&format!("@{}", path.trim_start_matches('@')))
            }
        }
    }

    fn load_graphql_request(&mut self, request: &GraphqlRequest) {
        self.set_kind(Kind::Graphql);
        self.url = single_line(&request.url);
        self.body = TextArea::new(request.query.lines().map(str::to_owned).collect());
        self.variables = TextArea::new(
            request
                .variables
                .as_deref()
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect(),
        );
        self.headers = HeaderEditor::from_headers(&request.headers);
        self.selected_method = None;
    }

    fn load_ws_request(&mut self, request: &WsRequest) {
        self.set_kind(Kind::Ws);
        self.url = single_line(&request.url);
        self.body = request
            .body
            .as_deref()
            .map(|b| TextArea::new(b.lines().map(str::to_owned).collect()))
            .unwrap_or_default();
        self.headers = HeaderEditor::from_headers(&request.headers);
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
        self.headers = HeaderEditor::from_headers(&request.metadata);
    }

    // ---- background work ------------------------------------------------

    pub fn send(&mut self) {
        self.absorb_query();
        self.sync_bound();
        self.flush_bound();
        if self.sending {
            self.status = Some("a request is already in flight".into());
            return;
        }
        match self.kind {
            Kind::Http => self.send_http(),
            Kind::Grpc => self.send_grpc(),
            Kind::Ws => self.send_ws(),
            Kind::Graphql => self.send_graphql(),
        }
    }

    /// Opens the socket; once it is open, sends the body as a text message.
    fn send_ws(&mut self) {
        self.register_literals();
        if let Some(out) = &self.ws_out {
            let text = self.body_text();
            if text.is_empty() {
                self.status = Some("type a message in the body first".into());
            } else if out.send(text).is_err() {
                self.status = Some("the socket is closed".into());
            }
            return;
        }
        let request = match self.try_ws_request() {
            Ok(request) => request,
            Err(message) => {
                self.status = Some(message);
                self.request_tab = RequestTab::Headers;
                self.set_focus(Focus::Headers);
                return;
            }
        };
        if request.url.is_empty() {
            self.status = Some("enter a ws:// or wss:// URL first".into());
            return;
        }
        self.sending = true;
        self.status = None;
        self.scroll = 0;
        self.log.push(Level::Info, format!("-> WS {}", request.url));
        self.live = Some(Outcome::Ws(WsOutcome::default()));

        let mut sent = request.clone();
        sent.headers = self.with_header_vars(&request.headers);
        let mut session = self.ws.connect(sent);
        self.ws_out = Some(session.sender());
        self.cancel = Some(session.canceller());
        let (store, tx) = (self.store.clone(), self.tx.clone());
        tokio::spawn(async move {
            let mut last = None;
            while let Some(event) = session.events.recv().await {
                if let WsEvent::Done(outcome) = &event {
                    last = Some(outcome.clone());
                }
                let _ = tx.send(Msg::WsEvent(event));
            }
            let outcome = last.unwrap_or_else(|| WsOutcome {
                error: Some("the session ended without a result".into()),
                ..Default::default()
            });
            if let Err(e) = store
                .record(&Request::Ws(request), &Outcome::Ws(outcome))
                .await
            {
                let _ = tx.send(Msg::Failed(format!("could not save history: {e}")));
            }
            record_done(&store, &tx).await;
        });
    }

    fn apply_ws_event(&mut self, event: WsEvent) {
        match event {
            WsEvent::Log(line) => self.log.push(Level::Detail, line),
            WsEvent::Connected(headers) => {
                self.sending = false;
                self.log.push(Level::Info, "connected");
                self.notice = Some("connected; Ctrl+S sends the body, Ctrl+X closes".into());
                if let Some(Outcome::Ws(live)) = &mut self.live {
                    live.connected = true;
                    live.headers = headers;
                }
            }
            WsEvent::Message(message) => {
                if let Some(Outcome::Ws(live)) = &mut self.live {
                    live.messages.push(message);
                }
            }
            WsEvent::Done(outcome) => {
                self.log_outcome(&Outcome::Ws(outcome.clone()));
                self.sending = false;
                self.ws_out = None;
                self.cancel = None;
                self.scroll = 0;
                self.live = Some(Outcome::Ws(outcome));
            }
        }
    }

    fn send_http(&mut self) {
        self.register_literals();
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

        // History keeps the request as written, so `$name` is resolved again on a replay.
        let mut sent = request.clone();
        if self.vars_body {
            sent.body = sent.body.map(|b| self.vars.apply(b));
        }
        if self.vars_headers {
            for h in &mut sent.headers {
                h.value = self.vars.substitute(&h.value, false);
            }
        }
        if self.vars_params
            && let Ok(url) = self.http_url_vars()
        {
            sent.url = url;
        }

        let (client, store, tx) = (self.client.clone(), self.store.clone(), self.tx.clone());
        tokio::spawn(async move {
            let trace = Trace::new({
                let tx = tx.clone();
                move |line| {
                    let _ = tx.send(Msg::Trace(line));
                }
            });
            let outcome: Outcome = client.send_traced(&sent, &trace).await.into();
            if let Err(e) = store.record(&Request::Http(request), &outcome).await {
                let _ = tx.send(Msg::Failed(format!("could not save history: {e}")));
            }
            let _ = tx.send(Msg::Sent(outcome));
            record_done(&store, &tx).await;
        });
    }

    fn send_graphql(&mut self) {
        self.register_literals();
        let request = match self.try_graphql_request() {
            Ok(request) => request,
            Err(message) => {
                self.status = Some(message);
                self.request_tab = RequestTab::Headers;
                self.set_focus(Focus::Headers);
                return;
            }
        };
        if request.url.is_empty() {
            self.status = Some("enter the endpoint URL first".into());
            return;
        }
        if request.query.trim().is_empty() {
            self.status = Some("write a query first".into());
            return;
        }
        let mut sendable = request.clone();
        sendable.headers = self.with_header_vars(&request.headers);
        let http = match sendable.to_http() {
            Ok(http) => http,
            Err(message) => {
                self.status = Some(message);
                self.request_tab = RequestTab::Variables;
                self.set_focus(Focus::Variables);
                return;
            }
        };
        self.sending = true;
        self.status = None;
        self.log
            .push(Level::Info, format!("-> GraphQL {}", request.url));

        let (client, store, tx) = (self.client.clone(), self.store.clone(), self.tx.clone());
        tokio::spawn(async move {
            let trace = Trace::new({
                let tx = tx.clone();
                move |line| {
                    let _ = tx.send(Msg::Trace(line));
                }
            });
            let outcome: Outcome = client.send_traced(&http, &trace).await.into();
            if let Err(e) = store.record(&Request::Graphql(request), &outcome).await {
                let _ = tx.send(Msg::Failed(format!("could not save history: {e}")));
            }
            let _ = tx.send(Msg::Sent(outcome));
            record_done(&store, &tx).await;
        });
    }

    fn send_grpc(&mut self) {
        self.register_literals();
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
            self.status = Some("pick a method first (Ctrl+P)".into());
            return;
        }
        if let Err(message) = self.schema_ready_to_load() {
            self.status = Some(message);
            return;
        }

        let mut sent = request.clone();
        sent.metadata = self.with_header_vars(&request.metadata);
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
                Ok(schema) => run_call(&*grpc, sent.clone(), schema, &cancel, &tx).await,
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

    pub fn refresh_projects(&self) {
        let (store, tx) = (self.store.clone(), self.tx.clone());
        tokio::spawn(async move {
            let msg = match store.list_projects().await {
                Ok(list) => Msg::Projects(list),
                Err(e) => Msg::Failed(format!("could not read projects: {e}")),
            };
            let _ = tx.send(msg);
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

/// The focus ring: list, URL, [schema, method (gRPC only)], the request editor, response.
fn next_focus(f: Focus, editor: Focus, kind: Kind, captures: bool) -> Focus {
    match (f, kind) {
        (Focus::MetaTitle | Focus::MetaDescription, _) => Focus::List,
        (Focus::List, _) if captures => Focus::Captures,
        (Focus::List | Focus::Captures, _) => Focus::Url,
        (Focus::Url, Kind::Http | Kind::Ws) => editor,
        (Focus::Url, Kind::Grpc) => Focus::Schema,
        (Focus::Url, Kind::Graphql) => Focus::Method,
        (Focus::Schema, _) => Focus::Method,
        (Focus::Method, _) => editor,
        (Focus::Body | Focus::Variables | Focus::Headers | Focus::Params | Focus::Auth, _) => {
            Focus::Response
        }
        (Focus::Response, _) => Focus::List,
    }
}

fn prev_focus(f: Focus, editor: Focus, kind: Kind, captures: bool) -> Focus {
    match (f, kind) {
        (Focus::MetaTitle | Focus::MetaDescription, _) => Focus::List,
        (Focus::List, _) => Focus::Response,
        (Focus::Captures, _) => Focus::List,
        (Focus::Url, _) if captures => Focus::Captures,
        (Focus::Url, _) => Focus::List,
        (Focus::Schema, _) => Focus::Url,
        (Focus::Method, Kind::Graphql) => Focus::Url,
        (Focus::Method, _) => Focus::Schema,
        (
            Focus::Body | Focus::Variables | Focus::Headers | Focus::Params | Focus::Auth,
            Kind::Http | Kind::Ws,
        ) => Focus::Url,
        (
            Focus::Body | Focus::Variables | Focus::Headers | Focus::Params | Focus::Auth,
            Kind::Grpc | Kind::Graphql,
        ) => Focus::Method,
        (Focus::Response, _) => editor,
    }
}

/// Clicking an input puts the cursor there; clicking the last line adds a row.
fn click_pairs(editor: &mut HeaderEditor, at: Position, inner: Rect) {
    let grid = ui::header_grid(inner, editor.rows.len(), editor.row);
    if grid.add.is_some_and(|r| r.contains(at)) {
        return editor.add_row();
    }
    for cells in &grid.rows {
        for (rect, col) in [(cells.name, Col::Name), (cells.value, Col::Value)] {
            if rect.contains(at) {
                let row = &editor.rows[cells.row];
                let ta = if col == Col::Name {
                    &row.name
                } else {
                    &row.value
                };
                let (_, scroll) = ui::editor_scroll(cursor_of(ta), rect);
                editor.select(cells.row, col, (at.x - rect.x) + scroll as u16);
                return;
            }
        }
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

/// Opens `url` with the system's browser; failures are ignored (there may be no browser).
fn open_in_browser(url: &str) {
    use std::process::{Command, Stdio};

    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = Command::new("xdg-open");

    let _ = command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
