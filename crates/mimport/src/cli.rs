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
    /// Raw Soulseek text query. Blocks until slskd's own search completes — mimport
    /// never sends a `searchTimeout`, so there's no `--timeout` flag here on purpose.
    ///
    /// First cut: takes free text directly, not a release mbid. DESIGN.md §5's
    /// mbid-driven flow (release mbid -> generated query -> search id stashed
    /// somewhere) needs the jobs DB (not built yet) to link a search back to a
    /// release/job, so it's layered on top of this primitive later.
    Search { query: String },

    /// Poll an in-flight search's state/results by id (returned by `search`).
    SearchStatus { id: String },

    /// Enqueue one file from `username` and wait for it to finish, up to a timeout
    /// scaled by file size (`[slskd] fetch_timeout_base_secs` +
    /// `fetch_timeout_per_mb_secs * size_mb`). The transfer itself keeps running on
    /// slskd even if this times out — check with `status` afterward.
    ///
    /// First cut: single file by exact filename+size (as returned by `search`'s
    /// results), not the `<search-id> <selector>` multi-file shape from DESIGN.md §5
    /// — that needs somewhere to cache recent search results to resolve a selector
    /// index back to filenames, which doesn't exist yet (jobs DB).
    Fetch {
        username: String,
        filename: String,
        size: i64,
    },

    /// Poll one transfer's state without waiting.
    Status { username: String, id: String },

    /// Cancel an in-progress transfer.
    Cancel {
        username: String,
        id: String,
        /// Also drop the transfer record server-side, not just stop it.
        #[arg(long)]
        remove: bool,
    },

    /// List one directory's contents for a user (`POST /directory`) — the per-folder
    /// browse primitive, not a full recursive tree (decided 2026-08-14).
    Browse {
        username: String,
        /// Omit for the user's share root.
        #[arg(default_value = "")]
        directory: String,
    },
}
