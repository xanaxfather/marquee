//! Score metadata search results against a filename guess.
//!
//! Title similarity dominates; the year is a strong tie-breaker (and a strong
//! veto when it disagrees); popularity only breaks ties between otherwise
//! equal candidates (e.g. "The Office" US vs UK with no year or country).

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: u64,
    pub titles: Vec<String>,
    pub year: Option<u16>,
    pub vote_count: u32,
    pub origin_country: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct Scored {
    pub index: usize,
    pub similarity: f64,
    pub total: f64,
}

/// Minimum title similarity before anything else is considered.
const MIN_SIMILARITY: f64 = 0.78;
/// Minimum combined score to accept a match.
const MIN_TOTAL: f64 = 0.88;

/// Lowercase, strip accents/punctuation, "&" → "and", join "S W A T" → "swat".
pub fn normalize(s: &str) -> String {
    let folded: String = s.nfd().filter(|c| !is_combining_mark(*c)).collect();
    let lowered = folded.to_lowercase().replace('&', " and ");
    let mut cleaned = String::with_capacity(lowered.len());
    for c in lowered.chars() {
        if c.is_alphanumeric() {
            cleaned.push(c);
        } else if !matches!(c, '\'' | '’' | '‘' | '`' | '´') {
            cleaned.push(' ');
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut run = String::new();
    let mut run_len = 0;
    let flush = |run: &mut String, run_len: &mut usize, out: &mut Vec<String>| {
        if *run_len > 0 {
            out.push(std::mem::take(run));
            *run_len = 0;
        }
    };
    for tok in cleaned.split_whitespace() {
        let mut chars = tok.chars();
        let single_letter = matches!((chars.next(), chars.next()), (Some(c), None) if c.is_alphabetic());
        if single_letter {
            run.push_str(tok);
            run_len += 1;
        } else {
            flush(&mut run, &mut run_len, &mut out);
            out.push(tok.to_string());
        }
    }
    flush(&mut run, &mut run_len, &mut out);
    out.join(" ")
}

fn strip_article(s: &str) -> &str {
    for a in ["the ", "a ", "an "] {
        if let Some(rest) = s.strip_prefix(a) {
            return rest;
        }
    }
    s
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn ratio(a: &str, b: &str) -> f64 {
    let max = a.chars().count().max(b.chars().count());
    if max == 0 {
        return 0.0;
    }
    1.0 - levenshtein(a, b) as f64 / max as f64
}

/// Similarity of two already-normalized titles in 0..=1.
pub fn similarity(q: &str, t: &str) -> f64 {
    if q.is_empty() || t.is_empty() {
        return 0.0;
    }
    if q == t {
        return 1.0;
    }
    let (qs, ts) = (strip_article(q), strip_article(t));
    if qs == ts {
        return 0.98;
    }
    // Ignore spacing differences ("spider man" vs "spiderman").
    let (qc, tc) = (qs.replace(' ', ""), ts.replace(' ', ""));
    if qc == tc {
        return 0.97;
    }
    ratio(q, t).max(ratio(qs, ts)).max(ratio(&qc, &tc) - 0.02)
}

/// Whole-word containment ("marvels the avengers" ⊃ "the avengers").
fn contains_words(long: &str, short: &str) -> bool {
    short.split(' ').count() >= 2 && format!(" {long} ").contains(&format!(" {short} "))
}

/// `not_after`: the candidate can't have started after this year
/// (a show with an episode in 1940 didn't premiere in 2023).
pub fn score(
    query: &str,
    year: Option<u16>,
    country: Option<&str>,
    not_after: Option<u16>,
    c: &Candidate,
) -> (f64, f64) {
    let q = normalize(query);
    let year_exact = matches!((year, c.year), (Some(a), Some(b)) if a == b);
    let mut sim: f64 = 0.0;
    for t in &c.titles {
        let t = normalize(t);
        let mut s = similarity(&q, &t);
        if year_exact && (contains_words(&q, &t) || contains_words(&t, &q)) {
            s = s.max(0.80);
        }
        sim = sim.max(s);
    }
    let year_adj = match (year, c.year) {
        (Some(a), Some(b)) => match a.abs_diff(b) {
            0 => 0.15,
            1 => 0.03,
            _ => -0.6,
        },
        (Some(_), None) => -0.1,
        (None, _) => 0.0,
    };
    let pop = 0.05 * ((1.0 + c.vote_count as f64).ln() / (1.0 + 10_000f64).ln()).min(1.0);
    let country_adj = match country {
        Some(cc) if c.origin_country.iter().any(|o| o.eq_ignore_ascii_case(cc) || (cc == "UK" && o == "GB")) => 0.1,
        Some(_) if !c.origin_country.is_empty() => -0.05,
        _ => 0.0,
    };
    let cap_adj = match (not_after, c.year) {
        (Some(cap), Some(cy)) if cy > cap => -0.6,
        _ => 0.0,
    };
    (sim, sim + year_adj + pop + country_adj + cap_adj)
}

/// Pick the best acceptable candidate, or `None` if nothing is a confident match.
#[cfg_attr(not(test), allow(dead_code))]
pub fn best(query: &str, year: Option<u16>, country: Option<&str>, cands: &[Candidate]) -> Option<Scored> {
    best_capped(query, year, country, None, cands)
}

pub fn best_capped(
    query: &str,
    year: Option<u16>,
    country: Option<&str>,
    not_after: Option<u16>,
    cands: &[Candidate],
) -> Option<Scored> {
    let mut scored: Vec<Scored> = cands
        .iter()
        .enumerate()
        .map(|(index, c)| {
            let (similarity, total) = score(query, year, country, not_after, c);
            Scored { index, similarity, total }
        })
        .collect();
    scored.sort_by(|a, b| b.total.total_cmp(&a.total));

    for s in scored.iter().take(3) {
        let c = &cands[s.index];
        tracing::debug!(
            query,
            ?year,
            candidate = c.titles.first().map(String::as_str).unwrap_or(""),
            candidate_year = ?c.year,
            id = c.id,
            similarity = format!("{:.3}", s.similarity),
            total = format!("{:.3}", s.total),
            "match candidate"
        );
    }

    let top = *scored.first()?;
    if top.similarity < MIN_SIMILARITY || top.total < MIN_TOTAL {
        return None;
    }
    if let Some(second) = scored.get(1)
        && top.total - second.total < 0.02
        && second.similarity >= MIN_SIMILARITY
    {
        tracing::warn!(
            query,
            first = cands[top.index].titles.first().map(String::as_str).unwrap_or(""),
            first_year = ?cands[top.index].year,
            second = cands[second.index].titles.first().map(String::as_str).unwrap_or(""),
            second_year = ?cands[second.index].year,
            "ambiguous match; picked the more popular one — add the year to the filename to disambiguate"
        );
    }
    Some(top)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: u64, title: &str, year: u16, votes: u32) -> Candidate {
        Candidate { id, titles: vec![title.into()], year: Some(year), vote_count: votes, origin_country: vec![] }
    }

    #[test]
    fn normalization() {
        assert_eq!(normalize("Under THe Skin"), "under the skin");
        assert_eq!(normalize("S.W.A.T."), normalize("S W A T"));
        assert_eq!(normalize("Amélie"), "amelie");
        assert_eq!(normalize("Ocean's Eleven"), "oceans eleven");
        assert_eq!(normalize("Law & Order"), "law and order");
        assert_eq!(normalize("A Quiet Place"), "a quiet place");
    }

    #[test]
    fn exact_title_and_year_wins() {
        let cands = vec![
            cand(1, "Under the Skin of the City", 2001, 50),
            cand(2, "Under the Skin", 1997, 80),
            cand(3, "Under the Skin", 2013, 4000),
        ];
        let b = best("Under THe Skin", Some(2013), None, &cands).unwrap();
        assert_eq!(cands[b.index].id, 3);
        let b = best("Under THe Skin", Some(1997), None, &cands).unwrap();
        assert_eq!(cands[b.index].id, 2);
    }

    #[test]
    fn wrong_year_is_rejected() {
        let cands = vec![cand(1, "Dune", 1984, 3000)];
        assert!(best("Dune", Some(2021), None, &cands).is_none());
        // off by one (regional release dates) is fine
        assert!(best("Dune", Some(1985), None, &cands).is_some());
    }

    #[test]
    fn similar_but_different_titles_rejected() {
        let cands = vec![cand(1, "Under the Skin of the City", 2001, 50)];
        assert!(best("Under the Skin", None, None, &cands).is_none());
        let cands = vec![cand(1, "Breaking Away", 1979, 500)];
        assert!(best("Breaking Bad", None, None, &cands).is_none());
    }

    #[test]
    fn popularity_breaks_ties_and_country_overrides() {
        let mut us = cand(1, "The Office", 2005, 3000);
        us.origin_country = vec!["US".into()];
        let mut uk = cand(2, "The Office", 2001, 1500);
        uk.origin_country = vec!["GB".into()];
        let cands = vec![uk, us];
        assert_eq!(cands[best("The Office", None, None, &cands).unwrap().index].id, 1);
        assert_eq!(cands[best("The Office", None, Some("UK"), &cands).unwrap().index].id, 2);
    }

    #[test]
    fn premiere_cap_breaks_same_name_ties() {
        let cands = vec![cand(1, "Tom and Jerry", 2023, 7), cand(2, "Tom and Jerry", 1940, 1)];
        // Without context the more-voted (2023) wins; an episode from 1940 rules it out.
        assert_eq!(cands[best("Tom and Jerry", None, None, &cands).unwrap().index].id, 1);
        assert_eq!(cands[best_capped("Tom and Jerry", None, None, Some(1940), &cands).unwrap().index].id, 2);
    }

    #[test]
    fn small_typos_and_articles() {
        let cands = vec![cand(1, "The Shawshank Redemption", 1994, 25000)];
        assert!(best("Shawshank Redemption", Some(1994), None, &cands).is_some());
        assert!(best("The Shawshank Redemtion", None, None, &cands).is_some());
        let cands = vec![cand(1, "Spider-Man: No Way Home", 2021, 18000)];
        assert!(best("Spider-Man No Way Home", Some(2021), None, &cands).is_some());
    }
}
