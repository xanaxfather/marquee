//! Path → TMDB metadata, with caching.

use crate::cache::{Cache, Lookup};
use crate::matching::{self, normalize};
use crate::parse::{self, EpisodeNo, Guess};
use crate::tmdb::{self, SearchResult, Tmdb, TmdbError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const POSTER_SIZE: &str = "w500";
const BACKDROP_SIZE: &str = "w780";
/// Discord shows the large image at ~80–120 px, so w300 stills are plenty.
const STILL_SIZE: &str = "w300";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Movie,
    Episode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpisodeInfo {
    /// Numbering as in the file name (shown on Discord).
    pub number: EpisodeNo,
    /// TMDB's own (season, episode) when it differs (year seasons, renumbered shows).
    #[serde(default)]
    pub tmdb: Option<(u32, u32)>,
    pub name: Option<String>,
    pub air_date: Option<String>,
    pub still: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Media {
    pub kind: MediaKind,
    pub tmdb_id: u64,
    pub title: String,
    pub year: Option<u16>,
    pub poster: Option<String>,
    #[serde(default)]
    pub backdrop: Option<String>,
    /// Poster of the episode's season (episodes only).
    #[serde(default)]
    pub season_poster: Option<String>,
    pub genres: Vec<String>,
    pub rating: Option<f32>,
    pub tagline: Option<String>,
    pub imdb_id: Option<String>,
    pub episode: Option<EpisodeInfo>,
}

impl Media {
    pub fn tmdb_url(&self) -> String {
        match (&self.kind, &self.episode) {
            (MediaKind::Episode, Some(e)) => {
                let (s, n) = e.tmdb.unwrap_or((e.number.season, e.number.episode));
                format!("https://www.themoviedb.org/tv/{}/season/{s}/episode/{n}", self.tmdb_id)
            }
            (MediaKind::Episode, None) => format!("https://www.themoviedb.org/tv/{}", self.tmdb_id),
            (MediaKind::Movie, _) => format!("https://www.themoviedb.org/movie/{}", self.tmdb_id),
        }
    }

    pub fn imdb_url(&self) -> Option<String> {
        self.imdb_id.as_deref().filter(|i| i.starts_with("tt")).map(|i| format!("https://www.imdb.com/title/{i}/"))
    }
}

/// Cached show-level data shared by all episodes of a series.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Show {
    id: u64,
    name: String,
    year: Option<u16>,
    poster: Option<String>,
    #[serde(default)]
    backdrop: Option<String>,
    #[serde(default)]
    season_posters: std::collections::HashMap<u32, String>,
    /// (season number, first air year)
    #[serde(default)]
    seasons: Vec<(u32, Option<u16>)>,
    genres: Vec<String>,
    rating: Option<f32>,
    imdb_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EpisodeDetails {
    /// Where TMDB actually has it, when that differs from the file name.
    #[serde(default)]
    at: Option<(u32, u32)>,
    name: Option<String>,
    air_date: Option<String>,
    still: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Resolution {
    pub path: String,
    /// Best parse of the filename, used for display when `media` is `None`.
    pub guess: Guess,
    pub media: Option<Media>,
    /// A network error prevented a complete answer; worth retrying.
    pub transient: bool,
    pub error: Option<String>,
}

pub struct Resolver {
    tmdb: Tmdb,
    cache: Cache,
}

fn genres(v: &Value) -> Vec<String> {
    v["genres"]
        .as_array()
        .map(|a| a.iter().filter_map(|g| g["name"].as_str().map(String::from)).take(3).collect())
        .unwrap_or_default()
}

fn rating(v: &Value) -> Option<f32> {
    let votes = v["vote_count"].as_u64().unwrap_or(0);
    v["vote_average"].as_f64().filter(|r| *r > 0.0 && votes >= 5).map(|r| r as f32)
}

fn non_empty(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

/// Extra spellings worth searching for ("S W A T" → "S.W.A.T.", "&" ↔ "and").
/// For `S1940E01`-style files the show must have premiered by that year.
fn premiere_cap(g: &Guess) -> Option<u16> {
    g.episode.filter(|e| e.is_year_season() && g.year.is_none()).map(|e| e.season as u16)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SeasonEpisode {
    number: u32,
    name: Option<String>,
    air_date: Option<String>,
    still: Option<String>,
}

fn year_of_date(d: &Option<String>) -> Option<u16> {
    d.as_deref().and_then(|d| d.get(..4)).and_then(|y| y.parse().ok())
}

/// Episode whose name best matches `title` (≥ `min` similarity).
fn by_title<'a>(eps: &[&'a SeasonEpisode], title: &str, min: f64) -> Option<&'a SeasonEpisode> {
    let q = normalize(title);
    eps.iter()
        .filter_map(|e| Some((*e, matching::similarity(&q, &normalize(e.name.as_deref()?)))))
        .filter(|(_, s)| *s >= min)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(e, _)| e)
}

fn query_variants(title: &str) -> Vec<String> {
    let mut out = vec![title.to_string()];
    let words: Vec<&str> = title.split(' ').collect();
    if words.iter().filter(|w| w.len() == 1 && w.chars().all(char::is_alphabetic)).count() >= 2 {
        let mut s = String::new();
        let mut prev_single = false;
        for w in &words {
            let single = w.len() == 1 && w.chars().all(char::is_alphabetic);
            if single {
                if !prev_single && !s.is_empty() {
                    s.push(' ');
                }
                s.push_str(w);
                s.push('.');
            } else {
                if !s.is_empty() {
                    s.push(' ');
                }
                s.push_str(w);
            }
            prev_single = single;
        }
        out.push(s);
    }
    if title.contains('&') {
        out.push(title.replace('&', "and"));
    } else if title.to_lowercase().contains(" and ") {
        out.push(title.replace(" and ", " & ").replace(" And ", " & "));
    }
    out.dedup();
    out
}

impl Resolver {
    pub fn new(tmdb_key: &str, language: &str) -> Self {
        Self { tmdb: Tmdb::new(tmdb_key, language), cache: Cache::open(crate::paths::cache_file()) }
    }

    pub fn clear_cache(&mut self) {
        let n = self.cache.len();
        self.cache.clear();
        tracing::info!(entries = n, "metadata cache cleared");
    }

    pub fn resolve(&mut self, path: &str) -> Resolution {
        let started = std::time::Instant::now();
        let hyps = parse::hypotheses(path);
        let fallback = hyps.first().cloned().unwrap_or_else(|| parse::display_fallback(path));
        let mut res =
            Resolution { path: path.to_string(), guess: fallback, media: None, transient: false, error: None };

        if !self.tmdb.has_key() {
            res.error = Some("no TMDB API key configured".into());
            tracing::warn!("no TMDB API key configured; showing file names without artwork");
            return res;
        }
        tracing::debug!(path, hypotheses = ?hyps, "resolving");

        for h in &hyps {
            let mut partial = false;
            let result = if h.is_episode() { self.resolve_episode(h, &mut partial) } else { self.resolve_movie(h) };
            match result {
                Ok(Some(media)) => {
                    tracing::info!(
                        path,
                        title = %media.title,
                        year = ?media.year,
                        tmdb_id = media.tmdb_id,
                        episode = ?media.episode.as_ref().map(|e| e.number.label()),
                        episode_name = ?media.episode.as_ref().and_then(|e| e.name.as_deref()),
                        ms = started.elapsed().as_millis() as u64,
                        "identified"
                    );
                    res.guess = h.clone();
                    res.media = Some(media);
                    res.transient = partial;
                    break;
                }
                Ok(None) => {
                    tracing::debug!(title = %h.title, year = ?h.year, "no confident match for hypothesis");
                }
                Err(TmdbError::Auth) => {
                    res.error = Some(TmdbError::Auth.to_string());
                    tracing::error!("TMDB rejected the API key; check tmdb_api_key in the config");
                    break;
                }
                Err(e @ TmdbError::Transient(_)) => {
                    tracing::warn!(path, error = %e, "metadata lookup failed; will retry");
                    res.error = Some(e.to_string());
                    res.transient = true;
                    break;
                }
                Err(e) => {
                    tracing::warn!(path, title = %h.title, error = %e, "metadata lookup error");
                    res.error = Some(e.to_string());
                }
            }
        }
        if res.media.is_none() && !res.transient && res.error.is_none() {
            tracing::info!(path, guess = %res.guess.title, year = ?res.guess.year, "no confident TMDB match; showing parsed title");
        }
        self.cache.flush();
        res
    }

    fn find(&self, g: &Guess, tv: bool) -> Result<Option<SearchResult>, TmdbError> {
        for q in query_variants(&g.title) {
            let years: Vec<Option<u16>> = match g.year {
                Some(y) => vec![Some(y), None],
                None => vec![None],
            };
            for y in years {
                let results = if tv { self.tmdb.search_tv(&q, y)? } else { self.tmdb.search_movie(&q, y)? };
                if results.is_empty() {
                    continue;
                }
                let cands: Vec<_> = results.iter().map(|r| r.candidate.clone()).collect();
                if let Some(best) =
                    matching::best_capped(&g.title, g.year, g.country.as_deref(), premiere_cap(g), &cands)
                {
                    return Ok(Some(results[best.index].clone()));
                }
            }
        }
        Ok(None)
    }

    fn resolve_movie(&mut self, g: &Guess) -> Result<Option<Media>, TmdbError> {
        let key = format!("movie:{}:{}", normalize(&g.title), g.year.map(|y| y.to_string()).unwrap_or_default());
        match self.cache.get::<Media>(&key) {
            Lookup::Hit(m) => {
                tracing::debug!(key, "cache hit");
                return Ok(Some(m));
            }
            Lookup::Negative => return Ok(None),
            Lookup::Miss => {}
        }
        let Some(found) = self.find(g, false)? else {
            self.cache.put::<Media>(&key, None);
            return Ok(None);
        };
        let id = found.candidate.id;
        let media = match self.tmdb.get(&format!("/movie/{id}"), &[]) {
            Ok(d) => Media {
                kind: MediaKind::Movie,
                tmdb_id: id,
                title: non_empty(&d["title"]).unwrap_or(found.title.clone()),
                year: tmdb::year_of(&d["release_date"]).or(found.candidate.year),
                poster: tmdb::image(&d["poster_path"], POSTER_SIZE),
                backdrop: tmdb::image(&d["backdrop_path"], BACKDROP_SIZE),
                season_poster: None,
                genres: genres(&d),
                rating: rating(&d),
                tagline: non_empty(&d["tagline"]),
                imdb_id: non_empty(&d["imdb_id"]),
                episode: None,
            },
            Err(TmdbError::NotFound) | Err(TmdbError::Other(_)) => Media {
                kind: MediaKind::Movie,
                tmdb_id: id,
                title: found.title.clone(),
                year: found.candidate.year,
                poster: found.poster_path.as_ref().map(|p| format!("{}/{POSTER_SIZE}{p}", tmdb::IMAGE_BASE)),
                backdrop: None,
                season_poster: None,
                genres: vec![],
                rating: None,
                tagline: None,
                imdb_id: None,
                episode: None,
            },
            Err(e) => return Err(e),
        };
        self.cache.put(&key, Some(&media));
        Ok(Some(media))
    }

    fn show(&mut self, g: &Guess) -> Result<Option<Show>, TmdbError> {
        let key = format!(
            "tv:{}:{}:{}:{}",
            normalize(&g.title),
            g.year.map(|y| y.to_string()).unwrap_or_default(),
            g.country.as_deref().unwrap_or(""),
            premiere_cap(g).map(|y| y.to_string()).unwrap_or_default()
        );
        match self.cache.get::<Show>(&key) {
            Lookup::Hit(s) => return Ok(Some(s)),
            Lookup::Negative => return Ok(None),
            Lookup::Miss => {}
        }
        let Some(found) = self.find(g, true)? else {
            self.cache.put::<Show>(&key, None);
            return Ok(None);
        };
        let id = found.candidate.id;
        let show = match self.tmdb.get(&format!("/tv/{id}"), &[("append_to_response", "external_ids".into())]) {
            Ok(d) => Show {
                id,
                name: non_empty(&d["name"]).unwrap_or(found.title.clone()),
                year: tmdb::year_of(&d["first_air_date"]).or(found.candidate.year),
                poster: tmdb::image(&d["poster_path"], POSTER_SIZE),
                backdrop: tmdb::image(&d["backdrop_path"], BACKDROP_SIZE),
                season_posters: d["seasons"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| {
                                let n = s["season_number"].as_u64()? as u32;
                                Some((n, tmdb::image(&s["poster_path"], POSTER_SIZE)?))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                seasons: d["seasons"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| Some((s["season_number"].as_u64()? as u32, tmdb::year_of(&s["air_date"]))))
                            .collect()
                    })
                    .unwrap_or_default(),
                genres: genres(&d),
                rating: rating(&d),
                imdb_id: non_empty(&d["external_ids"]["imdb_id"]),
            },
            Err(TmdbError::NotFound) | Err(TmdbError::Other(_)) => Show {
                id,
                name: found.title.clone(),
                year: found.candidate.year,
                poster: found.poster_path.as_ref().map(|p| format!("{}/{POSTER_SIZE}{p}", tmdb::IMAGE_BASE)),
                backdrop: None,
                season_posters: Default::default(),
                seasons: vec![],
                genres: vec![],
                rating: None,
                imdb_id: None,
            },
            Err(e) => return Err(e),
        };
        self.cache.put(&key, Some(&show));
        Ok(Some(show))
    }

    fn resolve_episode(&mut self, g: &Guess, partial: &mut bool) -> Result<Option<Media>, TmdbError> {
        let Some(number) = g.episode else { return Ok(None) };
        let Some(show) = self.show(g)? else { return Ok(None) };

        let key = format!("ep:{}:{}:{}", show.id, number.season, number.episode);
        let details = match self.cache.get::<EpisodeDetails>(&key) {
            Lookup::Hit(d) => Some(d),
            Lookup::Negative => None,
            Lookup::Miss => {
                let found = if number.is_year_season() {
                    self.episode_by_year(&show, number, g.episode_title.as_deref())
                } else {
                    self.episode_direct(&show, number, g.episode_title.as_deref())
                };
                match found {
                    Ok(Some(details)) => {
                        self.cache.put(&key, Some(&details));
                        Some(details)
                    }
                    Ok(None) => {
                        tracing::warn!(show = %show.name, episode = number.label(), "episode not found on TMDB");
                        self.cache.put::<EpisodeDetails>(&key, None);
                        None
                    }
                    Err(e @ TmdbError::Auth) => return Err(e),
                    Err(e) => {
                        tracing::warn!(error = %e, "episode details unavailable; showing series info for now");
                        *partial = true;
                        None
                    }
                }
            }
        };
        let details = details.unwrap_or(EpisodeDetails { at: None, name: None, air_date: None, still: None });
        Ok(Some(Media {
            kind: MediaKind::Episode,
            tmdb_id: show.id,
            title: show.name,
            year: show.year,
            poster: show.poster,
            backdrop: show.backdrop,
            season_poster: show.season_posters.get(&details.at.map_or(number.season, |a| a.0)).cloned(),
            genres: show.genres,
            rating: show.rating,
            tagline: None,
            imdb_id: show.imdb_id,
            episode: Some(EpisodeInfo {
                number,
                tmdb: details.at,
                name: details.name.or_else(|| g.episode_title.clone()),
                air_date: details.air_date,
                still: details.still,
            }),
        }))
    }
}

impl Resolver {
    /// All episodes of one season (cached; also used for title lookups).
    fn season_episodes(&mut self, show_id: u64, season: u32) -> Result<Option<Vec<SeasonEpisode>>, TmdbError> {
        let key = format!("season:{show_id}:{season}");
        match self.cache.get::<Vec<SeasonEpisode>>(&key) {
            Lookup::Hit(v) => return Ok(Some(v)),
            Lookup::Negative => return Ok(None),
            Lookup::Miss => {}
        }
        let eps = match self.tmdb.get(&format!("/tv/{show_id}/season/{season}"), &[]) {
            Ok(d) => d["episodes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|e| {
                            Some(SeasonEpisode {
                                number: e["episode_number"].as_u64()? as u32,
                                name: non_empty(&e["name"]),
                                air_date: non_empty(&e["air_date"]),
                                still: tmdb::image(&e["still_path"], STILL_SIZE),
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            Err(TmdbError::NotFound) => {
                self.cache.put::<Vec<SeasonEpisode>>(&key, None);
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        self.cache.put(&key, Some(&eps));
        Ok(Some(eps))
    }

    fn details(season: u32, e: &SeasonEpisode, moved: bool) -> EpisodeDetails {
        EpisodeDetails {
            at: moved.then_some((season, e.number)),
            name: e.name.clone(),
            air_date: e.air_date.clone(),
            still: e.still.clone(),
        }
    }

    /// Normal numbering; falls back to the episode title when the number
    /// doesn't exist on TMDB (specials, anime, re-cut seasons).
    fn episode_direct(
        &mut self,
        show: &Show,
        number: EpisodeNo,
        title: Option<&str>,
    ) -> Result<Option<EpisodeDetails>, TmdbError> {
        if let Some(eps) = self.season_episodes(show.id, number.season)?
            && let Some(e) = eps.iter().find(|e| e.number == number.episode)
        {
            return Ok(Some(Self::details(number.season, e, false)));
        }
        let Some(title) = title else { return Ok(None) };
        let mut seasons: Vec<u32> = show.seasons.iter().map(|(n, _)| *n).collect();
        seasons.sort_unstable_by_key(|n| (*n == 0, *n)); // specials last
        for s in seasons.into_iter().take(40) {
            if let Some(eps) = self.season_episodes(show.id, s)? {
                let refs: Vec<&SeasonEpisode> = eps.iter().collect();
                if let Some(e) = by_title(&refs, title, 0.85) {
                    tracing::info!(show = %show.name, file = number.label(), tmdb_season = s, tmdb_episode = e.number, "matched episode by title");
                    return Ok(Some(Self::details(s, e, true)));
                }
            }
        }
        Ok(None)
    }

    /// `S1940E01` = the first episode that aired in 1940. TMDB usually keeps
    /// such shows in ordinary seasons spanning several years, so find the
    /// season covering that year, then the episode by title or by position.
    fn episode_by_year(
        &mut self,
        show: &Show,
        number: EpisodeNo,
        title: Option<&str>,
    ) -> Result<Option<EpisodeDetails>, TmdbError> {
        let year = number.season as u16;
        // Some shows really are numbered by year on TMDB.
        if show.seasons.iter().any(|(n, _)| *n == number.season) {
            return self.episode_direct(show, number, title);
        }
        let mut candidates: Vec<u32> = show
            .seasons
            .iter()
            .filter(|(n, start)| *n > 0 && start.is_none_or(|y| y <= year))
            .map(|(n, _)| *n)
            .collect();
        candidates.sort_unstable_by(|a, b| b.cmp(a)); // latest season that started by then first
        if candidates.is_empty() {
            candidates.push(1);
        }
        for s in candidates.into_iter().take(10) {
            let Some(eps) = self.season_episodes(show.id, s)? else { continue };
            let in_year: Vec<&SeasonEpisode> = eps.iter().filter(|e| year_of_date(&e.air_date) == Some(year)).collect();
            if in_year.is_empty() {
                continue;
            }
            let pick = title
                .and_then(|t| by_title(&in_year, t, 0.75))
                .or_else(|| in_year.get(number.episode.saturating_sub(1) as usize).copied());
            if let Some(e) = pick {
                tracing::info!(show = %show.name, file = number.label(), tmdb_season = s, tmdb_episode = e.number, "mapped year season");
                return Ok(Some(Self::details(s, e, true)));
            }
        }
        Ok(None)
    }

    /// Every image TMDB has for the target, grouped for the artwork picker.
    pub fn artwork_options(&self, t: &crate::artwork::Target) -> Result<Value, TmdbError> {
        let lang = t.language.split('-').next().unwrap_or("en").to_string();
        let params = [("include_image_language", format!("{lang},null"))];
        let mut groups: Vec<Value> = Vec::new();
        let mut group = |id: &str,
                         label: &str,
                         aspect: &str,
                         path: String,
                         field: &str,
                         thumb: &str,
                         full: &str|
         -> Result<(), TmdbError> {
            let v = match self.tmdb.get(&path, &params) {
                Ok(v) => v,
                Err(TmdbError::NotFound) => return Ok(()),
                Err(e) => return Err(e),
            };
            let mut seen = std::collections::HashSet::new();
            let items: Vec<Value> = v[field]
                .as_array()
                .map(|a| a.as_slice())
                .unwrap_or(&[])
                .iter()
                .filter_map(|img| {
                    let fp = img["file_path"].as_str()?;
                    seen.insert(fp.to_string()).then(|| {
                        serde_json::json!({
                            "thumb": format!("{}/{thumb}{fp}", tmdb::IMAGE_BASE),
                            "full": format!("{}/{full}{fp}", tmdb::IMAGE_BASE),
                            "lang": img["iso_639_1"].as_str(),
                            "w": img["width"].as_u64(),
                            "h": img["height"].as_u64(),
                        })
                    })
                })
                .take(80)
                .collect();
            groups.push(serde_json::json!({"id": id, "label": label, "aspect": aspect, "items": items}));
            Ok(())
        };
        let id = t.tmdb_id;
        let (season, episode) = match t.tmdb_numbers {
            Some((s, e)) => (Some(s), Some(e)),
            None => (t.season, t.episode),
        };
        match (t.kind, season, episode) {
            (MediaKind::Episode, Some(s), Some(e)) => {
                group(
                    "stills",
                    "Episode",
                    "16/9",
                    format!("/tv/{id}/season/{s}/episode/{e}/images"),
                    "stills",
                    "w300",
                    STILL_SIZE,
                )?;
                group(
                    "season",
                    &format!("Season {s}"),
                    "2/3",
                    format!("/tv/{id}/season/{s}/images"),
                    "posters",
                    "w342",
                    POSTER_SIZE,
                )?;
                group("posters", "Show posters", "2/3", format!("/tv/{id}/images"), "posters", "w342", POSTER_SIZE)?;
                group(
                    "backdrops",
                    "Backdrops",
                    "16/9",
                    format!("/tv/{id}/images"),
                    "backdrops",
                    "w300",
                    BACKDROP_SIZE,
                )?;
            }
            (MediaKind::Episode, _, _) => {
                group("posters", "Posters", "2/3", format!("/tv/{id}/images"), "posters", "w342", POSTER_SIZE)?;
                group(
                    "backdrops",
                    "Backdrops",
                    "16/9",
                    format!("/tv/{id}/images"),
                    "backdrops",
                    "w300",
                    BACKDROP_SIZE,
                )?;
            }
            (MediaKind::Movie, _, _) => {
                group("posters", "Posters", "2/3", format!("/movie/{id}/images"), "posters", "w342", POSTER_SIZE)?;
                group(
                    "backdrops",
                    "Backdrops",
                    "16/9",
                    format!("/movie/{id}/images"),
                    "backdrops",
                    "w300",
                    BACKDROP_SIZE,
                )?;
            }
        }
        Ok(serde_json::json!({ "groups": groups }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants() {
        assert_eq!(query_variants("S W A T"), vec!["S W A T", "S.W.A.T."]);
        assert_eq!(query_variants("Law & Order"), vec!["Law & Order", "Law and Order"]);
        assert_eq!(query_variants("Heat"), vec!["Heat"]);
    }
}
