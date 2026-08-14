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
    pub country: Option<String>,
    pub disambiguation: Option<String>,
    pub label: Option<String>,
    /// Primary media format of the first medium (e.g. "Digital Media", "CD", "Vinyl").
    pub format: Option<String>,
    pub track_count: u32,
    /// Empty for summary-level fetches (`lidarr album`/`mb album` don't request full
    /// tracklists); populated for `lidarr tracks`/`mb tracks`.
    pub tracks: Vec<NormalizedTrack>,
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
        let format = r.media.first().and_then(|m| return m.format.clone());
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
            format,
            track_count,
            tracks,
        };
    }
}

impl From<LidarrRelease> for NormalizedRelease {
    fn from(r: LidarrRelease) -> Self {
        let format = r.media.first().and_then(|m| return m.format.clone());
        let country = r.country.first().cloned();
        let label = r.label.first().cloned();
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
            format,
            track_count: r.track_count,
            tracks,
        };
    }
}
