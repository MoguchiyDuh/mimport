//! Query functions for the `lidarr` command family.

use crate::error::Result;

use super::client::LidarrClient;
use super::types::{AlbumResource, ArtistResource, SearchResultItem};

pub fn search_artists(client: &LidarrClient, query: &str) -> Result<Vec<ArtistResource>> {
    let resp: Vec<ArtistResource> =
        client.get("/search", &[("type", "artist"), ("query", query)])?;
    return Ok(resp);
}

pub fn lookup_artist(client: &LidarrClient, mbid: &str) -> Result<ArtistResource> {
    return client.get(&format!("/artist/{mbid}"), &[]);
}

/// Full album with `Releases[]` nested (each carrying `Tracks[]`) — one call gets what
/// `mb` needs two for. Backs `lidarr album` (after §7 scoring) and `lidarr tracks`
/// (caller picks one `Releases[]` entry by id).
pub fn lookup_album(client: &LidarrClient, mbid: &str) -> Result<AlbumResource> {
    return client.get(&format!("/album/{mbid}"), &[]);
}

#[allow(dead_code)] // not wired into a command yet — kept for the combined-search path noted in RESEARCH.md
pub fn search_all(client: &LidarrClient, query: &str) -> Result<Vec<SearchResultItem>> {
    return client.get("/search", &[("type", "all"), ("query", query)]);
}
