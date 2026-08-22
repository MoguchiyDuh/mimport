use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::YtConfig;
use crate::error::{Error, Result};

/// Unit separator — not expected to appear in yt-dlp's own metadata fields,
/// used to split a single `--print` line into columns without JSON escaping.
const FIELD_SEP: char = '\u{1f}';

pub struct FetchedAudio {
    pub path: PathBuf,
    pub video_id: String,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub duration_secs: Option<f64>,
    pub source_url: String,
    pub playlist_index: Option<u32>,
    pub thumbnail: Option<PathBuf>,
}

/// Fetches a single video (`playlist == false`) or every entry in a playlist
/// (`playlist == true`) as Opus into `staging_dir`. Each entry's thumbnail is
/// also written (converted to JPEG) so it can be used as cover art. In playlist
/// mode `--ignore-errors` lets a dead/unavailable entry be skipped rather than
/// aborting the whole batch — yt-dlp still emits a `--print` line per entry that
/// did download, with its original `playlist_index` preserved.
pub fn fetch(
    cfg: &YtConfig,
    url: &str,
    staging_dir: &Path,
    playlist: bool,
) -> Result<Vec<FetchedAudio>> {
    std::fs::create_dir_all(staging_dir).map_err(|e| return Error::io(staging_dir, e))?;

    let out_template = staging_dir.join("%(id)s.%(ext)s");
    let print_template = format!(
        "after_move:%(playlist_index)s{FIELD_SEP}%(id)s{FIELD_SEP}%(title)s{FIELD_SEP}%(uploader)s{FIELD_SEP}%(duration)s{FIELD_SEP}%(filepath)s"
    );

    let mut cmd = Command::new(&cfg.yt_dlp_path);
    cmd.args(["-x", "--audio-format", "opus"])
        .args(["--write-thumbnail", "--convert-thumbnails", "jpg"])
        .arg("-o")
        .arg(&out_template)
        .arg("--print")
        .arg(&print_template);
    if playlist {
        cmd.arg("--ignore-errors");
    } else {
        cmd.arg("--no-playlist");
    }
    cmd.arg(url);

    let output = cmd.output().map_err(|e| {
        return match e.kind() {
            std::io::ErrorKind::NotFound => Error::ToolMissing { tool: "yt-dlp" },
            _ => Error::io(staging_dir, e),
        };
    })?;

    // In playlist mode `--ignore-errors` makes yt-dlp exit non-zero whenever any
    // entry failed, even if others downloaded fine — so a non-zero exit isn't
    // fatal there; we parse whatever `--print` lines came back and only bail if
    // that leaves us with nothing (the `tracks.is_empty()` check below). Single
    // videos have no partial-success case, so a non-zero exit stays fatal.
    if !output.status.success() && !playlist {
        return Err(Error::ToolFailed {
            tool: "yt-dlp",
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(600)
                .collect(),
        });
    }
    if !output.status.success() {
        tracing::warn!(
            "yt-dlp reported failures on some playlist entries (exit {}); importing the ones that succeeded",
            output.status
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut tracks = Vec::new();
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split(FIELD_SEP).collect();
        if parts.len() != 6 {
            return Err(Error::ToolFailed {
                tool: "yt-dlp",
                status: output.status.to_string(),
                stderr: format!("unexpected --print output shape: {line:?}"),
            });
        }
        let na = |s: &str| -> Option<String> {
            return if s == "NA" { None } else { Some(s.to_string()) };
        };
        let video_id = parts[1].to_string();
        let candidate = staging_dir.join(format!("{video_id}.jpg"));
        let thumbnail = if candidate.exists() {
            Some(candidate)
        } else {
            None
        };
        tracks.push(FetchedAudio {
            path: PathBuf::from(parts[5]),
            video_id,
            title: na(parts[2]),
            uploader: na(parts[3]),
            duration_secs: parts[4].parse::<f64>().ok(),
            source_url: url.to_string(),
            playlist_index: parts[0].parse::<u32>().ok(),
            thumbnail,
        });
    }

    if tracks.is_empty() {
        return Err(Error::ToolFailed {
            tool: "yt-dlp",
            status: output.status.to_string(),
            stderr: "no --print output captured on stdout".to_string(),
        });
    }
    return Ok(tracks);
}
