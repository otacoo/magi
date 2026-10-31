//! Native N64 recompilation launcher (the Miyoo64 ports).
//!
//! The ports are 32-bit ARM binaries built for SDL 1.2. MAGI ships a small
//! armhf SDL 1.2 (Miyoo's fbcon/OSS build) and `libmiyoofb.so`, an LD_PRELOAD
//! shim that turns SDL video/input/audio into /dev/fb0, the H700 pad and
//! ALSA. A port installs as `Games/N64/<Game>/` with the port files and
//! `rom.z64` side by side; older installs under `App/<Port>/` still work.

#[cfg(target_os = "linux")]
use std::ffi::CString;
use std::fs::File;
use std::io::Read;
#[cfg(target_os = "linux")]
use std::io::{BufRead, BufReader};
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt;
#[cfg(target_os = "linux")]
use std::os::unix::io::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "linux")]
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::util;

#[cfg(target_os = "linux")]
use crate::canvas::Canvas;
#[cfg(target_os = "linux")]
use crate::defaults::Defaults;
#[cfg(target_os = "linux")]
use crate::font::{Fonts, FONT_DATA};
#[cfg(target_os = "linux")]
use crate::input::Action;
#[cfg(target_os = "linux")]
use crate::theme::pal;

/// Keep these in step with the libretro host: POWER taps sleep, holding it
/// starts the same abortable EVA shutdown countdown.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const POWER_HOLD: Duration = Duration::from_millis(800);
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const SHUTDOWN_TOTAL: Duration = Duration::from_millis(2500);
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const SHUTDOWN_PAGE_REFRESH: Duration = Duration::from_millis(80);
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const POWER_WAKE_GUARD: Duration = Duration::from_millis(1200);

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Default)]
struct PowerState {
    down: Option<Instant>,
    countdown: Option<Instant>,
    sent: bool,
    guard_until: Option<Instant>,
    last_page: Option<Instant>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PowerAction {
    Sleep,
    Abort,
    Start,
    Shutdown,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl PowerState {
    fn press(&mut self, now: Instant) {
        if self.sent || self.guard_until.is_some_and(|t| now < t) {
            return;
        }
        self.down = Some(now);
    }

    fn release(&mut self, now: Instant) -> Option<PowerAction> {
        if self.guard_until.is_some_and(|t| now < t) {
            self.down = None;
            return None;
        }
        if self.countdown.take().is_some() {
            self.down = None;
            self.last_page = None;
            return Some(PowerAction::Abort);
        }
        let down = self.down.take()?;
        if now.saturating_duration_since(down) < POWER_HOLD {
            Some(PowerAction::Sleep)
        } else {
            None
        }
    }

    fn after_sleep(&mut self, now: Instant) {
        self.down = None;
        self.countdown = None;
        self.last_page = None;
        self.guard_until = Some(now + POWER_WAKE_GUARD);
    }

    fn tick(&mut self, now: Instant) -> Option<PowerAction> {
        if let Some(down) = self.down {
            if self.countdown.is_none() && now.saturating_duration_since(down) >= POWER_HOLD {
                self.countdown = Some(now);
                self.last_page = None;
                return Some(PowerAction::Start);
            }
        }
        if let Some(countdown) = self.countdown {
            if !self.sent && now.saturating_duration_since(countdown) >= SHUTDOWN_TOTAL {
                self.sent = true;
                return Some(PowerAction::Shutdown);
            }
        }
        None
    }

    fn page_due(&mut self, now: Instant) -> bool {
        let Some(_) = self.countdown else {
            return false;
        };
        if self
            .last_page
            .is_some_and(|last| now.saturating_duration_since(last) < SHUTDOWN_PAGE_REFRESH)
        {
            return false;
        }
        self.last_page = Some(now);
        true
    }

    fn countdown(&self) -> Option<Instant> {
        self.countdown
    }
}

struct Port {
    app: &'static str,
    exe: &'static str,
    env: &'static str,
    size: u64,
    needle: &'static str,
    recipes: Option<&'static str>,
    data_subdir: bool,
}

const PORTS: [Port; 3] = [
    Port {
        app: "SuperMario64",
        exe: "sm64",
        env: "SM64",
        size: 8_388_608,
        needle: "super mario 64",
        recipes: Some("recipes.bin"),
        data_subdir: false,
    },
    Port {
        app: "FZeroX",
        exe: "fzerox",
        env: "FZX",
        size: 16_777_216,
        needle: "f-zero x",
        recipes: None,
        data_subdir: true,
    },
    Port {
        app: "MajorasMask",
        exe: "mm",
        env: "MM",
        size: 33_554_432,
        needle: "majora",
        recipes: Some("mm_recipes.bin"),
        data_subdir: true,
    },
];

impl Port {
    fn installed(&self, dir: &Path) -> bool {
        dir.join(self.exe).exists() && self.recipes.is_none_or(|r| dir.join(r).exists())
    }

    fn launch(&self, app: &Path) -> Launch {
        Launch {
            exe: app.join(self.exe),
            env: Some(self.env.to_string()),
            data_dir: if self.data_subdir {
                app.join("data")
            } else {
                app.to_path_buf()
            },
        }
    }
}

/// A port installed beside its ROM describes itself in `config.json` and
/// `launch.sh`; only legacy loose-ROM installs need the table above.
struct Launch {
    exe: PathBuf,
    env: Option<String>,
    data_dir: PathBuf,
}

fn discover_port(dir: &Path) -> Option<Launch> {
    let config: Option<serde_json::Value> = std::fs::read_to_string(dir.join("config.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let script = config
        .as_ref()
        .and_then(|v| v.get("launch").and_then(|x| x.as_str()))
        .unwrap_or("launch.sh");
    let script = std::fs::read_to_string(dir.join(script)).ok();
    let exe = config
        .as_ref()
        .and_then(|v| v.get("exec").and_then(|x| x.as_str()))
        .or_else(|| script.as_deref().and_then(script_exec))
        .map(|name| local_path(dir, name))
        .filter(|p| p.is_file())
        .or_else(|| elf_in(dir))?;
    let env = script.as_deref().and_then(script_env);
    let data_dir = env
        .as_deref()
        .and_then(|prefix| script.as_deref().and_then(|s| script_data(s, prefix, dir)))
        .unwrap_or_else(|| dir.to_path_buf());
    Some(Launch { exe, env, data_dir })
}

fn local_path(dir: &Path, name: &str) -> PathBuf {
    let name = name.trim_matches('"').trim_matches('\'');
    let name = name.strip_prefix("./").unwrap_or(name);
    let name = name
        .strip_prefix("$DIR/")
        .or_else(|| name.strip_prefix("${DIR}/"))
        .unwrap_or(name);
    if Path::new(name).is_absolute() {
        PathBuf::from(name)
    } else {
        dir.join(name)
    }
}

fn script_exec(script: &str) -> Option<&str> {
    for line in script.lines() {
        let Some(rest) = line.trim().strip_prefix("exec ") else {
            continue;
        };
        return rest.split_whitespace().next();
    }
    None
}

fn script_env(script: &str) -> Option<String> {
    for line in script.lines() {
        let Some(rest) = line.trim().strip_prefix("export ") else {
            continue;
        };
        let name = rest.split('=').next()?.trim();
        if let Some(prefix) = name.strip_suffix("_DATA") {
            if !prefix.is_empty()
                && prefix
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            {
                return Some(prefix.to_string());
            }
        }
    }
    None
}

fn script_data(script: &str, prefix: &str, dir: &Path) -> Option<PathBuf> {
    let key = format!("{prefix}_DATA=");
    for line in script.lines() {
        let Some(rest) = line.trim().strip_prefix("export ") else {
            continue;
        };
        let Some(pos) = rest.find(&key) else {
            continue;
        };
        let value = rest[pos + key.len()..].trim();
        let value = if let Some(v) = value.strip_prefix('"') {
            v.split('"').next().unwrap_or("")
        } else {
            value.split_whitespace().next().unwrap_or("")
        };
        let value = value.replace("${DIR}", "").replace("$DIR", "");
        let rel = value.trim_matches('/');
        return Some(if rel.is_empty() {
            dir.to_path_buf()
        } else {
            dir.join(rel)
        });
    }
    None
}

fn elf_in(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !path.is_file() || name.ends_with(".so") || name.ends_with(".bin") || name == "rom.z64" {
            continue;
        }
        let mut magic = [0u8; 4];
        let Ok(mut f) = File::open(&path) else {
            continue;
        };
        if f.read_exact(&mut magic).is_err() || &magic != b"\x7fELF" {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        if best.as_ref().is_none_or(|(s, _)| size > *s) {
            best = Some((size, path));
        }
    }
    best.map(|(_, p)| p)
}

/// Send the port's PCM to an aarch64 `aplay` on the routed output (Bluetooth
/// or HDMI). The 32-bit port cannot load the arm64 ALSA plugins itself.
#[cfg(target_os = "linux")]
struct AudioBridge {
    fifo: PathBuf,
    child: std::process::Child,
    _hold: File,
}

#[cfg(target_os = "linux")]
fn pump_log<R: Read + Send + 'static>(reader: R, path: &'static str) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => util::append_capped_path(path, &line),
                Err(_) => break,
            }
        }
    });
}

#[cfg(target_os = "linux")]
impl AudioBridge {
    fn start(device: &str) -> Option<Self> {
        let fifo = PathBuf::from("/tmp/magi-n64-audio.fifo");
        let _ = std::fs::remove_file(&fifo);
        let path = CString::new(fifo.as_os_str().as_bytes()).ok()?;
        if unsafe { libc::mkfifo(path.as_ptr(), 0o600) } != 0 {
            return None;
        }
        // Keep a read/write endpoint so the non-blocking shim writer always
        // opens and never sees a short-lived EOF while aplay starts.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd < 0 {
            let _ = std::fs::remove_file(&fifo);
            return None;
        }
        let hold = unsafe { File::from_raw_fd(fd) };
        let _ = std::fs::write("Logs/magi-n64-aplay.log", b"");
        let mut cmd = Command::new("aplay");
        cmd.args(["-q", "-D", device])
            .arg(&fifo)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                util::log(&format!("n64: aplay unavailable: {e}"));
                let _ = std::fs::remove_file(&fifo);
                return None;
            }
        };
        if let Some(out) = child.stdout.take() {
            pump_log(out, "Logs/magi-n64-aplay.log");
        }
        if let Some(err) = child.stderr.take() {
            pump_log(err, "Logs/magi-n64-aplay.log");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        if child.try_wait().ok().flatten().is_some() {
            let _ = std::fs::remove_file(&fifo);
            return None;
        }
        util::log("n64: bluetooth audio bridge up");
        Some(Self {
            fifo,
            child,
            _hold: hold,
        })
    }
}

#[cfg(target_os = "linux")]
impl Drop for AudioBridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.fifo);
        util::log("n64: bluetooth audio bridge down");
    }
}

/// Legacy loose-ROM installs are matched by ROM name, game folder or config
/// label. Ports in a Games/N64 game folder describe themselves instead.
fn find_port(rom: &Path, size: u64) -> Option<&'static Port> {
    let mut hay = rom
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if let Some(parent) = rom.parent() {
        if let Some(folder) = parent.file_name() {
            hay.push(' ');
            hay.push_str(&folder.to_string_lossy().to_ascii_lowercase());
        }
        if let Ok(text) = std::fs::read_to_string(parent.join("config.json")) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(label) = json.get("label").and_then(|v| v.as_str()) {
                    hay.push(' ');
                    hay.push_str(&label.to_ascii_lowercase());
                }
            }
        }
    }
    PORTS
        .iter()
        .find(|p| hay.contains(p.needle) && size == p.size)
}

pub fn main(args: &[String]) -> Result<()> {
    let mut rom = None;
    let mut it = args.iter().skip(2);
    while let Some(a) = it.next() {
        if a == "--rom" {
            rom = it.next().cloned();
        }
    }
    let rom = rom.context("n64: missing --rom")?;
    let rom_path = PathBuf::from(&rom);
    let size = std::fs::metadata(&rom_path)
        .with_context(|| format!("n64: cannot read {rom}"))?
        .len();

    let base = std::env::current_dir().context("n64: no working directory")?;
    let beside = rom_path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .and_then(|p| discover_port(&p).map(|launch| (p, launch)));
    let (app, launch) = match beside {
        Some((app, launch)) => (app, launch),
        None => {
            let Some(port) = find_port(&rom_path, size) else {
                bail!("n64: no port folder beside {rom} ({size} bytes)");
            };
            let app = base
                .join("App")
                .join(port.app)
                .canonicalize()
                .with_context(|| format!("n64: port path App/{}", port.app))?;
            if !port.installed(&app) {
                bail!("n64: port not installed in App/{}", port.app);
            }
            let launch = port.launch(&app);
            (app, launch)
        }
    };
    std::fs::create_dir_all(&launch.data_dir).with_context(|| {
        format!(
            "n64: cannot create data directory {}",
            launch.data_dir.display()
        )
    })?;

    let shim_dir = base.join("System/magi/n64");
    let shim = shim_dir.join("libmiyoofb.so");
    let pathlib = shim_dir.join("libn64path.so");
    let sdl = shim_dir.join("libSDL-1.2.so.0");
    if !shim.exists() || !sdl.exists() {
        bail!("n64: runtime missing in System/magi/n64");
    }
    let shim = shim.canonicalize().context("n64: shim path")?;
    let preload = if pathlib.exists() {
        let pathlib = pathlib.canonicalize().context("n64: path shim")?;
        format!("{}:{}", shim.display(), pathlib.display())
    } else {
        shim.display().to_string()
    };
    let shim_dir = shim_dir.canonicalize().context("n64: runtime path")?;

    let dest = app.join("rom.z64");
    let same = rom_path.canonicalize().map(|p| p == dest).unwrap_or(false);
    let dest_len = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    if !same && dest_len != size {
        let tmp = app.join("rom.z64.tmp");
        std::fs::copy(&rom_path, &tmp)
            .with_context(|| format!("n64: cannot copy {rom} to {}", tmp.display()))?;
        std::fs::rename(&tmp, &dest).context("n64: cannot install rom.z64")?;
        util::log(&format!("n64: ROM copied to {}", dest.display()));
    }

    #[cfg(target_os = "linux")]
    {
        let _ = std::fs::write("Logs/magi-n64.log", b"");
    }
    let mut lib_paths = Vec::new();
    if app.join("lib").is_dir() {
        lib_paths.push(app.join("lib"));
    }
    lib_paths.push(shim_dir.clone());
    let lib_path = std::env::join_paths(&lib_paths).unwrap_or_else(|_| shim_dir.clone().into());
    let mut cmd = Command::new(&launch.exe);
    cmd.current_dir(&app)
        .env("HOME", &app)
        .env("LD_PRELOAD", &preload)
        .env("LD_LIBRARY_PATH", lib_path)
        .env("MAGI_N64_DATA", app.join("data"))
        .env("SDL_AUDIODRIVER", "dummy");
    if let Some(prefix) = &launch.env {
        cmd.env(format!("{prefix}_DATA"), &launch.data_dir)
            .env(format!("{prefix}_ROTATE"), "0")
            .env(format!("{prefix}_CPUCLOCK"), "0")
            .env(format!("{prefix}_SLEEPSAVE"), "0");
    }
    #[cfg(target_os = "linux")]
    let mut bridge: Option<AudioBridge> = None;
    #[cfg(target_os = "linux")]
    {
        // The port is 32-bit and cannot load the arm64 ALSA plugins, so a
        // routed output (Bluetooth, HDMI on a TV) goes through aplay.
        let route = util::bt_audio_device()
            .or_else(|| util::hdmi_active().then(|| "playback_hdmi".to_string()));
        if let Some(device) = route {
            util::log(&format!("n64: audio output {device}"));
            cmd.env("MAGI_ALSA_DEVICE", &device);
            if let Some(audio) = AudioBridge::start(&device) {
                cmd.env("MAGI_AUDIO_FIFO", &audio.fifo);
                bridge = Some(audio);
            }
        }
    }
    #[cfg(target_os = "linux")]
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    util::log(&format!(
        "n64: {} -> {}",
        rom_path.display(),
        launch.exe.display()
    ));
    #[cfg(target_os = "linux")]
    {
        util::set_cpu_governor("performance");
        let display = load_display();
        let result = supervise(cmd, display);
        drop(bridge);
        util::set_cpu_governor("schedutil");
        result
    }
    #[cfg(not(target_os = "linux"))]
    {
        let status = cmd.status().context("n64: cannot start the port")?;
        if status.success() {
            util::log("n64: port exited");
            Ok(())
        } else {
            bail!("n64: port exited: {status}")
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, PartialEq)]
enum HudKind {
    Volume,
    Brightness,
    Warmth,
}

#[cfg(target_os = "linux")]
impl HudKind {
    fn label(self) -> &'static str {
        match self {
            Self::Volume => "VOLUME",
            Self::Brightness => "BRIGHTNESS",
            Self::Warmth => "TEMPERATURE",
        }
    }
}

#[cfg(target_os = "linux")]
struct Display {
    volume: u32,
    brightness: u32,
    warmth: u32,
    theme: u8,
    background: u8,
    anim_bg: bool,
    glow: bool,
    text_scale: f32,
    lid_sleep: bool,
    shutdown_min: u32,
}

#[cfg(target_os = "linux")]
fn load_display() -> Display {
    let v: serde_json::Value = std::fs::read_to_string("Config/magi.json")
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    Display {
        volume: v
            .get("volume")
            .and_then(|x| x.as_u64())
            .map(|x| x.min(100) as u32)
            .unwrap_or(100),
        brightness: v
            .get("brightness")
            .and_then(|x| x.as_u64())
            .map(|x| util::clamp_brightness(x.min(100) as u32))
            .or_else(|| util::brightness_now().map(util::clamp_brightness))
            .unwrap_or(util::STOCK_BRIGHTNESS),
        warmth: v
            .get("warmth")
            .and_then(|x| x.as_u64())
            .map(|x| x.min(100) as u32)
            .unwrap_or(0),
        theme: v
            .get("theme")
            .and_then(|x| x.as_u64())
            .unwrap_or(Defaults::THEME as u64) as u8,
        background: v
            .get("background")
            .and_then(|x| x.as_u64())
            .unwrap_or(Defaults::BACKGROUND as u64) as u8,
        anim_bg: v
            .get("anim_bg")
            .and_then(|x| x.as_bool())
            .unwrap_or(Defaults::ANIM_BG),
        glow: v
            .get("glow")
            .and_then(|x| x.as_bool())
            .unwrap_or(Defaults::GLOW),
        text_scale: v
            .get("text_scale")
            .and_then(|x| x.as_f64())
            .map(|x| x.clamp(0.5, 2.0) as f32)
            .unwrap_or(Defaults::TEXT_SCALE),
        lid_sleep: v
            .get("lid_sleep")
            .and_then(|x| x.as_bool())
            .unwrap_or(Defaults::LID_SLEEP),
        shutdown_min: v
            .get("shutdown_min")
            .and_then(|x| x.as_u64())
            .map(|x| x as u32)
            .unwrap_or(Defaults::SHUTDOWN_MIN),
    }
}

#[cfg(target_os = "linux")]
fn save_display(d: &Display) {
    let path = std::path::Path::new("Config/magi.json");
    let mut v: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    v["volume"] = serde_json::json!(d.volume);
    v["brightness"] = serde_json::json!(d.brightness);
    v["warmth"] = serde_json::json!(d.warmth);
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::create_dir_all("Config");
        if let Err(e) = util::write_file_fast(path, text.as_bytes()) {
            util::log(&format!("n64: cannot save display: {e}"));
        }
    }
}

#[cfg(target_os = "linux")]
fn write_control_files(d: &Display) {
    let _ = util::write_file_atomic(
        std::path::Path::new("/tmp/magi-n64-volume"),
        d.volume.to_string().as_bytes(),
    );
    let _ = util::write_file_atomic(
        std::path::Path::new("/tmp/magi-n64-warmth"),
        d.warmth.to_string().as_bytes(),
    );
}

#[cfg(target_os = "linux")]
fn write_osd(fonts: &mut Fonts, kind: HudKind, pct: u32) {
    let (w, h) = (340i32, 64i32);
    let mut cv = Canvas::new(w as u32, h as u32);
    cv.clear(pal().bg0);
    cv.fill_rect(0, 0, w as u32, h as u32, pal().bg1);
    cv.rect_outline(0, 0, w as u32, h as u32, pal().border);
    cv.fill_rect(12, 14, 5, 20, pal().accent);
    fonts.draw(&mut cv, 26, 29, kind.label(), 12.0, pal().text);
    let value = format!("{pct}%");
    let vw = fonts.text_width(&value, 12.0);
    fonts.draw(&mut cv, w - 14 - vw, 29, &value, 12.0, pal().accent);
    cv.fill_rect(26, 44, (w - 40) as u32, 10, pal().bg2);
    cv.rect_outline(25, 43, (w - 38) as u32, 12, pal().border);
    let fill = (w - 44) as u32 * pct / 100;
    let heat = pal().heat[((pct as usize * 4 + 50) / 100).min(4)];
    cv.fill_rect(27, 45, fill, 6, heat);

    let dev = crate::device::detect();
    let x = (dev.width as i32 - w) / 2;
    let y = dev.height as i32 - h - 26;
    write_osd_canvas(&cv, x, y, now_ms() + 1500);
}

#[cfg(target_os = "linux")]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn write_osd_canvas(cv: &Canvas, x: i32, y: i32, expire: u64) {
    let mut bytes = Vec::with_capacity(32 + cv.px.len() * 4);
    bytes.extend_from_slice(&0x4F53_4431u32.to_le_bytes());
    bytes.extend_from_slice(&x.to_le_bytes());
    bytes.extend_from_slice(&y.to_le_bytes());
    bytes.extend_from_slice(&(cv.w as u32).to_le_bytes());
    bytes.extend_from_slice(&(cv.h as u32).to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&expire.to_le_bytes());
    for p in &cv.px {
        bytes.extend_from_slice(&p.to_le_bytes());
    }
    if let Err(e) = util::write_file_atomic(std::path::Path::new("/tmp/magi-n64-osd"), &bytes) {
        util::log(&format!("n64: cannot write osd: {e}"));
    }
}

#[cfg(target_os = "linux")]
fn write_shutdown_osd(fonts: &mut Fonts, remaining: Duration, t: f32) {
    let dev = crate::device::detect();
    let mut cv = Canvas::new(dev.width, dev.height);
    crate::theme::draw_shutdown(&mut cv, fonts, remaining, SHUTDOWN_TOTAL, t);
    crate::theme::apply_warmth(&mut cv);
    write_osd_canvas(&cv, 0, 0, now_ms() + 6000);
}

#[cfg(target_os = "linux")]
fn remove_osd() {
    let _ = std::fs::remove_file("/tmp/magi-n64-osd");
}

#[cfg(target_os = "linux")]
fn adjust(d: &mut Display, a: Action) -> Option<HudKind> {
    match a {
        Action::VolumeUp => {
            d.volume = (d.volume + 5).min(100);
            Some(HudKind::Volume)
        }
        Action::VolumeDown => {
            d.volume = d.volume.saturating_sub(5);
            Some(HudKind::Volume)
        }
        Action::BrightnessUp => {
            d.brightness = (d.brightness + 5).min(100);
            util::set_brightness(d.brightness);
            Some(HudKind::Brightness)
        }
        Action::BrightnessDown => {
            d.brightness = util::clamp_brightness(d.brightness.saturating_sub(5));
            util::set_brightness(d.brightness);
            Some(HudKind::Brightness)
        }
        Action::WarmthUp => {
            d.warmth = (d.warmth + 5).min(100);
            Some(HudKind::Warmth)
        }
        Action::WarmthDown => {
            d.warmth = d.warmth.saturating_sub(5);
            Some(HudKind::Warmth)
        }
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn hud_value(d: &Display, kind: HudKind) -> u32 {
    match kind {
        HudKind::Volume => d.volume,
        HudKind::Brightness => d.brightness,
        HudKind::Warmth => d.warmth,
    }
}

#[cfg(target_os = "linux")]
fn cleanup() {
    let _ = std::fs::remove_file("/tmp/magi-n64-volume");
    let _ = std::fs::remove_file("/tmp/magi-n64-warmth");
    remove_osd();
}

#[cfg(target_os = "linux")]
fn supervise(mut cmd: Command, mut display: Display) -> Result<()> {
    write_control_files(&display);
    let mut fonts = Fonts::new(FONT_DATA).context("n64: cannot load the OSD font")?;
    crate::theme::load_user_themes(std::path::Path::new(crate::systems::DIR_THEMES));
    crate::theme::set_theme_index(display.theme);
    crate::theme::set_bg_style(display.background);
    crate::theme::set_anim_bg(display.anim_bg);
    crate::theme::set_glow(display.glow);
    crate::theme::set_ui_scale(display.text_scale);
    crate::theme::set_warmth(display.warmth as u8);
    util::set_brightness(display.brightness);
    let mut evdev = crate::evdev::Evdev::open();
    let mut lid = util::LidWatch::new();
    let mut power = PowerState::default();
    let play_t0 = Instant::now();
    let mut child = cmd.spawn().context("n64: cannot start the port")?;
    if let Some(out) = child.stdout.take() {
        pump_log(out, "Logs/magi-n64.log");
    }
    if let Some(err) = child.stderr.take() {
        pump_log(err, "Logs/magi-n64.log");
    }
    loop {
        if lid.poll() {
            if display.lid_sleep {
                util::log("n64: lid closed; sleeping");
                util::sleep_now(display.shutdown_min * 60);
            } else {
                util::log("n64: lid closed; lid sleep is off");
            }
        }
        for action in evdev.poll() {
            match action {
                Action::Power => power.press(Instant::now()),
                Action::PowerRelease => match power.release(Instant::now()) {
                    Some(PowerAction::Sleep) => {
                        util::log("n64: power tap: sleep");
                        evdev.flush();
                        util::sleep_now(display.shutdown_min * 60);
                        power.after_sleep(Instant::now());
                        evdev.flush();
                    }
                    Some(PowerAction::Abort) => {
                        util::log("n64: power: shutdown aborted");
                        remove_osd();
                    }
                    _ => {}
                },
                _ if power.countdown().is_some() => {}
                _ => {
                    if let Some(kind) = adjust(&mut display, action) {
                        save_display(&display);
                        write_control_files(&display);
                        let pct = hud_value(&display, kind);
                        write_osd(&mut fonts, kind, pct);
                        util::log(&format!("n64: {} {pct}%", kind.label()));
                    }
                }
            }
        }
        let now = Instant::now();
        match power.tick(now) {
            Some(PowerAction::Start) => util::log("n64: power: shutdown countdown"),
            Some(PowerAction::Shutdown) => {
                util::log("n64: power: shutting down");
                util::set_brightness(0);
                let _ = child.kill();
                let _ = child.wait();
                util::spawn_detached("poweroff");
                return Ok(());
            }
            _ => {}
        }
        if power.page_due(now) {
            if let Some(t0) = power.countdown() {
                let remaining = SHUTDOWN_TOTAL.saturating_sub(now.saturating_duration_since(t0));
                write_shutdown_osd(&mut fonts, remaining, play_t0.elapsed().as_secs_f32());
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                cleanup();
                // These third-party ports often finish with SIGABRT after
                // writing their clean SESSION END marker; a non-zero status
                // is not a launch failure.
                util::log(&format!("n64: port exited: {status}"));
                return Ok(());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(8)),
            Err(e) => {
                cleanup();
                return Err(e).context("n64: cannot wait for the port");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{discover_port, find_port, script_data, script_env, script_exec};

    #[test]
    fn port_matches_the_game_folder_and_label() {
        let dir = std::env::temp_dir().join(format!("magi_n64_port_{}", std::process::id()));
        for (folder, label, size, app) in [
            ("port", "Super Mario 64", 8_388_608, "SuperMario64"),
            ("FZeroX", "F-Zero X", 16_777_216, "FZeroX"),
            ("MajorasMask", "Majora's Mask", 33_554_432, "MajorasMask"),
        ] {
            let game = dir.join("Games/N64").join(folder);
            std::fs::create_dir_all(&game).unwrap();
            let config = serde_json::json!({ "label": label }).to_string();
            std::fs::write(game.join("config.json"), config).unwrap();
            let p = find_port(&game.join("rom.z64"), size).unwrap();
            assert_eq!(p.app, app);
        }
        let rom = dir.join("Games/N64/port/rom.z64");
        assert!(find_port(&rom, 1).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ports_read_their_launch_script() {
        let sm64 = "export SM64_DATA=\"$DIR\" # game data\nexec ./sm64\n";
        assert_eq!(script_exec(sm64), Some("./sm64"));
        assert_eq!(script_env(sm64).as_deref(), Some("SM64"));
        assert_eq!(
            script_data(sm64, "SM64", std::path::Path::new("/game")),
            Some(std::path::PathBuf::from("/game"))
        );

        let fzx = "export FZX_DATA=\"$DIR/data\" # game data\nexec ./fzerox\n";
        assert_eq!(script_exec(fzx), Some("./fzerox"));
        assert_eq!(script_env(fzx).as_deref(), Some("FZX"));
        assert_eq!(
            script_data(fzx, "FZX", std::path::Path::new("/game")),
            Some(std::path::PathBuf::from("/game/data"))
        );
    }

    #[test]
    fn discovers_a_port_folder_without_a_table_entry() {
        let dir = std::env::temp_dir().join(format!("magi_n64_discover_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.json"),
            br#"{"label":"Some Port","launch":"launch.sh"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("launch.sh"),
            "export AWESOME_DATA=\"$DIR\"\nexec ./awesome\n",
        )
        .unwrap();
        std::fs::write(dir.join("awesome"), b"not an elf").unwrap();
        let launch = discover_port(&dir).unwrap();
        assert_eq!(launch.exe, dir.join("awesome"));
        assert_eq!(launch.env.as_deref(), Some("AWESOME"));
        assert_eq!(launch.data_dir, dir);
        std::fs::remove_dir_all(&dir).ok();
    }
}
