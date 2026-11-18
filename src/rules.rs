//! Per-player and per-folder rules: never show, show without the title, or
//! always show (overriding the privacy lists). They are checked before any
//! lookup, so a file a rule hides or anonymises never reaches TMDB.

use serde::{Deserialize, Serialize};
use std::cmp::Reverse;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    /// Nothing on Discord, nothing looked up, nothing in the history.
    Never,
    /// "Watching something": no title, artwork or links, nothing looked up.
    Generic,
    /// Shown normally even if the privacy lists would hide it.
    Always,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    /// Player name as shown in the menu ("VLC", "IINA", "MPC-HC", …) or a
    /// watcher id ("MPC", "SMTC"); empty = any player.
    pub player: String,
    /// Folder the file must be in (or below); `~/` is the home folder; empty = anywhere.
    pub folder: String,
    pub action: RuleAction,
}

impl Default for Rule {
    fn default() -> Self {
        Self { player: String::new(), folder: String::new(), action: RuleAction::Never }
    }
}

/// Folder paths are compared case-insensitively where the usual file system
/// is (Windows, macOS), and exactly elsewhere.
const CASE_INSENSITIVE: bool = cfg!(any(windows, target_os = "macos"));

fn normalize_path(p: &str) -> String {
    let p = match p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        Some(rest) => crate::paths::home().join(rest).to_string_lossy().into_owned(),
        None => p.to_string(),
    };
    let p = p.replace('\\', "/");
    if CASE_INSENSITIVE { p.to_lowercase() } else { p }
}

/// "MPC" matches "MPC-HC" and "MPC-BE", "GOM" matches "GOM Player".
fn player_matches(rule: &str, name: &str) -> bool {
    let (rule, name) = (rule.to_lowercase(), name.to_lowercase());
    name == rule || name.strip_prefix(&rule).is_some_and(|rest| rest.starts_with(['-', ' ']))
}

/// The action of the most specific rule covering this file, if any.
/// `players` holds the names the current player goes by (display name and watcher id).
///
/// Specificity: a deeper folder beats a shallower one, any folder beats none,
/// and naming a player beats "any player". On a tie the earlier rule wins.
/// Rules with neither a player nor a folder are ignored.
pub fn evaluate(rules: &[Rule], players: &[&str], path: Option<&str>) -> Option<RuleAction> {
    // Streams have no folder; only player rules can apply to them.
    let path = path.filter(|p| !p.contains("://")).map(normalize_path);
    rules
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let player = r.player.trim();
            let folder = r.folder.trim();
            if player.is_empty() && folder.is_empty() {
                return None;
            }
            if !player.is_empty() && !players.iter().any(|p| !p.is_empty() && player_matches(player, p)) {
                return None;
            }
            let depth = if folder.is_empty() {
                0
            } else {
                let f = normalize_path(folder);
                let f = f.trim_end_matches('/');
                let p = path.as_deref()?;
                if !(p == f || p.starts_with(&format!("{f}/"))) {
                    return None;
                }
                f.len() + 1
            };
            Some(((depth, !player.is_empty(), Reverse(i)), r.action))
        })
        .max_by_key(|(k, _)| *k)
        .map(|(_, a)| a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use RuleAction::*;

    fn rule(player: &str, folder: &str, action: RuleAction) -> Rule {
        Rule { player: player.into(), folder: folder.into(), action }
    }

    #[test]
    fn most_specific_rule_wins() {
        let rules = vec![
            rule("", "/Movies", Generic),
            rule("", "/Movies/Home", Never),
            rule("VLC", "", Always),
            rule("VLC", "/Movies", Never),
            rule("", "", Never), // ignored: matches nothing
        ];
        let eval = |player: &str, path: &str| evaluate(&rules, &[player], Some(path));
        assert_eq!(eval("IINA", "/Movies/Heat.mkv"), Some(Generic));
        assert_eq!(eval("IINA", "/Movies/Home/x.mkv"), Some(Never));
        // A deeper folder beats a player-specific rule on a shallower one.
        assert_eq!(eval("VLC", "/Movies/Home/x.mkv"), Some(Never));
        // Same folder: naming the player is more specific.
        assert_eq!(eval("VLC", "/Movies/Heat.mkv"), Some(Never));
        assert_eq!(eval("VLC", "/TV/x.mkv"), Some(Always));
        assert_eq!(eval("IINA", "/TV/x.mkv"), None);
        // A folder rule needs a real prefix, not a shared start.
        assert_eq!(eval("IINA", "/MoviesExtra/x.mkv"), None);
        // Streams only see player rules.
        assert_eq!(eval("VLC", "https://example.com/Movies/x"), Some(Always));
        assert_eq!(evaluate(&rules, &["IINA"], None), None);
    }

    #[test]
    fn players_and_paths_normalise() {
        let rules = vec![rule("mpc", "C:\\Videos\\Kids\\", Never)];
        assert_eq!(evaluate(&rules, &["MPC-HC"], Some("C:/Videos/Kids/a.mkv")), Some(Never));
        assert_eq!(evaluate(&rules, &["MPCX"], Some("C:/Videos/Kids/a.mkv")), None);
        // Display name or watcher id.
        assert_eq!(evaluate(&rules, &["Media Player", "MPC"], Some("C:\\Videos\\Kids\\a.mkv")), Some(Never));
        if CASE_INSENSITIVE {
            assert_eq!(evaluate(&rules, &["MPC-BE"], Some("c:\\videos\\KIDS\\a.mkv")), Some(Never));
        }
        let home = vec![rule("", "~/Private", Never)];
        let p = crate::paths::home().join("Private/a.mkv");
        assert_eq!(evaluate(&home, &[""], Some(&p.to_string_lossy())), Some(Never));
        // Root covers every absolute path.
        assert_eq!(evaluate(&[rule("", "/", Generic)], &[""], Some("/x/y.mkv")), Some(Generic));
    }

    #[test]
    fn non_latin_paths() {
        let rules = vec![rule("", "/Видео/Семья", Never), rule("", "/映画", Generic)];
        assert_eq!(evaluate(&rules, &["IINA"], Some("/Видео/Семья/фильм.mkv")), Some(Never));
        assert_eq!(evaluate(&rules, &["IINA"], Some("/映画/千と千尋.mkv")), Some(Generic));
    }
}
