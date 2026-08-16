mod cli;
mod output;

use clap::Parser;
use mimport_core::error::Error;
use mimport_core::jobs;
use mimport_core::lidarr::{queries as lidarr_q, LidarrClient};
use mimport_core::mb::{queries as mb_q, MbClient};
use mimport_core::postfix;
use mimport_core::release::{NormalizedRelease, NormalizedReleaseGroup};
use mimport_core::scorer::{self, ScoreContext};
use mimport_core::slskd::{queries as slskd_q, SlskdClient};
use mimport_core::Config;

use cli::{Cli, Command, LidarrCmd, MbCmd, SlskdCmd};

fn main() {
    tracing_subscriber::fmt().with_target(false).init();
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
        Command::Postfix { target, dry_run } => run_postfix(cli, &cfg, target, *dry_run),
        Command::Import { .. } => Err(Error::NotImplemented("import")),
    }
}

fn rank_releases(
    releases: &[NormalizedRelease],
    group: &NormalizedReleaseGroup,
    scoring: &mimport_core::config::Scoring,
) -> Vec<scorer::ScoreBreakdown> {
    let canonical = scorer::canonical_track_count(releases);
    let canonical_titles = scorer::canonical_track_titles(releases);
    let ctx = ScoreContext {
        cfg: scoring,
        group: Some(group),
        canonical_tracks: canonical,
        canonical_titles: &canonical_titles,
    };
    let scored = releases.iter().map(|r| return scorer::score_release(r, &ctx)).collect();
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
            let releases: Vec<NormalizedRelease> =
                raw_releases.into_iter().map(NormalizedRelease::from).collect();
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

fn run_postfix(cli: &Cli, cfg: &Config, target: &str, dry_run: bool) -> mimport_core::Result<()> {
    let opts = postfix::Options {
        dry_run,
        target_rate: cfg.quality.target_samplerate,
        target_depth: cfg.quality.target_bitdepth,
    };

    // Numeric id or a jobs.title match takes precedence; an existing
    // filesystem path not matching either is operated on ad hoc, outside
    // job tracking, per DESIGN.md §5.
    let as_path = std::path::Path::new(target);
    let is_numeric = target.parse::<i64>().is_ok();
    if !is_numeric && as_path.exists() {
        let report = postfix::run(as_path, &opts)?;
        output::print(&report, cli.json);
        return Ok(());
    }

    let db = jobs::open(&cfg.paths.database)?;
    let job = jobs::resolve_target(&db, target)?;
    let report = postfix::run(std::path::Path::new(&job.local_dir), &opts)?;
    if !dry_run {
        jobs::set_job_status(&db, job.id, jobs::STATUS_POSTFIXED)?;
    }
    output::print(&serde_json::json!({"job": job, "report": report}), cli.json);
    return Ok(());
}

fn run_slskd(cli: &Cli, cfg: &Config, cmd: &SlskdCmd) -> mimport_core::Result<()> {
    let client = SlskdClient::new(&cfg.slskd)?;
    match cmd {
        SlskdCmd::Search { query } => {
            let results = slskd_q::search(&client, query)?;
            output::print(&results, cli.json);
        }
        SlskdCmd::SearchStatus { id } => {
            let status = slskd_q::search_status(&client, id)?;
            output::print(&status, cli.json);
        }
        SlskdCmd::Fetch {
            search_id,
            username,
            directory,
            filename,
            title,
        } => {
            let search = slskd_q::search_status(&client, search_id)?;
            let files = slskd_q::resolve_selector(&search, username, directory, filename.as_deref())?;

            let db = jobs::open(&cfg.paths.database)?;
            let local_dir = jobs::local_dir_for(&cfg.paths.downloads, directory);
            let job_title = title.clone().unwrap_or_else(|| return jobs::default_title(directory));
            let job_id = jobs::create_job(&db, &job_title, search_id, username, directory, &local_dir)?;

            let result = slskd_q::fetch_and_wait(&client, &cfg.slskd, username, &files, |t| {
                return jobs::upsert_job_file(&db, job_id, &local_dir, t);
            });
            let transfers = match result {
                Ok(transfers) => {
                    jobs::set_job_status(&db, job_id, jobs::derive_status(&transfers))?;
                    transfers
                }
                Err(e) => {
                    return Err(e);
                }
            };
            let job = jobs::get_job(&db, job_id)?;
            output::print(&serde_json::json!({"job": job, "transfers": transfers}), cli.json);
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
                let transfer = slskd_q::transfer_status(&client, &job.username, &f.transfer_id)?;
                jobs::upsert_job_file(&db, job.id, std::path::Path::new(&job.local_dir), &transfer)?;
            }
            let files = jobs::get_job_files(&db, job.id)?;
            // Only overwrite job.status if something was actually cancelled here —
            // otherwise a no-op cancel on an already-terminal job would clobber a
            // downstream status like "postfixed" back to a raw fetch-outcome label.
            if cancelled_any {
                let states: Vec<&str> = files.iter().map(|f| return f.state.as_str()).collect();
                jobs::set_job_status(&db, job.id, jobs::derive_status_from_states(&states))?;
            }
            let job = jobs::get_job(&db, job.id)?;
            output::print(&serde_json::json!({"job": job, "files": files}), cli.json);
        }
        SlskdCmd::Browse { username, directory } => {
            let dirs = slskd_q::browse_directory(&client, username, directory)?;
            output::print(&dirs, cli.json);
        }
    }
    return Ok(());
}
