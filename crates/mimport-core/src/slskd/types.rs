use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct SessionResponse {
    pub token: String,
    #[serde(rename = "tokenType")]
    pub token_type: String,
}

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

#[derive(Debug, Serialize)]
pub struct QueueDownloadRequestItem {
    pub filename: String,
    pub size: i64,
}

#[derive(Debug, Deserialize)]
pub struct EnqueueResult {
    #[serde(default)]
    pub enqueued: Vec<Transfer>,
    #[serde(default)]
    pub failed: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Transfer {
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
    #[serde(rename = "elapsedTime")]
    pub elapsed_time: Option<String>,
    #[serde(rename = "remainingTime")]
    pub remaining_time: Option<String>,
    #[serde(rename = "placeInQueue")]
    pub place_in_queue: Option<i32>,
    pub exception: Option<String>,
}

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
