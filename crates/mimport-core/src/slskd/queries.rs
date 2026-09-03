use std::path::Path;
use std::time::{Duration, Instant};

use crate::config::SlskdConfig;
use crate::error::{Error, Result};

use super::client::SlskdClient;
use super::types::{
    Directory, DirectoryContentsRequest, DownloadsResponse, EnqueueResult, Search, SearchRequest,
    SlskdFile, Transfer,
};

pub use super::types::QueueDownloadRequestItem;

const SEARCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

const POLL_INTERVAL: Duration = Duration::from_millis(1500);

pub struct SearchOutcome {
    pub search: Search,
    pub reused: bool,
}

/// Runs a search, or reuses the latest completed search with the same query
/// text that returned files, unless `fresh`.
pub fn search(client: &SlskdClient, query: &str, fresh: bool) -> Result<SearchOutcome> {
    if !fresh {
        return match reusable_search_id(client, query)? {
            Some(id) => Ok(SearchOutcome {
                search: search_status(client, &id)?,
                reused: true,
            }),
            None => run_search(client, query),
        };
    }
    return run_search(client, query);
}

fn run_search(client: &SlskdClient, query: &str) -> Result<SearchOutcome> {
    let req = SearchRequest::new(query);
    let submitted: Search =
        client.post_with_timeout("/api/v0/searches", &req, SEARCH_REQUEST_TIMEOUT)?;

    let mut current = submitted;
    let deadline = Instant::now() + SEARCH_REQUEST_TIMEOUT;
    while !current.is_complete {
        if Instant::now() >= deadline {
            return Err(Error::Slskd {
                what: "search",
                status: 0,
                body: "search never completed".to_string(),
            });
        }
        std::thread::sleep(POLL_INTERVAL);
        current = search_status(client, &current.id)?;
    }

    // slskd can flip isComplete before the response payload becomes readable
    // (observed: complete + ResponseLimitReached with 0 responses, full payload
    // a moment later); drain briefly until responses show up.
    let mut attempts = 0;
    while current.responses.is_empty() && attempts < 5 {
        std::thread::sleep(Duration::from_millis(500));
        current = search_status(client, &current.id)?;
        attempts += 1;
    }

    return Ok(SearchOutcome {
        search: current,
        reused: false,
    });
}

fn reusable_search_id(client: &SlskdClient, query: &str) -> Result<Option<String>> {
    let ended_or_started = |s: &Search| -> String {
        return s
            .ended_at
            .clone()
            .or_else(|| return s.started_at.clone())
            .unwrap_or_default();
    };
    return Ok(list_searches(client)?
        .into_iter()
        .filter(|s| {
            return s.search_text == query && s.is_complete && s.file_count > 0;
        })
        .max_by(|a, b| {
            return ended_or_started(a)
                .cmp(&ended_or_started(b))
                .then_with(|| return a.id.cmp(&b.id));
        })
        .map(|s| return s.id));
}

pub fn list_searches(client: &SlskdClient) -> Result<Vec<Search>> {
    return client.get("/api/v0/searches");
}

pub fn search_status(client: &SlskdClient, id: &str) -> Result<Search> {
    let path = format!("/api/v0/searches/{}?includeResponses=true", urlencode(id));
    return client.get(&path);
}

pub fn search_remove(client: &SlskdClient, id: &str) -> Result<()> {
    return client.delete(&format!("/api/v0/searches/{}", urlencode(id)));
}

pub fn enqueue_downloads(
    client: &SlskdClient,
    username: &str,
    items: &[QueueDownloadRequestItem],
) -> Result<EnqueueResult> {
    let path = format!("/api/v0/transfers/downloads/{}", urlencode(username));
    return client.post(&path, &items);
}

pub fn list_downloads(client: &SlskdClient) -> Result<Vec<DownloadsResponse>> {
    return client.get("/api/v0/transfers/downloads");
}

/// Cancels (if pending) and removes one tracked transfer. Removing an entry
/// that is already gone is not an error.
pub fn remove_download(client: &SlskdClient, username: &str, id: &str) -> Result<()> {
    let path = format!(
        "/api/v0/transfers/downloads/{}/{}?remove=true",
        urlencode(username),
        urlencode(id)
    );
    return match client.delete(&path) {
        Ok(()) => Ok(()),
        Err(Error::SlskdNotFound { .. }) => Ok(()),
        Err(e) => return Err(e),
    };
}

/// Removes every completed (succeeded/errored/cancelled) tracked download.
pub fn clear_completed_downloads(client: &SlskdClient) -> Result<()> {
    return client.delete("/api/v0/transfers/downloads/all/completed");
}

/// Splits a Soulseek remote path (`\`-separated) into (directory, basename).
pub fn split_remote_path(path: &str) -> (&str, &str) {
    return match path.rfind('\\') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    };
}

/// Resolves a `<username> <directory> [<filename>...]` selector against an
/// already-fetched `Search`'s responses. `directory` must match exactly as it
/// appears in the search result's file paths; an empty `filenames` selects
/// every file in the directory.
pub fn resolve_selector<'a>(
    search: &'a Search,
    username: &str,
    directory: &str,
    filenames: &[String],
) -> Result<Vec<&'a SlskdFile>> {
    let response = search
        .responses
        .iter()
        .find(|r| return r.username == username)
        .ok_or_else(|| {
            return Error::SlskdSelectorNotFound {
                what: "peer",
                detail: format!("no response from {username} in search {}", search.id),
            };
        })?;

    let in_dir: Vec<&SlskdFile> = response
        .files
        .iter()
        .filter(|f| return split_remote_path(&f.filename).0 == directory)
        .collect();
    if in_dir.is_empty() {
        return Err(Error::SlskdSelectorNotFound {
            what: "directory",
            detail: format!("{username} has no files under {directory:?}"),
        });
    }

    if filenames.is_empty() {
        return Ok(in_dir);
    }

    let mut matched = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut missing = Vec::new();
    for name in filenames {
        if !seen.insert(name.as_str()) {
            continue;
        }
        match in_dir
            .iter()
            .find(|f| return split_remote_path(&f.filename).1 == name)
        {
            Some(f) => matched.push(*f),
            None => missing.push(name.clone()),
        }
    }
    if !missing.is_empty() {
        return Err(Error::SlskdSelectorNotFound {
            what: "file",
            detail: format!(
                "{username}: {directory}\\ (missing: {})",
                missing.join(", ")
            ),
        });
    }
    return Ok(matched);
}

pub fn transfer_status(client: &SlskdClient, username: &str, id: &str) -> Result<Transfer> {
    let path = format!(
        "/api/v0/transfers/downloads/{}/{}",
        urlencode(username),
        urlencode(id)
    );
    return client.get(&path);
}

pub fn cancel_transfer(client: &SlskdClient, username: &str, id: &str, remove: bool) -> Result<()> {
    let path = format!(
        "/api/v0/transfers/downloads/{}/{}?remove={}",
        urlencode(username),
        urlencode(id),
        remove
    );
    return client.delete(&path);
}

pub fn browse_directory(
    client: &SlskdClient,
    username: &str,
    directory: &str,
) -> Result<Vec<Directory>> {
    let path = format!("/api/v0/users/{}/directory", urlencode(username));
    let body = DirectoryContentsRequest {
        directory: directory.to_string(),
    };
    return client.post(&path, &body);
}

/// Wait window for a fetch batch: sized to the whole batch (base + size +
/// per-file) and re-armed on any observable progress, so a healthy download
/// never times out while a silent/stuck one is given up on promptly.
pub fn batch_timeout(
    cfg: &SlskdConfig,
    items: &[QueueDownloadRequestItem],
    override_secs: Option<u64>,
) -> Duration {
    if let Some(secs) = override_secs {
        return Duration::from_secs(secs.max(1));
    }
    let total_mb =
        items.iter().map(|i| return i.size.max(0)).sum::<i64>() as f64 / (1024.0 * 1024.0);
    let secs = cfg.fetch_timeout_base_secs as f64
        + total_mb * cfg.fetch_timeout_per_mb_secs
        + items.len() as f64 * cfg.fetch_timeout_per_file_secs as f64;
    return Duration::from_secs_f64(secs.max(1.0));
}

pub fn is_terminal_state(state: &str) -> bool {
    return state.starts_with("Completed")
        || state.contains("Cancelled")
        || state.contains("Errored")
        || state.contains("Rejected");
}

/// Enqueues `items` and polls each to a terminal state; `on_update` fires on
/// every observed `Transfer` so progress persists even on timeout. The batch
/// deadline re-arms whenever any pending transfer changes state, moves bytes,
/// or advances in the remote queue; after `window` of total silence the
/// stragglers are abandoned and an error is returned (states already persisted
/// via `on_update`).
pub fn fetch_and_wait(
    client: &SlskdClient,
    username: &str,
    window: Duration,
    items: &[QueueDownloadRequestItem],
    on_update: impl FnMut(&Transfer) -> Result<()>,
) -> Result<Vec<Transfer>> {
    let enqueued = enqueue_downloads(client, username, items)?;
    if enqueued.enqueued.is_empty() {
        return Err(Error::Slskd {
            what: "enqueue",
            status: 0,
            body: format!(
                "no transfers returned in enqueued[] (failed: {:?})",
                enqueued.failed
            ),
        });
    }
    return wait_for_transfers(client, username, enqueued.enqueued, window, on_update);
}

/// Polls already-enqueued transfers to terminal state; `on_update` fires on
/// every observed `Transfer`. The batch deadline re-arms whenever any pending
/// transfer changes state, moves bytes, or advances in the remote queue;
/// after `window` of total silence the stragglers are abandoned and an error
/// is returned (states already persisted via `on_update`).
pub fn wait_for_transfers(
    client: &SlskdClient,
    username: &str,
    transfers: Vec<Transfer>,
    window: Duration,
    mut on_update: impl FnMut(&Transfer) -> Result<()>,
) -> Result<Vec<Transfer>> {
    let mut pending: Vec<PendingFetch> = transfers
        .into_iter()
        .map(|t| {
            return PendingFetch {
                sig: signature(&t),
                transfer: t,
                deadline: Instant::now() + window,
                window,
            };
        })
        .collect();

    let mut results = Vec::with_capacity(pending.len());
    let mut timeout_err: Option<Error> = None;

    loop {
        if pending.is_empty() {
            break;
        }
        let mut i = 0;
        while i < pending.len() {
            if Instant::now() >= pending[i].deadline {
                let err = Error::SlskdFetchTimeout {
                    username: username.to_string(),
                    id: pending[i].transfer.id.clone(),
                    waited_secs: pending[i].window.as_secs(),
                    last_state: pending[i].transfer.state.clone(),
                };
                if timeout_err.is_none() {
                    timeout_err = Some(err);
                }
                pending.remove(i);
                continue;
            }

            let id = pending[i].transfer.id.clone();
            // A transient status-poll failure must not abort the whole batch;
            // keep the file pending and retry next round until its deadline.
            match transfer_status(client, username, &id) {
                Ok(transfer) => {
                    on_update(&transfer)?;
                    if is_terminal_state(&transfer.state) {
                        results.push(transfer);
                        pending.remove(i);
                        continue;
                    }
                    let sig = signature(&transfer);
                    if sig != pending[i].sig {
                        pending[i].sig = sig;
                        pending[i].deadline = Instant::now() + pending[i].window;
                    }
                    pending[i].transfer = transfer;
                }
                Err(e) => {
                    if Instant::now() >= pending[i].deadline {
                        if timeout_err.is_none() {
                            timeout_err = Some(e);
                        }
                        pending.remove(i);
                        continue;
                    }
                }
            }
            i += 1;
        }
        if pending.is_empty() {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    if let Some(e) = timeout_err {
        return Err(e);
    }
    return Ok(results);
}

fn signature(t: &Transfer) -> (String, i64, Option<i32>) {
    return (t.state.clone(), t.bytes_transferred, t.place_in_queue);
}

const CANCEL_SETTLE_POLLS: u32 = 10;
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(1000);

/// Cancels and removes `old_ids` from slskd's transfer list so the same files
/// can be re-enqueued: cancel each (waiting out any remotely-queued state),
/// remove it, then confirm via the downloads list — slskd's by-id lookup can
/// keep returning soft-deleted records, so the list is authoritative.
pub fn clear_transfers(client: &SlskdClient, username: &str, old_ids: &[String]) -> Result<()> {
    for id in old_ids {
        match cancel_transfer(client, username, id, false) {
            Ok(()) | Err(Error::SlskdNotFound { .. }) => {}
            Err(e) => {
                return Err(e);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match transfer_status(client, username, id) {
                Ok(t) if is_terminal_state(&t.state) => break,
                Ok(_) if Instant::now() >= deadline => break,
                Ok(_) => std::thread::sleep(CANCEL_POLL_INTERVAL / 2),
                Err(Error::SlskdNotFound { .. }) => break,
                Err(e) => {
                    return Err(e);
                }
            }
        }
        match cancel_transfer(client, username, id, true) {
            Ok(()) | Err(Error::SlskdNotFound { .. }) => {}
            Err(e) => {
                return Err(e);
            }
        }
    }

    let mut attempts = 0;
    while attempts < CANCEL_SETTLE_POLLS {
        let still = list_downloads(client)?.iter().any(|d| {
            return d.username == username
                && d.directories.iter().any(|dir| {
                    return dir.files.iter().any(|t| {
                        return old_ids.iter().any(|id| {
                            return id == &t.id;
                        });
                    });
                });
        });
        if !still {
            return Ok(());
        }
        std::thread::sleep(CANCEL_POLL_INTERVAL);
        attempts += 1;
    }
    tracing::warn!(
        "slskd still lists some of the removed transfers for {username}; re-enqueue may be rejected"
    );
    return Ok(());
}

/// Enqueues and polls a batch to terminal state, persisting every observed
/// transfer into `job_id`'s job_files and the outcome into the job status.
pub fn fetch_into_job(
    client: &SlskdClient,
    username: &str,
    items: &[QueueDownloadRequestItem],
    window: Duration,
    conn: &rusqlite::Connection,
    job_id: i64,
    local_dir: &Path,
) -> Result<()> {
    let result = fetch_and_wait(client, username, window, items, |t| {
        return crate::jobs::upsert_job_file(conn, job_id, local_dir, t);
    });
    let transfers = match result {
        Ok(transfers) => transfers,
        Err(e) => {
            return Err(e);
        }
    };
    crate::jobs::set_job_status(conn, job_id, crate::jobs::derive_status(&transfers))?;
    return Ok(());
}

struct PendingFetch {
    transfer: Transfer,
    deadline: Instant,
    window: Duration,
    sig: (String, i64, Option<i32>),
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    return out;
}
