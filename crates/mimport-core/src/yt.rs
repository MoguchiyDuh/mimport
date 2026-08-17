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
}

pub fn fetch(cfg: &YtConfig, url: &str, staging_dir: &Path) -> Result<FetchedAudio> {
    std::fs::create_dir_all(staging_dir).map_err(|e| return Error::io(staging_dir, e))?;

    let out_template = staging_dir.join("%(id)s.%(ext)s");
    let print_template = format!(
        "after_move:%(id)s{FIELD_SEP}%(title)s{FIELD_SEP}%(uploader)s{FIELD_SEP}%(duration)s{FIELD_SEP}%(filepath)s"
    );

    let output = Command::new(&cfg.yt_dlp_path)
        .args(["--no-playlist", "-x", "--audio-format", "opus"])
        .arg("-o")
        .arg(&out_template)
        .arg("--print")
        .arg(&print_template)
        .arg(url)
        .output()
        .map_err(|e| {
            return match e.kind() {
                std::io::ErrorKind::NotFound => Error::ToolMissing { tool: "yt-dlp" },
                _ => Error::io(staging_dir, e),
            };
        })?;

    if !output.status.success() {
        return Err(Error::ToolFailed {
            tool: "yt-dlp",
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).chars().take(600).collect(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next_back().ok_or_else(|| {
        return Error::ToolFailed {
            tool: "yt-dlp",
            status: output.status.to_string(),
            stderr: "no --print output captured on stdout".to_string(),
        };
    })?;

    let parts: Vec<&str> = line.split(FIELD_SEP).collect();
    if parts.len() != 5 {
        return Err(Error::ToolFailed {
            tool: "yt-dlp",
            status: output.status.to_string(),
            stderr: format!("unexpected --print output shape: {line:?}"),
        });
    }
    let na = |s: &str| -> Option<String> {
        return if s == "NA" { None } else { Some(s.to_string()) };
    };

    return Ok(FetchedAudio {
        path: PathBuf::from(parts[4]),
        video_id: parts[0].to_string(),
        title: na(parts[1]),
        uploader: na(parts[2]),
        duration_secs: parts[3].parse::<f64>().ok(),
        source_url: url.to_string(),
    });
}
