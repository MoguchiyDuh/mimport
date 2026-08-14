//! Wire types for direct MusicBrainz JSON (`?fmt=json`). Minimal — only what the `mb`
//! command family actually needs (artist search/lookup, release-group→releases browse,
//! release→tracks lookup, recording search).

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct ArtistSearchResponse {
    pub artists: Vec<ArtistSearchResult>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ArtistSearchResult {
    pub id: String,
    pub name: String,
    #[serde(rename = "sort-name")]
    pub sort_name: Option<String>,
    pub disambiguation: Option<String>,
    pub country: Option<String>,
    #[serde(rename = "type")]
    pub artist_type: Option<String>,
    pub score: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
    #[serde(rename = "sort-name")]
    pub sort_name: Option<String>,
    pub disambiguation: Option<String>,
    pub country: Option<String>,
    #[serde(rename = "type")]
    pub artist_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReleaseBrowseResponse {
    pub releases: Vec<Release>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Release {
    pub id: String,
    pub title: String,
    pub status: Option<String>,
    pub date: Option<String>,
    pub country: Option<String>,
    pub disambiguation: Option<String>,
    #[serde(default)]
    pub media: Vec<Media>,
    #[serde(rename = "label-info", default)]
    pub label_info: Vec<LabelInfo>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Media {
    pub format: Option<String>,
    #[serde(rename = "track-count", default)]
    pub track_count: u32,
    #[serde(default)]
    pub tracks: Vec<Track>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Track {
    pub id: String,
    pub number: String,
    pub title: String,
    /// Milliseconds.
    pub length: Option<u64>,
    pub recording: Recording,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Recording {
    pub id: String,
    pub title: String,
    pub length: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LabelInfo {
    pub label: Option<Label>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Label {
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RecordingSearchResponse {
    pub recordings: Vec<RecordingSearchResult>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RecordingSearchResult {
    pub id: String,
    pub title: String,
    pub length: Option<u64>,
    pub score: Option<u32>,
    #[serde(rename = "artist-credit", default)]
    pub artist_credit: Vec<ArtistCreditItem>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ArtistCreditItem {
    pub name: String,
}
