//! Box art from libretro-thumbnails.
//!
//! A thumbnail is filed under the game's No-Intro (or Redump) name. MAGI asks
//! for the ROM's own name first, then for the name the system's DAT gives the
//! dump, which is where renamed files and region variants come from. HTTPS is
//! the curl BaseOS ships; nothing here links a TLS stack.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use crate::systems::SystemDef;

/// Where box art is served.
const THUMBS: &str = "https://thumbnails.libretro.com";
/// Where libretro-database publishes the DATs that name a dump exactly.
const DATABASE: &str =
    "https://raw.githubusercontent.com/libretro/libretro-database/master/metadat";

/// The art folders, in art-type order.
pub const FOLDERS: [&str; 4] = [
    "Named_Boxarts",
    "Named_Snaps",
    "Named_Titles",
    "Named_Logos",
];
/// The ART TYPE choices.
pub const LABELS: [&str; 4] = ["BOXART", "SNAP", "TITLE", "LOGO"];

/// A game to look up, and the art files to fill.
pub struct Game {
    /// ROM path relative to the card root.
    pub rom: String,
    /// The system the ROM belongs to. The FAVORITES list mixes systems, so a
    /// job cannot assume one collection for every game.
    pub system: SystemDef,
    /// The art type and destination for the file still missing.
    pub cover: (u8, PathBuf),
}

pub struct Job {
    pub cwd: PathBuf,
    pub games: Vec<Game>,
}

pub enum Msg {
    Progress {
        done: usize,
        total: usize,
    },
    Saved {
        rom: String,
    },
    Finished {
        saved: usize,
        missed: usize,
        offline: bool,
    },
}

/// The libretro collection a game's art is filed under.
fn collection(sys: &SystemDef, rom: &str) -> Option<String> {
    let ext = Path::new(rom)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let base = match &sys.art {
        Some(name) => name.clone(),
        None => builtin(&sys.id)?.to_string(),
    };
    // One MAGI system can hold media the collection splits in two.
    if base == "SNK - Neo Geo Pocket" && (ext == "ngc" || ext == "ngpc") {
        return Some("SNK - Neo Geo Pocket Color".into());
    }
    if base == "NEC - PC Engine - TurboGrafx 16" {
        if ext == "cue" || ext == "chd" {
            return Some("NEC - PC Engine CD - TurboGrafx-CD".into());
        }
        if ext == "sgx" {
            return Some("NEC - PC Engine SuperGrafx".into());
        }
    }
    Some(base)
}

/// Collections for the systems MAGI ships with, so a card without `art`
/// fields still fetches.
fn builtin(id: &str) -> Option<&'static str> {
    Some(match id {
        "gb" => "Nintendo - Game Boy",
        "gbc" => "Nintendo - Game Boy Color",
        "gba" => "Nintendo - Game Boy Advance",
        "nes" => "Nintendo - Nintendo Entertainment System",
        "snes" => "Nintendo - Super Nintendo Entertainment System",
        "genesis" => "Sega - Mega Drive - Genesis",
        "ngp" => "SNK - Neo Geo Pocket",
        "psx" => "Sony - PlayStation",
        "pce" => "NEC - PC Engine - TurboGrafx 16",
        "n64" => "Nintendo - Nintendo 64",
        _ => return None,
    })
}

fn dat_set(collection: &str) -> &'static str {
    match collection {
        "Sony - PlayStation" | "NEC - PC Engine CD - TurboGrafx-CD" => "redump",
        _ => "no-intro",
    }
}

fn art_url(collection: &str, folder: &str, name: &str) -> String {
    format!(
        "{THUMBS}/{}/{folder}/{}.png",
        escape(collection),
        escape(&thumbnail_name(name))
    )
}

fn dat_url(collection: &str) -> String {
    format!(
        "{DATABASE}/{}/{}.dat",
        dat_set(collection),
        escape(collection)
    )
}

/// The name libretro files art under: the game's own name with the characters
/// a file name cannot hold replaced by `_`.
pub fn thumbnail_name(game: &str) -> String {
    game.chars()
        .map(|c| if "&*/:`<>?\\|\"".contains(c) { '_' } else { c })
        .collect()
}

/// A URL component, with everything a URL can carry as it is.
fn escape(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A GET through the curl BaseOS ships: `(status, body)`, or nothing when the
/// server could not be reached (no DNS, no route, timeout).
fn get(url: &str, timeout: u32) -> Option<(u16, Vec<u8>)> {
    use std::io::Write;
    let limit = timeout.to_string();
    let mut child = Command::new("curl")
        .args([
            "-s",
            "-L",
            "-m",
            limit.as_str(),
            "--connect-timeout",
            "8",
            "-A",
            "magi",
            "-w",
            "\n%{http_code}",
            "-K",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let config = format!(
        "url = \"{}\"\n",
        url.replace('\\', "\\\\").replace('"', "\\\"")
    );
    child.stdin.take()?.write_all(config.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    let split = out.stdout.iter().rposition(|&b| b == b'\n')?;
    let status: u16 = std::str::from_utf8(&out.stdout[split + 1..])
        .ok()?
        .trim()
        .parse()
        .ok()?;
    (status != 0).then(|| (status, out.stdout[..split].to_vec()))
}

fn is_png(bytes: &[u8]) -> bool {
    png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .is_ok_and(|r| r.info().width > 0 && r.info().height > 0)
}

/// A system's DAT: every dump's CRC-32, every game's name, and the names
/// grouped by their title without region/version tags.
#[derive(Default)]
struct Dat {
    by_crc: HashMap<u32, String>,
    by_name: HashMap<String, String>,
    by_base: HashMap<String, Vec<String>>,
}

/// The title without `(USA)`, `(Rev 1)`, `[!]` and similar groups.
fn base_key(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut depth = 0u32;
    for ch in name.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.replace('_', " ")
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

const REGIONS: [&str; 20] = [
    "usa",
    "europe",
    "japan",
    "world",
    "asia",
    "australia",
    "korea",
    "china",
    "brazil",
    "canada",
    "france",
    "germany",
    "spain",
    "italy",
    "netherlands",
    "sweden",
    "taiwan",
    "hong kong",
    "russia",
    "poland",
];

/// Region names inside a dump's parenthesised tags, lower-cased.
fn regions(name: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut rest = name;
    while let Some(start) = rest.find('(') {
        let after = &rest[start + 1..];
        let Some(end) = after.find(')') else { break };
        for part in after[..end].split([',', '+']) {
            let part = part.trim().to_lowercase();
            if let Some(region) = REGIONS
                .iter()
                .copied()
                .find(|region| *region == part.as_str())
            {
                if !out.contains(&region) {
                    out.push(region);
                }
            }
        }
        rest = &after[end + 1..];
    }
    out
}

/// The `1` in `(Disc 1)`, used to keep a fuzzy match on the same disc.
fn disc_tag(name: &str) -> Option<&str> {
    let at = name.find("(Disc ")? + 6;
    let rest = &name[at..];
    let end = rest.find(')')?;
    Some(&rest[..end])
}

fn disc_number(tag: &str) -> &str {
    tag.split_whitespace().next().unwrap_or(tag)
}

fn unstable(name: &str) -> bool {
    [
        "(Beta)",
        "(Proto)",
        "(Prototype)",
        "(Demo)",
        "(Sample)",
        "(Unl)",
        "(Kiosk)",
    ]
    .iter()
    .any(|tag| name.contains(tag))
}

/// Lower is better: a stable dump, then the same region, then the same disc.
fn fuzzy_rank(want: &str, candidate: &str) -> (u8, u8, u8) {
    let wanted = regions(want);
    let have = regions(candidate);
    let region = if wanted.is_empty() {
        1
    } else if wanted.iter().any(|r| have.contains(r)) {
        0
    } else if have.contains(&"world") {
        1
    } else {
        2
    };
    let disc = match (disc_tag(want), disc_tag(candidate)) {
        (Some(a), Some(b)) if disc_number(a).eq_ignore_ascii_case(disc_number(b)) => 0,
        (Some(_), None) => 1,
        (Some(_), Some(_)) => 2,
        _ => 1,
    };
    let dump = u8::from(unstable(candidate) && !unstable(want));
    (dump, region, disc)
}

impl Dat {
    /// clrmamepro text: `game ( name "..." rom ( ... crc ... ) )` blocks.
    fn parse(text: &str) -> Dat {
        let mut dat = Dat::default();
        let mut name: Option<String> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("game (") {
                name = None;
            } else if let Some(rest) = line.strip_prefix("name \"") {
                name = rest.rfind('"').map(|end| rest[..end].to_string());
                if let Some(game) = &name {
                    dat.by_name
                        .entry(game.to_lowercase())
                        .or_insert_with(|| game.clone());
                    let base = base_key(game);
                    if !base.is_empty() {
                        let list = dat.by_base.entry(base).or_default();
                        if !list.contains(game) {
                            list.push(game.clone());
                        }
                    }
                }
            } else if line.starts_with("rom (") {
                let crc = line
                    .split_whitespace()
                    .skip_while(|token| *token != "crc")
                    .nth(1)
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok());
                if let (Some(crc), Some(name)) = (crc, &name) {
                    dat.by_crc.entry(crc).or_insert_with(|| name.clone());
                }
            }
        }
        dat
    }

    fn game_for(&self, crc: u32) -> Option<&str> {
        self.by_crc.get(&crc).map(String::as_str)
    }

    fn game_named(&self, name: &str) -> Option<&str> {
        self.by_name
            .get(&name.trim().to_lowercase())
            .map(String::as_str)
    }

    /// Title-only candidates, best first. Used only after the exact stem and
    /// exact DAT name both missed, so a patched or renamed ROM can still get
    /// some cover art.
    fn fuzzy(&self, name: &str) -> Vec<String> {
        let mut list = self
            .by_base
            .get(&base_key(name))
            .cloned()
            .unwrap_or_default();
        list.sort_by_key(|candidate| fuzzy_rank(name, candidate));
        list
    }
}

fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    for (i, entry) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *entry = c;
    }
    table
}

fn crc32(bytes: &[u8]) -> u32 {
    let table = crc_table();
    !bytes.iter().fold(!0u32, |c, &b| {
        table[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8)
    })
}

fn crc32_reader(mut reader: impl Read) -> std::io::Result<u32> {
    let table = crc_table();
    let mut crc = !0u32;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(!crc);
        }
        for &b in &buf[..n] {
            crc = table[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8);
        }
    }
}

/// The CRCs the DAT may know a dump by. A SNES dump can start with a 512-byte
/// copier header the DATs leave out, so both sides of it are asked about.
fn crcs(path: &Path) -> Vec<u32> {
    let copier = std::fs::metadata(path)
        .map(|m| m.len() % 1024 == 512)
        .unwrap_or(false);
    if copier {
        return std::fs::read(path)
            .map(|bytes| vec![crc32(&bytes[512..]), crc32(&bytes)])
            .unwrap_or_default();
    }
    std::fs::File::open(path)
        .and_then(crc32_reader)
        .map(|crc| vec![crc])
        .unwrap_or_default()
}

/// Whether the system's games are disc images, which are matched by dump name.
fn is_disc(sys: &SystemDef, rom: &str) -> bool {
    let ext = Path::new(rom)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    sys.id == "psx" || (sys.id == "pce" && (ext == "cue" || ext == "chd"))
}

/// A multi-disc dump's game name without the disc tag.
fn without_disc(stem: &str) -> &str {
    stem.split(" (Disc ").next().unwrap_or(stem)
}

/// One name in one folder; `Err` means the server was unreachable.
fn fetch(collection: &str, folder: &str, name: &str) -> Result<Option<Vec<u8>>, ()> {
    match get(&art_url(collection, folder, name), 20) {
        Some((200, body)) if is_png(&body) => Ok(Some(body)),
        Some(_) => Ok(None),
        None => Err(()),
    }
}

const ART_MAX_W: u32 = 640;
const ART_MAX_H: u32 = 480;
const ART_MAX_BYTES: usize = 1536 * 1024;

fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let reader = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .ok()?;
    Some((reader.info().width, reader.info().height))
}

/// Shrink a downloaded cover to the shelf's texture budget. `None` keeps the
/// original file.
fn optimize_art(png: &[u8]) -> Option<Vec<u8>> {
    let (w, h) = png_size(png)?;
    if w <= ART_MAX_W && h <= ART_MAX_H && png.len() <= ART_MAX_BYTES {
        return None;
    }
    let img = crate::image::decode_png(png)?;
    let scale = (ART_MAX_W as f32 / img.w as f32)
        .min(ART_MAX_H as f32 / img.h as f32)
        .min(1.0);
    let tw = ((img.w as f32 * scale).round() as u32).max(1);
    let th = ((img.h as f32 * scale).round() as u32).max(1);
    let out = if (tw, th) == (img.w, img.h) {
        crate::image::encode_png(img.w, img.h, &img.px).ok()?
    } else {
        let scaled = img.resized(tw, th);
        crate::util::log(&format!("art: {}x{} -> {}x{}", img.w, img.h, tw, th));
        crate::image::encode_png(scaled.w, scaled.h, &scaled.px).ok()?
    };
    if out.len() <= ART_MAX_BYTES || out.len() < png.len() {
        Some(out)
    } else {
        None
    }
}

fn save(path: &Path, png: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let optimized = optimize_art(png);
    let data = optimized.as_deref().unwrap_or(png);
    if let Err(e) = crate::util::write_file_atomic(path, data) {
        crate::util::log(&format!("art: cannot save {}: {e}", path.display()));
    }
}

/// The stem a disc is matched by: a playlist's first disc, else the ROM's own.
fn disc_stem(cwd: &Path, rom: &str) -> Option<String> {
    let path = Path::new(rom);
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("m3u"))
    {
        let text = std::fs::read_to_string(cwd.join(rom)).ok()?;
        let text = text.trim_start_matches('\u{feff}');
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.replace('\\', "/");
            return Path::new(&line)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string);
        }
        return None;
    }
    path.file_stem()?.to_str().map(str::to_string)
}

/// The name the DAT gives a game, by CRC for carts and by dump name for discs.
fn dat_name(cwd: &Path, sys: &SystemDef, rom: &str, dat: &Dat) -> Option<String> {
    if is_disc(sys, rom) {
        let stem = disc_stem(cwd, rom)?;
        return dat
            .game_named(&stem)
            .or_else(|| dat.game_named(without_disc(&stem)))
            .map(str::to_string);
    }
    crcs(&cwd.join(rom))
        .into_iter()
        .find_map(|crc| dat.game_for(crc).map(str::to_string))
}

/// Matches each game and saves the art found, one file at a time, until done
/// or `stop`. Stops for good when libretro cannot be reached.
fn cover_stem(rom: &str) -> Option<&str> {
    let mut parts = rom.split('/').skip(2);
    let first = parts.next()?;
    if parts.next().is_some() {
        Some(first)
    } else {
        Path::new(first).file_stem()?.to_str()
    }
}

pub fn run(job: Job, tx: Sender<Msg>, stop: &AtomicBool) {
    let total = job.games.len();
    let mut saved = 0;
    let mut missed = 0;
    let mut dats: HashMap<String, Dat> = HashMap::new();
    for (i, game) in job.games.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let _ = tx.send(Msg::Progress { done: i, total });
        let Some(collection) = collection(&game.system, &game.rom) else {
            crate::util::log(&format!("art: {} has no libretro collection", game.rom));
            missed += 1;
            continue;
        };
        let Some(stem) = cover_stem(&game.rom) else {
            missed += 1;
            continue;
        };
        let mut canonical: Option<String> = None;
        let mut fuzzy: Option<Vec<String>> = None;
        let mut any = false;
        let mut offline = false;
        'fill: {
            let (art_type, cover) = &game.cover;
            let folder = FOLDERS[(*art_type as usize).min(FOLDERS.len() - 1)];
            let mut found = match fetch(&collection, folder, stem) {
                Ok(found) => found,
                Err(()) => {
                    offline = true;
                    break 'fill;
                }
            };
            if found.is_none() {
                if !dats.contains_key(&collection) {
                    let dat = match get(&dat_url(&collection), 60) {
                        Some((200, body)) => Dat::parse(&String::from_utf8_lossy(&body)),
                        // A missing DAT is remembered too, so a system without
                        // one does not re-request it for every game.
                        Some(_) => Dat::default(),
                        None => {
                            offline = true;
                            break 'fill;
                        }
                    };
                    dats.insert(collection.clone(), dat);
                }
                if canonical.is_none() {
                    canonical = dats
                        .get(&collection)
                        .and_then(|dat| dat_name(&job.cwd, &game.system, &game.rom, dat))
                        .filter(|name| name.as_str() != stem);
                }
                if let Some(name) = &canonical {
                    found = match fetch(&collection, folder, name) {
                        Ok(found) => found,
                        Err(()) => {
                            offline = true;
                            break 'fill;
                        }
                    };
                }
                if found.is_none() {
                    if fuzzy.is_none() {
                        if let Some(dat) = dats.get(&collection) {
                            let input = if is_disc(&game.system, &game.rom) {
                                disc_stem(&job.cwd, &game.rom).unwrap_or_else(|| stem.to_string())
                            } else {
                                stem.to_string()
                            };
                            let mut names = dat.fuzzy(&input);
                            names.retain(|n| {
                                n.as_str() != stem && canonical.as_deref() != Some(n.as_str())
                            });
                            fuzzy = Some(names);
                        }
                    }
                    if let Some(names) = &fuzzy {
                        for name in names.iter().take(6) {
                            match fetch(&collection, folder, name) {
                                Ok(Some(png)) => {
                                    crate::util::log(&format!("art: fuzzy {stem} -> {name}"));
                                    found = Some(png);
                                    break;
                                }
                                Ok(None) => {}
                                Err(()) => {
                                    offline = true;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            if offline {
                break 'fill;
            }
            if let Some(png) = found {
                save(cover, &png);
                any = true;
                let _ = tx.send(Msg::Saved {
                    rom: game.rom.clone(),
                });
            }
        }
        if offline {
            let _ = tx.send(Msg::Finished {
                saved,
                missed,
                offline: true,
            });
            return;
        }
        if any {
            saved += 1;
        } else {
            missed += 1;
        }
    }
    let _ = tx.send(Msg::Finished {
        saved,
        missed,
        offline: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sys(id: &str) -> SystemDef {
        SystemDef {
            id: id.into(),
            name: id.to_uppercase(),
            exts: Vec::new(),
            core: None,
            folder: None,
            art: None,
        }
    }

    #[test]
    fn urls_are_escaped_and_filed_under_the_collection() {
        assert_eq!(
            art_url("Nintendo - Game Boy", "Named_Boxarts", "Tetris (World)"),
            "https://thumbnails.libretro.com/Nintendo%20-%20Game%20Boy/Named_Boxarts/Tetris%20%28World%29.png"
        );
        assert_eq!(
            dat_url("Sony - PlayStation"),
            "https://raw.githubusercontent.com/libretro/libretro-database/master/metadat/redump/Sony%20-%20PlayStation.dat"
        );
    }

    #[test]
    fn playlists_match_their_first_disc() {
        let dir = std::env::temp_dir().join(format!("magi_m3u_art_{}", std::process::id()));
        let sub = dir.join("Games/PS1");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("Game.m3u"), b"# list\nGame (Disc 1).cue\n").unwrap();
        let dat = Dat::parse(
            "game (\n name \"Game (Disc 1)\"\n rom ( name \"Game (Disc 1).cue\" size 1 crc 00000000 )\n)\n",
        );
        assert_eq!(
            dat_name(&dir, &sys("psx"), "Games/PS1/Game.m3u", &dat).as_deref(),
            Some("Game (Disc 1)")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn downloaded_art_is_capped_to_the_screen() {
        let (w, h) = (800u32, 600u32);
        let mut px = vec![0u32; (w * h) as usize];
        for (i, p) in px.iter_mut().enumerate() {
            *p = 0xFF00_0000 | (i as u32 & 0xFFFF);
        }
        let png = crate::image::encode_png(w, h, &px).unwrap();
        let out = optimize_art(&png).expect("large art is resized");
        assert!(out.len() <= ART_MAX_BYTES);
        let got = crate::image::decode_png(&out).unwrap();
        assert_eq!((got.w, got.h), (ART_MAX_W, ART_MAX_H));

        let small_px = vec![0xFF11_2233u32; 320 * 240];
        let small = crate::image::encode_png(320, 240, &small_px).unwrap();
        assert!(optimize_art(&small).is_none());
    }

    #[test]
    fn the_dat_maps_crcs_and_names() {
        let text = "game (\n name \"Tetris (World)\"\n rom ( name \"Tetris (World).gb\" size 32768 crc 5E5C8A5C )\n)\n";
        let dat = Dat::parse(text);
        assert_eq!(dat.game_for(0x5E5C_8A5C), Some("Tetris (World)"));
        assert_eq!(dat.game_named("tetris (world)"), Some("Tetris (World)"));
    }

    #[test]
    fn fuzzy_names_prefer_the_same_region_and_stable_dump() {
        let text = "game (\n name \"Tetris (Japan)\"\n)\ngame (\n name \"Tetris (USA)\"\n)\ngame (\n name \"Tetris (USA) (Beta)\"\n)\ngame (\n name \"Tetris (World)\"\n)\n";
        let dat = Dat::parse(text);
        let names = dat.fuzzy("Tetris (USA) (Rev 1)");
        assert_eq!(names.first().map(String::as_str), Some("Tetris (USA)"));
        assert_eq!(names.get(1).map(String::as_str), Some("Tetris (World)"));
        assert!(names.iter().any(|name| name.contains("Beta")));
        assert_eq!(
            names.last().map(String::as_str),
            Some("Tetris (USA) (Beta)"),
            "unstable dumps rank last"
        );
    }

    #[test]
    fn fuzzy_keys_ignore_region_and_disc_tags() {
        assert_eq!(base_key("Game (USA) (Disc 1) [b]"), "game");
        assert_eq!(regions("Game (USA, Europe)"), vec!["usa", "europe"]);
        assert_eq!(disc_tag("Game (Disc 1 of 2)"), Some("1 of 2"));
        assert_eq!(disc_number("1 of 2"), "1");
        assert_eq!(disc_tag("Game"), None);
    }

    #[test]
    fn cover_stem_matches_the_shelf_rule() {
        assert_eq!(
            cover_stem("Games/GB/Tetris (World).gb"),
            Some("Tetris (World)")
        );
        assert_eq!(cover_stem("Games/PS1/Game/disc.cue"), Some("Game"));
        assert_eq!(cover_stem("Games/N64/Zelda/rom.z64"), Some("Zelda"));
    }

    #[test]
    fn crc32_matches_the_standard() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
