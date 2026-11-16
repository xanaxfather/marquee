//! Metadata stored with the video itself, read before falling back to the
//! file name:
//!
//! - Kodi / Jellyfin / Plex `.nfo` files next to the video (`movie.nfo`,
//!   `<name>.nfo`, `tvshow.nfo` one or two folders up), including the TMDB,
//!   IMDb or TVDB id they usually carry.
//! - MP4 / M4V / MOV iTunes tags (`tvsh`, `tvsn`, `tves`, `©nam`, `©day`).
//! - Matroska tags (`TITLE`, `PART_NUMBER` per target level, `TMDB`, `IMDB`)
//!   and the segment title.
//!
//! Only small parts of the file are read (headers and tag blocks), never
//! the video data.

use crate::parse::{EpisodeNo, Guess, Ids};
use regex::Regex;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// What was found for one file.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Embedded {
    /// Structured readings (show + season + episode, or a tagged movie):
    /// tried before anything parsed from the file name.
    pub strong: Vec<Guess>,
    /// Loose readings (a container title that is just another release name):
    /// tried after the file name.
    pub weak: Vec<Guess>,
    /// Ids from `tvshow.nfo`, for episode guesses that have none.
    pub show_ids: Ids,
    /// Ids for the movie (from its `.nfo` or container tags).
    pub movie_ids: Ids,
}

pub fn read(path: &str) -> Embedded {
    let p = Path::new(path);
    if path.contains("://") || !p.is_file() {
        return Embedded::default();
    }
    let mut out = Embedded::default();
    nfo(p, &mut out);
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let tags = match ext.as_str() {
        "mp4" | "m4v" | "mov" => mp4_tags(p),
        "mkv" | "webm" | "mk3d" => mkv_tags(p),
        _ => None,
    };
    if let Some(t) = tags {
        t.apply(&mut out);
    }
    if !out.strong.is_empty() || !out.weak.is_empty() || !out.show_ids.is_empty() || !out.movie_ids.is_empty() {
        tracing::debug!(path, embedded = ?out, "embedded metadata");
    }
    out
}

// ------------------------------------------------------------------ tags

/// Container tags, normalised across formats.
#[derive(Debug, Default)]
struct Tags {
    show: Option<String>,
    season: Option<u32>,
    episode: Option<u32>,
    /// Episode title for episodes, the title for movies.
    title: Option<String>,
    year: Option<u16>,
    /// The container says it's a movie / an episode.
    movie: Option<bool>,
    ids: Ids,
    /// `TMDB` tag kind ("movie" / "tv").
    tmdb_kind: Option<String>,
    /// Matroska segment title (often a release name).
    segment_title: Option<String>,
}

impl Tags {
    fn apply(self, out: &mut Embedded) {
        let clean = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        let (show, title) = (clean(&self.show), clean(&self.title));
        if let (Some(show), Some(episode)) = (show.clone(), self.episode) {
            let mut ids = Ids::default();
            if self.tmdb_kind.as_deref() == Some("tv") {
                ids.tmdb = self.ids.tmdb;
            }
            out.strong.push(Guess {
                title: show,
                episode: Some(EpisodeNo { season: self.season.unwrap_or(1), episode, last: None }),
                episode_title: title.clone(),
                ids,
                ..Default::default()
            });
        } else if let Some(t) = title.clone().filter(|_| {
            self.movie == Some(true) || self.tmdb_kind.as_deref() == Some("movie") || self.ids.imdb.is_some()
        }) {
            let mut ids = self.ids.clone();
            if self.tmdb_kind.as_deref() == Some("tv") {
                ids.tmdb = None;
            }
            out.movie_ids = ids.clone();
            out.strong.push(Guess { title: t, year: self.year, ids, ..Default::default() });
        } else if let Some(t) = title {
            // A bare title tag: often the release name. Parse it like a file name.
            out.weak.extend(crate::parse::parse_candidates(&t).into_iter().take(2));
        }
        if let Some(seg) = clean(&self.segment_title) {
            out.weak.extend(crate::parse::parse_candidates(&seg).into_iter().take(2));
        }
        out.weak.retain(|g| !g.title.trim().is_empty());
    }
}

/// `TMDB` tag values look like "movie/603" or "tv/1396".
fn tmdb_tag(v: &str, t: &mut Tags) {
    let v = v.trim();
    match v.split_once('/') {
        Some((kind, id)) => {
            t.tmdb_kind = Some(kind.to_ascii_lowercase());
            t.ids.tmdb = id.trim().parse().ok();
        }
        None => t.ids.tmdb = v.parse().ok(),
    }
}

fn year_of(s: &str) -> Option<u16> {
    s.get(..4).and_then(|y| y.parse().ok()).filter(|y| (1880..=2100).contains(y))
}

fn read_at(f: &mut File, pos: u64, len: usize) -> Option<Vec<u8>> {
    f.seek(SeekFrom::Start(pos)).ok()?;
    let mut buf = vec![0; len];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

// ------------------------------------------------------------------ MP4

const MAX_BOX: u64 = 4 << 20;

/// (type, payload start, payload len) of the boxes in `[start, end)`.
fn mp4_boxes(f: &mut File, start: u64, end: u64) -> Vec<([u8; 4], u64, u64)> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos + 8 <= end && out.len() < 512 {
        let Some(h) = read_at(f, pos, 8) else { break };
        let mut size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let kind = [h[4], h[5], h[6], h[7]];
        let mut header = 8;
        if size == 1 {
            let Some(x) = read_at(f, pos + 8, 8) else { break };
            size = u64::from_be_bytes(x.try_into().ok().unwrap_or([0; 8]));
            header = 16;
        } else if size == 0 {
            size = end - pos;
        }
        if size < header || pos + size > end {
            break;
        }
        out.push((kind, pos + header, size - header));
        pos += size;
    }
    out
}

fn mp4_tags(p: &Path) -> Option<Tags> {
    let mut f = File::open(p).ok()?;
    let len = f.metadata().ok()?.len();
    let find = |f: &mut File, s: u64, e: u64, k: &[u8; 4]| mp4_boxes(f, s, e).into_iter().find(|b| &b.0 == k);
    let (_, ms, ml) = find(&mut f, 0, len, b"moov")?;
    let (_, us, ul) = find(&mut f, ms, ms + ml, b"udta")?;
    let (_, mts, mtl) = find(&mut f, us, us + ul, b"meta")?;
    // `meta` is a full box (4 bytes of version/flags) in MP4, a plain one in QuickTime.
    let peek = read_at(&mut f, mts, 8)?;
    let skip = if &peek[4..8] == b"hdlr" || &peek[4..8] == b"ilst" { 0 } else { 4 };
    let (_, is, il) = find(&mut f, mts + skip, mts + mtl, b"ilst")?;
    if il > MAX_BOX {
        return None;
    }
    let mut t = Tags::default();
    for (kind, s, l) in mp4_boxes(&mut f, is, is + il) {
        let Some((_, ds, dl)) = find(&mut f, s, s + l, b"data") else { continue };
        if !(8..=64 * 1024).contains(&dl) {
            continue;
        }
        let Some(data) = read_at(&mut f, ds + 8, (dl - 8) as usize) else { continue };
        let text = || String::from_utf8_lossy(&data).trim().to_string();
        let int = || match data.len() {
            1 => Some(data[0] as u32),
            2 => Some(u16::from_be_bytes([data[0], data[1]]) as u32),
            4 => Some(u32::from_be_bytes([data[0], data[1], data[2], data[3]])),
            8 => Some(u64::from_be_bytes(data[..8].try_into().ok()?) as u32),
            _ => None,
        };
        match &kind {
            b"\xa9nam" => t.title = Some(text()),
            b"tvsh" => t.show = Some(text()),
            b"tvsn" => t.season = int().filter(|n| *n < 10_000),
            b"tves" => t.episode = int().filter(|n| *n > 0 && *n < 100_000),
            b"\xa9day" => t.year = year_of(&text()),
            // Media kind: 9 = movie, 10 = TV show.
            b"stik" => {
                t.movie = int().and_then(|k| match k {
                    9 => Some(true),
                    10 => Some(false),
                    _ => None,
                })
            }
            _ => {}
        }
    }
    Some(t)
}

// ------------------------------------------------------------------ Matroska

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const SEEK_HEAD: u32 = 0x114D_9B74;
const SEEK: u32 = 0x4DBB;
const SEEK_ID: u32 = 0x53AB;
const SEEK_POS: u32 = 0x53AC;
const INFO: u32 = 0x1549_A966;
const TITLE: u32 = 0x7BA9;
const TAGS: u32 = 0x1254_C367;
const TAG: u32 = 0x7373;
const TARGETS: u32 = 0x63C0;
const TARGET_TYPE_VALUE: u32 = 0x68CA;
const SIMPLE_TAG: u32 = 0x67C8;
const TAG_NAME: u32 = 0x45A3;
const TAG_STRING: u32 = 0x4487;
const CLUSTER: u32 = 0x1F43_B675;

/// EBML variable-length integer: (value, length). `keep_marker` for ids.
fn vint(b: &[u8], keep_marker: bool) -> Option<(u64, usize)> {
    let first = *b.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || b.len() < len {
        return None;
    }
    let mut v = if keep_marker { first as u64 } else { (first as u64) & (0xFF >> len) };
    for x in &b[1..len] {
        v = (v << 8) | *x as u64;
    }
    Some((v, len))
}

/// Unknown-size elements have all value bits set.
fn unknown_size(v: u64, len: usize) -> bool {
    v == (1u64 << (7 * len)) - 1
}

/// Children of an element body held in memory: (id, body).
fn ebml_children(b: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let Some((id, il)) = vint(&b[i..], true) else { break };
        let Some((size, sl)) = vint(&b[i + il..], false) else { break };
        let start = i + il + sl;
        let end = (start as u64).saturating_add(size).min(b.len() as u64) as usize;
        if unknown_size(size, sl) {
            break;
        }
        out.push((id as u32, &b[start..end]));
        i = end;
    }
    out
}

fn uint(b: &[u8]) -> u64 {
    b.iter().take(8).fold(0, |v, x| (v << 8) | *x as u64)
}

fn string(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim_end_matches('\0').trim().to_string()
}

/// Read an element header at `pos`: (id, body start, body size or None if unknown).
fn ebml_header(f: &mut File, pos: u64) -> Option<(u32, u64, Option<u64>)> {
    let h = read_at(f, pos, 12).or_else(|| {
        // Near the end of the file.
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut v = Vec::new();
        f.take(12).read_to_end(&mut v).ok()?;
        Some(v)
    })?;
    let (id, il) = vint(&h, true)?;
    let (size, sl) = vint(&h[il..], false)?;
    Some((id as u32, pos + (il + sl) as u64, (!unknown_size(size, sl)).then_some(size)))
}

fn parse_tags(body: &[u8], t: &mut Tags) {
    for (id, tag) in ebml_children(body) {
        if id != TAG {
            continue;
        }
        let mut level = 50;
        let mut simple = Vec::new();
        for (cid, c) in ebml_children(tag) {
            match cid {
                TARGETS => {
                    if let Some((_, v)) = ebml_children(c).into_iter().find(|(i, _)| *i == TARGET_TYPE_VALUE) {
                        level = uint(v);
                    }
                }
                SIMPLE_TAG => {
                    let kids = ebml_children(c);
                    let name = kids.iter().find(|(i, _)| *i == TAG_NAME).map(|(_, v)| string(v));
                    let value = kids.iter().find(|(i, _)| *i == TAG_STRING).map(|(_, v)| string(v));
                    if let (Some(n), Some(v)) = (name, value) {
                        simple.push((n.to_ascii_uppercase(), v));
                    }
                }
                _ => {}
            }
        }
        for (name, v) in simple {
            match (level, name.as_str()) {
                (70, "TITLE") => t.show = Some(v),
                (60, "PART_NUMBER") => t.season = v.parse().ok(),
                (50, "PART_NUMBER") => t.episode = v.parse().ok().filter(|n| *n > 0),
                (50, "TITLE") => t.title = Some(v),
                (_, "DATE_RELEASED" | "DATE_RECORDED") => t.year = t.year.or(year_of(&v)),
                (_, "CONTENT_TYPE") => {
                    let v = v.to_ascii_lowercase();
                    if v.contains("movie") || v.contains("film") {
                        t.movie = Some(true);
                    } else if v.contains("tv") || v.contains("episode") {
                        t.movie = Some(false);
                    }
                }
                (_, "TMDB") => tmdb_tag(&v, t),
                (_, "IMDB") => t.ids.imdb = Some(v).filter(|i| i.starts_with("tt")),
                (_, "TVDB" | "TVDB2") => t.ids.tvdb = t.ids.tvdb.or(v.parse().ok()),
                _ => {}
            }
        }
    }
}

fn mkv_tags(p: &Path) -> Option<Tags> {
    let mut f = File::open(p).ok()?;
    let len = f.metadata().ok()?.len();
    let (id, body, size) = ebml_header(&mut f, 0)?;
    if id != EBML {
        return None;
    }
    let (sid, seg_start, seg_size) = ebml_header(&mut f, body + size?)?;
    if sid != SEGMENT {
        return None;
    }
    let seg_end = seg_size.map_or(len, |s| (seg_start + s).min(len));
    let mut t = Tags::default();
    let mut tags_at: Option<u64> = None;
    let mut have_tags = false;
    let mut pos = seg_start;
    for _ in 0..64 {
        if pos >= seg_end {
            break;
        }
        let Some((id, body, size)) = ebml_header(&mut f, pos) else { break };
        let Some(size) = size else { break };
        let small = size <= MAX_BOX;
        match id {
            SEEK_HEAD if small => {
                let b = read_at(&mut f, body, size as usize)?;
                for (_, seek) in ebml_children(&b).into_iter().filter(|(i, _)| *i == SEEK) {
                    let kids = ebml_children(seek);
                    let target = kids.iter().find(|(i, _)| *i == SEEK_ID).map(|(_, v)| uint(v) as u32);
                    let at = kids.iter().find(|(i, _)| *i == SEEK_POS).map(|(_, v)| uint(v));
                    if let (Some(TAGS), Some(at)) = (target, at) {
                        tags_at = Some(seg_start + at);
                    }
                }
            }
            INFO if small => {
                let b = read_at(&mut f, body, size as usize)?;
                if let Some((_, v)) = ebml_children(&b).into_iter().find(|(i, _)| *i == TITLE) {
                    t.segment_title = Some(string(v));
                }
            }
            TAGS if small => {
                let b = read_at(&mut f, body, size as usize)?;
                parse_tags(&b, &mut t);
                have_tags = true;
            }
            CLUSTER => break,
            _ => {}
        }
        pos = body + size;
    }
    if !have_tags
        && let Some(at) = tags_at
        && let Some((TAGS, body, Some(size))) = ebml_header(&mut f, at)
        && size <= MAX_BOX
        && let Some(b) = read_at(&mut f, body, size as usize)
    {
        parse_tags(&b, &mut t);
    }
    Some(t)
}

// ------------------------------------------------------------------ .nfo

static RE_UNIQUEID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<uniqueid[^>]*\btype\s*=\s*"(\w+)"[^>]*>\s*([^<]+?)\s*</uniqueid>"#).unwrap());
static RE_IMDB_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)imdb\.com/title/(tt\d{5,10})").unwrap());
static RE_TMDB_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)themoviedb\.org/(movie|tv)/(\d+)").unwrap());

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Text of the first `<tag>…</tag>` (or `<![CDATA[…]]>`).
fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let re = Regex::new(&format!(r"(?is)<{tag}(?:\s[^>]*)?>(.*?)</{tag}>")).ok()?;
    let raw = re.captures(xml)?.get(1)?.as_str().trim();
    let raw = raw.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")).unwrap_or(raw);
    let v = xml_unescape(raw).trim().to_string();
    (!v.is_empty()).then_some(v)
}

fn nfo_ids(xml: &str) -> Ids {
    let mut ids = Ids::default();
    for c in RE_UNIQUEID.captures_iter(xml) {
        let v = c[2].trim();
        match c[1].to_ascii_lowercase().as_str() {
            "tmdb" => ids.tmdb = ids.tmdb.or(v.parse().ok()),
            "imdb" => ids.imdb = ids.imdb.clone().or(Some(v.to_string()).filter(|i| i.starts_with("tt"))),
            "tvdb" => ids.tvdb = ids.tvdb.or(v.parse().ok()),
            _ => {}
        }
    }
    ids.tmdb = ids.tmdb.or(xml_tag(xml, "tmdbid").and_then(|v| v.parse().ok()));
    ids.imdb = ids.imdb.or(xml_tag(xml, "imdbid").filter(|i| i.starts_with("tt")));
    ids.imdb = ids.imdb.or(xml_tag(xml, "id").filter(|i| i.starts_with("tt")));
    ids.tvdb = ids.tvdb.or(xml_tag(xml, "tvdbid").and_then(|v| v.parse().ok()));
    ids
}

fn read_text(p: &Path) -> Option<String> {
    let f = File::open(p).ok()?;
    let mut buf = Vec::new();
    f.take(512 * 1024).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn nfo(p: &Path, out: &mut Embedded) {
    let dir = p.parent().unwrap_or(Path::new("."));
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");

    // Show-level ids: tvshow.nfo in the show folder ("Show/Season 1/ep.mkv" or "Show/ep.mkv").
    let show_nfo: Option<PathBuf> = p.ancestors().skip(1).take(3).map(|d| d.join("tvshow.nfo")).find(|f| f.is_file());
    let show_title = show_nfo.as_deref().and_then(read_text).map(|xml| {
        out.show_ids = nfo_ids(&xml);
        xml_tag(&xml, "title")
    });

    let own = [dir.join(format!("{stem}.nfo")), dir.join("movie.nfo")];
    let Some(xml) = own.iter().filter(|f| f.is_file()).find_map(|f| read_text(f)) else { return };
    let lower = xml.to_ascii_lowercase();
    if lower.contains("<episodedetails") {
        let show = xml_tag(&xml, "showtitle").or_else(|| show_title.clone().flatten());
        let season = xml_tag(&xml, "season").and_then(|v| v.parse().ok());
        let episode = xml_tag(&xml, "episode").and_then(|v| v.parse().ok()).filter(|n: &u32| *n > 0);
        if let (Some(title), Some(episode)) = (show, episode) {
            out.strong.push(Guess {
                title,
                episode: Some(EpisodeNo { season: season.unwrap_or(1), episode, last: None }),
                episode_title: xml_tag(&xml, "title"),
                // The episode's own ids are for the episode, not the show.
                ids: out.show_ids.clone(),
                air_date: None,
                ..Default::default()
            });
        }
    } else if lower.contains("<movie") {
        let ids = nfo_ids(&xml);
        out.movie_ids = ids.clone();
        if let Some(title) = xml_tag(&xml, "title") {
            let year = xml_tag(&xml, "year").and_then(|y| year_of(&y)).or_else(|| {
                xml_tag(&xml, "premiered").or_else(|| xml_tag(&xml, "releasedate")).and_then(|d| year_of(&d))
            });
            out.strong.push(Guess { title, year, ids, ..Default::default() });
        }
    } else {
        // Scene NFOs: plain text, but they usually link the IMDb or TMDB page.
        let mut ids = Ids::default();
        if let Some(c) = RE_IMDB_URL.captures(&xml) {
            ids.imdb = Some(c[1].to_string());
        }
        if let Some(c) = RE_TMDB_URL.captures(&xml)
            && &c[1].to_ascii_lowercase() == "movie"
        {
            ids.tmdb = c[2].parse().ok();
        }
        out.movie_ids = ids;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("marquee-embedded-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn kodi_nfo_files() {
        let d = temp_dir("nfo");
        let show = d.join("Breaking Bad (2008)");
        let season = show.join("Season 2");
        std::fs::create_dir_all(&season).unwrap();
        std::fs::write(
            show.join("tvshow.nfo"),
            r#"<?xml version="1.0"?><tvshow><title>Breaking Bad</title><uniqueid type="tmdb" default="true">1396</uniqueid><uniqueid type="imdb">tt0903747</uniqueid></tvshow>"#,
        )
        .unwrap();
        let video = season.join("bb203.mkv");
        std::fs::write(&video, b"not really a video").unwrap();
        std::fs::write(
            season.join("bb203.nfo"),
            "<episodedetails><title>Bit by a Dead Bee</title><season>2</season><episode>3</episode><uniqueid type=\"tmdb\">62093</uniqueid></episodedetails>",
        )
        .unwrap();
        let e = read(video.to_str().unwrap());
        assert_eq!(e.show_ids.tmdb, Some(1396));
        let g = &e.strong[0];
        assert_eq!(g.title, "Breaking Bad");
        assert_eq!(g.episode, Some(EpisodeNo { season: 2, episode: 3, last: None }));
        assert_eq!(g.episode_title.as_deref(), Some("Bit by a Dead Bee"));
        assert_eq!(g.ids.tmdb, Some(1396), "the show's id, not the episode's");

        let movie_dir = d.join("Heat");
        std::fs::create_dir_all(&movie_dir).unwrap();
        let movie = movie_dir.join("heat.1995.mkv");
        std::fs::write(&movie, b"x").unwrap();
        std::fs::write(
            movie_dir.join("movie.nfo"),
            "<movie><title>Heat &amp; Dust</title><year>1995</year><uniqueid type=\"imdb\">tt0113277</uniqueid></movie>",
        )
        .unwrap();
        let e = read(movie.to_str().unwrap());
        assert_eq!(e.strong[0].title, "Heat & Dust");
        assert_eq!(e.strong[0].year, Some(1995));
        assert_eq!(e.movie_ids.imdb.as_deref(), Some("tt0113277"));

        // Scene NFO with an IMDb link.
        std::fs::write(movie_dir.join("movie.nfo"), "  ▄▄▄ GROUP ▄▄▄\n https://www.imdb.com/title/tt0113277/ \n")
            .unwrap();
        let e = read(movie.to_str().unwrap());
        assert!(e.strong.is_empty());
        assert_eq!(e.movie_ids.imdb.as_deref(), Some("tt0113277"));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    }

    fn mp4_item(kind: &[u8; 4], value: &[u8]) -> Vec<u8> {
        let mut data = vec![0, 0, 0, 1, 0, 0, 0, 0];
        data.extend_from_slice(value);
        mp4_box(kind, &mp4_box(b"data", &data))
    }

    #[test]
    fn mp4_itunes_tags() {
        let mut ilst = Vec::new();
        ilst.extend(mp4_item(b"tvsh", b"Severance"));
        ilst.extend(mp4_item(b"tvsn", &1u32.to_be_bytes()));
        ilst.extend(mp4_item(b"tves", &2u32.to_be_bytes()));
        ilst.extend(mp4_item(b"\xa9nam", b"Half Loop"));
        ilst.extend(mp4_item(b"stik", &[10]));
        let mut meta = vec![0, 0, 0, 0];
        meta.extend(mp4_box(b"hdlr", &[0; 25]));
        meta.extend(mp4_box(b"ilst", &ilst));
        let moov = mp4_box(b"moov", &mp4_box(b"udta", &mp4_box(b"meta", &meta)));
        let mut file = mp4_box(b"ftyp", b"isom\0\0\0\0");
        file.extend(moov);
        file.extend(mp4_box(b"mdat", &[0; 64]));
        let d = temp_dir("mp4");
        let p = d.join("video.mp4");
        std::fs::write(&p, file).unwrap();
        let e = read(p.to_str().unwrap());
        let g = &e.strong[0];
        assert_eq!(g.title, "Severance");
        assert_eq!(g.episode, Some(EpisodeNo { season: 1, episode: 2, last: None }));
        assert_eq!(g.episode_title.as_deref(), Some("Half Loop"));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn el(id: u32, body: &[u8]) -> Vec<u8> {
        let mut v: Vec<u8> = id.to_be_bytes().iter().copied().skip_while(|b| *b == 0).collect();
        // 8-byte size vint
        v.push(0x01);
        v.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
        v.extend_from_slice(body);
        v
    }

    fn simple(name: &str, value: &str) -> Vec<u8> {
        let mut b = el(TAG_NAME, name.as_bytes());
        b.extend(el(TAG_STRING, value.as_bytes()));
        el(SIMPLE_TAG, &b)
    }

    fn tag(level: u8, tags: &[(&str, &str)]) -> Vec<u8> {
        let mut b = el(TARGETS, &el(TARGET_TYPE_VALUE, &[level]));
        for (n, v) in tags {
            b.extend(simple(n, v));
        }
        el(TAG, &b)
    }

    #[test]
    fn matroska_tags_after_the_clusters() {
        let mut tags = tag(70, &[("TITLE", "The Bear")]);
        tags.extend(tag(60, &[("PART_NUMBER", "2")]));
        tags.extend(tag(50, &[("PART_NUMBER", "7"), ("TITLE", "Forks"), ("TMDB", "tv/136315")]));
        let tags = el(TAGS, &tags);
        let info = el(INFO, &el(TITLE, b"The.Bear.S02E07.1080p.WEB.H264-GRP"));
        let cluster = el(CLUSTER, &[0; 32]);
        // SeekHead points at Tags (offsets are relative to the segment body).
        let seek_len =
            el(SEEK_HEAD, &el(SEEK, &[el(SEEK_ID, &TAGS.to_be_bytes()), el(SEEK_POS, &[0; 8])].concat())).len();
        let tags_pos = (seek_len + info.len() + cluster.len()) as u64;
        let seek = el(
            SEEK_HEAD,
            &el(SEEK, &[el(SEEK_ID, &TAGS.to_be_bytes()), el(SEEK_POS, &tags_pos.to_be_bytes())].concat()),
        );
        let segment = el(SEGMENT, &[seek, info, cluster, tags].concat());
        let mut file = el(EBML, &el(0x4282, b"matroska"));
        file.extend(segment);
        let d = temp_dir("mkv");
        let p = d.join("x.mkv");
        std::fs::write(&p, file).unwrap();
        let e = read(p.to_str().unwrap());
        let g = &e.strong[0];
        assert_eq!(g.title, "The Bear");
        assert_eq!(g.episode, Some(EpisodeNo { season: 2, episode: 7, last: None }));
        assert_eq!(g.episode_title.as_deref(), Some("Forks"));
        assert_eq!(g.ids.tmdb, Some(136315));
        // The segment title is kept as a loose reading.
        assert_eq!(e.weak[0].title, "The Bear");
        let _ = std::fs::remove_dir_all(&d);
    }
}
