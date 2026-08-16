use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::slskd::queries::split_remote_path;
use crate::slskd::types::Transfer;

pub const STATUS_INCOMPLETE: &str = "incomplete";
pub const STATUS_FETCHED: &str = "fetched";
pub const STATUS_PARTIAL: &str = "partial";
pub const STATUS_FAILED: &str = "failed";
pub const STATUS_POSTFIXED: &str = "postfixed";
pub const STATUS_IMPORTED: &str = "imported";

#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub id: i64,
    pub title: String,
    pub status: String,
    pub search_id: String,
    pub username: String,
    pub directory: String,
    pub local_dir: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobFile {
    pub id: i64,
    pub job_id: i64,
    pub remote_path: String,
    pub transfer_id: String,
    pub size: i64,
    pub state: String,
    pub local_path: String,
}

pub fn open(path: impl AsRef<Path>) -> Result<Connection> {
    let path = path.as_ref();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| return Error::io(dir, e))?;
    }
    let conn = Connection::open(path)?;
    ensure_schema(&conn)?;
    return Ok(conn);
}

fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS jobs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL,
            status TEXT NOT NULL,
            search_id TEXT NOT NULL,
            username TEXT NOT NULL,
            directory TEXT NOT NULL,
            local_dir TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
        CREATE TABLE IF NOT EXISTS job_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id INTEGER NOT NULL REFERENCES jobs(id),
            remote_path TEXT NOT NULL,
            transfer_id TEXT NOT NULL,
            size INTEGER NOT NULL,
            state TEXT NOT NULL,
            local_path TEXT NOT NULL,
            UNIQUE(job_id, transfer_id)
        );
        CREATE INDEX IF NOT EXISTS job_files_job_id ON job_files(job_id);",
    )?;
    return Ok(());
}

/// The local directory slskd will place a fetch's files under, by observed
/// convention: `<downloads_root>/<basename of the remote directory>`.
pub fn local_dir_for(downloads_root: &Path, remote_directory: &str) -> PathBuf {
    let basename = remote_directory.rsplit('\\').next().unwrap_or(remote_directory);
    return downloads_root.join(basename);
}

/// Default `jobs.title` when `--title` isn't given: same basename used for `local_dir`.
pub fn default_title(remote_directory: &str) -> String {
    return remote_directory
        .rsplit('\\')
        .next()
        .unwrap_or(remote_directory)
        .to_string();
}

pub fn create_job(
    conn: &Connection,
    title: &str,
    search_id: &str,
    username: &str,
    directory: &str,
    local_dir: &Path,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO jobs (title, status, search_id, username, directory, local_dir)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            title,
            STATUS_INCOMPLETE,
            search_id,
            username,
            directory,
            local_dir.to_string_lossy(),
        ],
    )?;
    return Ok(conn.last_insert_rowid());
}

pub fn set_job_status(conn: &Connection, job_id: i64, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE jobs SET status = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?2",
        params![status, job_id],
    )?;
    return Ok(());
}

/// Insert-or-update a job's file row keyed by (job_id, transfer_id), computing
/// `local_path` from the job's `local_dir` and the transfer's remote filename.
/// Safe to call repeatedly as a transfer's state changes during polling.
pub fn upsert_job_file(conn: &Connection, job_id: i64, local_dir: &Path, transfer: &Transfer) -> Result<()> {
    let basename = split_remote_path(&transfer.filename).1;
    let local_path = local_dir.join(basename);
    conn.execute(
        "INSERT INTO job_files (job_id, remote_path, transfer_id, size, state, local_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(job_id, transfer_id) DO UPDATE SET state = excluded.state",
        params![
            job_id,
            transfer.filename,
            transfer.id,
            transfer.size,
            transfer.state,
            local_path.to_string_lossy(),
        ],
    )?;
    conn.execute(
        "UPDATE jobs SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
        params![job_id],
    )?;
    return Ok(());
}

pub fn get_job(conn: &Connection, job_id: i64) -> Result<Job> {
    return conn
        .query_row("SELECT * FROM jobs WHERE id = ?1", params![job_id], row_to_job)
        .map_err(|e| {
            return match e {
                rusqlite::Error::QueryReturnedNoRows => Error::JobNotFound {
                    target: job_id.to_string(),
                },
                other => Error::Db(other),
            };
        });
}

pub fn get_job_files(conn: &Connection, job_id: i64) -> Result<Vec<JobFile>> {
    let mut stmt = conn.prepare("SELECT * FROM job_files WHERE job_id = ?1 ORDER BY id")?;
    let rows = stmt
        .query_map(params![job_id], row_to_job_file)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    return Ok(rows);
}

/// Resolves a job-scoped command target: a numeric string is a job id
/// lookup; anything else matches `jobs.title` exactly, most-recent (highest
/// id) wins on multiple hits.
pub fn resolve_target(conn: &Connection, target: &str) -> Result<Job> {
    if let Ok(id) = target.parse::<i64>() {
        return get_job(conn, id);
    }
    let found: Option<Job> = conn
        .query_row(
            "SELECT * FROM jobs WHERE title = ?1 ORDER BY id DESC LIMIT 1",
            params![target],
            row_to_job,
        )
        .optional()?;
    return found.ok_or_else(|| {
        return Error::JobNotFound {
            target: target.to_string(),
        };
    });
}

fn row_to_job(row: &rusqlite::Row) -> rusqlite::Result<Job> {
    return Ok(Job {
        id: row.get("id")?,
        title: row.get("title")?,
        status: row.get("status")?,
        search_id: row.get("search_id")?,
        username: row.get("username")?,
        directory: row.get("directory")?,
        local_dir: row.get("local_dir")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    });
}

fn row_to_job_file(row: &rusqlite::Row) -> rusqlite::Result<JobFile> {
    return Ok(JobFile {
        id: row.get("id")?,
        job_id: row.get("job_id")?,
        remote_path: row.get("remote_path")?,
        transfer_id: row.get("transfer_id")?,
        size: row.get("size")?,
        state: row.get("state")?,
        local_path: row.get("local_path")?,
    });
}

/// Aggregates a completed batch of terminal `Transfer` states into a job status.
pub fn derive_status(transfers: &[Transfer]) -> &'static str {
    let states: Vec<&str> = transfers.iter().map(|t| return t.state.as_str()).collect();
    return derive_status_from_states(&states);
}

/// Same aggregation, from raw state strings (e.g. `JobFile::state` after a cancel).
pub fn derive_status_from_states(states: &[&str]) -> &'static str {
    if states.is_empty() {
        return STATUS_INCOMPLETE;
    }
    let succeeded = states.iter().filter(|s| return is_succeeded(s)).count();
    if succeeded == states.len() {
        return STATUS_FETCHED;
    }
    if succeeded == 0 {
        return STATUS_FAILED;
    }
    return STATUS_PARTIAL;
}

fn is_succeeded(state: &str) -> bool {
    return state.starts_with("Completed") && state.contains("Succeeded");
}
