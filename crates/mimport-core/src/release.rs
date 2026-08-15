//! Backend-agnostic release/track model. Both `mb::types::Release` and
//! `lidarr::types::ReleaseResource` map into this — the (not-yet-written, §7) scorer
//! consumes only this type, never either backend's raw wire format directly. Required
//! because `mb album` and `lidarr album` both need to run the same scorer despite
//! fetching from differently-shaped APIs.

use serde::Serialize;

use crate::lidarr::types::ReleaseResource as LidarrRelease;
use crate::mb::types::Release as MbRelease;

#[derive(Debug, Clone, Serialize)]
pub struct NormalizedRelease {
    pub id: String,
    pub title: String,
    pub status: Option<String>,
    /// Display-only. **Not** used by the §7 scorer — the two backends represent country
    /// differently (MB: ISO codes; the Lidarr proxy: area display names, e.g.
    /// `"[Worldwide]"`) and neither exposes it identically enough to score on without
    /// per-backend normalization the project has decided isn't worth the complexity.
    pub country: Option<String>,
    pub disambiguation: Option<String>,
    pub label: Option<String>,
    /// Every medium's format (e.g. `["Digital Media"]`, or `["CD", "DVD"]` for a mixed
    /// box set) — the §7 scorer takes the worst across all of them, not just the first.
    pub formats: Vec<String>,
    pub track_count: u32,
    /// Issue date, used for the §7 ranking tie-break (earliest wins, undated sorts
    /// last) — not itself a scored signal.
    pub date: Option<String>,
    /// Populated at album-summary level too (both backends return full nested
    /// tracklists for a release-group/album fetch) — the §7 scorer needs real track
    /// titles to compute term penalties even though the CLI's summary output doesn't
    /// display them.
    pub tracks: Vec<NormalizedTrack>,
}

/// Release-group-level fields the §7 scorer needs alongside each release — fetched
/// once per `lidarr album`/`mb album` call, not per release.
#[derive(Debug, Clone, Serialize)]
pub struct NormalizedReleaseGroup {
    pub primary_type: Option<String>,
    pub secondary_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NormalizedTrack {
    pub position: Option<u32>,
    pub title: String,
    pub length_ms: Option<u64>,
    pub recording_id: Option<String>,
}

impl From<MbRelease> for NormalizedRelease {
    fn from(r: MbRelease) -> Self {
        let track_count = r.media.iter().map(|m| return m.track_count).sum();
        let formats = r
            .media
            .iter()
            .filter_map(|m| return m.format.clone())
            .collect();
        let label = r
            .label_info
            .first()
            .and_then(|li| return li.label.as_ref())
            .map(|l| return l.name.clone());
        let tracks = r
            .media
            .into_iter()
            .flat_map(|m| return m.tracks)
            .map(|t| {
                return NormalizedTrack {
                    position: t.number.parse().ok(),
                    title: t.title,
                    length_ms: t.length.or(t.recording.length),
                    recording_id: Some(t.recording.id),
                };
            })
            .collect();

        return NormalizedRelease {
            id: r.id,
            title: r.title,
            status: r.status,
            country: r.country,
            disambiguation: r.disambiguation,
            label,
            formats,
            track_count,
            date: r.date,
            tracks,
        };
    }
}

impl From<LidarrRelease> for NormalizedRelease {
    fn from(r: LidarrRelease) -> Self {
        let formats = r.media.iter().filter_map(|m| return m.format.clone()).collect();
        let country = r.country.first().cloned();
        let label = r.label.first().cloned();
        let date = r.release_date.clone();
        let tracks = r
            .tracks
            .into_iter()
            .map(|t| {
                return NormalizedTrack {
                    position: t.track_number.and_then(|n| return n.parse().ok()),
                    title: t.track_name,
                    length_ms: t.duration_ms,
                    recording_id: t.recording_id,
                };
            })
            .collect();

        return NormalizedRelease {
            id: r.id,
            title: r.title,
            status: r.status,
            country,
            disambiguation: r.disambiguation,
            label,
            formats,
            track_count: r.track_count,
            date,
            tracks,
        };
    }
}

impl From<&crate::lidarr::types::AlbumResource> for NormalizedReleaseGroup {
    fn from(a: &crate::lidarr::types::AlbumResource) -> Self {
        return NormalizedReleaseGroup {
            primary_type: a.album_type.clone(),
            secondary_types: a.secondary_types.clone(),
        };
    }
}

impl From<&crate::mb::types::ReleaseGroup> for NormalizedReleaseGroup {
    fn from(g: &crate::mb::types::ReleaseGroup) -> Self {
        return NormalizedReleaseGroup {
            primary_type: g.primary_type.clone(),
            secondary_types: g.secondary_types.clone(),
        };
    }
}
