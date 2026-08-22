use std::path::{Path, PathBuf};
use std::process::Command;

use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;

use crate::error::{Error, Result};

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "m4a", "mp4", "ogg", "opus", "wav", "ape", "wv",
];

pub fn is_audio(path: &Path) -> bool {
    return path
        .extension()
        .and_then(|e| return e.to_str())
        .is_some_and(|e| {
            return AUDIO_EXTENSIONS
                .iter()
                .any(|a| return e.eq_ignore_ascii_case(a));
        });
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioProperties {
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub channels: Option<u32>,
    pub duration_secs: Option<f64>,
    pub file_size: u64,
}

impl AudioProperties {
    pub fn bitrate_bps(&self) -> Option<f64> {
        let secs = self.duration_secs?;
        if secs <= 0.0 {
            return None;
        }
        return Some((self.file_size as f64 * 8.0) / secs);
    }

    pub fn pcm_bps(&self) -> Option<f64> {
        let sr = self.sample_rate? as f64;
        let bd = self.bit_depth? as f64;
        let ch = self.channels.unwrap_or(2) as f64;
        return Some(sr * bd * ch);
    }

    pub fn compression_ratio(&self) -> Option<f64> {
        return Some(self.bitrate_bps()? / self.pcm_bps()?);
    }
}

pub fn probe(path: &Path) -> Result<AudioProperties> {
    let file = std::fs::File::open(path).map_err(|e| return Error::io(path, e))?;
    let file_size = file
        .metadata()
        .map_err(|e| return Error::io(path, e))?
        .len();

    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| return e.to_str()) {
        hint.with_extension(ext);
    }

    let format = symphonia::default::get_probe()
        .probe(&hint, mss, Default::default(), Default::default())
        .map_err(|e| {
            return Error::Probe {
                path: path.to_path_buf(),
                reason: e.to_string(),
            };
        })?;

    let track = format
        .tracks()
        .iter()
        .find(|t| return t.codec_params.is_some())
        .ok_or_else(|| {
            return Error::Probe {
                path: path.to_path_buf(),
                reason: "no decodable audio track".to_string(),
            };
        })?;

    let params = track.codec_params.as_ref().and_then(|p| return p.audio());
    let sample_rate = params.and_then(|p| return p.sample_rate);
    let bit_depth = params.and_then(|p| return p.bits_per_sample);
    let channels = params
        .and_then(|p| return p.channels.as_ref())
        .map(|c| return c.count() as u32);

    let duration_secs = match (track.num_frames, sample_rate) {
        (Some(frames), Some(sr)) if sr > 0 => Some(frames as f64 / f64::from(sr)),
        _ => None,
    };

    return Ok(AudioProperties {
        sample_rate,
        bit_depth,
        channels,
        duration_secs,
        file_size,
    });
}

pub fn needs_downsample(props: &AudioProperties, target_rate: u32, target_depth: u16) -> bool {
    if props.sample_rate.is_some_and(|sr| return sr > target_rate) {
        return true;
    }
    if props
        .bit_depth
        .is_some_and(|bd| return bd > u32::from(target_depth))
    {
        return true;
    }
    return false;
}

pub fn downsample(path: &Path, target_rate: u32, target_depth: u16) -> Result<()> {
    let sample_fmt = match target_depth {
        16 => "s16",
        24 => "s32",
        other => {
            return Err(Error::ConfigInvalid(format!(
                "unsupported target bit depth {other}"
            )));
        }
    };

    let tmp = temp_sibling(path);
    let filter = format!(
        "aresample={target_rate}:resampler=soxr:precision=28:osf={sample_fmt}:\
         dither_method=triangular_hp"
    );

    let output = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .arg("-i")
        .arg(path)
        .args(["-map", "0"])
        .args(["-c:v", "copy"])
        .args(["-af", &filter])
        .args(["-sample_fmt", sample_fmt])
        .args(["-map_metadata", "0"])
        .args(["-metadata", "encoder="])
        .arg(&tmp)
        .output()
        .map_err(|e| {
            return match e.kind() {
                std::io::ErrorKind::NotFound => Error::ToolMissing { tool: "ffmpeg" },
                _ => Error::io(path, e),
            };
        })?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::ToolFailed {
            tool: "ffmpeg",
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(600)
                .collect(),
        });
    }

    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::io(path, e));
    }
    return Ok(());
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".mimport-tmp.{}.flac", std::process::id()));
    return path.with_file_name(name);
}

/// True for mimport's own per-process temp files so walks skip them instead of
/// treating a leftover `.mimport-tmp.<pid>.flac` as audio to import/postfix.
pub fn is_temp_file(path: &Path) -> bool {
    return path
        .file_name()
        .and_then(|n| return n.to_str())
        .is_some_and(|n| return n.contains(".mimport-tmp."));
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Suspicion {
    ImplausibleCompression,
    ImplausibleBitrate,
    Unknown,
}

pub fn suspect_lossy(props: &AudioProperties) -> Option<Suspicion> {
    let ratio = props.compression_ratio()?;
    if ratio < 0.30 {
        return Some(Suspicion::ImplausibleCompression);
    }
    if props.bitrate_bps().is_some_and(|b| return b < 400_000.0)
        && props.sample_rate.is_some_and(|sr| return sr >= 44_100)
    {
        return Some(Suspicion::ImplausibleBitrate);
    }
    return None;
}
