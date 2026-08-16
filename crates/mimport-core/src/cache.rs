//! Shared disk cache: JSON bodies keyed by URL hash, with a TTL.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::error::{Error, Result};

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
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        let hash = hasher.finish();
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
        std::fs::write(&path, body).map_err(|e| return Error::io(&path, e))?;
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
