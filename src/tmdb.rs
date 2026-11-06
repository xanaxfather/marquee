//! Minimal blocking TMDB v3 client.

use crate::matching::Candidate;
use serde_json::Value;
use std::time::Duration;

const BASE: &str = "https://api.themoviedb.org/3";
pub const IMAGE_BASE: &str = "https://image.tmdb.org/t/p";

#[derive(Debug)]
pub enum TmdbError {
    /// No key configured or TMDB rejected it (401). Not retried.
    Auth,
    NotFound,
    /// Network trouble, timeouts, 5xx and 429 — worth retrying later.
    Transient(String),
    Other(String),
}

impl std::fmt::Display for TmdbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TmdbError::Auth => write!(f, "TMDB rejected the API key (or none is configured)"),
            TmdbError::NotFound => write!(f, "not found on TMDB"),
            TmdbError::Transient(m) => write!(f, "temporary TMDB/network error: {m}"),
            TmdbError::Other(m) => write!(f, "TMDB error: {m}"),
        }
    }
}

impl std::error::Error for TmdbError {}

enum Auth {
    /// v3 API key, sent as `api_key` query parameter.
    Key(String),
    /// v4 read access token, sent as a bearer token.
    Bearer(String),
}

pub struct Tmdb {
    agent: ureq::Agent,
    auth: Option<Auth>,
    language: String,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub candidate: Candidate,
    pub title: String,
    pub poster_path: Option<String>,
}

pub fn year_of(date: &Value) -> Option<u16> {
    date.as_str().and_then(|d| d.get(..4)).and_then(|y| y.parse().ok())
}

pub fn image(path: &Value, size: &str) -> Option<String> {
    path.as_str().filter(|p| p.starts_with('/')).map(|p| format!("{IMAGE_BASE}/{size}{p}"))
}

impl Tmdb {
    pub fn new(key: &str, language: &str) -> Self {
        let key = key.trim();
        let auth = if key.is_empty() {
            None
        } else if key.starts_with("eyJ") || key.len() > 64 {
            Some(Auth::Bearer(key.to_string()))
        } else {
            Some(Auth::Key(key.to_string()))
        };
        let mut builder = ureq::AgentBuilder::new();
        match native_tls::TlsConnector::new() {
            Ok(tls) => builder = builder.tls_connector(std::sync::Arc::new(tls)),
            Err(e) => tracing::error!(error = %e, "TLS unavailable; TMDB lookups will fail"),
        }
        let agent = builder
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .user_agent(concat!("marquee/", env!("CARGO_PKG_VERSION")))
            .build();
        Self { agent, auth, language: language.to_string() }
    }

    pub fn has_key(&self) -> bool {
        self.auth.is_some()
    }

    /// GET with up to three attempts for transient failures.
    pub fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Value, TmdbError> {
        let Some(auth) = &self.auth else { return Err(TmdbError::Auth) };
        let mut delay = Duration::from_millis(400);
        let mut last = TmdbError::Other("no attempt".into());
        for attempt in 1..=3 {
            let started = std::time::Instant::now();
            let mut req = self.agent.get(&format!("{BASE}{path}")).query("language", &self.language);
            for (k, v) in params {
                req = req.query(k, v);
            }
            req = match auth {
                Auth::Key(k) => req.query("api_key", k),
                Auth::Bearer(t) => req.set("Authorization", &format!("Bearer {t}")),
            };
            let result = req.call();
            let ms = started.elapsed().as_millis() as u64;
            match result {
                Ok(resp) => {
                    tracing::debug!(path, ?params, ms, "TMDB request ok");
                    return resp.into_json::<Value>().map_err(|e| TmdbError::Transient(format!("bad JSON: {e}")));
                }
                Err(ureq::Error::Status(401, _)) => return Err(TmdbError::Auth),
                Err(ureq::Error::Status(404, _)) => return Err(TmdbError::NotFound),
                Err(ureq::Error::Status(code, resp)) if code == 429 || code >= 500 => {
                    if let Some(retry) = resp.header("retry-after").and_then(|r| r.parse::<u64>().ok()) {
                        delay = Duration::from_secs(retry.min(10));
                    }
                    tracing::warn!(path, code, attempt, ms, "TMDB request failed; retrying");
                    last = TmdbError::Transient(format!("HTTP {code}"));
                }
                Err(ureq::Error::Status(code, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    return Err(TmdbError::Other(format!(
                        "HTTP {code}: {}",
                        body.chars().take(200).collect::<String>()
                    )));
                }
                Err(ureq::Error::Transport(t)) => {
                    tracing::warn!(path, attempt, ms, error = %t, "TMDB network error");
                    last = TmdbError::Transient(t.to_string());
                }
            }
            if attempt < 3 {
                std::thread::sleep(delay);
                delay *= 2;
            }
        }
        Err(last)
    }

    pub fn search_movie(&self, query: &str, year: Option<u16>) -> Result<Vec<SearchResult>, TmdbError> {
        let mut params = vec![("query", query.to_string()), ("include_adult", "false".into())];
        if let Some(y) = year {
            params.push(("primary_release_year", y.to_string()));
        }
        let v = self.get("/search/movie", &params)?;
        Ok(results(&v, "title", "original_title", "release_date"))
    }

    pub fn search_tv(&self, query: &str, year: Option<u16>) -> Result<Vec<SearchResult>, TmdbError> {
        let mut params = vec![("query", query.to_string()), ("include_adult", "false".into())];
        if let Some(y) = year {
            params.push(("first_air_date_year", y.to_string()));
        }
        let v = self.get("/search/tv", &params)?;
        Ok(results(&v, "name", "original_name", "first_air_date"))
    }
}

fn results(v: &Value, title_key: &str, original_key: &str, date_key: &str) -> Vec<SearchResult> {
    v["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    let id = r["id"].as_u64()?;
                    let title = r[title_key].as_str()?.to_string();
                    let mut titles = vec![title.clone()];
                    if let Some(o) = r[original_key].as_str().filter(|o| *o != title) {
                        titles.push(o.to_string());
                    }
                    let origin_country = r["origin_country"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    Some(SearchResult {
                        candidate: Candidate {
                            id,
                            titles,
                            year: year_of(&r[date_key]),
                            vote_count: r["vote_count"].as_u64().unwrap_or(0) as u32,
                            origin_country,
                        },
                        title,
                        poster_path: r["poster_path"].as_str().map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}
