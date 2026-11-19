use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use toasty::Db;
use toasty::migration::{MigrationFile, MigrationSet};
use toasty::stmt::Value;
use toasty_driver_sqlite::Sqlite;

use serde::{Deserialize, Serialize};

use crate::model::Project;

use crate::client::{
    Body, GraphqlRequest, GrpcOutcome, GrpcRequest, GrpcStatus, Header, HttpRequest, HttpResponse,
    Kind, Method, Outcome, Request, SchemaSource, WsClose, WsMessage, WsOutcome, WsRequest,
};

/// Bodies larger than this are truncated before being stored.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

/// The schema migrations, `toasty/history.toml` and the `toasty/migrations/*.sql` it names.
static MIGRATIONS: MigrationSet = toasty::embed_migrations!();

/// A row of the `history` table.
#[derive(Debug, toasty::Model)]
#[table = "history"]
struct HistoryRow {
    #[key]
    #[auto]
    id: i64,
    created_at_ms: i64,
    kind: String,
    method: String,
    url: String,
    request_headers: String,
    request_body: Option<String>,
    status: Option<i64>,
    response_headers: Option<String>,
    response_body: Option<String>,
    elapsed_ms: Option<i64>,
    error: Option<String>,
    grpc: Option<String>,
}

/// A row of the `projects` table.
#[derive(Debug, toasty::Model)]
#[table = "projects"]
struct ProjectRow {
    #[key]
    id: String,
    name: String,
    created_at_ms: i64,
    data: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("the database {0} is from a newer hozd; please upgrade hozd")]
    NewerDatabase(String),
    #[error("database migration failed: {0}")]
    Migration(toasty::Error),
    #[error("database error: {0}")]
    Db(#[from] toasty::Error),
    #[error("corrupt history entry {id}: {reason}")]
    Corrupt { id: i64, reason: String },
    #[error("the request and its outcome are of different kinds")]
    KindMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: i64,
    pub created_at_ms: i64,
    pub request: Request,
    pub outcome: Outcome,
}

/// One row of the history list: enough to render a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub id: i64,
    pub kind: Kind,
    /// The HTTP verb, or `package.Service/Method` for gRPC.
    pub method: String,
    /// The URL, or the server address for gRPC.
    pub url: String,
    /// The HTTP status, or the gRPC status code; `None` when nothing was received.
    pub status: Option<u16>,
    /// The local date it was sent on, `YYYY-MM-DD`.
    pub day: String,
}

impl Summary {
    /// The `host:port` the request went to: the URL without scheme, credentials and path.
    pub fn host(&self) -> &str {
        split_url(&self.url).0
    }

    /// The path and query of the URL; `/` when it has none.
    pub fn uri(&self) -> &str {
        split_url(&self.url).1
    }
}

/// `https://u@api.dev:8080/a?b=1` -> (`api.dev:8080`, `/a?b=1`).
fn split_url(url: &str) -> (&str, &str) {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, uri) = rest.split_at(end);
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    (host, if uri.is_empty() { "/" } else { uri })
}

#[derive(Clone)]
pub struct History {
    db: Db,
}

impl History {
    /// Opens (creating if missing) the database at `path` and applies pending migrations.
    pub async fn open(path: &Path) -> Result<Self, HistoryError> {
        Self::open_with(path, &MIGRATIONS).await
    }

    pub(crate) async fn open_with(
        path: &Path,
        migrations: &MigrationSet,
    ) -> Result<Self, HistoryError> {
        let db = Db::builder()
            .models(toasty::models!(HistoryRow, ProjectRow))
            .max_pool_size(4)
            .build(Sqlite::open(path))
            .await?;

        let mut conn = db.connection().await?; // opening creates the file
        restrict_permissions(path);
        Self::migrate(&mut conn, &db, migrations, path).await?;
        Ok(Self { db })
    }

    /// Applies pending migrations; refuses, before writing anything, a database that has
    /// migrations this binary does not know.
    ///
    /// Databases written by hozd before Toasty record their migrations in sqlx's
    /// `_sqlx_migrations`; those are adopted into `__toasty_migrations` under the same ids.
    /// `_sqlx_migrations` is left in place.
    async fn migrate(
        conn: &mut toasty::Connection,
        db: &Db,
        migrations: &MigrationSet,
        path: &Path,
    ) -> Result<(), HistoryError> {
        let known = |id: u64| migrations.migrations().iter().find(|m| m.id() == id);
        let toasty_ids = if table_exists(conn, "__toasty_migrations").await? {
            ids(conn, "SELECT id FROM __toasty_migrations").await?
        } else {
            Vec::new()
        };
        let sqlx_ids = if table_exists(conn, "_sqlx_migrations").await? {
            ids(
                conn,
                "SELECT version FROM _sqlx_migrations WHERE success = 1",
            )
            .await?
        } else {
            Vec::new()
        };
        if toasty_ids
            .iter()
            .chain(&sqlx_ids)
            .any(|&id| known(id).is_none())
        {
            return Err(HistoryError::NewerDatabase(path.display().to_string()));
        }

        let adopt: Vec<&MigrationFile> = sqlx_ids
            .iter()
            .filter(|id| !toasty_ids.contains(id))
            .filter_map(|&id| known(id))
            .collect();
        if !adopt.is_empty() {
            // the same table the SQLite driver creates on its first migration
            toasty::sql::statement(
                "CREATE TABLE IF NOT EXISTS __toasty_migrations (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    applied_at TEXT NOT NULL
                )",
            )
            .exec(conn)
            .await?;
            for migration in adopt {
                toasty::sql::statement(
                    "INSERT INTO __toasty_migrations (id, name, applied_at)
                     VALUES (?1, ?2, datetime('now'))",
                )
                .bind(migration.id() as i64)
                .bind(migration.name())
                .exec(conn)
                .await?;
            }
        }

        migrations
            .apply(db)
            .await
            .map(drop)
            .map_err(HistoryError::Migration)
    }

    /// Records an executed request and its outcome, returning the new entry id.
    pub async fn record(&self, request: &Request, outcome: &Outcome) -> Result<i64, HistoryError> {
        let row = match (request, outcome) {
            (Request::Http(req), Outcome::Response(res)) => http_row(req, Some(res), None),
            (Request::Http(req), Outcome::Error(msg)) => http_row(req, None, Some(msg)),
            (Request::Grpc(req), Outcome::Grpc(out)) => grpc_row(req, out),
            (Request::Ws(req), Outcome::Ws(out)) => ws_row(req, out),
            (Request::Graphql(req), Outcome::Response(res)) => graphql_row(req, Some(res), None),
            (Request::Graphql(req), Outcome::Error(msg)) => graphql_row(req, None, Some(msg)),
            _ => return Err(HistoryError::KindMismatch),
        };
        let entry = toasty::create!(HistoryRow {
            created_at_ms: now_ms(),
            kind: row.kind.as_str(),
            method: row.method,
            url: row.url,
            request_headers: row.request_headers,
            request_body: row.request_body,
            status: row.status,
            response_headers: row.response_headers,
            response_body: row.response_body,
            elapsed_ms: row.elapsed_ms,
            error: row.error.map(str::to_owned),
            grpc: row.grpc,
        })
        .exec(&mut self.db.clone())
        .await?;
        Ok(entry.id)
    }

    /// Stores `project`, replacing the one with the same id.
    pub async fn save_project(&self, project: &Project) -> Result<(), HistoryError> {
        let data = project.to_json().map_err(|e| HistoryError::Corrupt {
            id: 0,
            reason: e.to_string(),
        })?;
        // a replaced project keeps its creation time, and so its place in the list
        ProjectRow::upsert_by_id(&project.id)
            .name(&project.name)
            .data(data)
            .on_create(|c| c.created_at_ms(now_ms()))
            .exec(&mut self.db.clone())
            .await?;
        Ok(())
    }

    /// All projects, oldest first.
    pub async fn list_projects(&self) -> Result<Vec<Project>, HistoryError> {
        // raw SQL: projects created in the same millisecond keep their insertion (rowid) order
        let rows = toasty::sql::query("SELECT data FROM projects ORDER BY created_at_ms, rowid")
            .exec(&mut self.db.clone())
            .await?;
        rows.into_iter()
            .map(|row| {
                let corrupt = |reason: String| HistoryError::Corrupt {
                    id: 0,
                    reason: format!("project: {reason}"),
                };
                let [data] = columns(row).map_err(corrupt)?;
                let data = text(data).map_err(corrupt)?.unwrap_or_default();
                Project::from_json(&data).map_err(|e| corrupt(e.to_string()))
            })
            .collect()
    }

    pub async fn delete_project(&self, id: &str) -> Result<(), HistoryError> {
        ProjectRow::filter_by_id(id)
            .delete()
            .exec(&mut self.db.clone())
            .await?;
        Ok(())
    }

    /// Newest first.
    pub async fn list(&self, limit: u32) -> Result<Vec<Summary>, HistoryError> {
        // Raw SQL: the list must not load the bodies, and the day is cut by SQLite in the
        // local time zone.
        let rows = toasty::sql::query(
            "SELECT id, kind, method, url, status,
                    date(created_at_ms / 1000, 'unixepoch', 'localtime') AS day
             FROM history ORDER BY id DESC LIMIT ?1",
        )
        .bind(i64::from(limit))
        .exec(&mut self.db.clone())
        .await?;

        rows.into_iter()
            .map(|row| {
                let corrupt = |reason: String| HistoryError::Corrupt { id: 0, reason };
                let [id, kind, method, url, status, day] = columns(row).map_err(corrupt)?;
                let id = integer(id).map_err(corrupt)?.unwrap_or_default();
                let corrupt = |reason: String| HistoryError::Corrupt { id, reason };
                Ok(Summary {
                    id,
                    kind: parse_kind(id, &text(kind).map_err(corrupt)?.unwrap_or_default())?,
                    method: text(method).map_err(corrupt)?.unwrap_or_default(),
                    url: text(url).map_err(corrupt)?.unwrap_or_default(),
                    status: integer(status).map_err(corrupt)?.map(|s| s as u16),
                    day: text(day).map_err(corrupt)?.unwrap_or_default(),
                })
            })
            .collect()
    }

    pub async fn get(&self, id: i64) -> Result<Option<HistoryEntry>, HistoryError> {
        let Some(r) = HistoryRow::filter_by_id(id)
            .first()
            .exec(&mut self.db.clone())
            .await?
        else {
            return Ok(None);
        };

        let corrupt = |reason: String| HistoryError::Corrupt { id, reason };
        let headers = |json: &str| {
            serde_json::from_str::<Vec<Header>>(json).map_err(|e| corrupt(e.to_string()))
        };
        let elapsed = Duration::from_millis(r.elapsed_ms.unwrap_or(0) as u64);

        let (request, outcome) = match parse_kind(id, &r.kind)? {
            Kind::Http => {
                let request = Request::Http(HttpRequest {
                    method: parse_method(id, &r.method)?,
                    url: r.url,
                    headers: headers(&r.request_headers)?,
                    body: r.request_body.as_deref().map(Body::from_stored),
                });
                let outcome = match (r.status, r.error) {
                    (Some(status), _) => Outcome::Response(HttpResponse {
                        status: status as u16,
                        headers: headers(r.response_headers.as_deref().unwrap_or("[]"))?,
                        body: r.response_body.unwrap_or_default(),
                        elapsed,
                        image: None,
                    }),
                    (None, Some(msg)) => Outcome::Error(msg),
                    (None, None) => return Err(corrupt("neither response nor error".into())),
                };
                (request, outcome)
            }
            Kind::Graphql => {
                let request = Request::Graphql(GraphqlRequest::from_stored(
                    r.url,
                    headers(&r.request_headers)?,
                    r.request_body.as_deref(),
                ));
                let outcome = match (r.status, r.error) {
                    (Some(status), _) => Outcome::Response(HttpResponse {
                        status: status as u16,
                        headers: headers(r.response_headers.as_deref().unwrap_or("[]"))?,
                        body: r.response_body.unwrap_or_default(),
                        elapsed,
                        image: None,
                    }),
                    (None, Some(msg)) => Outcome::Error(msg),
                    (None, None) => return Err(corrupt("neither response nor error".into())),
                };
                (request, outcome)
            }
            Kind::Grpc => {
                let extra: GrpcExtra = serde_json::from_str(r.grpc.as_deref().unwrap_or("{}"))
                    .map_err(|e| corrupt(e.to_string()))?;
                let request = Request::Grpc(GrpcRequest {
                    address: r.url,
                    method: r.method,
                    schema: extra.schema,
                    insecure: extra.insecure,
                    metadata: headers(&r.request_headers)?,
                    body: r.request_body,
                });
                let outcome = Outcome::Grpc(GrpcOutcome {
                    metadata: headers(r.response_headers.as_deref().unwrap_or("[]"))?,
                    messages: extra.messages,
                    status: r.status.map(|code| GrpcStatus {
                        code: code as i32,
                        message: extra.status_message,
                        trailers: extra.trailers,
                    }),
                    error: r.error,
                    elapsed,
                    response_stream: extra.response_stream,
                });
                (request, outcome)
            }
            Kind::Ws => {
                let extra: WsExtra =
                    serde_json::from_str(r.response_body.as_deref().unwrap_or("{}"))
                        .map_err(|e| corrupt(e.to_string()))?;
                let request = Request::Ws(WsRequest {
                    url: r.url,
                    headers: headers(&r.request_headers)?,
                    body: r.request_body,
                });
                let outcome = Outcome::Ws(WsOutcome {
                    connected: r.status.is_some(),
                    headers: headers(r.response_headers.as_deref().unwrap_or("[]"))?,
                    messages: extra.messages,
                    close: extra.close,
                    error: r.error,
                    elapsed,
                });
                (request, outcome)
            }
        };

        Ok(Some(HistoryEntry {
            id,
            created_at_ms: r.created_at_ms,
            request,
            outcome,
        }))
    }
}

/// Everything about a gRPC call that the shared columns cannot hold.
#[derive(Debug, Default, Serialize, Deserialize)]
struct GrpcExtra {
    #[serde(default)]
    schema: SchemaSource,
    #[serde(default)]
    insecure: bool,
    #[serde(default)]
    messages: Vec<String>,
    #[serde(default)]
    status_message: String,
    #[serde(default)]
    trailers: Vec<Header>,
    #[serde(default)]
    response_stream: bool,
    /// Messages were dropped to stay under [`MAX_BODY_BYTES`].
    #[serde(default)]
    truncated: bool,
}

/// Everything about a WebSocket session that the shared columns cannot hold; stored as the
/// JSON `response_body`. The status column is 101 when the handshake succeeded.
#[derive(Debug, Default, Serialize, Deserialize)]
struct WsExtra {
    #[serde(default)]
    messages: Vec<WsMessage>,
    #[serde(default)]
    close: Option<WsClose>,
    /// Messages were dropped to stay under [`MAX_BODY_BYTES`].
    #[serde(default)]
    truncated: bool,
}

/// A history row before insertion, whichever the kind.
struct Row<'a> {
    kind: Kind,
    method: String,
    url: &'a str,
    request_headers: String,
    request_body: Option<String>,
    status: Option<i64>,
    response_headers: Option<String>,
    response_body: Option<String>,
    elapsed_ms: Option<i64>,
    error: Option<&'a str>,
    grpc: Option<String>,
}

fn http_row<'a>(
    request: &'a HttpRequest,
    response: Option<&'a HttpResponse>,
    error: Option<&'a String>,
) -> Row<'a> {
    Row {
        kind: Kind::Http,
        method: request.method.as_str().to_owned(),
        url: &request.url,
        request_headers: serde_json::to_string(&request.headers).expect("headers serialize"),
        request_body: request
            .body
            .as_ref()
            .map(|b| cap(&b.to_stored()).to_owned()),
        status: response.map(|r| i64::from(r.status)),
        response_headers: response
            .map(|r| serde_json::to_string(&r.headers).expect("headers serialize")),
        response_body: response.map(|r| cap(&r.body).to_owned()),
        elapsed_ms: response.map(|r| r.elapsed.as_millis() as i64),
        error: error.map(String::as_str),
        grpc: None,
    }
}

fn grpc_row<'a>(request: &'a GrpcRequest, outcome: &'a GrpcOutcome) -> Row<'a> {
    // Keep messages until the total reaches the cap; later ones are dropped.
    let mut kept = Vec::new();
    let mut size = 0;
    let mut truncated = false;
    for message in &outcome.messages {
        if size + message.len() > MAX_BODY_BYTES {
            truncated = true;
            break;
        }
        size += message.len();
        kept.push(message.clone());
    }
    let extra = GrpcExtra {
        schema: request.schema.clone(),
        insecure: request.insecure,
        messages: kept,
        status_message: outcome
            .status
            .as_ref()
            .map(|s| s.message.clone())
            .unwrap_or_default(),
        trailers: outcome
            .status
            .as_ref()
            .map(|s| s.trailers.clone())
            .unwrap_or_default(),
        response_stream: outcome.response_stream,
        truncated,
    };
    Row {
        kind: Kind::Grpc,
        method: request.method.clone(),
        url: &request.address,
        request_headers: serde_json::to_string(&request.metadata).expect("headers serialize"),
        request_body: request.body.as_deref().map(|b| cap(b).to_owned()),
        status: outcome.status.as_ref().map(|s| i64::from(s.code)),
        response_headers: Some(
            serde_json::to_string(&outcome.metadata).expect("headers serialize"),
        ),
        response_body: None,
        elapsed_ms: Some(outcome.elapsed.as_millis() as i64),
        error: outcome.error.as_deref(),
        grpc: Some(serde_json::to_string(&extra).expect("extra serializes")),
    }
}

/// The stored request body is the query and variables as JSON, see
/// [`GraphqlRequest::to_stored`]. The query is not capped: cutting it would make it unusable.
fn graphql_row<'a>(
    request: &'a GraphqlRequest,
    response: Option<&'a HttpResponse>,
    error: Option<&'a String>,
) -> Row<'a> {
    Row {
        kind: Kind::Graphql,
        method: "GRAPHQL".to_owned(),
        url: &request.url,
        request_headers: serde_json::to_string(&request.headers).expect("headers serialize"),
        request_body: Some(request.to_stored()),
        status: response.map(|r| i64::from(r.status)),
        response_headers: response
            .map(|r| serde_json::to_string(&r.headers).expect("headers serialize")),
        response_body: response.map(|r| cap(&r.body).to_owned()),
        elapsed_ms: response.map(|r| r.elapsed.as_millis() as i64),
        error: error.map(String::as_str),
        grpc: None,
    }
}

fn ws_row<'a>(request: &'a WsRequest, outcome: &'a WsOutcome) -> Row<'a> {
    // Keep messages until the total reaches the cap; later ones are dropped.
    let mut kept = Vec::new();
    let mut size = 0;
    let mut truncated = false;
    for message in &outcome.messages {
        if size + message.text.len() > MAX_BODY_BYTES {
            truncated = true;
            break;
        }
        size += message.text.len();
        kept.push(message.clone());
    }
    let extra = WsExtra {
        messages: kept,
        close: outcome.close.clone(),
        truncated,
    };
    Row {
        kind: Kind::Ws,
        method: "WS".to_owned(),
        url: &request.url,
        request_headers: serde_json::to_string(&request.headers).expect("headers serialize"),
        request_body: request.body.as_deref().map(|b| cap(b).to_owned()),
        status: outcome.connected.then_some(101),
        response_headers: Some(serde_json::to_string(&outcome.headers).expect("headers serialize")),
        response_body: Some(serde_json::to_string(&extra).expect("extra serializes")),
        elapsed_ms: Some(outcome.elapsed.as_millis() as i64),
        error: outcome.error.as_deref(),
        grpc: None,
    }
}

fn parse_method(id: i64, s: &str) -> Result<Method, HistoryError> {
    Method::from_str(s).map_err(|reason| HistoryError::Corrupt { id, reason })
}

fn parse_kind(id: i64, s: &str) -> Result<Kind, HistoryError> {
    Kind::from_str(s).map_err(|reason| HistoryError::Corrupt { id, reason })
}

async fn table_exists(conn: &mut toasty::Connection, name: &str) -> Result<bool, HistoryError> {
    let rows = toasty::sql::query("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")
        .bind(name)
        .exec(conn)
        .await?;
    Ok(!rows.is_empty())
}

/// The ids a one-column migration-table query returns.
async fn ids(conn: &mut toasty::Connection, sql: &str) -> Result<Vec<u64>, HistoryError> {
    let rows = toasty::sql::query(sql).exec(conn).await?;
    rows.into_iter()
        .map(|row| {
            let id = columns(row).and_then(|[id]: [Value; 1]| integer(id));
            match id {
                Ok(Some(id)) => Ok(id as u64),
                Ok(None) => Err("null migration id".to_owned()),
                Err(e) => Err(e),
            }
            .map_err(|e| {
                HistoryError::Migration(toasty::Error::from_args(format_args!(
                    "reading applied migrations: {e}"
                )))
            })
        })
        .collect()
}

/// The `N` columns of a raw-SQL row.
fn columns<const N: usize, E: From<String>>(row: Value) -> Result<[Value; N], E> {
    let Value::Record(record) = row else {
        return Err(format!("expected a row, got {row:?}").into());
    };
    let len = record.fields.len();
    <[Value; N]>::try_from(record.fields)
        .map_err(|_| format!("expected {N} columns, got {len}").into())
}

fn text(value: Value) -> Result<Option<String>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s)),
        other => Err(format!("expected text, got {other:?}")),
    }
}

fn integer(value: Value) -> Result<Option<i64>, String> {
    match value {
        Value::Null => Ok(None),
        Value::I64(n) => Ok(Some(n)),
        other => Err(format!("expected an integer, got {other:?}")),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Truncates to `MAX_BODY_BYTES` on a char boundary.
fn cap(body: &str) -> &str {
    if body.len() <= MAX_BODY_BYTES {
        return body;
    }
    let mut end = MAX_BODY_BYTES;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    &body[..end]
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a migration-style script, statements split on Toasty breakpoints.
    async fn exec_script(history: &History, sql: &str) {
        for statement in sql.split("\n-- #[toasty::breakpoint]\n") {
            toasty::sql::statement(statement)
                .exec(&mut history.db.clone())
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn failing_migration_aborts_with_error() {
        static BAD: &[MigrationFile] = &[MigrationFile::new(1, "0001_bad.sql", "THIS IS NOT SQL;")];
        let tmp = tempfile::tempdir().unwrap();

        let err = History::open_with(&tmp.path().join("db.sqlite3"), &MigrationSet::new(BAD))
            .await
            .err()
            .expect("must fail");
        assert!(matches!(err, HistoryError::Migration(_)), "{err}");
    }

    #[tokio::test]
    async fn database_migrated_by_sqlx_is_adopted_without_rerunning() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("db.sqlite3");

        // A database as hozd left it before Toasty: every migration recorded by sqlx only.
        {
            let history = History::open_with(&path, &MigrationSet::default())
                .await
                .unwrap();
            exec_script(&history, "DROP TABLE IF EXISTS __toasty_migrations").await;
            for migration in MIGRATIONS.migrations() {
                exec_script(&history, migration.sql()).await;
            }
            exec_script(
                &history,
                "CREATE TABLE _sqlx_migrations (
                    version BIGINT PRIMARY KEY,
                    description TEXT NOT NULL,
                    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    success BOOLEAN NOT NULL,
                    checksum BLOB NOT NULL,
                    execution_time BIGINT NOT NULL
                )",
            )
            .await;
            for migration in MIGRATIONS.migrations() {
                toasty::sql::statement(
                    "INSERT INTO _sqlx_migrations
                        (version, description, success, checksum, execution_time)
                     VALUES (?1, 'm', 1, x'00', 0)",
                )
                .bind(migration.id() as i64)
                .exec(&mut history.db.clone())
                .await
                .unwrap();
            }
            exec_script(
                &history,
                "INSERT INTO history (created_at_ms, method, url, request_headers, status)
                 VALUES (1, 'GET', 'http://x/kept', '[]', 200)",
            )
            .await;
        }

        // re-running any migration would fail: their tables and columns exist already
        let history = History::open(&path).await.unwrap();
        let list = history.list(10).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].url, "http://x/kept");

        let mut conn = history.db.connection().await.unwrap();
        let adopted = ids(&mut conn, "SELECT id FROM __toasty_migrations ORDER BY id")
            .await
            .unwrap();
        let all: Vec<u64> = MIGRATIONS.migrations().iter().map(|m| m.id()).collect();
        assert_eq!(adopted, all);
        drop(conn);
        History::open(&path).await.unwrap(); // adopting again is a no-op
    }

    #[tokio::test]
    async fn upgrade_keeps_pre_grpc_rows_as_http_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("db.sqlite3");

        // A database as the release before gRPC support left it: only migration 0001.
        let old = MigrationSet::new(&MIGRATIONS.migrations()[..1]);
        {
            let history = History::open_with(&path, &old).await.unwrap();
            exec_script(
                &history,
                "INSERT INTO history
                    (created_at_ms, method, url, request_headers, request_body,
                     status, response_headers, response_body, elapsed_ms, error)
                 VALUES (1, 'POST', 'http://x/old', '[{\"name\":\"A\",\"value\":\"1\"}]', '{}',
                         201, '[]', 'made', 5, NULL)",
            )
            .await;
        }

        let history = History::open(&path).await.unwrap(); // applies 0002
        let list = history.list(10).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, Kind::Http);
        assert_eq!(
            (list[0].method.as_str(), list[0].status),
            ("POST", Some(201))
        );

        let entry = history.get(list[0].id).await.unwrap().unwrap();
        assert_eq!(
            entry.request,
            Request::Http(HttpRequest {
                method: Method::Post,
                url: "http://x/old".into(),
                headers: vec![Header::new("A", "1")],
                body: Some("{}".into()),
            })
        );
        let Outcome::Response(res) = entry.outcome else {
            panic!("expected a response");
        };
        assert_eq!((res.status, res.body.as_str()), (201, "made"));
    }
}
