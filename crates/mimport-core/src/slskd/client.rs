use std::sync::Mutex;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::config::SlskdConfig;
use crate::error::{Error, Result};

use super::types::SessionResponse;

pub struct SlskdClient {
    http: reqwest::blocking::Client,
    base_url: String,
    username: String,
    password: String,
    token: Mutex<Option<String>>,
}

impl SlskdClient {
    pub fn new(cfg: &SlskdConfig) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(cfg.request_timeout_secs))
            .build()?;
        return Ok(SlskdClient {
            http,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            username: cfg.username.clone(),
            password: cfg.password.clone(),
            token: Mutex::new(None),
        });
    }

    fn login(&self) -> Result<String> {
        let url = format!("{}/api/v0/session", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&serde_json::json!({
                "username": self.username,
                "password": self.password,
            }))
            .send()?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(Error::SlskdAuth {
                status: status.as_u16(),
                body,
            });
        }

        let text = resp.text()?;
        let session: SessionResponse = serde_json::from_str(&text)?;
        let mut guard = self.token.lock().unwrap_or_else(|e| return e.into_inner());
        *guard = Some(session.token.clone());
        return Ok(session.token);
    }

    fn cached_or_login(&self) -> Result<String> {
        let cached = self
            .token
            .lock()
            .unwrap_or_else(|e| return e.into_inner())
            .clone();
        if let Some(t) = cached {
            return Ok(t);
        }
        return self.login();
    }

    fn call<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&(impl Serialize + ?Sized)>,
        timeout_override: Option<Duration>,
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let mut retried_login = false;

        loop {
            let token = self.cached_or_login()?;
            let mut req = self.http.request(method.clone(), &url).bearer_auth(&token);
            if let Some(t) = timeout_override {
                req = req.timeout(t);
            }
            if let Some(b) = body {
                req = req.json(b);
            }

            let resp = req.send()?;
            let status = resp.status();

            if status.is_success() {
                let text = resp.text()?;
                if text.trim().is_empty() {
                    return Ok(serde_json::from_value(serde_json::Value::Null)?);
                }
                return Ok(serde_json::from_str(&text)?);
            }

            if status.as_u16() == 401 && !retried_login {
                retried_login = true;
                self.login()?;
                continue;
            }

            let body_text = resp.text().unwrap_or_default();
            if status.as_u16() == 404 {
                return Err(Error::SlskdNotFound {
                    what: path.to_string(),
                });
            }
            return Err(Error::Slskd {
                what: "request",
                status: status.as_u16(),
                body: body_text,
            });
        }
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        return self.call(reqwest::Method::GET, path, None::<&()>, None);
    }

    pub fn get_with_timeout<T: DeserializeOwned>(&self, path: &str, timeout: Duration) -> Result<T> {
        return self.call(reqwest::Method::GET, path, None::<&()>, Some(timeout));
    }

    pub fn post<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: &B) -> Result<T> {
        return self.call(reqwest::Method::POST, path, Some(body), None);
    }

    pub fn post_with_timeout<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
        timeout: Duration,
    ) -> Result<T> {
        return self.call(reqwest::Method::POST, path, Some(body), Some(timeout));
    }

    pub fn delete(&self, path: &str) -> Result<()> {
        return self.call(reqwest::Method::DELETE, path, None::<&()>, None);
    }
}
