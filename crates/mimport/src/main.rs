mod cli;
mod output;

use clap::Parser;
use mimport_core::error::Error;
use mimport_core::lidarr::{queries as lidarr_q, LidarrClient};
use mimport_core::mb::{queries as mb_q, MbClient};
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

/// `mbid:<uuid>` prefix → authoritative lookup, no search involved (DESIGN.md §5,
/// borrowed from Lidarr's own search-box convention).
fn is_mbid_query(q: &str) -> Option<&str> {
    return q.strip_prefix("mbid:");
}

fn run(cli: &Cli) -> mimport_core::Result<()> {
    let cfg = Config::load(&cli.config)?;

    match &cli.command {
        Command::Lidarr(cmd) => run_lidarr(cli, &cfg, cmd),
        Command::Mb(cmd) => run_mb(cli, &cfg, cmd),
        Command::Slskd(cmd) => run_slskd(cli, &cfg, cmd),
        Command::Postfix { .. } => Err(Error::NotImplemented("postfix")),
        Command::Import { .. } => Err(Error::NotImplemented("import")),
    }
}

/// §7 scoring, shared by `lidarr album` and `mb album` — the scorer is backend-agnostic
/// (DESIGN.md §5), so this is the one place either command's release list gets ranked.
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
            username,
            filename,
            size,
        } => {
            let transfer = slskd_q::fetch_and_wait(&client, &cfg.slskd, username, filename, *size)?;
            output::print(&transfer, cli.json);
        }
        SlskdCmd::Status { username, id } => {
            let transfer = slskd_q::transfer_status(&client, username, id)?;
            output::print(&transfer, cli.json);
        }
        SlskdCmd::Cancel {
            username,
            id,
            remove,
        } => {
            slskd_q::cancel_transfer(&client, username, id, *remove)?;
            output::print(&serde_json::json!({"cancelled": true}), cli.json);
        }
        SlskdCmd::Browse { username, directory } => {
            let dirs = slskd_q::browse_directory(&client, username, directory)?;
            output::print(&dirs, cli.json);
        }
    }
    return Ok(());
}
