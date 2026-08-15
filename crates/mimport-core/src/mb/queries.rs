//! Query functions for the `mb` command family. Release-group/release lookups take an
//! MBID directly (per DESIGN.md §5 — no text search for albums, only artists/recordings
//! search by text).

use crate::error::Result;

use super::client::MbClient;
use super::types::{
    Artist, ArtistSearchResponse, ArtistSearchResult, RecordingSearchResponse,
    RecordingSearchResult, Release, ReleaseBrowseResponse, ReleaseGroup,
};

/// Escapes MB's Lucene special characters: `+ - && || ! ( ) { } [ ] ^ " ~ * ? : \`.
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

/// Browse every release in a release-group. `inc=media+labels+recordings` — the
/// `+recordings` is needed even though `mb album`'s own CLI output stays
/// summary-level: the §7 scorer reads real track titles (term-penalty detection of
/// live/remix bonus content) from every release it scores, not just ones fetched via
/// `mb tracks`.
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

/// Release-group lookup — feeds the §7 scorer's primary/secondary-type component.
/// Separate call rather than an `inc=release-groups` on every release in the browse
/// above: the group is identical across every release in the group, so fetching it
/// once avoids paying for N duplicate copies of the same object in one response.
pub fn lookup_release_group(client: &MbClient, release_group_mbid: &str) -> Result<ReleaseGroup> {
    return client.get(&format!("/release-group/{release_group_mbid}"), &[]);
}

/// Single release with full tracklists (`inc=recordings`) — backs `mb tracks`.
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
