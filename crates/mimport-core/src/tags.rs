//! Manual tag overrides + MB-alias romanization, applied to a `NormalizedRelease`
//! before matching/writing. Everything MB can set on a release/track (barring
//! mbids and cover art fetch, which have their own dedicated paths) is
//! overridable here: artist, album, date, label, genre, and per-track title.
//!
//! Resolution order per field: manual override (unconditional) > MB
//! romanization alias (only if the field is non-Latin) > left as-is. A
//! non-Latin field with neither wins ends up in the returned `Unresolved`
//! list, which is fatal unless the caller passes `--allow-native`.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{Error, Result};
use crate::mb::MbClient;
use crate::mb::queries as mb_q;
use crate::release::NormalizedRelease;
use crate::romanize::{is_non_latin, pick_alias};

#[derive(Debug, Default, serde::Deserialize)]
pub struct TagOverrides {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub date: Option<String>,
    pub label: Option<String>,
    pub genre: Option<String>,
    pub cover: Option<std::path::PathBuf>,
    /// Track-position -> manual title, single-disc only. Multi-disc releases
    /// with ambiguous positions across discs aren't addressable this way yet.
    #[serde(default)]
    pub tracks: BTreeMap<u32, String>,
}

/// One-shot CLI flag values layered on top of a loaded `TagOverrides`.
pub struct FlagOverrides<'a> {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub date: Option<String>,
    pub label: Option<String>,
    pub genre: Option<String>,
    pub cover: Option<std::path::PathBuf>,
    pub track_titles: &'a [String],
}

impl TagOverrides {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| return Error::io(path, e))?;
        return serde_json::from_str(&text)
            .map_err(|e| return Error::TagOverrides(format!("{}: {e}", path.display())));
    }

    /// CLI flags win over whatever the `--tags` file set for the same field.
    pub fn apply_flag_overrides(&mut self, flags: FlagOverrides<'_>) -> Result<()> {
        if flags.artist.is_some() {
            self.artist = flags.artist;
        }
        if flags.album.is_some() {
            self.album = flags.album;
        }
        if flags.date.is_some() {
            self.date = flags.date;
        }
        if flags.label.is_some() {
            self.label = flags.label;
        }
        if flags.genre.is_some() {
            self.genre = flags.genre;
        }
        if flags.cover.is_some() {
            self.cover = flags.cover;
        }
        for raw in flags.track_titles {
            let (pos_str, title) = raw.split_once('=').ok_or_else(|| {
                return Error::TagOverrides(format!(
                    "--track-title {raw:?} must be \"<position>=<title>\""
                ));
            })?;
            let pos: u32 = pos_str.trim().parse().map_err(|_| {
                return Error::TagOverrides(format!(
                    "--track-title {raw:?} has a non-numeric position"
                ));
            })?;
            self.tracks.insert(pos, title.to_string());
        }
        return Ok(());
    }
}

#[derive(Debug)]
pub struct Unresolved {
    pub field: String,
    pub native: String,
}

pub fn format_unresolved(unresolved: &[Unresolved]) -> String {
    return unresolved
        .iter()
        .map(|u| return format!("  {}: {}", u.field, u.native))
        .collect::<Vec<_>>()
        .join("\n");
}

/// Applies `overrides` and MB romanization aliases to `release` in place.
/// Returns every non-Latin field that neither an override nor an alias
/// resolved.
pub fn resolve(
    client: &MbClient,
    release: &mut NormalizedRelease,
    overrides: &TagOverrides,
) -> Vec<Unresolved> {
    let mut unresolved = Vec::new();

    resolve_album(client, release, overrides, &mut unresolved);
    resolve_artist(client, release, overrides, &mut unresolved);
    release.date = overrides
        .date
        .clone()
        .or_else(|| return release.date.clone());
    release.label = overrides
        .label
        .clone()
        .or_else(|| return release.label.clone());
    release.genre = overrides.genre.clone();
    resolve_tracks(client, release, overrides, &mut unresolved);

    return unresolved;
}

fn resolve_album(
    client: &MbClient,
    release: &mut NormalizedRelease,
    overrides: &TagOverrides,
    unresolved: &mut Vec<Unresolved>,
) {
    if let Some(album) = &overrides.album {
        if *album != release.title {
            release.title_native = Some(release.title.clone());
            release.title = album.clone();
        }
        return;
    }
    if !is_non_latin(&release.title) {
        return;
    }
    let native = release.title.clone();
    let resolved = release
        .release_group_id
        .as_deref()
        .and_then(|rg| return mb_q::release_group_aliases(client, rg).ok())
        .as_deref()
        .and_then(pick_alias);
    match resolved {
        Some(r) => {
            release.title_native = Some(native);
            release.title = r;
        }
        None => unresolved.push(Unresolved {
            field: "album".to_string(),
            native,
        }),
    }
}

fn resolve_artist(
    client: &MbClient,
    release: &mut NormalizedRelease,
    overrides: &TagOverrides,
    unresolved: &mut Vec<Unresolved>,
) {
    if release.artist_credit_parts.is_empty() {
        // Lidarr path (no artist field at all) or synthetic yt release: still
        // honor a manual override, but there's nothing to romanize.
        if let Some(artist) = &overrides.artist {
            release.artist_credit = Some(artist.clone());
        }
        return;
    }

    let native_joined: String = release
        .artist_credit_parts
        .iter()
        .map(|p| return format!("{}{}", p.name, p.join_phrase))
        .collect();

    if let Some(artist) = &overrides.artist {
        if *artist != native_joined {
            release.artist_credit_native = Some(native_joined);
            release.artist_credit = Some(artist.clone());
        }
        return;
    }

    if !is_non_latin(&native_joined) {
        return;
    }

    let mut rejoined = String::new();
    let mut all_resolved = true;
    for part in &release.artist_credit_parts {
        if is_non_latin(&part.name) {
            match mb_q::artist_aliases(client, &part.id)
                .ok()
                .as_deref()
                .and_then(pick_alias)
            {
                Some(r) => rejoined.push_str(&r),
                None => {
                    all_resolved = false;
                    rejoined.push_str(&part.name);
                }
            }
        } else {
            rejoined.push_str(&part.name);
        }
        rejoined.push_str(&part.join_phrase);
    }
    if all_resolved {
        release.artist_credit_native = Some(native_joined);
        release.artist_credit = Some(rejoined);
    } else {
        unresolved.push(Unresolved {
            field: "artist".to_string(),
            native: native_joined,
        });
    }
}

fn resolve_tracks(
    client: &MbClient,
    release: &mut NormalizedRelease,
    overrides: &TagOverrides,
    unresolved: &mut Vec<Unresolved>,
) {
    for t in &mut release.tracks {
        let manual = t.position.and_then(|p| return overrides.tracks.get(&p));
        if let Some(title) = manual {
            if *title != t.title {
                t.title_native = Some(t.title.clone());
                t.title = title.clone();
            }
            continue;
        }
        if !is_non_latin(&t.title) {
            continue;
        }
        let native = t.title.clone();
        let resolved = t
            .recording_id
            .as_deref()
            .and_then(|rid| return mb_q::recording_aliases(client, rid).ok())
            .as_deref()
            .and_then(pick_alias);
        match resolved {
            Some(r) => {
                t.title_native = Some(native);
                t.title = r;
            }
            None => {
                let field = match t.position {
                    Some(p) => format!("track {p}"),
                    None => "track (no position)".to_string(),
                };
                unresolved.push(Unresolved { field, native });
            }
        }
    }
}
