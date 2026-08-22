use std::time::{Duration, Instant};

use crate::config::SlskdConfig;
use crate::error::{Error, Result};

use super::client::SlskdClient;
use super::types::{
    Directory, DirectoryContentsRequest, EnqueueResult, QueueDownloadRequestItem, Search,
    SearchRequest, SlskdFile, Transfer,
};

const SEARCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

const POLL_INTERVAL: Duration = Duration::from_millis(1500);

pub fn search(client: &SlskdClient, query: &str) -> Result<Search> {
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
    return Ok(current);
}

pub fn search_status(client: &SlskdClient, id: &str) -> Result<Search> {
    let path = format!("/api/v0/searches/{}?includeResponses=true", urlencode(id));
    return client.get(&path);
}

pub fn enqueue_downloads(
    client: &SlskdClient,
    username: &str,
    items: &[QueueDownloadRequestItem],
) -> Result<EnqueueResult> {
    let path = format!("/api/v0/transfers/downloads/{}", urlencode(username));
    return client.post(&path, &items);
}

/// Splits a Soulseek remote path (`\`-separated) into (directory, basename).
pub fn split_remote_path(path: &str) -> (&str, &str) {
    return match path.rfind('\\') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    };
}

/// Resolves a `<username> <directory> [<filename>]` selector against an
/// already-fetched `Search`'s responses. `directory` must match exactly as
/// it appears in the search result's file paths.
pub fn resolve_selector<'a>(
    search: &'a Search,
    username: &str,
    directory: &str,
    filename: Option<&str>,
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

    let Some(filename) = filename else {
        return Ok(in_dir);
    };

    let matched: Vec<&SlskdFile> = in_dir
        .into_iter()
        .filter(|f| return split_remote_path(&f.filename).1 == filename)
        .collect();
    if matched.is_empty() {
        return Err(Error::SlskdSelectorNotFound {
            what: "file",
            detail: format!("{username}: {directory}\\{filename}"),
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

pub fn fetch_wait_timeout(cfg: &SlskdConfig, size_bytes: i64) -> Duration {
    let size_mb = (size_bytes.max(0) as f64) / (1024.0 * 1024.0);
    let secs = cfg.fetch_timeout_base_secs as f64 + size_mb * cfg.fetch_timeout_per_mb_secs;
    return Duration::from_secs_f64(secs.max(1.0));
}

pub fn is_terminal_state(state: &str) -> bool {
    return state.starts_with("Completed")
        || state.contains("Cancelled")
        || state.contains("Errored")
        || state.contains("Rejected");
}

/// Enqueues `files` and polls each to a terminal state; `on_update` fires on
/// every observed `Transfer` so progress persists even on timeout.
pub fn fetch_and_wait(
    client: &SlskdClient,
    cfg: &SlskdConfig,
    username: &str,
    files: &[&SlskdFile],
    mut on_update: impl FnMut(&Transfer) -> Result<()>,
) -> Result<Vec<Transfer>> {
    let items: Vec<QueueDownloadRequestItem> = files
        .iter()
        .map(|f| {
            return QueueDownloadRequestItem {
                filename: f.filename.clone(),
                size: f.size,
            };
        })
        .collect();
    let enqueued = enqueue_downloads(client, username, &items)?;
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
    for t in &enqueued.enqueued {
        on_update(t)?;
    }

    let mut pending: Vec<PendingFetch> = enqueued
        .enqueued
        .iter()
        .map(|t| {
            let timeout = fetch_wait_timeout(cfg, t.size);
            return PendingFetch {
                transfer: t.clone(),
                deadline: Instant::now() + timeout,
                timeout_secs: timeout.as_secs(),
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
            let deadline = pending[i].deadline;
            let timeout_secs = pending[i].timeout_secs;
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
                    pending[i].transfer = transfer;
                }
                Err(e) => {
                    if Instant::now() >= deadline {
                        if timeout_err.is_none() {
                            timeout_err = Some(e);
                        }
                        pending.remove(i);
                        continue;
                    }
                    i += 1;
                    continue;
                }
            }

            if Instant::now() >= deadline {
                let err = Error::SlskdFetchTimeout {
                    username: username.to_string(),
                    id,
                    waited_secs: timeout_secs,
                    last_state: pending[i].transfer.state.clone(),
                };
                if timeout_err.is_none() {
                    timeout_err = Some(err);
                }
                pending.remove(i);
                continue;
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

struct PendingFetch {
    transfer: Transfer,
    deadline: Instant,
    timeout_secs: u64,
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
