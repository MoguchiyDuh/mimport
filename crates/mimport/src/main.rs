mod cli;
mod output;

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use clap::Parser;
use mimport_core::Config;
use mimport_core::config::YtConfig;
use mimport_core::coverart::CoverArtClient;
use mimport_core::error::Error;
use mimport_core::import;
use mimport_core::jobs;
use mimport_core::library;
use mimport_core::lidarr::{LidarrClient, queries as lidarr_q};
use mimport_core::mb::{MbClient, queries as mb_q};
use mimport_core::postfix;
use mimport_core::release::{NormalizedRelease, NormalizedReleaseGroup, NormalizedTrack};
use mimport_core::scorer::{self, ScoreContext};
use mimport_core::slskd::{SlskdClient, queries as slskd_q};
use mimport_core::tags;
use mimport_core::yt;

use cli::{Cli, Command, LibraryCmd, LidarrCmd, MbCmd, SlskdCmd, YtCmd};

fn main() {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();

    if let Err(e) = run(&cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn is_mbid_query(q: &str) -> Option<&str> {
    return q.strip_prefix("mbid:");
}

fn run(cli: &Cli) -> mimport_core::Result<()> {
    let cfg = Config::load(&cli.config)?;

    match &cli.command {
        Command::Lidarr(cmd) => run_lidarr(cli, &cfg, cmd),
        Command::Mb(cmd) => run_mb(cli, &cfg, cmd),
        Command::Slskd(cmd) => run_slskd(cli, &cfg, cmd),
        Command::Import {
            target,
            release,
            force,
            tags,
            artist,
            album,
            date,
            label,
            genre,
            cover,
            cover_art,
            track_title,
            allow_native,
            move_files,
            allow_partial,
            cleanup,
            dry_run,
        } => run_import(
            cli,
            &cfg,
            target,
            release,
            ImportFlags {
                force: force.as_deref(),
                tags: tags.as_deref(),
                artist: artist.clone(),
                album: album.clone(),
                date: date.clone(),
                label: label.clone(),
                genre: genre.clone(),
                cover: cover.clone(),
                cover_art: *cover_art,
                track_title,
                allow_native: *allow_native,
                move_files: *move_files,
                allow_partial: *allow_partial,
                cleanup: *cleanup,
                dry_run: *dry_run,
            },
        ),
        Command::Library(cmd) => run_library(cli, &cfg, cmd),
        Command::Cover { query, fetch } => {
            let mut query = query.clone();
            let fetch = *fetch
                || query
                    .iter()
                    .position(|q| return q == "--fetch")
                    .is_some_and(|i| {
                        query.remove(i);
                        return true;
                    });
            run_cover(cli, &cfg, &query, fetch)
        }
        Command::Yt(cmd) => run_yt(cli, &cfg, cmd),
    }
}

fn rank_releases(
    releases: &[NormalizedRelease],
    group: &NormalizedReleaseGroup,
    scoring: &mimport_core::config::Scoring,
) -> Vec<scorer::ScoreBreakdown> {
    let canonical = scorer::canonical_track_count(releases);
    let ctx = ScoreContext {
        cfg: scoring,
        group: Some(group),
        canonical_tracks: canonical,
    };
    let scored = releases
        .iter()
        .map(|r| return scorer::score_release(r, &ctx))
        .collect();
    return scorer::rank(scored);
}

fn run_lidarr(cli: &Cli, cfg: &Config, cmd: &LidarrCmd) -> mimport_core::Result<()> {
    let client = LidarrClient::new(&cfg.lidarr)?;
    match cmd {
        LidarrCmd::Artist { query } => {
            if let Some(mbid) = is_mbid_query(query) {
                let artist = lidarr_q::lookup_artist(&client, mbid)?;
                output::print(&artist, cli.json);
            } else {
                let results = lidarr_q::search_artists(&client, query)?;
                output::print(&results, cli.json);
            }
        }
        LidarrCmd::Album { mbid } => {
            let album = lidarr_q::lookup_album(&client, mbid)?;
            let group = NormalizedReleaseGroup::from(&album);
            let releases: Vec<NormalizedRelease> = album
                .releases
                .into_iter()
                .map(NormalizedRelease::from)
                .collect();
            let ranked = rank_releases(&releases, &group, &cfg.scoring);
            output::print(&ranked, cli.json);
        }
        LidarrCmd::Tracks {
            release_group_mbid,
            release_mbid,
        } => {
            let album = lidarr_q::lookup_album(&client, release_group_mbid)?;
            let release = album
                .releases
                .into_iter()
                .find(|r| return &r.id == release_mbid)
                .ok_or_else(|| {
                    return Error::LidarrNotFound {
                        entity: "release",
                        mbid: release_mbid.clone(),
                    };
                })?;
            let normalized = NormalizedRelease::from(release);
            output::print(&normalized, cli.json);
        }
    }
    return Ok(());
}

fn run_mb(cli: &Cli, cfg: &Config, cmd: &MbCmd) -> mimport_core::Result<()> {
    tracing::warn!(
        "mb is rate-limited (~1 req/s, multi-round-trip); prefer the lidarr commands unless you specifically need direct MusicBrainz data"
    );
    let client = MbClient::new(&cfg.musicbrainz)?;
    match cmd {
        MbCmd::Artist { query } => {
            if let Some(mbid) = is_mbid_query(query) {
                let artist = mb_q::lookup_artist(&client, mbid)?;
                output::print(&artist, cli.json);
            } else {
                let results = mb_q::search_artists(&client, query)?;
                output::print(&results, cli.json);
            }
        }
        MbCmd::Album { mbid } => {
            let raw_releases = mb_q::release_group_releases(&client, mbid)?;
            let group = mb_q::lookup_release_group(&client, mbid)?;
            let group = NormalizedReleaseGroup::from(&group);
            let releases: Vec<NormalizedRelease> = raw_releases
                .into_iter()
                .map(NormalizedRelease::from)
                .collect();
            let ranked = rank_releases(&releases, &group, &cfg.scoring);
            output::print(&ranked, cli.json);
        }
        MbCmd::Tracks { release_mbid } => {
            let release = mb_q::release_with_tracks(&client, release_mbid)?;
            let normalized = NormalizedRelease::from(release);
            output::print(&normalized, cli.json);
        }
        MbCmd::Track { text } => {
            let results = mb_q::search_recordings(&client, text)?;
            output::print(&results, cli.json);
        }
    }
    return Ok(());
}

/// Numeric id or a jobs.title match takes precedence; an existing filesystem
/// path not matching either is operated on ad hoc, outside job tracking.
fn resolve_job_or_path(
    cfg: &Config,
    target: &str,
) -> mimport_core::Result<(Option<jobs::Job>, std::path::PathBuf)> {
    let as_path = std::path::Path::new(target);
    let is_numeric = target.parse::<i64>().is_ok();
    if !is_numeric && as_path.exists() {
        return Ok((None, as_path.to_path_buf()));
    }
    let db = jobs::open(&cfg.paths.database)?;
    match jobs::resolve_target(&db, target) {
        Ok(job) => {
            let dir = std::path::PathBuf::from(&job.local_dir);
            return Ok((Some(job), dir));
        }
        Err(Error::JobNotFound { .. }) if as_path.exists() => {
            return Ok((None, as_path.to_path_buf()));
        }
        Err(e) => return Err(e),
    }
}

struct ImportFlags<'a> {
    force: Option<&'a std::path::Path>,
    tags: Option<&'a std::path::Path>,
    artist: Option<String>,
    album: Option<String>,
    date: Option<String>,
    label: Option<String>,
    genre: Option<String>,
    cover: Option<std::path::PathBuf>,
    cover_art: bool,
    track_title: &'a [String],
    allow_native: bool,
    move_files: bool,
    allow_partial: bool,
    cleanup: bool,
    dry_run: bool,
}

fn run_import(
    cli: &Cli,
    cfg: &Config,
    target: &str,
    release_mbid: &str,
    flags: ImportFlags,
) -> mimport_core::Result<()> {
    let (job, dir) = resolve_job_or_path(cfg, target)?;

    // Postfix is part of import: strip junk tags and downsample lossless files
    // to the [quality] target before matching. Dry-run only reports.
    let postfix_report = postfix::run(
        &dir,
        &postfix::Options {
            dry_run: flags.dry_run,
            target_rate: cfg.quality.target_samplerate,
            target_depth: cfg.quality.target_bitdepth,
        },
    )?;

    let mb_client = MbClient::new(&cfg.musicbrainz)?;
    let raw_release = mb_q::release_with_tracks(&mb_client, release_mbid)?;
    let mut release = NormalizedRelease::from(raw_release);

    let mut overrides = match flags.tags {
        Some(path) => tags::TagOverrides::load(path)?,
        None => tags::TagOverrides::default(),
    };
    overrides.apply_flag_overrides(tags::FlagOverrides {
        artist: flags.artist,
        album: flags.album,
        date: flags.date,
        label: flags.label,
        genre: flags.genre,
        cover: flags.cover.clone(),
        track_titles: flags.track_title,
    })?;

    let unresolved = tags::resolve(&mb_client, &mut release, &overrides);
    if !unresolved.is_empty() && !flags.allow_native {
        return Err(Error::UnresolvedTitles(tags::format_unresolved(
            &unresolved,
        )));
    }

    let locals = import::scan_local_tracks(&dir)?;
    let report = match flags.force {
        Some(mapping_path) => import::apply_force_mapping(&locals, &release, mapping_path)?,
        None => import::match_tracks(&locals, &release),
    };

    let blocked = match flags.force {
        Some(_) => false,
        None if flags.allow_partial => report.blocked_unmatched(),
        None => report.blocked(),
    };
    // Resolve cover art only once we know we'll write (not blocked, not dry-run);
    // the CAA network fetch is opt-in (--cover-art) so the common path stays offline.
    let cover_art = if blocked || flags.dry_run {
        None
    } else {
        match &overrides.cover {
            Some(path) => Some(mimport_core::coverart::from_local_file(path)?),
            None if flags.cover_art => {
                let cover_client =
                    CoverArtClient::new(&cfg.cover_art, &cfg.musicbrainz.user_agent)?;
                let result = if cover_client.itunes_fallback_enabled() {
                    let artist = release.artist_credit.clone().unwrap_or_default();
                    cover_client.front_cover_with_fallback(&release.id, &artist, &release.title)
                } else {
                    cover_client.front_cover(&release.id)
                };
                match result {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(
                            "cover art fetch failed for {}: {e}; importing without art",
                            release.id
                        );
                        None
                    }
                }
            }
            None => None,
        }
    };

    let mut imported: Vec<import::ImportedFile> = Vec::new();
    let mut cleanup: Option<import::CleanupReport> = None;
    let mut job = job;
    if !blocked {
        let opts = import::ImportOptions {
            dry_run: flags.dry_run,
            library_root: cfg.paths.library.clone(),
            move_files: flags.move_files,
        };
        imported = import::write_and_copy(&report.matched, &release, &opts, cover_art.as_ref())?;
        if !flags.dry_run {
            let db = jobs::open(&cfg.paths.database)?;
            // index each copy (job_id NULL for ad hoc path targets)
            for (m, f) in report.matched.iter().zip(imported.iter()) {
                library::insert_track(&db, job.as_ref().map(|j| return j.id), &release, m, f)?;
            }
            if let Some(j) = &job {
                jobs::set_job_status(&db, j.id, jobs::STATUS_IMPORTED)?;
                job = Some(jobs::get_job(&db, j.id)?);
            }
            if flags.cleanup {
                cleanup = Some(import::cleanup_sources(&imported, &dir)?);
            }
        }
    }

    output::print(
        &serde_json::json!({
            "job": job,
            "release": release.id,
            "blocked": blocked,
            "cover_art": cover_art.is_some(),
            "postfix": postfix_report,
            "matched": report.matched,
            "unmatched_files": report.unmatched_files,
            "missing_tracks": report.missing_tracks,
            "imported": imported,
            "cleanup": cleanup,
        }),
        cli.json,
    );
    return Ok(());
}

fn run_library(cli: &Cli, cfg: &Config, cmd: &LibraryCmd) -> mimport_core::Result<()> {
    let db = jobs::open(&cfg.paths.database)?;
    match cmd {
        LibraryCmd::List { query } => {
            let clauses = library::parse_query(query)?;
            let tracks = library::list_tracks(&db, &clauses)?;
            output::print(&tracks, cli.json);
        }
        LibraryCmd::Show { id } => {
            let track = library::get_track(&db, *id)?;
            output::print(&track, cli.json);
        }
        LibraryCmd::Edit {
            id,
            rename,
            dry_run,
            fields,
        } => {
            let edits = library::parse_edits(fields)?;
            let track = library::get_track(&db, *id)?;
            let new_path = if *rename {
                library::planned_rename(&track, &edits, &cfg.paths.library)
            } else {
                None
            };
            if edits.is_empty() && new_path.is_none() {
                output::print(
                    &serde_json::json!({"track": track, "edits": edits, "renamed_to": new_path}),
                    cli.json,
                );
                return Ok(());
            }
            if let Some(np) = &new_path
                && Path::new(np).exists()
            {
                return Err(Error::io(
                    np,
                    std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "renamed destination already exists",
                    ),
                ));
            }
            if !*dry_run {
                library::rewrite_tags(Path::new(&track.path), &edits)?;
                if let Some(np) = &new_path {
                    if let Some(parent) = Path::new(np).parent() {
                        std::fs::create_dir_all(parent).map_err(|e| return Error::io(parent, e))?;
                    }
                    std::fs::rename(&track.path, np).map_err(|e| return Error::io(np, e))?;
                    if let Some(old_parent) = Path::new(&track.path).parent() {
                        let _ = std::fs::remove_dir(old_parent);
                    }
                }
                let updated = library::edit_track(&db, *id, &edits, new_path.as_deref())?;
                output::print(
                    &serde_json::json!({"track": updated, "edits": edits, "renamed_to": new_path}),
                    cli.json,
                );
            } else {
                output::print(
                    &serde_json::json!({"track": track, "edits": edits, "renamed_to": new_path, "dry_run": true}),
                    cli.json,
                );
            }
        }
        LibraryCmd::Remove { files, query } => {
            let mut query = query.clone();
            let files = *files
                || query
                    .iter()
                    .position(|q| return q == "--files")
                    .is_some_and(|i| {
                        query.remove(i);
                        return true;
                    });
            let clauses = library::parse_query(&query)?;
            let tracks = library::list_tracks(&db, &clauses)?;
            let deleted_files =
                library::remove(&db, &tracks, files, Some(cfg.paths.library.as_path()))?;
            output::print(
                &serde_json::json!({"removed": tracks, "deleted_files": deleted_files}),
                cli.json,
            );
        }
    }
    return Ok(());
}

fn run_cover(cli: &Cli, cfg: &Config, query: &[String], fetch: bool) -> mimport_core::Result<()> {
    let db = jobs::open(&cfg.paths.database)?;
    let clauses = library::parse_query(query)?;
    let tracks = library::list_tracks(&db, &clauses)?;

    let mut by_release: BTreeMap<Option<String>, Vec<&library::LibraryTrack>> = BTreeMap::new();
    for t in &tracks {
        by_release
            .entry(t.release_mbid.clone())
            .or_default()
            .push(t);
    }

    let client = CoverArtClient::new(&cfg.cover_art, &cfg.musicbrainz.user_agent)?;
    let mb_client = MbClient::new(&cfg.musicbrainz)?;

    let mut results = Vec::new();
    for (mbid, ts) in &by_release {
        if !fetch {
            let covered = ts
                .iter()
                .filter(|t| return mimport_core::coverart::has_embedded_cover(Path::new(&t.path)))
                .count();
            results.push(serde_json::json!({
                "release": mbid,
                "tracks": ts.len(),
                "covered": covered,
                "missing": ts.len() - covered,
            }));
            continue;
        }

        let embed_into = |ts: &[&library::LibraryTrack], cover: &mimport_core::coverart::CoverArt| {
            let mut embedded = 0;
            let mut failed = Vec::new();
            for t in ts {
                match mimport_core::coverart::embed_cover(Path::new(&t.path), cover) {
                    Ok(()) => embedded += 1,
                    Err(e) => {
                        tracing::warn!("embed failed for {}: {e}", t.path);
                        failed.push(serde_json::json!({ "path": t.path, "error": e.to_string() }));
                    }
                }
            }
            return (embedded, failed);
        };

        let Some(mbid) = mbid else {
            // NULL release (e.g. YT imports): no CAA lookup is possible, but the
            // iTunes fallback keys off artist/album, which we have. Sub-group by
            // (artist, album) so distinct albums sharing NULL don't cross-embed.
            if !client.itunes_fallback_enabled() {
                results.push(serde_json::json!({
                    "release": null,
                    "tracks": ts.len(),
                    "embedded": 0,
                    "reason": "no release mbid",
                }));
                continue;
            }
            let mut groups: BTreeMap<(&str, &str), Vec<&library::LibraryTrack>> = BTreeMap::new();
            for t in ts {
                groups
                    .entry((t.artist.as_str(), t.album.as_str()))
                    .or_default()
                    .push(*t);
            }
            for ((artist, album), gts) in &groups {
                match client.fetch_itunes(artist, album) {
                    Ok(Some(cover)) => {
                        let (embedded, failed) = embed_into(gts, &cover);
                        results.push(serde_json::json!({
                            "release": null,
                            "album": album,
                            "tracks": gts.len(),
                            "source": "itunes",
                            "embedded": embedded,
                            "failed": failed,
                        }));
                    }
                    Ok(None) => results.push(serde_json::json!({
                        "release": null,
                        "album": album,
                        "tracks": gts.len(),
                        "embedded": 0,
                        "reason": "no cover in itunes",
                    })),
                    Err(e) => results.push(serde_json::json!({
                        "release": null,
                        "album": album,
                        "tracks": gts.len(),
                        "embedded": 0,
                        "reason": "fetch error",
                        "failed": [{"error": e.to_string()}],
                    })),
                }
            }
            continue;
        };
        let mut fetch_error: Option<String> = None;
        let mut source = "caa";
        let cover = match client.front_cover(mbid) {
            Ok(Some(c)) => Some(c),
            Ok(None) => match mb_q::release_with_tracks(&mb_client, mbid) {
                Ok(release) => match release.release_group {
                    Some(rg) => match client.front_cover_release_group(&rg.id) {
                        Ok(Some(c)) => Some(c),
                        Ok(None) => None,
                        Err(e) => {
                            fetch_error = Some(e.to_string());
                            None
                        }
                    },
                    None => None,
                },
                Err(_) => None,
            },
            Err(e) => {
                fetch_error = Some(e.to_string());
                None
            }
        };
        let cover = match cover {
            Some(c) => Some(c),
            None if fetch_error.is_none() && client.itunes_fallback_enabled() => {
                source = "itunes";
                match client.fetch_itunes(&ts[0].artist, &ts[0].album) {
                    Ok(c) => c,
                    Err(e) => {
                        fetch_error = Some(e.to_string());
                        None
                    }
                }
            }
            None => None,
        };
        if let Some(err) = fetch_error {
            results.push(serde_json::json!({
                "release": mbid,
                "tracks": ts.len(),
                "embedded": 0,
                "reason": "fetch error",
                "failed": [{"error": err}],
            }));
            continue;
        }
        let Some(cover) = cover else {
            results.push(serde_json::json!({
                "release": mbid,
                "tracks": ts.len(),
                "embedded": 0,
                "reason": "no cover in archive or itunes",
            }));
            continue;
        };
        let (embedded, failed) = embed_into(ts, &cover);
        results.push(serde_json::json!({
            "release": mbid,
            "tracks": ts.len(),
            "source": source,
            "embedded": embedded,
            "failed": failed,
        }));
    }
    output::print(&results, cli.json);
    return Ok(());
}

fn run_yt(cli: &Cli, cfg: &Config, cmd: &YtCmd) -> mimport_core::Result<()> {
    let YtCmd::Fetch {
        url,
        title,
        artist,
        album,
        track,
        disc,
        year,
        release,
        playlist,
        tags,
        allow_native,
        cookies,
        cookies_from_browser,
        dry_run,
    } = cmd;

    let mut yt_cfg = cfg.yt.clone();
    if cookies.is_some() || cookies_from_browser.is_some() {
        yt_cfg.cookies = cookies.clone();
        yt_cfg.cookies_from_browser = cookies_from_browser.clone();
    }

    if *playlist {
        return run_yt_playlist(
            cli,
            cfg,
            url,
            YtPlaylistFlags {
                release: release.as_deref(),
                tags: tags.as_deref(),
                allow_native: *allow_native,
                dry_run: *dry_run,
                yt: yt_cfg,
            },
        );
    }

    let fetched = yt::fetch(&yt_cfg, url, &cfg.paths.staging, false)?;
    let fetched = fetched.into_iter().next().ok_or(Error::YtEmptyFetch)?;

    let (norm_release, backfill) = match release {
        Some(mbid) => {
            let position = track.ok_or(Error::YtReleaseNeedsTrack)?;
            let mb_client = MbClient::new(&cfg.musicbrainz)?;
            let raw = mb_q::release_with_tracks(&mb_client, mbid)?;
            let mut norm = NormalizedRelease::from(raw);

            // --tags/--allow-native only reach here with --release (clap `requires`).
            let mut overrides = match tags {
                Some(path) => tags::TagOverrides::load(path)?,
                None => tags::TagOverrides::default(),
            };
            overrides.apply_flag_overrides(tags::FlagOverrides {
                artist: artist.clone(),
                album: album.clone(),
                date: year.clone(),
                label: None,
                genre: None,
                cover: None,
                track_titles: &[],
            })?;
            let unresolved = tags::resolve(&mb_client, &mut norm, &overrides);
            if !unresolved.is_empty() && !*allow_native {
                return Err(Error::UnresolvedTitles(tags::format_unresolved(
                    &unresolved,
                )));
            }

            let backfill_track = match *disc {
                Some(d) => norm
                    .tracks
                    .iter()
                    .find(|t| {
                        return t.position == Some(position) && t.medium_position.unwrap_or(1) == d;
                    })
                    .cloned()
                    .ok_or_else(|| {
                        return Error::YtTrackNotFound {
                            release_mbid: mbid.clone(),
                            position,
                        };
                    })?,
                None => {
                    let candidates: Vec<&NormalizedTrack> = norm
                        .tracks
                        .iter()
                        .filter(|t| return t.position == Some(position))
                        .collect();
                    if candidates.len() > 1 {
                        return Err(Error::YtTrackNotFound {
                            release_mbid: mbid.clone(),
                            position,
                        });
                    }
                    candidates.into_iter().next().cloned().ok_or_else(|| {
                        return Error::YtTrackNotFound {
                            release_mbid: mbid.clone(),
                            position,
                        };
                    })?
                }
            };
            (norm, Some(backfill_track))
        }
        None => {
            let synthetic = NormalizedRelease {
                id: String::new(),
                title: album
                    .clone()
                    .unwrap_or_else(|| return "Unknown Album".to_string()),
                status: None,
                country: None,
                disambiguation: None,
                label: None,
                formats: vec!["Opus".to_string()],
                track_count: 0,
                date: year.clone(),
                artist_credit: artist.clone(),
                artist_credit_parts: Vec::new(),
                release_group_id: None,
                title_native: None,
                artist_credit_native: None,
                genre: None,
                tracks: Vec::new(),
            };
            (synthetic, None)
        }
    };

    // `--title` still wins over the (already resolved) backfill track title, so a
    // manual per-video title override is possible without a --tags file.
    let final_title = title
        .clone()
        .or_else(|| return backfill.as_ref().map(|t| return t.title.clone()))
        .ok_or(Error::YtTitleRequired)?;
    // In the --release path norm_release.artist_credit is already the resolved
    // (override + romanization) artist; in the synthetic path it's the raw --artist.
    let final_artist = norm_release
        .artist_credit
        .clone()
        .or_else(|| return artist.clone());
    let final_track = track.or_else(|| return backfill.as_ref().and_then(|t| return t.position));
    let final_disc =
        disc.or_else(|| return backfill.as_ref().and_then(|t| return t.medium_position));
    let recording_id = backfill
        .as_ref()
        .and_then(|t| return t.recording_id.clone());
    let title_native = backfill
        .as_ref()
        .and_then(|t| return t.title_native.clone())
        .filter(|_| return title.is_none());

    let mut norm_release = norm_release;
    norm_release.artist_credit = final_artist;

    let raw_position = backfill
        .as_ref()
        .and_then(|t| return t.raw_position.clone());
    let matched = vec![import::MatchedTrack {
        file: fetched.path.clone(),
        position: final_track,
        medium_position: final_disc,
        title: final_title,
        title_native,
        source_url: Some(fetched.source_url.clone()),
        recording_id,
        raw_position,
        distance: 0.0,
        reasons: std::collections::BTreeMap::new(),
    }];

    // Cover art is always the YouTube thumbnail (no manual override for yt).
    let cover_art = match &fetched.thumbnail {
        Some(thumb) => Some(mimport_core::coverart::from_local_file(thumb)?),
        None => None,
    };

    let opts = import::ImportOptions {
        dry_run: *dry_run,
        library_root: cfg.paths.library.clone(),
        move_files: false,
    };
    let imported = import::write_and_copy(&matched, &norm_release, &opts, cover_art.as_ref())?;

    if !dry_run {
        let db = jobs::open(&cfg.paths.database)?;
        library::insert_track(&db, None, &norm_release, &matched[0], &imported[0])?;
    }

    output::print(
        &serde_json::json!({
            "source_url": fetched.source_url,
            "video_id": fetched.video_id,
            "yt_title": fetched.title,
            "yt_uploader": fetched.uploader,
            "release": if norm_release.id.is_empty() { None } else { Some(norm_release.id.clone()) },
            "cover_art": cover_art.is_some(),
            "imported": imported,
        }),
        cli.json,
    );
    return Ok(());
}

struct YtPlaylistFlags<'a> {
    release: Option<&'a str>,
    tags: Option<&'a Path>,
    allow_native: bool,
    dry_run: bool,
    yt: YtConfig,
}

fn run_yt_playlist(
    cli: &Cli,
    cfg: &Config,
    url: &str,
    flags: YtPlaylistFlags<'_>,
) -> mimport_core::Result<()> {
    let release_mbid = flags.release.ok_or(Error::YtPlaylistNeedsRelease)?;
    let fetched = yt::fetch(&flags.yt, url, &cfg.paths.staging, true)?;

    let mb_client = MbClient::new(&cfg.musicbrainz)?;
    let raw_release = mb_q::release_with_tracks(&mb_client, release_mbid)?;
    let mut release = NormalizedRelease::from(raw_release);

    let mut overrides = match flags.tags {
        Some(path) => tags::TagOverrides::load(path)?,
        None => tags::TagOverrides::default(),
    };
    overrides.apply_flag_overrides(tags::FlagOverrides {
        artist: None,
        album: None,
        date: None,
        label: None,
        genre: None,
        cover: None,
        track_titles: &[],
    })?;

    let unresolved = tags::resolve(&mb_client, &mut release, &overrides);
    if !unresolved.is_empty() && !flags.allow_native {
        return Err(Error::UnresolvedTitles(tags::format_unresolved(
            &unresolved,
        )));
    }

    // Cover art is always the YouTube thumbnail; use the first entry that has one.
    let cover_art = match fetched.iter().find_map(|f| return f.thumbnail.as_ref()) {
        Some(thumb) => Some(mimport_core::coverart::from_local_file(thumb)?),
        None => None,
    };

    // yt is single-disc, so playlist_index maps straight onto track.position.
    // Entries yt-dlp skipped (--ignore-errors) simply don't appear in `fetched`;
    // a fetched entry whose index has no release track is warned and skipped
    // rather than aborting the whole import.
    let mut matched: Vec<import::MatchedTrack> = Vec::new();
    let mut skipped: Vec<u32> = Vec::new();
    let mut claimed: HashSet<u32> = HashSet::new();
    for (idx, f) in fetched.iter().enumerate() {
        let position = match f.playlist_index {
            Some(p) => p,
            None => {
                tracing::warn!(
                    "playlist entry {} has no playlist_index; assuming position {}",
                    f.video_id,
                    idx + 1
                );
                (idx as u32) + 1
            }
        };
        if !claimed.insert(position) {
            tracing::warn!(
                "playlist entry {position} ({}) duplicates an earlier entry; skipping",
                f.video_id
            );
            skipped.push(position);
            continue;
        }
        let track = match release
            .tracks
            .iter()
            .find(|t| return t.position == Some(position))
        {
            Some(t) => t,
            None => {
                tracing::warn!(
                    "playlist entry {position} ({}) has no matching track in release {release_mbid}; skipping",
                    f.video_id
                );
                skipped.push(position);
                continue;
            }
        };
        matched.push(import::MatchedTrack {
            file: f.path.clone(),
            position: track.position,
            medium_position: track.medium_position,
            title: track.title.clone(),
            title_native: track.title_native.clone(),
            source_url: Some(f.source_url.clone()),
            recording_id: track.recording_id.clone(),
            raw_position: track.raw_position.clone(),
            distance: 0.0,
            reasons: std::collections::BTreeMap::new(),
        });
    }

    let opts = import::ImportOptions {
        dry_run: flags.dry_run,
        library_root: cfg.paths.library.clone(),
        move_files: false,
    };
    let imported = import::write_and_copy(&matched, &release, &opts, cover_art.as_ref())?;

    if !flags.dry_run {
        let db = jobs::open(&cfg.paths.database)?;
        for (m, f) in matched.iter().zip(imported.iter()) {
            library::insert_track(&db, None, &release, m, f)?;
        }
    }

    output::print(
        &serde_json::json!({
            "source_url": url,
            "release": release.id,
            "cover_art": cover_art.is_some(),
            "matched": matched,
            "skipped_positions": skipped,
            "imported": imported,
        }),
        cli.json,
    );
    return Ok(());
}

fn run_slskd(cli: &Cli, cfg: &Config, cmd: &SlskdCmd) -> mimport_core::Result<()> {
    let client = SlskdClient::new(&cfg.slskd)?;
    match cmd {
        SlskdCmd::Search { query, fresh, all } => {
            let outcome = slskd_q::search(&client, query, *fresh)?;
            let view = slskd_q::view_search(
                &outcome.search,
                slskd_q::SearchView {
                    lossless_only: true,
                    all: *all,
                },
            );
            output::print(
                &serde_json::json!({"reused": outcome.reused, "search": view}),
                cli.json,
            );
        }
        SlskdCmd::Searches => {
            let searches = slskd_q::list_searches(&client)?;
            output::print(&searches, cli.json);
        }
        SlskdCmd::SearchStatus { id, all } => {
            let status = slskd_q::search_status(&client, id)?;
            let view = slskd_q::view_search(
                &status,
                slskd_q::SearchView {
                    lossless_only: true,
                    all: *all,
                },
            );
            output::print(&view, cli.json);
        }
        SlskdCmd::SearchRemove { id } => {
            slskd_q::search_remove(&client, id)?;
            output::print(&serde_json::json!({"removed": id}), cli.json);
        }
        SlskdCmd::Fetch {
            search_id,
            username,
            directory,
            filenames,
            title,
            wait_secs,
        } => {
            let search = slskd_q::search_status(&client, search_id)?;
            let files = slskd_q::resolve_selector(&search, username, directory, filenames)?;
            let items: Vec<slskd_q::QueueDownloadRequestItem> = files
                .iter()
                .map(|f| {
                    return slskd_q::QueueDownloadRequestItem {
                        filename: f.filename.clone(),
                        size: f.size,
                    };
                })
                .collect();
            let window = slskd_q::batch_timeout(&cfg.slskd, &items, *wait_secs);

            let db = jobs::open(&cfg.paths.database)?;
            let local_dir = jobs::local_dir_for(&cfg.paths.downloads, username, directory);
            let job_title = title
                .clone()
                .unwrap_or_else(|| return jobs::default_title(directory));
            let job_id =
                jobs::create_job(&db, &job_title, search_id, username, directory, &local_dir)?;

            slskd_q::fetch_into_job(&client, username, &items, window, &db, job_id, &local_dir)?;
            let job = jobs::get_job(&db, job_id)?;
            let files = jobs::get_job_files(&db, job_id)?;
            output::print(&serde_json::json!({"job": job, "files": files}), cli.json);
        }
        SlskdCmd::Status { target } => {
            let db = jobs::open(&cfg.paths.database)?;
            let job = jobs::resolve_target(&db, target)?;
            let files = jobs::get_job_files(&db, job.id)?;
            output::print(&serde_json::json!({"job": job, "files": files}), cli.json);
        }
        SlskdCmd::Cancel { target, remove } => {
            let db = jobs::open(&cfg.paths.database)?;
            let job = jobs::resolve_target(&db, target)?;
            let files = jobs::get_job_files(&db, job.id)?;
            let mut cancelled_any = false;
            for f in &files {
                if slskd_q::is_terminal_state(&f.state) {
                    continue;
                }
                cancelled_any = true;
                slskd_q::cancel_transfer(&client, &job.username, &f.transfer_id, *remove)?;
                // With --remove slskd may have already deleted the transfer,
                // in which case the follow-up status fetch 404s; record the
                // cancel locally instead of failing the whole command.
                match slskd_q::transfer_status(&client, &job.username, &f.transfer_id) {
                    Ok(transfer) => {
                        jobs::upsert_job_file(
                            &db,
                            job.id,
                            std::path::Path::new(&job.local_dir),
                            &transfer,
                        )?;
                    }
                    Err(Error::SlskdNotFound { .. }) => {
                        jobs::set_job_file_state(&db, job.id, &f.transfer_id, "Cancelled")?;
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }
            let files = jobs::get_job_files(&db, job.id)?;
            // Only overwrite job.status if something was actually cancelled here —
            // otherwise a no-op cancel on an already-terminal job would clobber a
            // downstream status like "imported" back to a raw fetch-outcome label.
            if cancelled_any {
                let states: Vec<&str> = files.iter().map(|f| return f.state.as_str()).collect();
                jobs::set_job_status(&db, job.id, jobs::derive_status_from_states(&states))?;
            }
            let job = jobs::get_job(&db, job.id)?;
            output::print(&serde_json::json!({"job": job, "files": files}), cli.json);
        }
        SlskdCmd::Retry { target, wait_secs } => {
            let db = jobs::open(&cfg.paths.database)?;
            let job = jobs::resolve_target(&db, target)?;
            let files = jobs::get_job_files(&db, job.id)?;
            let retryable = jobs::retryable_files(&files);
            if retryable.is_empty() {
                output::print(
                    &serde_json::json!({
                        "job": job,
                        "retried": 0,
                        "files": files,
                    }),
                    cli.json,
                );
                return Ok(());
            }

            let dropped_ids: Vec<String> = retryable
                .iter()
                .map(|f| return f.transfer_id.clone())
                .collect();
            let items: Vec<slskd_q::QueueDownloadRequestItem> = retryable
                .iter()
                .map(|f| {
                    return slskd_q::QueueDownloadRequestItem {
                        filename: f.remote_path.clone(),
                        size: f.size,
                    };
                })
                .collect();

            // Old transfers must be gone from slskd before the same files can
            // be re-enqueued; DB rows are swapped only once that succeeded.
            slskd_q::clear_transfers(&client, &job.username, &dropped_ids)?;
            let enqueued = slskd_q::enqueue_downloads(&client, &job.username, &items)?;
            if enqueued.enqueued.is_empty() {
                return Err(Error::Slskd {
                    what: "enqueue",
                    status: 0,
                    body: format!(
                        "no transfers returned in enqueued[] (failed: {:?})",
                        enqueued.failed
                    ),
                });
            }
            jobs::delete_job_files(&db, job.id, &dropped_ids)?;
            jobs::set_job_status(&db, job.id, jobs::STATUS_INCOMPLETE)?;

            let local_dir = std::path::PathBuf::from(&job.local_dir);
            let window = slskd_q::batch_timeout(&cfg.slskd, &items, *wait_secs);
            let result = slskd_q::wait_for_transfers(
                &client,
                &job.username,
                enqueued.enqueued,
                window,
                |t| {
                    return jobs::upsert_job_file(&db, job.id, &local_dir, t);
                },
            );
            let transfers = match result {
                Ok(transfers) => transfers,
                Err(e) => {
                    return Err(e);
                }
            };
            jobs::set_job_status(&db, job.id, jobs::derive_status(&transfers))?;
            let job = jobs::get_job(&db, job.id)?;
            let files = jobs::get_job_files(&db, job.id)?;
            output::print(&serde_json::json!({"job": job, "files": files}), cli.json);
        }
        SlskdCmd::Downloads => {
            let downloads = slskd_q::list_downloads(&client)?;
            output::print(&downloads, cli.json);
        }
        SlskdCmd::Remove {
            username,
            transfer_id,
        } => {
            slskd_q::remove_download(&client, username, transfer_id)?;
            output::print(
                &serde_json::json!({"username": username, "transfer_id": transfer_id, "removed": true}),
                cli.json,
            );
        }
        SlskdCmd::ClearCompleted => {
            slskd_q::clear_completed_downloads(&client)?;
            output::print(&serde_json::json!({"cleared": true}), cli.json);
        }
        SlskdCmd::Browse {
            username,
            directory,
        } => {
            let dirs = slskd_q::browse_directory(&client, username, directory)?;
            output::print(&dirs, cli.json);
        }
    }
    return Ok(());
}
