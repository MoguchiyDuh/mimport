use crate::error::Result;

use super::client::MbClient;
use super::types::{
    Artist, ArtistSearchResponse, ArtistSearchResult, RecordingSearchResponse,
    RecordingSearchResult, Release, ReleaseBrowseResponse, ReleaseGroup,
};

pub fn escape_lucene(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "+-&|!(){}[]^\"~*?:\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    return out;
}

pub fn search_artists(client: &MbClient, query: &str) -> Result<Vec<ArtistSearchResult>> {
    let q = format!("artist:{}", escape_lucene(query));
    let resp: ArtistSearchResponse = client.get("/artist", &[("query", &q), ("limit", "10")])?;
    return Ok(resp.artists);
}

pub fn lookup_artist(client: &MbClient, mbid: &str) -> Result<Artist> {
    return client.get(&format!("/artist/{mbid}"), &[]);
}

pub fn release_group_releases(client: &MbClient, release_group_mbid: &str) -> Result<Vec<Release>> {
    let resp: ReleaseBrowseResponse = client.get(
        "/release",
        &[
            ("release-group", release_group_mbid),
            ("inc", "media+labels+recordings"),
            ("limit", "100"),
        ],
    )?;
    return Ok(resp.releases);
}

pub fn lookup_release_group(client: &MbClient, release_group_mbid: &str) -> Result<ReleaseGroup> {
    return client.get(&format!("/release-group/{release_group_mbid}"), &[]);
}

pub fn release_with_tracks(client: &MbClient, release_mbid: &str) -> Result<Release> {
    return client.get(
        &format!("/release/{release_mbid}"),
        &[("inc", "media+labels+recordings")],
    );
}

pub fn search_recordings(client: &MbClient, query: &str) -> Result<Vec<RecordingSearchResult>> {
    let q = format!("recording:{}", escape_lucene(query));
    let resp: RecordingSearchResponse =
        client.get("/recording", &[("query", &q), ("limit", "10")])?;
    return Ok(resp.recordings);
}
