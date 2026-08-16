//! Wire types for `api.lidarr.audio`. Key casing is inconsistent within the same object,
//! so every field is explicitly renamed — don't switch to a blanket `rename_all` without
//! re-verifying against a live response.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct ArtistResource {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "artistname")]
    pub artist_name: String,
    #[serde(rename = "sortname")]
    pub sort_name: Option<String>,
    #[serde(rename = "disambiguation")]
    pub disambiguation: Option<String>,
    #[serde(rename = "type")]
    pub artist_type: Option<String>,
    #[serde(rename = "status")]
    pub status: Option<String>,
    #[serde(rename = "genres", default)]
    pub genres: Vec<String>,
    /// Empty on search results; summary-only on lookups.
    #[serde(rename = "Albums", default)]
    pub albums: Vec<AlbumSummary>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AlbumSummary {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Title")]
    pub title: String,
    #[serde(rename = "Type")]
    pub album_type: Option<String>,
    #[serde(rename = "SecondaryTypes", default)]
    pub secondary_types: Vec<String>,
    #[serde(rename = "ReleaseStatuses", default)]
    pub release_statuses: Vec<String>,
    #[serde(rename = "ReleaseDate")]
    pub release_date: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AlbumResource {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "title")]
    pub title: String,
    #[serde(rename = "disambiguation")]
    pub disambiguation: Option<String>,
    #[serde(rename = "releasedate")]
    pub release_date: Option<String>,
    #[serde(rename = "type")]
    pub album_type: Option<String>,
    #[serde(rename = "secondarytypes", default)]
    pub secondary_types: Vec<String>,
    #[serde(rename = "Releases", default)]
    pub releases: Vec<ReleaseResource>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReleaseResource {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Title")]
    pub title: String,
    #[serde(rename = "Status")]
    pub status: Option<String>,
    #[serde(rename = "Disambiguation")]
    pub disambiguation: Option<String>,
    #[serde(rename = "Country", default)]
    pub country: Vec<String>,
    #[serde(rename = "Label", default)]
    pub label: Vec<String>,
    #[serde(rename = "Media", default)]
    pub media: Vec<MediaResource>,
    #[serde(rename = "ReleaseDate")]
    pub release_date: Option<String>,
    #[serde(rename = "TrackCount", default)]
    pub track_count: u32,
    #[serde(rename = "Tracks", default)]
    pub tracks: Vec<TrackResource>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MediaResource {
    #[serde(rename = "Position")]
    pub position: Option<u32>,
    #[serde(rename = "Format")]
    pub format: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TrackResource {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "RecordingId")]
    pub recording_id: Option<String>,
    #[serde(rename = "OldRecordingIds", default)]
    pub old_recording_ids: Vec<String>,
    #[serde(rename = "TrackName")]
    pub track_name: String,
    #[serde(rename = "TrackNumber")]
    pub track_number: Option<String>,
    #[serde(rename = "DurationMs")]
    pub duration_ms: Option<u64>,
    #[serde(rename = "MediumNumber")]
    pub medium_number: Option<u32>,
}

/// Combined `search?type=all` result item — exactly one of `artist`/`album` is set.
#[derive(Debug, Deserialize, Serialize)]
pub struct SearchResultItem {
    #[serde(rename = "score")]
    pub score: f64,
    #[serde(rename = "artist")]
    pub artist: Option<ArtistResource>,
    #[serde(rename = "album")]
    pub album: Option<AlbumResource>,
}
