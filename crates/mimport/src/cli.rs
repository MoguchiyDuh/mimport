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
        #[arg(long)]
        dry_run: bool,
    },

    #[command(subcommand)]
    Library(LibraryCmd),
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
