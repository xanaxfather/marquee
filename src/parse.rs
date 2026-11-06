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
    pub fn label(&self) -> String {
        match self.last {
            Some(l) => format!("S{} - E{}–{}", self.season, self.episode, l),
            None => format!("S{} - E{}", self.season, self.episode),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guess {
    pub title: String,
    pub year: Option<u16>,
    pub episode: Option<EpisodeNo>,
    /// Country hint for shows like "The Office US".
    pub country: Option<String>,
    pub episode_title: Option<String>,
}

impl Guess {
    pub fn is_episode(&self) -> bool {
        self.episode.is_some()
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
    LazyLock::new(|| re(r"(?i)\bS(\d{1,2}) ?[-]? ?E(\d{1,3})(?: ?(?:- ?E?|E)(\d{1,3})\b)?"));
static RE_SEASON_EPISODE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\bSeason ?(\d{1,2})[ ,-]*(?:Episode|Ep) ?(\d{1,3})\b"));
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

/// Parse a single file or folder name.
pub fn parse_name(name: &str) -> Guess {
    let s = clean(strip_video_ext(name));

    if let Some((start, end, ep)) = find_episode(&s) {
        let (title, year, country) = split_show_title(&s[..start]);
        let after = trim_junk(cut_at_tags(trim_junk(&s[end..])));
        let episode_title = (!after.is_empty() && !is_tag(after)).then(|| after.to_string());
        return Guess { title, year, episode: Some(ep), country, episode_title };
    }

    // Movie: the last plausible year preceded by some title text.
    let max_year = current_year() + 1;
    let year_match = RE_YEAR
        .find_iter(&s)
        .filter(|m| m.start() > 0)
        .filter(|m| m.as_str().parse::<u16>().is_ok_and(|y| (1900..=max_year).contains(&y)))
        .last();
    let (title, year) = match year_match {
        Some(m) => (trim_junk(cut_at_tags(&s[..m.start()])).to_string(), m.as_str().parse().ok()),
        None => (trim_junk(cut_at_tags(&s)).to_string(), None),
    };
    Guess { title, year, episode: None, country: None, episode_title: None }
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
    let mut g = parse_name(file);

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
            episode_title: g.episode_title.clone(),
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
            .filter(|d| !is_generic_dir(&clean(d)))
            .map(|d| parse_name(d))
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

    let mut seen = std::collections::HashSet::new();
    out.retain(|g| !g.title.trim().is_empty() && seen.insert((g.title.to_lowercase(), g.year, g.episode)));
    out
}

/// Something presentable when no metadata could be found.
pub fn display_fallback(path: &str) -> Guess {
    hypotheses(path).into_iter().next().unwrap_or_else(|| {
        let name = Path::new(path).file_name().and_then(|f| f.to_str()).unwrap_or(path);
        Guess {
            title: strip_video_ext(name).to_string(),
            year: None,
            episode: None,
            country: None,
            episode_title: None,
        }
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
