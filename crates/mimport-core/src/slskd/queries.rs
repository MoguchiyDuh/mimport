use std::time::{Duration, Instant};

use crate::config::SlskdConfig;
use crate::error::{Error, Result};

use super::client::SlskdClient;
use super::types::{
    Directory, DirectoryContentsRequest, EnqueueResult, QueueDownloadRequestItem, Search,
    SearchRequest, Transfer,
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

pub fn enqueue_download(
    client: &SlskdClient,
    username: &str,
    filename: &str,
    size: i64,
) -> Result<EnqueueResult> {
    let path = format!("/api/v0/transfers/downloads/{}", urlencode(username));
    let body = vec![QueueDownloadRequestItem {
        filename: filename.to_string(),
        size,
    }];
    return client.post(&path, &body);
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

fn is_terminal_state(state: &str) -> bool {
    return state.starts_with("Completed")
        || state.contains("Cancelled")
        || state.contains("Errored")
        || state.contains("Rejected");
}

pub fn fetch_and_wait(
    client: &SlskdClient,
    cfg: &SlskdConfig,
    username: &str,
    filename: &str,
    size: i64,
) -> Result<Transfer> {
    let enqueued = enqueue_download(client, username, filename, size)?;
    let id = enqueued
        .enqueued
        .first()
        .map(|t| return t.id.clone())
        .ok_or_else(|| {
            return Error::Slskd {
                what: "enqueue",
                status: 0,
                body: format!("no transfer returned in enqueued[] (failed: {:?})", enqueued.failed),
            };
        })?;

    let deadline = Instant::now() + fetch_wait_timeout(cfg, size);

    loop {
        let transfer = transfer_status(client, username, &id)?;
        if is_terminal_state(&transfer.state) {
            return Ok(transfer);
        }
        if Instant::now() >= deadline {
            return Err(Error::SlskdFetchTimeout {
                username: username.to_string(),
                id,
                waited_secs: fetch_wait_timeout(cfg, size).as_secs(),
                last_state: transfer.state,
            });
        }
        std::thread::sleep(POLL_INTERVAL);
    }
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

