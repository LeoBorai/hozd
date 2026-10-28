use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use sqlx::migrate::{MigrateError, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::client::{ClientError, Header, Method, Request, Response};

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
}

/// What came back from an executed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Response(Response),
    Error(String),
}

impl From<Result<Response, ClientError>> for Outcome {
    fn from(r: Result<Response, ClientError>) -> Self {
        match r {
            Ok(res) => Outcome::Response(res),
            Err(e) => Outcome::Error(e.to_string()),
        }
    }
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
    pub method: Method,
    pub url: String,
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
        let now = now_ms();
        let method = request.method.as_str();
        let req_headers = serde_json::to_string(&request.headers).expect("headers serialize");
        let req_body = request.body.as_deref().map(cap);

        let (status, res_headers, res_body, elapsed_ms, error) = match outcome {
            Outcome::Response(r) => (
                Some(i64::from(r.status)),
                Some(serde_json::to_string(&r.headers).expect("headers serialize")),
                Some(cap(&r.body)),
                Some(r.elapsed.as_millis() as i64),
                None,
            ),
            Outcome::Error(msg) => (None, None, None, None, Some(msg.as_str())),
        };

        let id = sqlx::query!(
            "INSERT INTO history
                (created_at_ms, method, url, request_headers, request_body,
                 status, response_headers, response_body, elapsed_ms, error)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            now,
            method,
            request.url,
            req_headers,
            req_body,
            status,
            res_headers,
            res_body,
            elapsed_ms,
            error,
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
            r#"SELECT id as "id!", method, url, status FROM history ORDER BY id DESC LIMIT ?"#,
            limit
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| {
                Ok(Summary {
                    id: r.id,
                    method: parse_method(r.id, &r.method)?,
                    url: r.url,
                    status: r.status.map(|s| s as u16),
                })
            })
            .collect()
    }

    pub async fn get(&self, id: i64) -> Result<Option<HistoryEntry>, HistoryError> {
        let Some(r) = sqlx::query!(
            r#"SELECT id as "id!", created_at_ms, method, url, request_headers, request_body,
                      status, response_headers, response_body, elapsed_ms, error
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

        let request = Request {
            method: parse_method(id, &r.method)?,
            url: r.url,
            headers: headers(&r.request_headers)?,
            body: r.request_body,
        };
        let outcome = match (r.status, r.error) {
            (Some(status), _) => Outcome::Response(Response {
                status: status as u16,
                headers: headers(r.response_headers.as_deref().unwrap_or("[]"))?,
                body: r.response_body.unwrap_or_default(),
                elapsed: Duration::from_millis(r.elapsed_ms.unwrap_or(0) as u64),
            }),
            (None, Some(msg)) => Outcome::Error(msg),
            (None, None) => return Err(corrupt("neither response nor error".into())),
        };

        Ok(Some(HistoryEntry {
            id,
            created_at_ms: r.created_at_ms,
            request,
            outcome,
        }))
    }
}

fn parse_method(id: i64, s: &str) -> Result<Method, HistoryError> {
    Method::from_str(s).map_err(|reason| HistoryError::Corrupt { id, reason })
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
}
