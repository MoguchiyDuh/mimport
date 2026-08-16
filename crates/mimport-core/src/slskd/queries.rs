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
    let submitted: Search = client.post_with_timeout("/api/v0/searches", &req, SEARCH_REQUEST_TIMEOUT)?;

    let mut current = submitted;
    while !current.is_complete {
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

pub fn browse_directory(client: &SlskdClient, username: &str, directory: &str) -> Result<Vec<Directory>> {
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

/// Enqueues `files` and polls each to a terminal state. `on_update` is called
/// with every observed `Transfer` (including the initial post-enqueue state
/// and every subsequent poll) so a caller can persist progress incrementally
/// — it still runs, and its result still propagates, even on a timeout below,
/// so callers see the last-known state of every transfer either way.
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
            body: format!("no transfers returned in enqueued[] (failed: {:?})", enqueued.failed),
        });
    }
    for t in &enqueued.enqueued {
        on_update(t)?;
    }

    let total_size: i64 = files.iter().map(|f| return f.size).sum();
    let timeout = fetch_wait_timeout(cfg, total_size);
    let deadline = Instant::now() + timeout;

    let mut results = Vec::with_capacity(enqueued.enqueued.len());
    for t in &enqueued.enqueued {
        loop {
            let transfer = transfer_status(client, username, &t.id)?;
            on_update(&transfer)?;
            if is_terminal_state(&transfer.state) {
                results.push(transfer);
                break;
            }
            if Instant::now() >= deadline {
                return Err(Error::SlskdFetchTimeout {
                    username: username.to_string(),
                    id: t.id.clone(),
                    waited_secs: timeout.as_secs(),
                    last_state: transfer.state,
                });
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
    return Ok(results);
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

