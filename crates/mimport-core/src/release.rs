use serde::Serialize;

use crate::lidarr::types::ReleaseResource as LidarrRelease;
use crate::mb::types::{join_artist_credit, Release as MbRelease};

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
    /// Release-level artist credit; `None` from the Lidarr proxy (no artist field).
    pub artist_credit: Option<String>,
    /// Per-member artist credit, only populated from the MB path; used to look up
    /// per-artist romanization aliases. Empty from the Lidarr proxy.
    pub artist_credit_parts: Vec<ArtistCreditPart>,
    /// Release-group mbid, only populated from the MB path; used to look up a
    /// romanized album title alias.
    pub release_group_id: Option<String>,
    /// Original (pre-romanization) title, set only when `title` was swapped.
    pub title_native: Option<String>,
    /// Original (pre-romanization) artist credit, set only when `artist_credit` was swapped.
    pub artist_credit_native: Option<String>,
    /// Manual-only field; MB isn't queried for genre, so this is `None` unless
    /// a `TagOverrides.genre` was applied.
    pub genre: Option<String>,
    pub tracks: Vec<NormalizedTrack>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtistCreditPart {
    pub id: String,
    pub name: String,
    pub join_phrase: String,
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
    pub medium_format: Option<String>,
    /// 1-based disc/medium number. `None` is treated as disc 1 by consumers.
    pub medium_position: Option<u32>,
    pub raw_position: Option<String>,
    /// Original (pre-romanization) title, set only when `title` was swapped.
    pub title_native: Option<String>,
}

impl NormalizedRelease {
    /// First `-`-delimited component of `date` (the year), or `None`.
    pub fn year(&self) -> Option<&str> {
        return self.date.as_deref().and_then(|d| return d.split('-').next());
    }
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
        let artist_credit = join_artist_credit(&r.artist_credit);
        let artist_credit_parts = r
            .artist_credit
            .iter()
            .map(|c| {
                return ArtistCreditPart {
                    id: c.artist.id.clone(),
                    name: c.name.clone(),
                    join_phrase: c.join_phrase.clone(),
                };
            })
            .collect();
        let release_group_id = r.release_group.as_ref().map(|g| return g.id.clone());
        let tracks = r
            .media
            .into_iter()
            .flat_map(|m| {
                let format = m.format.clone();
                let position = m.position;
                return m
                    .tracks
                    .into_iter()
                    .map(move |t| return (format.clone(), position, t))
                    .collect::<Vec<_>>();
            })
            .map(|(medium_format, medium_position, t)| {
                return NormalizedTrack {
                    position: t.number.parse().ok(),
                    title: t.title,
                    length_ms: t.length.or(t.recording.length),
                    recording_id: Some(t.recording.id),
                    medium_format,
                    medium_position,
                    raw_position: Some(t.number),
                    title_native: None,
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
            artist_credit,
            artist_credit_parts,
            release_group_id,
            title_native: None,
            artist_credit_native: None,
            genre: None,
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
        let media = r.media.clone();
        let tracks = r
            .tracks
            .into_iter()
            .map(|t| {
                let medium_format = t
                    .medium_number
                    .and_then(|n| return media.iter().find(|m| return m.position == Some(n)))
                    .and_then(|m| return m.format.clone());
                return NormalizedTrack {
                    position: t.track_number.and_then(|n| return n.parse().ok()),
                    title: t.track_name,
                    length_ms: t.duration_ms,
                    recording_id: t.recording_id,
                    medium_format,
                    medium_position: t.medium_number,
                    raw_position: None,
                    title_native: None,
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
            artist_credit: None,
            artist_credit_parts: Vec::new(),
            release_group_id: None,
            title_native: None,
            artist_credit_native: None,
            genre: None,
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
