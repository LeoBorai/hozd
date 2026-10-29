use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use sqlx::migrate::{MigrateError, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use serde::{Deserialize, Serialize};

use crate::client::{
    GrpcOutcome, GrpcRequest, GrpcStatus, Header, HttpRequest, HttpResponse, Kind, Method, Outcome,
    Request, SchemaSource,
};

/// Bodies larger than this are truncated before being stored.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

static MIGRATOR: Migrator = sqlx::migrate!();

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("the database {0} is from a newer hozd; please upgrade hozd")]
    NewerDatabase(String),
    #[error("database migration failed: {0}")]
    Migration(MigrateError),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
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
}

#[derive(Clone)]
pub struct History {
    pool: SqlitePool,
}

impl History {
    /// Opens (creating if missing) the database at `path` and applies pending migrations.
    pub async fn open(path: &Path) -> Result<Self, HistoryError> {
        Self::open_with(path, &MIGRATOR).await
    }

    pub(crate) async fn open_with(path: &Path, migrator: &Migrator) -> Result<Self, HistoryError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;

        restrict_permissions(path);
        Self::migrate(&pool, migrator, path).await?;
        Ok(Self { pool })
    }

    /// In-pool handle for `#[sqlx::test]`, which supplies an already-migrated pool.
    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn migrate(
        pool: &SqlitePool,
        migrator: &Migrator,
        path: &Path,
    ) -> Result<(), HistoryError> {
        migrator.run(pool).await.map_err(|e| match e {
            MigrateError::VersionMissing(_) => {
                HistoryError::NewerDatabase(path.display().to_string())
            }
            other => HistoryError::Migration(other),
        })
    }

    /// Records an executed request and its outcome, returning the new entry id.
    pub async fn record(&self, request: &Request, outcome: &Outcome) -> Result<i64, HistoryError> {
        let row = match (request, outcome) {
            (Request::Http(req), Outcome::Response(res)) => http_row(req, Some(res), None),
            (Request::Http(req), Outcome::Error(msg)) => http_row(req, None, Some(msg)),
            (Request::Grpc(req), Outcome::Grpc(out)) => grpc_row(req, out),
            _ => return Err(HistoryError::KindMismatch),
        };
        let now = now_ms();
        let kind = row.kind.as_str();

        let id = sqlx::query!(
            "INSERT INTO history
                (created_at_ms, kind, method, url, request_headers, request_body,
                 status, response_headers, response_body, elapsed_ms, error, grpc)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            now,
            kind,
            row.method,
            row.url,
            row.request_headers,
            row.request_body,
            row.status,
            row.response_headers,
            row.response_body,
            row.elapsed_ms,
            row.error,
            row.grpc,
        )
        .execute(&self.pool)
        .await?
        .last_insert_rowid();
        Ok(id)
    }

    /// Newest first.
    pub async fn list(&self, limit: u32) -> Result<Vec<Summary>, HistoryError> {
        let limit = i64::from(limit);
        let rows = sqlx::query!(
            r#"SELECT id as "id!", kind, method, url, status FROM history ORDER BY id DESC LIMIT ?"#,
            limit
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| {
                Ok(Summary {
                    id: r.id,
                    kind: parse_kind(r.id, &r.kind)?,
                    method: r.method,
                    url: r.url,
                    status: r.status.map(|s| s as u16),
                })
            })
            .collect()
    }

    pub async fn get(&self, id: i64) -> Result<Option<HistoryEntry>, HistoryError> {
        let Some(r) = sqlx::query!(
            r#"SELECT id as "id!", created_at_ms, kind, method, url, request_headers, request_body,
                      status, response_headers, response_body, elapsed_ms, error, grpc
               FROM history WHERE id = ?"#,
            id
        )
        .fetch_optional(&self.pool)
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
                    body: r.request_body,
                });
                let outcome = match (r.status, r.error) {
                    (Some(status), _) => Outcome::Response(HttpResponse {
                        status: status as u16,
                        headers: headers(r.response_headers.as_deref().unwrap_or("[]"))?,
                        body: r.response_body.unwrap_or_default(),
                        elapsed,
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

/// A history row before insertion, whichever the kind.
struct Row<'a> {
    kind: Kind,
    method: String,
    url: &'a str,
    request_headers: String,
    request_body: Option<&'a str>,
    status: Option<i64>,
    response_headers: Option<String>,
    response_body: Option<&'a str>,
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
        request_body: request.body.as_deref().map(cap),
        status: response.map(|r| i64::from(r.status)),
        response_headers: response
            .map(|r| serde_json::to_string(&r.headers).expect("headers serialize")),
        response_body: response.map(|r| cap(&r.body)),
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
        request_body: request.body.as_deref().map(cap),
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

fn parse_method(id: i64, s: &str) -> Result<Method, HistoryError> {
    Method::from_str(s).map_err(|reason| HistoryError::Corrupt { id, reason })
}

fn parse_kind(id: i64, s: &str) -> Result<Kind, HistoryError> {
    Kind::from_str(s).map_err(|reason| HistoryError::Corrupt { id, reason })
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

    #[tokio::test]
    async fn failing_migration_aborts_with_error() {
        let tmp = tempfile::tempdir().unwrap();
        let migrations = tmp.path().join("migrations");
        std::fs::create_dir(&migrations).unwrap();
        std::fs::write(migrations.join("0001_bad.sql"), "THIS IS NOT SQL;").unwrap();
        let migrator = Migrator::new(migrations).await.unwrap();

        let err = History::open_with(&tmp.path().join("db.sqlite3"), &migrator)
            .await
            .err()
            .expect("must fail");
        assert!(matches!(err, HistoryError::Migration(_)), "{err}");
    }

    #[tokio::test]
    async fn upgrade_keeps_pre_grpc_rows_as_http_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("db.sqlite3");

        // A database as the release before gRPC support left it: only migration 0001.
        let old_migrations = tmp.path().join("migrations");
        std::fs::create_dir(&old_migrations).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations/0001_init.sql"),
            old_migrations.join("0001_init.sql"),
        )
        .unwrap();
        let old = Migrator::new(old_migrations).await.unwrap();
        {
            let history = History::open_with(&path, &old).await.unwrap();
            sqlx::query(
                "INSERT INTO history
                    (created_at_ms, method, url, request_headers, request_body,
                     status, response_headers, response_body, elapsed_ms, error)
                 VALUES (1, 'POST', 'http://x/old', '[{\"name\":\"A\",\"value\":\"1\"}]', '{}',
                         201, '[]', 'made', 5, NULL)",
            )
            .execute(&history.pool)
            .await
            .unwrap();
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
