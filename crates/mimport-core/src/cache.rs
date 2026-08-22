//! Shared disk cache: JSON bodies keyed by URL hash, with a TTL.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::error::{Error, Result};

/// FNV-1a 64-bit. Fixed algorithm so cache filenames stay stable across Rust
/// versions/platforms (unlike `DefaultHasher`, which is not guaranteed stable).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    return hash;
}

pub struct DiskCache {
    dir: PathBuf,
    ttl: Duration,
}

impl DiskCache {
    pub fn new(dir: impl Into<PathBuf>, ttl_secs: u64) -> Self {
        return DiskCache {
            dir: dir.into(),
            ttl: Duration::from_secs(ttl_secs),
        };
    }

    fn path_for(&self, key: &str) -> PathBuf {
        let hash = fnv1a(key.as_bytes());
        let slug: String = key
            .chars()
            .filter(|c| return c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        return self.dir.join(format!("{slug}-{hash:016x}.json"));
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let path = self.path_for(key);
        let meta = std::fs::metadata(&path).ok()?;
        let modified = meta.modified().ok()?;
        let age = SystemTime::now().duration_since(modified).ok()?;
        if age > self.ttl {
            return None;
        }
        return std::fs::read_to_string(&path).ok();
    }

    pub fn put(&self, key: &str, body: &str) -> Result<()> {
        std::fs::create_dir_all(&self.dir).map_err(|e| return Error::io(&self.dir, e))?;
        let path = self.path_for(key);
        let file_name = path
            .file_name()
            .and_then(|n| return n.to_str())
            .unwrap_or("entry");
        let tmp_path = self
            .dir
            .join(format!(".{file_name}.{}.tmp", std::process::id()));
        if let Err(e) = std::fs::write(&tmp_path, body) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(Error::io(&tmp_path, e));
        }
        if let Err(e) = std::fs::rename(&tmp_path, &path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(Error::io(&path, e));
        }
        return Ok(());
    }
}

pub fn cache_dir_default(sub: &str) -> PathBuf {
    let base = dirs_home_cache();
    return base.join("mimport").join(sub);
}

fn dirs_home_cache() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        return Path::new(&xdg).to_path_buf();
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| return ".".to_string());
    return Path::new(&home).join(".cache");
}
