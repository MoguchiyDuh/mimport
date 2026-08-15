//! §7 edition-scoring algorithm. Ported from a prior build's `scorer.rs` (weights
//! hand-tuned against a real 405-album library) onto mimport's backend-agnostic
//! `NormalizedRelease`/`NormalizedReleaseGroup`, with everything the two backends don't
//! expose identically dropped rather than faked — see `config::Scoring`'s doc comment
//! for what was cut and why. Nothing here returns a bare number: a scorer that decides
//! which edition gets imported has to be able to show its working, so every component
//! is retained in a [`ScoreBreakdown`].

use serde::Serialize;

use crate::config::Scoring;
use crate::release::{NormalizedRelease, NormalizedReleaseGroup};

/// One contribution to a score. Positive and negative share one list so totals are
/// verifiable by eye.
#[derive(Debug, Clone, Serialize)]
pub struct Component {
    pub label: String,
    pub points: f64,
}

impl Component {
    fn new(label: impl Into<String>, points: f64) -> Self {
        return Component {
            label: label.into(),
            points,
        };
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ScoreBreakdown {
    pub release_id: String,
    pub title: String,
    pub total: f64,
    pub components: Vec<Component>,
    // Display-only fields, not consumed by scoring — carried through so the ranked
    // output is scannable without a second lookup per DESIGN.md §5's `lidarr
    // album`/`mb album` requirement (country, label, media/format, track count, status).
    pub status: Option<String>,
    pub country: Option<String>,
    pub label: Option<String>,
    pub formats: Vec<String>,
    pub track_count: u32,
    pub date: Option<String>,
}

impl ScoreBreakdown {
    fn push(&mut self, label: impl Into<String>, points: f64) {
        if points != 0.0 {
            self.total += points;
            self.components.push(Component::new(label, points));
        }
    }
}

/// The number of tracks the album "really" has: the mode across the group's releases,
/// since bonus-track pressings and anniversary editions are outliers.
pub fn canonical_track_count(releases: &[NormalizedRelease]) -> Option<u32> {
    let mut counts: Vec<u32> = releases
        .iter()
        .map(|r| return r.track_count)
        .filter(|c| return *c > 0)
        .collect();
    if counts.is_empty() {
        return None;
    }
    counts.sort_unstable();

    let mut best = (counts[0], 0usize);
    let mut current = (counts[0], 0usize);
    for c in &counts {
        if *c == current.0 {
            current.1 += 1;
        } else {
            current = (*c, 1);
        }
        // `>` not `>=`, so a tie keeps the smaller count: between an 11-track and a
        // 13-track edition, the shorter one is the album and the longer carries bonus
        // tracks.
        if current.1 > best.1 {
            best = current;
        }
    }
    return Some(best.0);
}

pub struct ScoreContext<'a> {
    pub cfg: &'a Scoring,
    pub group: Option<&'a NormalizedReleaseGroup>,
    /// From [`canonical_track_count`].
    pub canonical_tracks: Option<u32>,
}

pub fn score_release(release: &NormalizedRelease, ctx: &ScoreContext<'_>) -> ScoreBreakdown {
    let cfg = ctx.cfg;

    let mut b = ScoreBreakdown {
        release_id: release.id.clone(),
        title: release.title.clone(),
        total: 0.0,
        components: Vec::new(),
        status: release.status.clone(),
        country: release.country.clone(),
        label: release.label.clone(),
        formats: release.formats.clone(),
        track_count: release.track_count,
        date: release.date.clone(),
    };

    score_status(release, cfg, &mut b);
    score_media(&release.formats, cfg, &mut b);
    score_tracks(release, ctx, &mut b);
    score_metadata(release, cfg, &mut b);
    let tracklist_penalty = score_terms(release, cfg, &mut b);
    score_edition(release, cfg, tracklist_penalty, &mut b);
    score_group(ctx, cfg, &mut b);

    return b;
}

fn score_status(release: &NormalizedRelease, cfg: &Scoring, b: &mut ScoreBreakdown) {
    // Bootleg/Pseudo-Release/Withdrawn are penalised, never hard-rejected: an album
    // whose only releases are bootlegs must stay selectable; -120 still puts any
    // Official release 220 points clear.
    match release.status.as_deref().map(str::to_lowercase).as_deref() {
        Some("official") => b.push("official", cfg.status.official),
        Some(other) => b.push(format!("status={other}"), cfg.status.other),
        None => {}
    }
}

fn score_media(formats: &[String], cfg: &Scoring, b: &mut ScoreBreakdown) {
    // No stated format is not evidence of physical media, so it costs nothing.
    if formats.is_empty() {
        return;
    }
    let lowered: Vec<String> = formats.iter().map(|f| return f.to_lowercase()).collect();
    if lowered.iter().any(|f| return f.contains("digital")) {
        b.push("digital media", cfg.media.digital);
        return;
    }

    // Substring matching is load-bearing: MusicBrainz says `12" Vinyl`, never `Vinyl`.
    let worst = lowered
        .iter()
        .map(|f| {
            let points = if f.contains("vinyl") {
                cfg.media.vinyl
            } else if f.contains("cassette")
                || f.contains("shellac")
                || f.contains("8-track")
                || f.contains("cylinder")
            {
                cfg.media.other_physical
            } else if f.contains("cd") {
                cfg.media.cd
            } else {
                cfg.media.other_non_digital
            };
            return (f.clone(), points);
        })
        .min_by(|a, x| return a.1.total_cmp(&x.1));

    if let Some((format, points)) = worst {
        b.push(format!("media={format}"), points);
    }
}

fn score_tracks(release: &NormalizedRelease, ctx: &ScoreContext<'_>, b: &mut ScoreBreakdown) {
    let (Some(canonical), count) = (ctx.canonical_tracks, release.track_count) else {
        return;
    };
    if count == 0 {
        return;
    }
    if count == canonical {
        b.push(
            format!("tracks={count} (canonical)"),
            ctx.cfg.bonus.canonical_track_count,
        );
    } else {
        b.components.push(Component::new(
            format!("tracks={count} vs canonical {canonical}"),
            0.0,
        ));
    }
}

fn score_metadata(release: &NormalizedRelease, cfg: &Scoring, b: &mut ScoreBreakdown) {
    if release.label.is_some() {
        b.push("label", cfg.bonus.label_present);
    }
}

/// Returns the tracklist penalty, which the deluxe gate needs.
fn score_terms(release: &NormalizedRelease, cfg: &Scoring, b: &mut ScoreBreakdown) -> f64 {
    let title_penalty = term_penalty(&release.title, cfg);
    if title_penalty > 0.0 {
        b.push(format!("title terms ({title_penalty})"), -title_penalty);
    }

    let mut tracklist = 0.0;
    for track in &release.tracks {
        tracklist += term_penalty(&track.title, cfg);
    }
    // Capped, or a long live album accumulates an unbounded penalty and distorts every
    // comparison.
    let tracklist = tracklist.min(cfg.terms.cap);
    if tracklist > 0.0 {
        b.push(format!("tracklist terms ({tracklist})"), -tracklist);
    }
    return tracklist;
}

/// Bare substring matching: deliberately not word-bounded (so `live` matches `(Live at
/// Leeds)`) and deliberately not deduplicated (a title holding both `live` and `edit`
/// is charged for both).
fn term_penalty(text: &str, cfg: &Scoring) -> f64 {
    let hay = text.to_lowercase();
    let mut total = 0.0;
    for term in &cfg.terms.strong {
        if hay.contains(term.as_str()) {
            total += cfg.terms.strong_cost;
        }
    }
    for term in &cfg.terms.medium {
        if hay.contains(term.as_str()) {
            total += cfg.terms.medium_cost;
        }
    }
    for term in &cfg.terms.weak {
        if hay.contains(term.as_str()) {
            total += cfg.terms.weak_cost;
        }
    }
    return total;
}

fn score_edition(
    release: &NormalizedRelease,
    cfg: &Scoring,
    tracklist_penalty: f64,
    b: &mut ScoreBreakdown,
) {
    let title = release.title.to_lowercase();
    if !cfg
        .edition
        .terms
        .iter()
        .any(|t| return title.contains(t.as_str()))
    {
        return;
    }
    // A deluxe edition is preferable, but not when its surplus material is live or
    // remix filler.
    if tracklist_penalty <= cfg.edition.deluxe_gate {
        b.push("expanded edition", cfg.edition.deluxe_bonus);
    } else {
        b.components.push(Component::new(
            "expanded edition rejected: non-studio tracklist",
            0.0,
        ));
    }
}

fn score_group(ctx: &ScoreContext<'_>, cfg: &Scoring, b: &mut ScoreBreakdown) {
    let Some(group) = ctx.group else {
        return;
    };

    // Without grading, a Single could out-score the Album it was taken from.
    match group
        .primary_type
        .as_deref()
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("album") => b.push("type=Album", cfg.release_group.primary_album),
        Some("ep") => b.push("type=EP", cfg.release_group.primary_ep),
        Some("single") => b.push("type=Single", cfg.release_group.primary_single),
        _ => {}
    }

    for secondary in &group.secondary_types {
        let lowered = secondary.to_lowercase();
        if cfg
            .release_group
            .penalised_secondary_types
            .iter()
            .any(|t| return *t == lowered)
        {
            b.push(
                format!("secondary={secondary}"),
                cfg.release_group.secondary_penalty,
            );
        }
    }
}

/// Highest score first. Ties break to the earliest date; undated releases sort last
/// rather than first, since an ascending string sort would put `""` at the front. The
/// final fallback is the MBID, so otherwise-identical releases always resolve the same
/// way and a retag is reproducible.
pub fn rank(mut scored: Vec<ScoreBreakdown>) -> Vec<ScoreBreakdown> {
    scored.sort_by(|a, b| {
        return b
            .total
            .total_cmp(&a.total)
            .then_with(|| return date_key(&a.date).cmp(&date_key(&b.date)))
            .then_with(|| return a.release_id.cmp(&b.release_id));
    });
    return scored;
}

fn date_key(date: &Option<String>) -> (u8, String) {
    return match date {
        Some(d) if !d.trim().is_empty() => (0, d.clone()),
        _ => (1, String::new()),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::NormalizedTrack;

    fn release(status: &str, format: &str, tracks: u32) -> NormalizedRelease {
        return NormalizedRelease {
            id: "a".to_string(),
            title: "Mezzanine".to_string(),
            status: Some(status.to_string()),
            country: Some("XW".to_string()),
            disambiguation: None,
            label: Some("Virgin".to_string()),
            formats: vec![format.to_string()],
            track_count: tracks,
            date: Some("1998-04-20".to_string()),
            tracks: Vec::new(),
        };
    }

    fn score(r: &NormalizedRelease, canonical: Option<u32>) -> f64 {
        let cfg = Scoring::default();
        let ctx = ScoreContext {
            cfg: &cfg,
            group: None,
            canonical_tracks: canonical,
        };
        return score_release(r, &ctx).total;
    }

    #[test]
    fn digital_beats_cd_beats_vinyl() {
        let digital = score(&release("Official", "Digital Media", 11), None);
        let cd = score(&release("Official", "CD", 11), None);
        let vinyl = score(&release("Official", r#"12" Vinyl"#, 11), None);
        assert!(digital > cd, "{digital} !> {cd}");
        assert!(cd > vinyl, "{cd} !> {vinyl}");
    }

    #[test]
    fn official_beats_bootleg_but_bootleg_is_still_selectable() {
        let official = score(&release("Official", "CD", 11), None);
        let bootleg = score(&release("Bootleg", "Digital Media", 11), None);
        assert!(official > bootleg);
        assert!(bootleg.is_finite());
    }

    #[test]
    fn canonical_count_beats_the_padded_edition() {
        let album = release("Official", "Digital Media", 11);
        let padded = release("Official", "Digital Media", 19);
        let canonical = Some(11);
        assert!(score(&album, canonical) > score(&padded, canonical));
    }

    #[test]
    fn canonical_count_is_the_mode_and_ties_go_shorter() {
        let mode = canonical_track_count(&[
            release("Official", "CD", 11),
            release("Official", "CD", 11),
            release("Official", "CD", 19),
        ]);
        assert_eq!(mode, Some(11));

        let tie = canonical_track_count(&[release("Official", "CD", 11), release("Official", "CD", 13)]);
        assert_eq!(tie, Some(11), "a tie must keep the shorter tracklist");
    }

    #[test]
    fn missing_format_is_not_penalised_as_physical() {
        let mut unknown = release("Official", "CD", 11);
        unknown.formats = Vec::new();
        let cd = score(&release("Official", "CD", 11), None);
        assert!(
            score(&unknown, None) > cd,
            "absence is not evidence of vinyl"
        );
    }

    #[test]
    fn undated_releases_sort_last_not_first() {
        let mut early = release("Official", "CD", 11);
        early.id = "a".to_string();
        early.date = Some("1998-04-20".into());
        let mut undated = release("Official", "CD", 11);
        undated.id = "b".to_string();
        undated.date = Some(String::new());

        let cfg = Scoring::default();
        let ctx = ScoreContext {
            cfg: &cfg,
            group: None,
            canonical_tracks: None,
        };
        let ranked = rank(vec![
            score_release(&undated, &ctx),
            score_release(&early, &ctx),
        ]);
        assert_eq!(ranked[0].release_id, "a");
    }

    #[test]
    fn ties_resolve_deterministically_by_mbid() {
        let cfg = Scoring::default();
        let ctx = ScoreContext {
            cfg: &cfg,
            group: None,
            canonical_tracks: None,
        };
        let mut a = release("Official", "CD", 11);
        a.id = "aaa".to_string();
        let mut b = release("Official", "CD", 11);
        b.id = "bbb".to_string();
        let sa = score_release(&a, &ctx);
        let sb = score_release(&b, &ctx);
        assert_eq!(rank(vec![sa.clone(), sb.clone()])[0].release_id, "aaa");
        assert_eq!(rank(vec![sb, sa])[0].release_id, "aaa");
    }

    #[test]
    fn deluxe_bonus_is_gated_on_a_studio_tracklist() {
        let cfg = Scoring::default();
        let mut deluxe = release("Official", "Digital Media", 11);
        deluxe.title = "Mezzanine (Deluxe Edition)".into();

        let ctx = ScoreContext {
            cfg: &cfg,
            group: None,
            canonical_tracks: None,
        };
        let clean = score_release(&deluxe, &ctx);
        assert!(clean.components.iter().any(|c| return c.label == "expanded edition"));

        // Same edition, but the surplus is live material.
        deluxe.tracks = (1..=6)
            .map(|i| {
                return NormalizedTrack {
                    position: Some(i),
                    title: format!("Angel (Live {i})"),
                    length_ms: None,
                    recording_id: None,
                };
            })
            .collect();
        let filler = score_release(&deluxe, &ctx);
        assert!(
            filler
                .components
                .iter()
                .any(|c| return c.label.starts_with("expanded edition rejected"))
        );
        assert!(filler.total < clean.total);
    }

    #[test]
    fn secondary_types_are_charged_per_match() {
        let cfg = Scoring::default();
        let group = NormalizedReleaseGroup {
            primary_type: Some("Album".into()),
            secondary_types: vec!["Live".into(), "Remix".into()],
        };
        let ctx = ScoreContext {
            cfg: &cfg,
            group: Some(&group),
            canonical_tracks: None,
        };
        let scored = score_release(&release("Official", "Digital Media", 11), &ctx);
        let charged = scored
            .components
            .iter()
            .filter(|c| return c.label.starts_with("secondary="))
            .count();
        assert_eq!(charged, 2);
    }

    #[test]
    fn label_presence_is_a_small_bonus() {
        let cfg = Scoring::default();
        let ctx = ScoreContext {
            cfg: &cfg,
            group: None,
            canonical_tracks: None,
        };
        let mut no_label = release("Official", "CD", 11);
        no_label.label = None;
        let with_label = release("Official", "CD", 11);
        assert!(score_release(&with_label, &ctx).total > score_release(&no_label, &ctx).total);
    }
}
