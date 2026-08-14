use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mimport", version, about = "Personal music library pipeline")]
pub struct Cli {
    #[arg(long, global = true, default_value = "config.toml")]
    pub config: PathBuf,

    /// Compact single-line JSON instead of pretty-printed.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Proxy-backed metadata (api.lidarr.audio) — idiomatic to the proxy's own shape.
    #[command(subcommand)]
    Lidarr(LidarrCmd),

    /// Direct MusicBrainz — full parity with the official API, fallback if the proxy
    /// is unavailable or untrustworthy.
    #[command(subcommand)]
    Mb(MbCmd),

    /// Soulseek daemon (slskd).
    #[command(subcommand)]
    Slskd(SlskdCmd),

    /// Strip junk Vorbis comment tags, downsample if needed.
    Postfix {
        /// job-id, path, or title.
        target: String,
        #[arg(long)]
        dry_run: bool,
    },

    /// Match files to the chosen release's tracks, tag, copy into the library.
    Import {
        /// job-id, path, or title.
        target: String,
        #[arg(long)]
        force: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum LidarrCmd {
    /// text search, or `mbid:<uuid>` for an authoritative direct lookup.
    Artist { query: String },
    /// release-group mbid → scored editions.
    Album { mbid: String },
    /// release-group mbid + one of its release mbids (from `lidarr album`'s output) →
    /// full tracklist for that one edition. Two args, not one — the proxy has no
    /// standalone release-lookup route, only `/album/{release-group-mbid}` with every
    /// release nested, so we always refetch the album and pick one release out of it.
    Tracks {
        release_group_mbid: String,
        release_mbid: String,
    },
}

#[derive(Subcommand)]
pub enum MbCmd {
    Artist { query: String },
    Album { mbid: String },
    Tracks { release_mbid: String },
    /// recording text search — no proxy equivalent exists.
    Track { text: String },
}

#[derive(Subcommand)]
pub enum SlskdCmd {
    Search {
        release_mbid: String,
        #[arg(long)]
        fresh: bool,
    },
    Fetch {
        search_id: String,
        username: String,
        /// omitted = whole dir/user; `10` = index; `1,3,5` = list; `1-3` = range.
        selector: Option<String>,
    },
    /// job-id, path, or title.
    Status { target: String },
}
