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
    pub formats: Vec<String>,
    pub track_count: u32,
    pub date: Option<String>,
    pub tracks: Vec<NormalizedTrack>,
}

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
