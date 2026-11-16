//! Turn file paths into title/year/season/episode guesses.
//!
//! The parser is deliberately conservative: it produces an ordered list of
//! hypotheses (file name first, then folder names) and leaves it to the
//! metadata matcher to accept or reject each one.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::LazyLock;

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "wmv", "flv", "webm", "ts", "m2ts", "mts", "mpg", "mpeg", "vob", "ogv", "3gp",
    "rmvb", "divx", "f4v", "asf", "mxf",
];

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "aac", "wav", "ogg", "oga", "opus", "alac", "aiff", "aif", "wma", "ape", "wv", "dsf", "dff",
    "mka", "m4b",
];

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "heic", "bmp", "tiff"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EpisodeNo {
    pub season: u32,
    pub episode: u32,
    /// Last episode for multi-episode files (`S01E01E02`, `S01E01-03`).
    pub last: Option<u32>,
}

impl EpisodeNo {
    /// Year-numbered seasons (`S1940E01`, used for long-running shorts).
    pub fn is_year_season(&self) -> bool {
        (1900..=2100).contains(&self.season)
    }

    pub fn label(&self) -> String {
        if self.is_year_season() && self.episode == 0 {
            return self.season.to_string(); // dated episode not (yet) resolved
        }
        if self.is_year_season() {
            return match self.last {
                Some(l) => format!("{} - E{}–{}", self.season, self.episode, l),
                None => format!("{} - E{}", self.season, self.episode),
            };
        }
        match self.last {
            Some(l) => format!("S{} - E{}–{}", self.season, self.episode, l),
            None => format!("S{} - E{}", self.season, self.episode),
        }
    }
}

/// Database ids written into names by Plex/Jellyfin/Sonarr/Radarr:
/// `{tmdb-603}`, `[tmdbid-603]`, `{imdb-tt0133093}`, `{tvdb-81189}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ids {
    pub tmdb: Option<u64>,
    pub imdb: Option<String>,
    pub tvdb: Option<u64>,
}

impl Ids {
    pub fn is_empty(&self) -> bool {
        self.tmdb.is_none() && self.imdb.is_none() && self.tvdb.is_none()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guess {
    pub title: String,
    pub year: Option<u16>,
    pub episode: Option<EpisodeNo>,
    /// Country hint for shows like "The Office US".
    pub country: Option<String>,
    pub episode_title: Option<String>,
    #[serde(default)]
    pub ids: Ids,
    /// The episode number counts from the start of the show (anime style:
    /// "One Piece - 1071"), not within a season.
    #[serde(default)]
    pub absolute: bool,
    /// Daily shows named by air date: "2024-03-15".
    #[serde(default)]
    pub air_date: Option<String>,
}

impl Guess {
    pub fn is_episode(&self) -> bool {
        self.episode.is_some()
    }

    /// Episode label for display before/without a TMDB match.
    pub fn episode_label(&self) -> Option<String> {
        let ep = self.episode?;
        Some(match (&self.air_date, self.absolute) {
            (Some(d), _) => d.clone(),
            (None, true) => format!("E{}", ep.episode),
            (None, false) => ep.label(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Video,
    Audio,
    Image,
    /// No or unknown extension; IINA can play it, so treat it as video.
    Unknown,
}

pub fn file_kind(path: &str) -> FileKind {
    let ext = Path::new(path).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some(e) if VIDEO_EXTENSIONS.contains(&e) => FileKind::Video,
        Some(e) if AUDIO_EXTENSIONS.contains(&e) => FileKind::Audio,
        Some(e) if IMAGE_EXTENSIONS.contains(&e) => FileKind::Image,
        _ => FileKind::Unknown,
    }
}

fn re(s: &str) -> Regex {
    Regex::new(s).expect("static regex")
}

static RE_SXXEXX: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\bS(\d{1,2}|(?:19|20)\d{2}) ?[-]? ?EP?(\d{1,3})(?:v\d)?(?: ?(?:- ?E?|E)(\d{1,3})\b)?"));
/// "Season 1 Episode 2" and its German/French/Spanish/Italian/Portuguese forms.
static RE_SEASON_EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\b(?:Season|Series|Staffel|Saison|Temporada|Stagione) ?(\d{1,2})[ ,-]*(?:Episode|Episodio|Episódio|Épisode|Folge|Capitulo|Capítulo|Ep) ?(\d{1,3})\b",
    )
});
/// "{tmdb-603}", "[tmdbid-603]", "{imdb-tt0133093}", "[tvdbid=81189]".
static RE_ID_TAG: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)[\[{(]\s*(tmdb|imdb|tvdb)(?:id)?\s*[-=: ]\s*(tt\d+|\d+)\s*[\]})]"));
/// "www.Site.com - " / "[ www.site.org ] - " in front of the real name.
static RE_SITE_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^\s*(?:\[\s*)?www\.[\w-]+\.[a-z]{2,6}\s*\]?[\s._–-]+"));
/// Daily shows: "2024.03.15", "2023-11-02".
static RE_DATE: LazyLock<Regex> =
    LazyLock::new(|| re(r"\b((?:19|20)\d{2})[ -](0[1-9]|1[0-2])[ -](0[1-9]|[12]\d|3[01])\b"));
/// Miniseries: "1of5", "2 of 11".
static RE_N_OF_M: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\b(\d{1,2}) ?of ?(\d{1,2})\b"));
/// "Frieren S2 - 05".
static RE_S_DASH: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bS(\d{1,2}) - (\d{1,3})(?:v\d)?\b"));
/// "Naruto Episode 135", "Ep 135", "Folge 3".
static RE_EP_WORD: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\b(?:Episode|Episodio|Episódio|Épisode|Folge|Capitulo|Capítulo|Ep) ?(\d{1,4})(?:v\d)?\b")
});
/// "Bleach E12", "Show EP03".
static RE_E_NUM: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bEP?(\d{1,4})(?:v\d)?\b"));
/// "Dragon Ball Z #45".
static RE_HASH_NUM: LazyLock<Regex> = LazyLock::new(|| re(r"#(\d{1,4})\b"));
/// "進撃の巨人 第12話".
static RE_CJK_EP: LazyLock<Regex> = LazyLock::new(|| re(r"第 ?(\d{1,4}) ?[話话集回]"));
/// "[Group] Show Name - 12", "One Piece - 1071v2".
static RE_DASH_NUM: LazyLock<Regex> = LazyLock::new(|| re(r"(?:^|\s)- (\d{1,4})(?:v\d)?(?:\s|$)"));
/// Season named inside an anime title: "Jujutsu Kaisen 2nd Season", "Show Season 2", "Show S2".
static RE_TITLE_SEASON: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^(.*\S)\s+(?:(\d)(?:st|nd|rd|th) Season|Season (\d{1,2})|S(\d{1,2}))$"));
/// "2013 - Under the Skin", "1995 Heat".
static RE_LEADING_YEAR: LazyLock<Regex> = LazyLock::new(|| re(r"^((?:19|20)\d{2})\s*(-)?\s*(\S.*)$"));
/// Edition names that aren't part of the title.
static RE_EDITION: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\s+(?:the\s+)?(?:(?:director'?s|final|ultimate|special|collector'?s|theatrical|extended|unrated|(?:\d+th\s+)?anniversary|criterion)\s+(?:cut|edition|version|collection)|remastered|redux|uncut|unrated|extended|imax)$",
    )
});
static RE_GROUP_SUFFIX: LazyLock<Regex> = LazyLock::new(|| re(r"\s+-\w+$"));
static RE_NXNN: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\b(\d{1,2})x(\d{2,3})(?:-(\d{2,3}))?\b"));
static RE_YEAR: LazyLock<Regex> = LazyLock::new(|| re(r"\b((?:19|20)\d{2})\b"));
static RE_TRAILING_YEAR: LazyLock<Regex> = LazyLock::new(|| re(r"^(.*\S)\s+((?:19|20)\d{2})$"));
static RE_TRAILING_COUNTRY: LazyLock<Regex> = LazyLock::new(|| re(r"^(.*\S)\s+(US|UK|AU|NZ|CA|IE)$"));
static RE_SQUARE: LazyLock<Regex> = LazyLock::new(|| re(r"[\[{【]([^\]}】]*)[\]}】]"));
static RE_PAREN: LazyLock<Regex> = LazyLock::new(|| re(r"\(([^()]*)\)"));
static RE_SPACES: LazyLock<Regex> = LazyLock::new(|| re(r"\s+"));
/// Folder that only names a season: "Season 2", "S02", "Series 1", "Staffel 3".
static RE_SEASON_DIR: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^(?:season|series|staffel|saison|temporada|stagione|s)\s?(\d{1,2})(?:\s.*)?$"));
/// Season marker inside a pack folder name: "Breaking.Bad.S02.1080p", "Show Season 2".
static RE_SEASON_MARK: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\b(?:S(\d{1,2})|Season\s?(\d{1,2})|Complete|Series)\b"));
/// File named only by episode number, used with a season folder: "03 - Name", "E03", "Episode 3".
static RE_EP_ONLY: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^(?:e|ep|episode)?\s?(\d{1,3})(?:\b|\s)(.*)$"));
static RE_STRONG_TAG: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)^(?:\d{3,4}[pi]|[xh]\.?26[45]|ddp?\d*|dd\+\d*|e?ac3|aac\d*|dts(?:hd|x)?|truehd|atmos|\d{1,2}bit|hdr(?:10)?(?:plus|\+)?|dovi|4k|uhd|blu-?ray|bdrip|brrip|bdremux|remux|dvdrip|dvdscr|hdrip|hdtv|pdtv|web-?dl|web-?rip|webdl|hdcam|camrip|telesync|xvid|divx|hevc|avc|av1|vp9|yify|yts|rarbg|eztv|ettv)$",
    )
});
/// Ambiguous tags that only count when written in upper case ("WEB" vs "Charlotte's Web").
const WEAK_TAGS: &[&str] = &[
    "WEB",
    "PROPER",
    "REPACK",
    "RERIP",
    "EXTENDED",
    "UNRATED",
    "UNCUT",
    "INTERNAL",
    "LIMITED",
    "DUBBED",
    "SUBBED",
    "MULTI",
    "MULTi",
    "DUAL",
    "NF",
    "AMZN",
    "DSNP",
    "HMAX",
    "ATVP",
    "HULU",
    "PCOK",
    "PMTP",
    "iT",
    "REMASTERED",
    "IMAX",
    "DC",
    "SDR",
    "DV",
    "HC",
    "HDR",
    "x264",
    "x265",
    "FRENCH",
    "TRUEFRENCH",
    "VOSTFR",
    "GERMAN",
    "ITALIAN",
    "SPANISH",
    "LATINO",
    "JAPANESE",
    "KOREAN",
    "HINDI",
    "NORDIC",
    "NORDiC",
    "ITA",
    "ENG",
    "SUBS",
];

/// Language/audio words that follow an episode number but aren't its title.
const LANGUAGE_WORDS: &[&str] = &[
    "french",
    "truefrench",
    "vostfr",
    "german",
    "italian",
    "spanish",
    "latino",
    "castellano",
    "english",
    "japanese",
    "korean",
    "hindi",
    "nordic",
    "ita",
    "eng",
    "ger",
    "fre",
    "spa",
    "jap",
    "multi",
    "dual",
    "audio",
    "dubbed",
    "subbed",
    "subs",
    "dub",
    "sub",
    "end",
    "final",
];

const GENERIC_DIRS: &[&str] = &[
    "movies",
    "movie",
    "films",
    "film",
    "downloads",
    "download",
    "videos",
    "video",
    "desktop",
    "documents",
    "media",
    "tv",
    "tv shows",
    "tvshows",
    "shows",
    "series",
    "torrents",
    "complete",
    "completed",
    "incoming",
    "volumes",
    "users",
    "plex",
    "library",
    "watch",
    "new",
    "temp",
    "tmp",
    "anime",
    "cartoons",
    "documentaries",
    "home",
];

fn current_year() -> u16 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    (1970 + secs / 31_556_952) as u16
}

fn is_junk(c: char) -> bool {
    c.is_whitespace() || "-–—_.,:;([{+~#|".contains(c)
}

fn trim_junk(s: &str) -> &str {
    s.trim_matches(|c: char| is_junk(c) || c == ')' || c == ']')
}

fn strip_video_ext(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty()
                && [VIDEO_EXTENSIONS, AUDIO_EXTENSIONS, IMAGE_EXTENSIONS]
                    .iter()
                    .any(|list| list.iter().any(|e| e.eq_ignore_ascii_case(ext))) =>
        {
            stem
        }
        _ => name,
    }
}

fn is_tag(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    if RE_STRONG_TAG.is_match(&lower) {
        return true;
    }
    // "x264-SPARKS", "WEB-DL" → check the part before the group dash too
    let base = token.split('-').next().unwrap_or(token);
    if base != token && RE_STRONG_TAG.is_match(&base.to_ascii_lowercase()) {
        return true;
    }
    WEAK_TAGS.contains(&base)
}

/// Truncate at the first release tag ("1080p", "BluRay", "x264", …).
fn cut_at_tags(s: &str) -> &str {
    let mut offset = 0;
    for token in s.split(' ') {
        if !token.is_empty() && offset > 0 && is_tag(token) {
            return &s[..offset];
        }
        offset += token.len() + 1;
    }
    s
}

/// Remove brackets/release groups, turn separators into spaces.
fn clean(name: &str) -> String {
    // [Group] / {tags} / [1080p] go away entirely; a bracketed year survives.
    let s = RE_SQUARE.replace_all(name, |c: &regex::Captures| {
        let inner = c[1].trim();
        if RE_YEAR.is_match(inner) && inner.len() == 4 { format!(" {inner} ") } else { " ".into() }
    });
    let s = s.replace(['_', '.'], " ");
    // (2013) → 2013, (1080p BluRay) → removed, (US) → US, (Wo)Man → WoMan
    let s = RE_PAREN.replace_all(&s, |c: &regex::Captures| {
        let inner = c[1].trim();
        if inner.split_whitespace().any(is_tag) { " ".to_string() } else { format!(" {inner} ") }
    });
    let s = s.replace(['(', ')'], " ");
    RE_SPACES.replace_all(s.trim(), " ").into_owned()
}

/// Split "Doctor Who 2005" / "The Office US" into title, year and country.
fn split_show_title(part: &str) -> (String, Option<u16>, Option<String>) {
    let mut title = trim_junk(cut_at_tags(part)).to_string();
    let mut year = None;
    let mut country = None;
    for _ in 0..2 {
        if let Some(c) = RE_TRAILING_YEAR.captures(&title) {
            year = c[2].parse().ok();
            title = trim_junk(&c[1]).to_string();
        }
        if let Some(c) = RE_TRAILING_COUNTRY.captures(&title) {
            country = Some(c[2].to_string());
            title = trim_junk(&c[1]).to_string();
        }
    }
    (title, year, country)
}

fn find_episode(s: &str) -> Option<(usize, usize, EpisodeNo)> {
    let parse = |c: &regex::Captures, i: usize| c.get(i).and_then(|m| m.as_str().parse::<u32>().ok());
    for regex in [&*RE_SXXEXX, &*RE_SEASON_EPISODE, &*RE_NXNN] {
        if let Some(c) = regex.captures(s) {
            let m = c.get(0)?;
            let (season, episode) = (parse(&c, 1)?, parse(&c, 2)?);
            let last = parse(&c, 3).filter(|&l| l > episode && l - episode < 10);
            return Some((m.start(), m.end(), EpisodeNo { season, episode, last }));
        }
    }
    None
}

/// Pull `{tmdb-603}`-style id tags out of a raw name.
fn extract_ids(name: &str) -> (String, Ids) {
    let mut ids = Ids::default();
    for c in RE_ID_TAG.captures_iter(name) {
        let value = &c[2];
        match c[1].to_ascii_lowercase().as_str() {
            "tmdb" => ids.tmdb = value.parse().ok(),
            "tvdb" => ids.tvdb = value.parse().ok(),
            "imdb" if value.starts_with("tt") => ids.imdb = Some(value.to_string()),
            _ => {}
        }
    }
    (RE_ID_TAG.replace_all(name, " ").into_owned(), ids)
}

/// Text after an episode marker → episode title, if it is one.
fn episode_title_from(after: &str) -> Option<String> {
    let mut t = trim_junk(after);
    if t.split(' ').next().is_some_and(is_tag) {
        return None;
    }
    t = trim_junk(cut_at_tags(t));
    let t = RE_GROUP_SUFFIX.replace(t, "");
    let t = trim_junk(&t);
    let all_noise = t.split_whitespace().all(|w| LANGUAGE_WORDS.contains(&w.to_lowercase().as_str()));
    (!t.is_empty() && !all_noise && !is_tag(t)).then(|| t.to_string())
}

fn strip_editions(title: &str) -> String {
    let mut t = title.to_string();
    for _ in 0..2 {
        let stripped = RE_EDITION.replace(&t, "").into_owned();
        // "The Final Cut" is a film, not an edition of "The".
        if matches!(stripped.trim().to_lowercase().as_str(), "" | "the" | "a" | "an") {
            break;
        }
        t = stripped;
    }
    trim_junk(&t).to_string()
}

fn plausible_year(y: u16) -> bool {
    (1900..=current_year() + 1).contains(&y)
}

/// Episode markers without a normal SxxEyy: "Show - 12", "Episode 135",
/// "E12", "#45", "第12話", "S2 - 05". Returns (start, end, season, episode).
fn find_loose_episode(s: &str) -> Option<(usize, usize, Option<u32>, u32)> {
    if let Some(c) = RE_S_DASH.captures(s) {
        let m = c.get(0)?;
        return Some((m.start(), m.end(), c[1].parse().ok(), c[2].parse().ok()?));
    }
    for regex in [&*RE_EP_WORD, &*RE_CJK_EP, &*RE_HASH_NUM, &*RE_E_NUM] {
        if let Some(c) = regex.captures(s) {
            let m = c.get(0)?;
            if m.start() > 0 {
                return Some((m.start(), m.end(), None, c[1].parse().ok()?));
            }
        }
    }
    // "Title - 12": only when nothing but tags or " - Episode Title" follows,
    // and the number isn't a year ("Movie - 2013").
    for c in RE_DASH_NUM.captures_iter(s) {
        let m = c.get(0)?;
        let digits = c.get(1)?;
        let n: u32 = digits.as_str().parse().ok()?;
        if m.start() == 0 || (digits.len() == 4 && plausible_year(n as u16)) {
            continue;
        }
        let rest = s[m.end()..].trim_start();
        let rest_is_noise = rest.is_empty() || rest.starts_with('-') || rest.split(' ').next().is_some_and(is_tag);
        if rest_is_noise {
            return Some((m.start(), m.end(), None, n));
        }
    }
    None
}

/// Every reasonable reading of one file or folder name, best first.
pub fn parse_candidates(name: &str) -> Vec<Guess> {
    let (raw, ids) = extract_ids(strip_video_ext(name));
    let raw = RE_SITE_PREFIX.replace(&raw, "");
    let s = clean(&raw);
    let base = Guess { ids, ..Default::default() };

    // 1. SxxEyy, 1x02, "Season 1 Episode 2" (and translations).
    if let Some((start, end, ep)) = find_episode(&s) {
        let (title, year, country) = split_show_title(&s[..start]);
        let episode_title = episode_title_from(&s[end..]);
        return vec![Guess { title, year, episode: Some(ep), country, episode_title, ..base }];
    }

    // 2. Daily shows by air date.
    if let Some(c) = RE_DATE.captures(&s)
        && let Some(m) = c.get(0).filter(|m| m.start() > 0)
        && let Ok(year) = c[1].parse::<u16>()
        && plausible_year(year)
    {
        let (title, _, country) = split_show_title(&s[..m.start()]);
        if !title.is_empty() {
            return vec![Guess {
                title,
                country,
                // Placeholder until TMDB tells us the real season/episode.
                episode: Some(EpisodeNo { season: year as u32, episode: 0, last: None }),
                episode_title: episode_title_from(&s[m.end()..]),
                air_date: Some(format!("{}-{}-{}", &c[1], &c[2], &c[3])),
                ..base
            }];
        }
    }

    let movies = movie_candidates(&s, &base);
    // An id tag names one thing; never apply it to a reading of the other kind.
    let without_ids = |mut g: Guess| {
        g.ids = Ids::default();
        g
    };

    // 3. Miniseries: "1of5".
    if let Some(c) = RE_N_OF_M.captures(&s)
        && let Some(m) = c.get(0).filter(|m| m.start() > 0)
        && let (Ok(n), Ok(total)) = (c[1].parse::<u32>(), c[2].parse::<u32>())
        && n >= 1
        && n <= total
    {
        let (title, year, country) = split_show_title(&s[..m.start()]);
        if !title.is_empty() {
            let ep = Guess {
                title,
                year,
                country,
                episode: Some(EpisodeNo { season: 1, episode: n, last: None }),
                episode_title: episode_title_from(&s[m.end()..]),
                ..base.clone()
            };
            return std::iter::once(ep).chain(movies.into_iter().map(without_ids)).collect();
        }
    }

    // 4. Anime / absolute numbering.
    if let Some((start, end, season, episode)) = find_loose_episode(&s) {
        let (mut title, year, country) = split_show_title(&s[..start]);
        let mut season = season;
        if season.is_none()
            && let Some(c) = RE_TITLE_SEASON.captures(&title)
        {
            season = c.get(2).or(c.get(3)).or(c.get(4)).and_then(|m| m.as_str().parse().ok());
            title = trim_junk(&c[1]).to_string();
        }
        if !title.is_empty() && episode > 0 {
            let ep = Guess {
                title,
                year,
                country,
                episode: Some(EpisodeNo { season: season.unwrap_or(1), episode, last: None }),
                episode_title: episode_title_from(&s[end..]),
                absolute: season.is_none(),
                ..base.clone()
            };
            // "Star Wars Episode 3 … 2005": a year after the marker means a movie.
            let year_after = RE_YEAR.find(&s[end..]).is_some_and(|m| m.as_str().parse().is_ok_and(plausible_year));
            return if year_after {
                movies.into_iter().chain(std::iter::once(without_ids(ep))).collect()
            } else {
                std::iter::once(ep).chain(movies.into_iter().map(without_ids)).collect()
            };
        }
    }

    movies
}

/// Movie readings of an already-cleaned name.
fn movie_candidates(s: &str, base: &Guess) -> Vec<Guess> {
    let movie = |title: &str, year: Option<u16>| Guess { title: title.to_string(), year, ..base.clone() };
    // The last plausible year preceded by some title text.
    let year_match = RE_YEAR
        .find_iter(s)
        .filter(|m| m.start() > 0)
        .filter(|m| m.as_str().parse::<u16>().is_ok_and(plausible_year))
        .last();
    let (full, year) = match year_match {
        Some(m) => (trim_junk(cut_at_tags(&s[..m.start()])).to_string(), m.as_str().parse().ok()),
        None => (trim_junk(cut_at_tags(s)).to_string(), None),
    };
    let stripped = strip_editions(&full);
    let mut out = vec![movie(&stripped, year)];
    if stripped != full {
        out.push(movie(&full, year)); // "The Final Cut" might be the actual title
    }
    // "2013 - Under the Skin" / "(1995) Heat"
    if year.is_none()
        && let Some(c) = RE_LEADING_YEAR.captures(s)
        && let Ok(y) = c[1].parse::<u16>()
        && plausible_year(y)
    {
        let rest = strip_editions(trim_junk(cut_at_tags(&c[3])));
        if !rest.is_empty() && !is_tag(&rest) {
            let lead = movie(&rest, Some(y));
            // An explicit dash makes the intent clear; otherwise "2001 A Space Odyssey" goes first.
            if c.get(2).is_some() { out.insert(0, lead) } else { out.push(lead) }
        }
    }
    out
}

/// The best reading of a single file or folder name.
pub fn parse_name(name: &str) -> Guess {
    parse_candidates(name).into_iter().next().unwrap_or_default()
}

fn is_generic_dir(name: &str) -> bool {
    let lower = name.trim().to_lowercase();
    lower.is_empty()
        || GENERIC_DIRS.contains(&lower.as_str())
        || lower.starts_with("disc")
        || lower.starts_with("disk")
        || lower.starts_with("cd ")
        || std::env::var("USER").is_ok_and(|u| u.eq_ignore_ascii_case(&lower))
}

fn season_from_dir(dir: &str) -> Option<u32> {
    let cleaned = clean(dir);
    if cleaned.eq_ignore_ascii_case("specials") || cleaned.eq_ignore_ascii_case("special") {
        return Some(0);
    }
    if let Some(c) = RE_SEASON_DIR.captures(&cleaned) {
        return c[1].parse().ok();
    }
    RE_SEASON_MARK.captures(&cleaned).and_then(|c| c.get(1).or(c.get(2))).and_then(|m| m.as_str().parse().ok())
}

/// Show name from the nearest meaningful folder ("Breaking Bad/Season 2/…",
/// "Breaking.Bad.S02.1080p.BluRay/…").
fn show_from_dirs(dirs: &[String]) -> Option<(String, Option<u16>, Option<String>)> {
    for dir in dirs {
        let cleaned = clean(dir);
        if RE_SEASON_DIR.is_match(&cleaned) || cleaned.eq_ignore_ascii_case("specials") {
            continue;
        }
        if is_generic_dir(&cleaned) {
            return None;
        }
        let head = match RE_SEASON_MARK.find(&cleaned) {
            Some(m) => &cleaned[..m.start()],
            None => &cleaned,
        };
        let (title, year, country) = split_show_title(head);
        if !title.is_empty() && !is_generic_dir(&title) {
            return Some((title, year, country));
        }
    }
    None
}

/// Ordered hypotheses for a full path. Earlier entries are tried first.
pub fn hypotheses(path: &str) -> Vec<Guess> {
    let p = Path::new(path);
    let Some(file) = p.file_name().and_then(|f| f.to_str()) else {
        return vec![];
    };
    let dirs: Vec<String> = p
        .ancestors()
        .skip(1)
        .filter_map(|a| a.file_name().and_then(|n| n.to_str()).map(String::from))
        .take(3)
        .collect();

    let mut out: Vec<Guess> = Vec::new();
    let mut candidates = parse_candidates(file).into_iter();
    let mut g = candidates.next().unwrap_or_default();
    let alternates: Vec<Guess> = candidates.collect();

    // Plex/Jellyfin put the id tag on the folder: "The Matrix (1999) {tmdb-603}/…"
    let folder_ids = dirs.iter().map(|d| extract_ids(d).1).find(|i| !i.is_empty()).unwrap_or_default();
    if g.ids.is_empty() {
        g.ids = folder_ids.clone();
    }
    // "Show/Season 2/Show - 03.mkv": the folder supplies the season.
    if g.absolute
        && let (Some(ep), Some(season)) = (g.episode.as_mut(), dirs.first().and_then(|d| season_from_dir(d)))
    {
        ep.season = season;
        g.absolute = false;
    }

    if g.episode.is_none() {
        // "Breaking Bad/Season 2/03 - Bit by a Fly.mkv"
        if let Some(season) = dirs.first().and_then(|d| season_from_dir(d)) {
            let stem = clean(strip_video_ext(file));
            if let Some(c) = RE_EP_ONLY.captures(&stem)
                && let (Ok(episode), Some((title, year, country))) = (c[1].parse::<u32>(), show_from_dirs(&dirs))
            {
                let rest = trim_junk(cut_at_tags(trim_junk(&c[2])));
                out.push(Guess {
                    title,
                    year,
                    country,
                    episode: Some(EpisodeNo { season, episode, last: None }),
                    episode_title: (!rest.is_empty()).then(|| rest.to_string()),
                    ids: folder_ids.clone(),
                    ..Default::default()
                });
            }
        }
    }

    if let Some(ep) = g.episode {
        let folder_show = show_from_dirs(&dirs);
        if (g.title.is_empty() || is_generic_dir(&g.title))
            && let Some((title, year, country)) = folder_show.clone()
        {
            g.title = title;
            g.year = g.year.or(year);
            g.country = g.country.or(country);
        }
        // In a "Show/Season N/" library layout the folder name is curated and
        // beats abbreviated file names ("bb.s02e03.mkv"); otherwise trust the file.
        let library_layout = dirs.first().is_some_and(|d| RE_SEASON_DIR.is_match(&clean(d)));
        let folder_guess = folder_show.map(|(title, year, country)| Guess {
            title,
            year: year.or(g.year),
            country,
            episode: Some(ep),
            ..g.clone()
        });
        let file_guess = (!g.title.is_empty()).then(|| g.clone());
        if library_layout {
            out.extend(folder_guess.into_iter().chain(file_guess));
        } else {
            out.extend(file_guess.into_iter().chain(folder_guess));
        }
    } else if out.is_empty() {
        let parent = dirs
            .first()
            .filter(|d| !is_generic_dir(&clean(&extract_ids(d).0)))
            .map(|d| Guess { ids: g.ids.clone(), ..parse_name(d) })
            .filter(|pg| pg.episode.is_none() && pg.year.is_some() && !pg.title.is_empty());
        match parent {
            // Scene releases: "Under.the.Skin.2013.1080p.BluRay-GRP/grp-uts-1080p.mkv"
            Some(pg) if g.year.is_none() => {
                out.push(pg);
                out.push(g.clone());
            }
            Some(pg) => {
                out.push(g.clone());
                out.push(pg);
            }
            None => out.push(g.clone()),
        }
        // "Wonder.Woman.1984.1080p" — the year may be part of the title.
        if let Some(y) = g.year {
            out.push(Guess { title: format!("{} {}", g.title, y), year: None, ..g.clone() });
        }
    }

    out.extend(alternates.into_iter().map(|mut a| {
        if a.ids.is_empty() {
            a.ids = folder_ids.clone();
        }
        a
    }));

    let mut seen = std::collections::HashSet::new();
    out.retain(|g| !g.title.trim().is_empty() && seen.insert((g.title.to_lowercase(), g.year, g.episode)));
    out
}

/// Something presentable when no metadata could be found.
pub fn display_fallback(path: &str) -> Guess {
    hypotheses(path).into_iter().next().unwrap_or_else(|| {
        let name = Path::new(path).file_name().and_then(|f| f.to_str()).unwrap_or(path);
        Guess { title: strip_video_ext(name).to_string(), ..Default::default() }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(season: u32, episode: u32) -> Option<EpisodeNo> {
        Some(EpisodeNo { season, episode, last: None })
    }

    fn first(path: &str) -> Guess {
        hypotheses(path).into_iter().next().expect("a hypothesis")
    }

    #[test]
    fn movie_with_year_in_parens() {
        let g = parse_name("Under THe Skin (2013).mp4");
        assert_eq!(g.title, "Under THe Skin");
        assert_eq!(g.year, Some(2013));
        assert!(g.episode.is_none());
    }

    #[test]
    fn scene_movie() {
        let g = parse_name("Blade.Runner.2049.2017.2160p.UHD.BluRay.x265-TERMiNAL.mkv");
        assert_eq!((g.title.as_str(), g.year), ("Blade Runner 2049", Some(2017)));
        let g = parse_name("Blade.Runner.2049.1080p.WEB-DL.mkv");
        assert_eq!((g.title.as_str(), g.year), ("Blade Runner 2049", None));
        let g = parse_name("2001.A.Space.Odyssey.1968.REMASTERED.1080p.mkv");
        assert_eq!((g.title.as_str(), g.year), ("2001 A Space Odyssey", Some(1968)));
        let g = parse_name("1917.mkv");
        assert_eq!((g.title.as_str(), g.year), ("1917", None));
        let g = parse_name("[YTS.MX] The Matrix (1999) [1080p] [BluRay].mp4");
        assert_eq!((g.title.as_str(), g.year), ("The Matrix", Some(1999)));
        let g = parse_name("Charlotte's Web.mkv");
        assert_eq!(g.title, "Charlotte's Web");
        let g = parse_name("the_dark_knight_[2008]_bdrip.avi");
        assert_eq!((g.title.as_str(), g.year), ("the dark knight", Some(2008)));
    }

    #[test]
    fn wonder_woman_alternative() {
        let hs = hypotheses("/Users/x/Movies/Wonder.Woman.1984.1080p.mkv");
        assert_eq!(hs[0].title, "Wonder Woman");
        assert!(hs.iter().any(|h| h.title == "Wonder Woman 1984" && h.year.is_none()));
    }

    #[test]
    fn tv_basic() {
        let g = parse_name("Breaking.Bad.S02E03.mkv");
        assert_eq!(g.title, "Breaking Bad");
        assert_eq!(g.episode, ep(2, 3));
        let g = parse_name("breaking bad s2e3 720p hdtv.mkv");
        assert_eq!((g.title.as_str(), g.episode), ("breaking bad", ep(2, 3)));
        let g = parse_name("Breaking Bad - 2x03 - Bit by a Fly.avi");
        assert_eq!((g.title.as_str(), g.episode), ("Breaking Bad", ep(2, 3)));
        assert_eq!(g.episode_title.as_deref(), Some("Bit by a Fly"));
        let g = parse_name("Breaking Bad Season 2 Episode 3.mp4");
        assert_eq!((g.title.as_str(), g.episode), ("Breaking Bad", ep(2, 3)));
    }

    #[test]
    fn tv_episode_title_and_tags() {
        let g = parse_name("American Horror Stories - S01E01 - Rubber (Wo)Man Part 1.mkv");
        assert_eq!(g.title, "American Horror Stories");
        assert_eq!(g.episode, ep(1, 1));
        assert!(g.episode_title.unwrap().starts_with("Rubber"));
        let g = parse_name("The.Office.US.S05E14.Lecture.Circuit.Part.1.720p.WEB-DL.mkv");
        assert_eq!(g.title, "The Office");
        assert_eq!(g.country.as_deref(), Some("US"));
        assert_eq!(g.episode_title.as_deref(), Some("Lecture Circuit Part 1"));
        let g = parse_name("Doctor.Who.2005.S01E01.Rose.mkv");
        assert_eq!((g.title.as_str(), g.year), ("Doctor Who", Some(2005)));
        let g = parse_name("[SubsPlease] Frieren S01E05 (1080p) [ABCD1234].mkv");
        assert_eq!((g.title.as_str(), g.episode), ("Frieren", ep(1, 5)));
    }

    #[test]
    fn year_in_parens_before_episode() {
        let g = parse_name("Slasher (2016) - S01E08 - Soon Your Own Eyes Will See (1080p BluRay x265 RCVR).mkv");
        assert_eq!((g.title.as_str(), g.year, g.episode), ("Slasher", Some(2016), ep(1, 8)));
        assert_eq!(g.episode_title.as_deref(), Some("Soon Your Own Eyes Will See"));
    }

    #[test]
    fn year_seasons() {
        let g = parse_name("Tom and Jerry - S1940E01 - Puss Gets The Boot.mkv");
        assert_eq!(g.title, "Tom and Jerry");
        assert_eq!(g.episode, ep(1940, 1));
        assert_eq!(g.episode_title.as_deref(), Some("Puss Gets The Boot"));
        assert_eq!(g.episode.unwrap().label(), "1940 - E1");
        assert!(g.year.is_none());
    }

    fn abs(episode: u32) -> (Option<EpisodeNo>, bool) {
        (ep(1, episode), true)
    }

    #[test]
    fn anime_and_absolute_numbering() {
        for (name, title, episode) in [
            ("[SubsPlease] Sousou no Frieren - 12 (1080p) [ABCD1234].mkv", "Sousou no Frieren", 12),
            ("One Piece - 1071v2 [1080p].mkv", "One Piece", 1071),
            ("Naruto Episode 135.mkv", "Naruto", 135),
            ("Naruto Ep. 135 - Title Here.mkv", "Naruto", 135),
            ("Bleach E12.mkv", "Bleach", 12),
            ("Dragon Ball Z #45.mkv", "Dragon Ball Z", 45),
            ("進撃の巨人 第12話.mkv", "進撃の巨人", 12),
        ] {
            let g = parse_name(name);
            assert_eq!((g.title.as_str(), (g.episode, g.absolute)), (title, abs(episode)), "{name}");
        }
        // A season in the title or before the dash is a real season, not absolute.
        let g = parse_name("[Erai-raws] Jujutsu Kaisen 2nd Season - 05 [1080p][Multiple Subtitle].mkv");
        assert_eq!((g.title.as_str(), g.episode, g.absolute), ("Jujutsu Kaisen", ep(2, 5), false));
        let g = parse_name("Frieren S2 - 05.mkv");
        assert_eq!((g.title.as_str(), g.episode, g.absolute), ("Frieren", ep(2, 5), false));
        // The season folder wins over "absolute".
        let g = first("/x/Anime/Frieren/Season 2/Frieren - 05.mkv");
        assert_eq!((g.episode, g.absolute), (ep(2, 5), false));
        // Not episodes: a year after a dash, or a number that starts a title.
        assert!(parse_name("Blade Runner - 1982.mkv").episode.is_none());
        assert!(parse_name("Movie - 12 Angry Men.mkv").episode.is_none());
    }

    #[test]
    fn movie_named_like_an_episode_stays_a_movie_first() {
        let c = parse_candidates("Star Wars Episode 3 Revenge of the Sith 2005 1080p.mkv");
        assert!(c[0].episode.is_none());
        assert_eq!(c[0].year, Some(2005));
        assert!(c.iter().any(|g| g.absolute));
    }

    #[test]
    fn dated_and_miniseries() {
        let g = parse_name("The.Daily.Show.2024.03.14.Guest.Name.1080p.WEB.h264.mkv");
        assert_eq!(g.title, "The Daily Show");
        assert_eq!(g.air_date.as_deref(), Some("2024-03-14"));
        assert_eq!(g.episode_label().as_deref(), Some("2024-03-14"));
        assert_eq!(parse_name("Jeopardy! 2023-11-02.mp4").air_date.as_deref(), Some("2023-11-02"));
        // A year followed by non-date numbers is still a movie year.
        assert!(parse_name("Movie.2019.1080p.mkv").air_date.is_none());

        let g = parse_name("Chernobyl 1of5 1-23-45.mkv");
        assert_eq!((g.title.as_str(), g.episode, g.absolute), ("Chernobyl", ep(1, 1), false));
        let g = parse_name("Planet Earth (1 of 11) From Pole to Pole.mkv");
        assert_eq!((g.episode, g.episode_title.as_deref()), (ep(1, 1), Some("From Pole to Pole")));
    }

    #[test]
    fn other_languages_and_ep_spelling() {
        for (name, title, s, e) in [
            ("Dark.Staffel.1.Folge.3.German.1080p.mkv", "Dark", 1, 3),
            ("La Casa de Papel Temporada 1 Capitulo 2.mkv", "La Casa de Papel", 1, 2),
            ("Lupin.Saison.1.Episode.2.FRENCH.720p.mkv", "Lupin", 1, 2),
            ("Severance.S01.EP02.mkv", "Severance", 1, 2),
            ("Show.Name.S01E01v2.mkv", "Show Name", 1, 1),
        ] {
            let g = parse_name(name);
            assert_eq!((g.title.as_str(), g.episode), (title, ep(s, e)), "{name}");
            assert!(g.episode_title.is_none(), "{name}: {:?}", g.episode_title);
        }
    }

    #[test]
    fn id_tags() {
        let g = parse_name("The Matrix (1999) {tmdb-603}.mkv");
        assert_eq!((g.title.as_str(), g.year, g.ids.tmdb), ("The Matrix", Some(1999), Some(603)));
        assert_eq!(parse_name("Inception [tmdbid-27205].mkv").ids.tmdb, Some(27205));
        assert_eq!(parse_name("Heat {imdb-tt0113277}.mkv").ids.imdb.as_deref(), Some("tt0113277"));
        let g = parse_name(
            "Breaking Bad (2008) {tvdb-81189} - S02E03 - Bit by a Dead Bee [WEBDL-1080p][AAC 2.0][x264]-GRP.mkv",
        );
        assert_eq!(
            (g.title.as_str(), g.year, g.episode, g.ids.tvdb),
            ("Breaking Bad", Some(2008), ep(2, 3), Some(81189))
        );
        assert_eq!(g.episode_title.as_deref(), Some("Bit by a Dead Bee"));
        // Plex/Jellyfin tag the folder.
        let g = first("/x/Movies/The Matrix (1999) {tmdb-603}/The Matrix (1999).mkv");
        assert_eq!(g.ids.tmdb, Some(603));
        let g = first("/x/TV/Breaking Bad (2008) {tvdb-81189}/Season 02/S02E03.mkv");
        assert_eq!((g.title.as_str(), g.ids.tvdb), ("Breaking Bad", Some(81189)));
    }

    #[test]
    fn movie_variations() {
        let c = parse_candidates("2013 - Under the Skin.mkv");
        assert_eq!((c[0].title.as_str(), c[0].year), ("Under the Skin", Some(2013)));
        let c = parse_candidates("(1995) Heat.mkv");
        assert!(c.iter().any(|g| g.title == "Heat" && g.year == Some(1995)));
        // "2001 A Space Odyssey" keeps its number first.
        assert_eq!(parse_name("2001 A Space Odyssey.mkv").title, "2001 A Space Odyssey");

        let g = parse_name("www.Torrenting.com - Oppenheimer.2023.1080p.BluRay.x264.mkv");
        assert_eq!((g.title.as_str(), g.year), ("Oppenheimer", Some(2023)));
        let g = parse_name("[ www.UsaBit.com ] - Gravity 2013 720p.mkv");
        assert_eq!((g.title.as_str(), g.year), ("Gravity", Some(2013)));

        let c = parse_candidates("Blade.Runner.The.Final.Cut.1982.1080p.mkv");
        assert_eq!((c[0].title.as_str(), c[0].year), ("Blade Runner", Some(1982)));
        assert!(c.iter().any(|g| g.title == "Blade Runner The Final Cut"));
        assert_eq!(parse_name("Aliens.Directors.Cut.1080p.BluRay.mkv").title, "Aliens");
        assert_eq!(parse_name("Apocalypse Now Redux (1979).mkv").title, "Apocalypse Now");
        // Edition words inside a title are left alone.
        assert_eq!(parse_name("Uncut Gems (2019).mkv").title, "Uncut Gems");
        assert_eq!(parse_name("The Final Cut (2004).mkv").title, "The Final Cut");

        let g = parse_name("Amelie.2001.FRENCH.1080p.BluRay.mkv");
        assert_eq!((g.title.as_str(), g.year), ("Amelie", Some(2001)));
        assert_eq!(parse_name("Parasite.KOREAN.1080p.BluRay.mkv").title, "Parasite");
    }

    #[test]
    fn multi_episode() {
        let g = parse_name("Friends.S01E16E17.mkv");
        assert_eq!(g.episode, Some(EpisodeNo { season: 1, episode: 16, last: Some(17) }));
        let g = parse_name("Friends S01E16-E17.mkv");
        assert_eq!(g.episode.unwrap().last, Some(17));
        let g = parse_name("Show S01E01 720p.mkv");
        assert_eq!(g.episode.unwrap().last, None);
    }

    #[test]
    fn folders_supply_show_and_season() {
        let g = first("/Volumes/Media/TV/Breaking Bad/Season 2/03 - Bit by a Fly.mkv");
        assert_eq!((g.title.as_str(), g.episode), ("Breaking Bad", ep(2, 3)));
        let g = first("/Volumes/Media/Breaking Bad (2008)/Season 02/S02E03.mkv");
        assert_eq!((g.title.as_str(), g.year, g.episode), ("Breaking Bad", Some(2008), ep(2, 3)));
        let g = first("/Users/x/Downloads/Breaking.Bad.S02.1080p.BluRay.x264/E03.mkv");
        assert_eq!((g.title.as_str(), g.episode), ("Breaking Bad", ep(2, 3)));
        let hs = hypotheses("/Users/x/TV/Breaking Bad/Season 2/bb.s02e03.mkv");
        assert_eq!(hs[0].title, "Breaking Bad");
        assert_eq!(hs[1].title, "bb");
        let hs = hypotheses("/Users/x/Downloads/Some.Pack/bb.s02e03.mkv");
        assert_eq!(hs[0].title, "bb");
    }

    #[test]
    fn scene_folder_beats_garbage_file_name() {
        let hs = hypotheses("/Users/x/Downloads/Under.the.Skin.2013.1080p.BluRay.x264-SPARKS/sparks-uts-1080p.mkv");
        assert_eq!((hs[0].title.as_str(), hs[0].year), ("Under the Skin", Some(2013)));
        // Generic folders never become titles.
        let hs = hypotheses("/Users/x/Movies/Heat (1995).mkv");
        assert_eq!(hs[0].title, "Heat");
        assert!(hs.iter().all(|h| h.title != "Movies"));
    }

    #[test]
    fn file_kinds() {
        assert_eq!(file_kind("/a/b.MKV"), FileKind::Video);
        assert_eq!(file_kind("/a/b.flac"), FileKind::Audio);
        assert_eq!(file_kind("/a/b"), FileKind::Unknown);
    }
}
