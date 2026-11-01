//! System catalogue and card layout.
//!
//! Source of truth is `System/magi/systems.json` on the card, with an
//! optional user override at `Config/systems.json`; the embedded
//! `assets/systems.json` is the fallback so a fresh card works untuned.

use std::path::Path;

use serde::{Deserialize, Serialize};

pub const CARD_PATH: &str = "System/magi/systems.json";
pub const USER_PATH: &str = "Config/systems.json";

pub const DIR_CONFIG: &str = "Config";
pub const DIR_GAMES: &str = "Games";
pub const DIR_BIOS: &str = "BIOS";
pub const DIR_OVERLAYS: &str = "Overlays";
pub const DIR_SAVES: &str = "Saves";
pub const DIR_SHOTS: &str = "Screenshots";
pub const DIR_THEMES: &str = "Themes";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub exts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core: Option<String>,
    /// Games/<FOLDER>/ on the card; upper-cased id when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// libretro-thumbnails collection, for art downloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
}

impl SystemDef {
    pub fn folder(&self) -> String {
        self.folder
            .clone()
            .unwrap_or_else(|| self.id.to_uppercase())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SystemsFile {
    #[serde(default)]
    pub schema: u32,
    pub systems: Vec<SystemDef>,
}

pub struct Catalogue {
    pub systems: Vec<SystemDef>,
    pub source: &'static str,
}

impl Catalogue {
    pub fn load(cwd: &Path) -> Self {
        for (rel, source) in [(USER_PATH, USER_PATH), (CARD_PATH, CARD_PATH)] {
            let path = cwd.join(rel);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            match serde_json::from_str::<SystemsFile>(&text) {
                Ok(f) if !f.systems.is_empty() => {
                    crate::util::log(&format!("{} systems from {source}", f.systems.len()));
                    return Self {
                        systems: f.systems,
                        source,
                    };
                }
                Ok(_) => crate::util::log(&format!("{rel} has no systems; trying next")),
                Err(e) => crate::util::log(&format!("{rel} is invalid ({e}); trying next")),
            }
        }
        Self {
            systems: builtin(),
            source: "built-in",
        }
    }

    /// Best-effort card layout: base folders plus one Games/<FOLDER> per
    /// system. FAT is FAT; missing folders are made on every boot.
    pub fn ensure_layout(&self) {
        let mut dirs: Vec<String> = [
            DIR_CONFIG,
            DIR_BIOS,
            DIR_OVERLAYS,
            DIR_SAVES,
            DIR_GAMES,
            DIR_SHOTS,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for s in &self.systems {
            dirs.push(format!("{}/{}", DIR_GAMES, s.folder()));
        }
        let mut made = 0;
        for d in &dirs {
            if !Path::new(d).is_dir() {
                match std::fs::create_dir_all(d) {
                    Ok(()) => made += 1,
                    Err(e) => crate::util::log(&format!("cannot create {d}: {e}")),
                }
            }
        }
        if made > 0 {
            crate::util::log(&format!("card layout: created {made} folder(s)"));
        }
        write_default_catalogue();
    }
}

/// The catalogue embedded in the binary, used when the card has none.
const BUNDLED: &str = include_str!("../assets/systems.json");

/// Give the card the embedded catalogue when it has none, so the file exists
/// to read and edit (and `--probe` reports the card instead of the fallback).
pub fn write_default_catalogue() {
    let path = Path::new(CARD_PATH);
    if path.exists() {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = crate::util::write_file_atomic(path, BUNDLED.as_bytes()) {
        crate::util::log(&format!("cannot write {CARD_PATH}: {e}"));
    }
}

pub fn builtin() -> Vec<SystemDef> {
    let f: SystemsFile = serde_json::from_str(BUNDLED).expect("embedded systems.json must parse");
    f.systems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_has_core_systems() {
        let s = builtin();
        assert!(s.len() >= 5);
        for id in [
            "gb", "gbc", "gba", "nes", "snes", "genesis", "psx", "ngp", "pce",
        ] {
            assert!(s.iter().any(|d| d.id == id), "missing {id}");
        }
        assert!(s.iter().all(|d| !d.exts.is_empty()));
    }

    #[test]
    fn folder_defaults_to_upper_id_and_psx_is_ps1() {
        let s = builtin();
        let gb = s.iter().find(|d| d.id == "gb").unwrap();
        assert_eq!(gb.folder(), "GB");
        let psx = s.iter().find(|d| d.id == "psx").unwrap();
        assert_eq!(psx.folder(), "PS1");
    }
}
