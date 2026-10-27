//! Game discovery: Games/<FOLDER>/ scanning, display-name cleanup and a
//! natural ordering ("Game 2" before "Game 10").

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameEntry {
    /// Path relative to the card root, e.g. "Games/GB/Tetris.gb".
    pub path: String,
    pub title: String,
}

/// Strip region/version tags and clean separators for display.
/// "Tetris (World) (Rev 1) [!].gb" -> "Tetris"
pub fn clean_title(stem: &str) -> String {
    let mut out = String::with_capacity(stem.len());
    let mut depth = 0i32;
    for c in stem.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let cleaned = out.replace('_', " ");
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Case-insensitive natural ordering with numeric runs compared as numbers.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let na = take_number(&mut ai);
                    let nb = take_number(&mut bi);
                    match na.cmp(&nb) {
                        Ordering::Equal => {}
                        ord => return ord,
                    }
                } else {
                    let la = ca.to_ascii_lowercase();
                    let lb = cb.to_ascii_lowercase();
                    match la.cmp(&lb) {
                        Ordering::Equal => {}
                        ord => return ord,
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
}

fn take_number(it: &mut std::iter::Peekable<std::str::Chars>) -> u64 {
    let mut n: u64 = 0;
    while let Some(c) = it.peek().copied() {
        if c.is_ascii_digit() {
            n = n.saturating_mul(10).saturating_add((c as u8 - b'0') as u64);
            it.next();
        } else {
            break;
        }
    }
    n
}

/// A card-relative path with `/` separators and no `./` prefix. ROM paths are
/// stored and compared in this form, whatever the scan root looked like.
pub fn card_path(text: &str) -> String {
    let text = text.replace('\\', "/");
    text.strip_prefix("./").unwrap_or(&text).to_string()
}

/// Native N64 ports put the game and `rom.z64` in one folder under
/// `Games/N64/`; the display name comes from the port's `config.json` label.
pub fn scan_n64(root: &Path, dir: &str) -> Vec<GameEntry> {
    let base = root.join(dir);
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut games = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let rom = path.join("rom.z64");
        if !rom.is_file() {
            continue;
        }
        let label = std::fs::read_to_string(path.join("config.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|json| {
                json.get("label")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .filter(|s| !s.trim().is_empty());
        let title = label.unwrap_or_else(|| clean_title(&entry.file_name().to_string_lossy()));
        let rel = rom
            .strip_prefix(&base)
            .unwrap_or(&rom)
            .to_string_lossy()
            .replace('\\', "/");
        games.push(GameEntry {
            path: format!("{dir}/{rel}"),
            title,
        });
    }
    games.sort_by(|a, b| natural_cmp(&a.title, &b.title).then_with(|| a.path.cmp(&b.path)));
    games
}

/// Recursively scan `dir`, keeping files whose extension is in `exts`
/// (lower-case, no dot). Returns a naturally sorted list.
pub fn scan_dir(root: &Path, dir: &str, exts: &[String]) -> Vec<GameEntry> {
    let base = root.join(dir);
    let mut found: Vec<PathBuf> = Vec::new();
    walk(&base, &mut found);
    let mut games: Vec<GameEntry> = found
        .into_iter()
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
                .unwrap_or(false)
        })
        .filter_map(|p| {
            let stem = p.file_stem()?.to_str()?.to_string();
            let rel = p
                .strip_prefix(&base)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            Some(GameEntry {
                path: format!("{dir}/{rel}"),
                title: clean_title(&stem),
            })
        })
        .collect();
    hide_playlist_entries(root, &mut games);
    games.sort_by(|a, b| natural_cmp(&a.title, &b.title).then_with(|| a.path.cmp(&b.path)));
    games
}

/// A multi-disc `.m3u` is the game; the discs it points at are not separate
/// titles. Relative names resolve against the playlist's own folder, as the
/// libretro core reads them.
fn hide_playlist_entries(root: &Path, games: &mut Vec<GameEntry>) {
    let playlists: Vec<&GameEntry> = games
        .iter()
        .filter(|g| g.path.to_ascii_lowercase().ends_with(".m3u"))
        .collect();
    if playlists.is_empty() {
        return;
    }
    let mut discs: HashSet<String> = HashSet::new();
    for list in playlists {
        let Ok(text) = std::fs::read_to_string(root.join(&list.path)) else {
            continue;
        };
        let text = text.trim_start_matches('\u{feff}');
        let dir = Path::new(&list.path).parent().unwrap_or(Path::new(""));
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let path = if let Some(rest) = line.strip_prefix("/mnt/sdcard/") {
                PathBuf::from(rest)
            } else if line.starts_with('/') {
                continue;
            } else {
                dir.join(line.replace('\\', "/"))
            };
            discs.insert(card_path(&path.to_string_lossy().replace('\\', "/")));
        }
    }
    games.retain(|g| !discs.contains(&g.path));
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            walk(&path, out);
        } else if ft.is_file() {
            out.push(path);
        }
    }
}

/// A safe FAT32 filename stem: letters, digits, space and a tame symbol
/// subset, no leading/trailing space or dot, length-bounded. `None` when
/// the input cannot be a filename.
pub fn sanitize_game_name(input: &str) -> Option<String> {
    let name = input.trim().trim_matches('.');
    if name.is_empty() || name.chars().count() > 100 {
        return None;
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || " -_()[].,'&!+".contains(c);
    if !name.chars().all(allowed) {
        return None;
    }
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_titles() {
        assert_eq!(clean_title("Tetris (World) (Rev 1) [!]"), "Tetris");
        assert_eq!(clean_title("Super_Mario_Land (USA)"), "Super Mario Land");
        assert_eq!(clean_title("Game [h1] (USA, Europe)"), "Game");
        assert_eq!(clean_title("plain"), "plain");
    }

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("Game 2", "Game 10"), Ordering::Less);
        assert_eq!(natural_cmp("game 10", "Game 2"), Ordering::Greater);
        assert_eq!(natural_cmp("Mario", "mario"), Ordering::Equal);
        assert_eq!(natural_cmp("A", "Ab"), Ordering::Less);
    }

    #[test]
    fn playlists_hide_their_discs() {
        let dir = std::env::temp_dir().join(format!("magi_m3u_test_{}", std::process::id()));
        let sub = dir.join("Games").join("PS1");
        std::fs::create_dir_all(&sub).unwrap();
        for name in ["Game (Disc 1).cue", "Game (Disc 2).cue", "Other.cue"] {
            std::fs::write(sub.join(name), b"x").unwrap();
        }
        std::fs::write(
            sub.join("Game.m3u"),
            b"\xEF\xBB\xBF# multi-disc\r\nGame (Disc 1).cue\r\nGame (Disc 2).cue\r\n",
        )
        .unwrap();
        let exts = ["cue".to_string(), "m3u".to_string()];
        let games = scan_dir(&dir, "Games/PS1", &exts);
        let names: Vec<&str> = games.iter().map(|g| g.path.as_str()).collect();
        assert_eq!(names, vec!["Games/PS1/Game.m3u", "Games/PS1/Other.cue"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scans_temp_dir() {
        let dir = std::env::temp_dir().join(format!("magi_scan_test_{}", std::process::id()));
        let sub = dir.join("Games").join("GB");
        std::fs::create_dir_all(sub.join("nested")).unwrap();
        std::fs::write(sub.join("Game 10.gb"), b"x").unwrap();
        std::fs::write(sub.join("Game 2.gb"), b"x").unwrap();
        std::fs::write(sub.join("NotAGame.txt"), b"x").unwrap();
        std::fs::write(sub.join("nested").join("Nested (USA).gb"), b"x").unwrap();
        let games = scan_dir(&dir, "Games/GB", &["gb".to_string()]);
        assert_eq!(games.len(), 3);
        assert_eq!(games[0].title, "Game 2");
        assert_eq!(games[1].title, "Game 10");
        assert_eq!(games[2].title, "Nested");
        assert!(games[0].path.starts_with("Games/GB/"), "{}", games[0].path);
        std::fs::remove_dir_all(&dir).ok();
    }
}
