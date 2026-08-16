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
    #[serde(rename = "artist-credit", default)]
    pub artist_credit: Vec<ArtistCreditItem>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Media {
    pub format: Option<String>,
    #[serde(default)]
    pub position: Option<u32>,
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
pub struct ReleaseGroup {
    pub id: String,
    pub title: String,
    #[serde(rename = "primary-type")]
    pub primary_type: Option<String>,
    #[serde(rename = "secondary-types", default)]
    pub secondary_types: Vec<String>,
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
    #[serde(rename = "joinphrase", default)]
    pub join_phrase: String,
}

/// Joins the artist-credit array into one display string.
pub fn join_artist_credit(credits: &[ArtistCreditItem]) -> Option<String> {
    if credits.is_empty() {
        return None;
    }
    let mut out = String::new();
    for c in credits {
        out.push_str(&c.name);
        out.push_str(&c.join_phrase);
    }
    return Some(out);
}
