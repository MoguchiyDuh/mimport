use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mimport", version, about = "Personal music library pipeline")]
pub struct Cli {
    #[arg(long, global = true, default_value = "config.toml")]
    pub config: PathBuf,

    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    #[command(subcommand)]
    Lidarr(LidarrCmd),

    #[command(subcommand)]
    Mb(MbCmd),

    #[command(subcommand)]
    Slskd(SlskdCmd),

    Postfix {
        /// job id, jobs.title, or a raw filesystem path (ad hoc, outside job tracking)
        target: String,
        #[arg(long)]
        dry_run: bool,
    },

    Import {
        /// job id, jobs.title, or a raw filesystem path (ad hoc, outside job tracking)
        target: String,
        /// MB release mbid to match against
        #[arg(long)]
        release: String,
        /// {"<file path>": <track position>} mapping that bypasses Munkres matching entirely
        #[arg(long)]
        force: Option<PathBuf>,
        /// JSON file with manual tag overrides: artist, album, date, label, genre, cover
        /// (image path), tracks ({"<position>": "<title>"}); wins over MB, loses to flags below
        #[arg(long)]
        tags: Option<PathBuf>,
        #[arg(long)]
        artist: Option<String>,
        #[arg(long)]
        album: Option<String>,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        genre: Option<String>,
        /// local image file to embed as cover art instead of fetching from Cover Art Archive
        #[arg(long)]
        cover: Option<PathBuf>,
        /// "<position>=<title>" manual track title override; repeatable
        #[arg(long = "track-title")]
        track_title: Vec<String>,
        /// keep native (CJK/etc) script for any field with no romanization alias or manual override,
        /// instead of blocking the import
        #[arg(long)]
        allow_native: bool,
        #[arg(long)]
        dry_run: bool,
    },

    #[command(subcommand)]
    Library(LibraryCmd),

    #[command(subcommand)]
    Yt(YtCmd),
}

/// Query terms are trailing args, one shell token per clause; `--files`/`--`
/// must come before them (a leading `-`/`^` on a term is negation, not a flag).
#[derive(Subcommand)]
pub enum LibraryCmd {
    /// Lists library_tracks rows matching every query clause (AND).
    List {
        #[arg(trailing_var_arg = true)]
        query: Vec<String>,
    },
    /// Full metadata for one row by id.
    Show { id: i64 },
    /// Deletes matching rows from the index; --files also deletes the
    /// underlying files from disk (best-effort — already-missing files
    /// aren't an error).
    Remove {
        #[arg(long)]
        files: bool,
        #[arg(trailing_var_arg = true)]
        query: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum LidarrCmd {
    Artist { query: String },
    Album { mbid: String },
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
    Track { text: String },
}

#[derive(Subcommand)]
pub enum YtCmd {
    /// Fetch a YouTube/YouTube Music URL as Opus and import it. URL only —
    /// no search-by-name yet. Tags are manual (no auto-tagging): pass
    /// --title/--artist/etc., or --release <mbid> --track <n> to backfill
    /// title/artist/recording-id from a specific MB release track.
    ///
    /// With --playlist, fetch every entry (in playlist order) and map each to
    /// its release track by position; requires --release. Unavailable entries
    /// are skipped. Cover art is always the YouTube thumbnail.
    Fetch {
        url: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        artist: Option<String>,
        #[arg(long)]
        album: Option<String>,
        /// Track position — also selects which --release track to backfill from
        #[arg(long)]
        track: Option<u32>,
        #[arg(long)]
        disc: Option<u32>,
        #[arg(long)]
        year: Option<String>,
        /// MB release mbid to backfill metadata/MBIDs from; requires --track
        #[arg(long)]
        release: Option<String>,
        /// fetch the whole playlist/album instead of a single video
        #[arg(long)]
        playlist: bool,
        /// JSON manual tag overrides (same schema as `import --tags`); applied
        /// against the --release metadata, so it needs --release to be useful
        #[arg(long, requires = "release")]
        tags: Option<PathBuf>,
        /// keep native (CJK/etc) script for any field with no romanization alias
        /// or manual override, instead of blocking; needs --release
        #[arg(long, requires = "release")]
        allow_native: bool,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
pub enum SlskdCmd {
    Search { query: String },

    SearchStatus { id: String },

    Fetch {
        search_id: String,
        username: String,
        directory: String,
        filename: Option<String>,
        /// jobs.title for this job; defaults to the last path component of `directory`
        #[arg(long)]
        title: Option<String>,
    },

    /// job id or jobs.title (see `fetch --title`)
    Status { target: String },

    /// job id or jobs.title (see `fetch --title`)
    Cancel {
        target: String,
        #[arg(long)]
        remove: bool,
    },

    Browse {
        username: String,
        #[arg(default_value = "")]
        directory: String,
    },
}
