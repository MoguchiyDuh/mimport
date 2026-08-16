use std::time::Duration;

use crate::config::CoverArtConfig;
use crate::error::{Error, Result};

pub struct CoverArt {
    pub mime: String,
    pub bytes: Vec<u8>,
}

pub struct CoverArtClient {
    http: reqwest::blocking::Client,
    base_url: String,
    user_agent: String,
}

impl CoverArtClient {
    pub fn new(cfg: &CoverArtConfig, user_agent: &str) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        return Ok(CoverArtClient {
            http,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            user_agent: user_agent.to_string(),
        });
    }

    pub fn front_cover(&self, release_mbid: &str) -> Result<Option<CoverArt>> {
        let url = format!("{}/release/{}/front-500", self.base_url, release_mbid);
        let resp = self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .send()?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(Error::CoverArt {
                status: status.as_u16(),
                body: resp.text().unwrap_or_default(),
            });
        }
        let mime = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| return v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        let bytes = resp.bytes()?.to_vec();
        return Ok(Some(CoverArt { mime, bytes }));
    }
}
