use serde::Serialize;

use crate::config::Scoring;
use crate::release::{NormalizedRelease, NormalizedReleaseGroup};

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
    pub disambiguation: Option<String>,
    pub total: f64,
    pub components: Vec<Component>,
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
        if current.1 > best.1 {
            best = current;
        }
    }
    return Some(best.0);
}

pub struct ScoreContext<'a> {
    pub cfg: &'a Scoring,
    pub group: Option<&'a NormalizedReleaseGroup>,
    pub canonical_tracks: Option<u32>,
}

pub fn score_release(release: &NormalizedRelease, ctx: &ScoreContext<'_>) -> ScoreBreakdown {
    let cfg = ctx.cfg;

    let mut b = ScoreBreakdown {
        release_id: release.id.clone(),
        title: release.title.clone(),
        disambiguation: release.disambiguation.clone(),
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
    match release.status.as_deref().map(str::to_lowercase).as_deref() {
        Some("official") => b.push("official", cfg.status.official),
        Some(other) => b.push(format!("status={other}"), cfg.status.other),
        None => {}
    }
}

fn score_media(formats: &[String], cfg: &Scoring, b: &mut ScoreBreakdown) {
    if formats.is_empty() {
        return;
    }
    let lowered: Vec<String> = formats.iter().map(|f| return f.to_lowercase()).collect();
    if lowered.iter().any(|f| return f.contains("digital")) {
        b.push("digital media", cfg.media.digital);
        return;
    }

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

fn score_terms(release: &NormalizedRelease, cfg: &Scoring, b: &mut ScoreBreakdown) -> f64 {
    let title_penalty = term_penalty(&release.title, cfg);
    if title_penalty > 0.0 {
        b.push(format!("title terms ({title_penalty})"), -title_penalty);
    }

    let mut tracklist = 0.0;
    for track in &release.tracks {
        tracklist += term_penalty(&track.title, cfg);
    }
    let tracklist = tracklist.min(cfg.terms.cap);
    if tracklist > 0.0 {
        b.push(format!("tracklist terms ({tracklist})"), -tracklist);
    }
    return tracklist;
}

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
    let disambiguation = release
        .disambiguation
        .as_deref()
        .unwrap_or_default()
        .to_lowercase();
    if !cfg
        .edition
        .terms
        .iter()
        .any(|t| return title.contains(t.as_str()) || disambiguation.contains(t.as_str()))
    {
        return;
    }
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
