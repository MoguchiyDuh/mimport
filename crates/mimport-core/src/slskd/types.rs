//! Wire types for slskd 0.25.1's `/api/v0` REST API. Field shapes confirmed live
//! (2026-08-14) against `myvps`'s instance for auth + `/application`; search/download/
//! browse shapes are transcribed from the 0.25.1 controller/DTO source (swagger is
//! disabled on this instance) — see `phases/02-implementation/RESEARCH.md`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct SessionResponse {
    pub token: String,
    #[serde(rename = "tokenType")]
    pub token_type: String,
    pub expires: i64,
    pub issued: i64,
}

/// `POST /api/v0/searches` request body. Deliberately never sets `searchTimeout` —
/// omitted so slskd's own server-side default applies (mimport owns no search timeout,
/// see DESIGN decision 2026-08-14).
#[derive(Debug, Serialize)]
pub struct SearchRequest {
    #[serde(rename = "searchText")]
    pub search_text: String,
}

impl SearchRequest {
    pub fn new(search_text: impl Into<String>) -> Self {
        return SearchRequest {
            search_text: search_text.into(),
        };
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Search {
    pub id: String,
    pub token: i64,
    #[serde(rename = "searchText")]
    pub search_text: String,
    pub state: String,
    #[serde(rename = "isComplete")]
    pub is_complete: bool,
    #[serde(rename = "fileCount", default)]
    pub file_count: u32,
    #[serde(rename = "lockedFileCount", default)]
    pub locked_file_count: u32,
    #[serde(rename = "responseCount", default)]
    pub response_count: u32,
    #[serde(default)]
    pub responses: Vec<SearchResponseItem>,
    #[serde(rename = "startedAt")]
    pub started_at: Option<String>,
    #[serde(rename = "endedAt")]
    pub ended_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SearchResponseItem {
    pub username: String,
    pub token: i64,
    #[serde(rename = "fileCount", default)]
    pub file_count: u32,
    #[serde(default)]
    pub files: Vec<SlskdFile>,
    #[serde(rename = "lockedFileCount", default)]
    pub locked_file_count: u32,
    #[serde(rename = "lockedFiles", default)]
    pub locked_files: Vec<SlskdFile>,
    #[serde(rename = "hasFreeUploadSlot")]
    pub has_free_upload_slot: bool,
    #[serde(rename = "queueLength")]
    pub queue_length: i64,
    #[serde(rename = "uploadSpeed")]
    pub upload_speed: i64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SlskdFile {
    pub filename: String,
    pub size: i64,
    pub extension: Option<String>,
    #[serde(rename = "bitRate")]
    pub bit_rate: Option<i32>,
    #[serde(rename = "bitDepth")]
    pub bit_depth: Option<i32>,
    #[serde(rename = "sampleRate")]
    pub sample_rate: Option<i32>,
    pub length: Option<i32>,
    #[serde(rename = "isVariableBitRate", default)]
    pub is_variable_bit_rate: bool,
    #[serde(rename = "isLocked", default)]
    pub is_locked: bool,
}

/// `POST /api/v0/transfers/downloads/{username}` request body — an array of these.
#[derive(Debug, Serialize)]
pub struct QueueDownloadRequestItem {
    pub filename: String,
    pub size: i64,
}

/// `enqueued[]` reuses `Transfer`'s shape (confirmed live 2026-08-14 — the enqueue
/// response's `id` is a real GUID assigned server-side at enqueue time, plus a couple
/// of enqueue-only fields like `stateDescription`/`requestedAt`/`attempts` that
/// `Transfer` doesn't model but serde silently ignores). `failed[]`'s shape is
/// unconfirmed (never actually hit one live) — kept loose.
#[derive(Debug, Deserialize)]
pub struct EnqueueResult {
    #[serde(default)]
    pub enqueued: Vec<Transfer>,
    #[serde(default)]
    pub failed: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Transfer {
    /// Real GUID assigned by slskd at enqueue time.
    ///
    /// **Correction (2026-08-14, live-tested):** earlier research claimed this was
    /// `sha1(filename)`. Wrong — confirmed live, it's a proper GUID returned in the
    /// enqueue response itself. Must be captured from `EnqueueResult`, never
    /// recomputed client-side.
    pub id: String,
    pub username: String,
    pub filename: String,
    pub direction: String,
    pub state: String,
    pub size: i64,
    #[serde(rename = "bytesTransferred", default)]
    pub bytes_transferred: i64,
    #[serde(rename = "bytesRemaining", default)]
    pub bytes_remaining: i64,
    #[serde(rename = "percentComplete", default)]
    pub percent_complete: f64,
    #[serde(rename = "averageSpeed", default)]
    pub average_speed: f64,
    /// **Correction (2026-08-14, live-tested):** earlier research claimed these were
    /// numeric milliseconds. Wrong — confirmed live, they're .NET `TimeSpan` strings
    /// (`"hh:mm:ss.fffffff"`, e.g. `"00:00:00.6893938"`).
    #[serde(rename = "elapsedTime")]
    pub elapsed_time: Option<String>,
    #[serde(rename = "remainingTime")]
    pub remaining_time: Option<String>,
    #[serde(rename = "placeInQueue")]
    pub place_in_queue: Option<i32>,
    pub exception: Option<String>,
}

/// `POST /api/v0/users/{username}/directory` request body.
#[derive(Debug, Serialize)]
pub struct DirectoryContentsRequest {
    pub directory: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Directory {
    pub name: String,
    #[serde(default)]
    pub files: Vec<SlskdFile>,
    #[serde(default)]
    pub directories: Vec<Directory>,
}
