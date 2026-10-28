//! Runtime preferences: volume, display, scale mode and the JSON store.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::defaults::Defaults;
use crate::util;

pub(super) fn runtime_config_path() -> &'static str {
    "Config/magi.json"
}

pub(super) fn load_runtime_volume() -> u8 {
    std::fs::read_to_string(runtime_config_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("volume").and_then(|x| x.as_u64()))
        .map(|v| v.min(100) as u8)
        .unwrap_or(100)
}

/// Fast-forward preferences: steps per presented frame and whether audio
/// keeps playing while fast-forwarding.
pub(super) fn load_runtime_prefs() -> (u32, bool) {
    let v: serde_json::Value = std::fs::read_to_string(runtime_config_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let steps = v
        .get("ff_steps")
        .and_then(|x| x.as_u64())
        .map(|x| x.clamp(2, 6) as u32)
        .unwrap_or(Defaults::FF_STEPS);
    let sound = v
        .get("ff_sound")
        .and_then(|x| x.as_bool())
        .unwrap_or(Defaults::FF_SOUND);
    (steps, sound)
}

/// The screenshot-chord preference (MISC page), read once per launch.
pub(super) fn load_shot_chord_pref() -> bool {
    std::fs::read_to_string(runtime_config_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("shot_chord").and_then(|x| x.as_bool()))
        .unwrap_or(Defaults::SHOT_CHORD)
}

pub(super) fn save_runtime_volume(pct: u8) {
    let path = Path::new(runtime_config_path());
    let mut v: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    v["volume"] = serde_json::json!(pct);
    match serde_json::to_string_pretty(&v) {
        Ok(text) => {
            let _ = std::fs::create_dir_all("Config");
            if let Err(e) = util::write_file_fast(path, text.as_bytes()) {
                util::log(&format!("volume: cannot save: {e}"));
            }
        }
        Err(e) => util::log(&format!("volume: cannot encode: {e}")),
    }
}

/// Two R2 presses this close together latch fast-forward on or off.
pub(super) const DOUBLE_TAP: Duration = Duration::from_millis(400);
/// SELECT + d-pad: first repeat after this, then this often.
pub(super) const COMBO_FIRST_REPEAT: Duration = Duration::from_millis(350);
pub(super) const COMBO_REPEAT: Duration = Duration::from_millis(110);
/// Brightness and warmth steps, in percent.
pub(super) const COMBO_STEP: i32 = 5;
/// Analog triggers read as pressed past this (Xbox axes span 0..1023).
pub(super) const TRIGGER_ON: i32 = 300;

/// How the game image fills the panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ScaleMode {
    /// Fractional, aspect preserved (the historic default).
    Fit,
    /// Whole-number pixels, aspect preserved.
    Integer,
    /// Fill the panel, aspect ignored.
    Stretch,
}

impl ScaleMode {
    pub(super) fn next(self) -> Self {
        match self {
            ScaleMode::Fit => ScaleMode::Integer,
            ScaleMode::Integer => ScaleMode::Stretch,
            ScaleMode::Stretch => ScaleMode::Fit,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            ScaleMode::Fit => "SCALE FIT",
            ScaleMode::Integer => "SCALE INTEGER",
            ScaleMode::Stretch => "SCALE STRETCH",
        }
    }

    pub(super) fn as_u8(self) -> u8 {
        match self {
            ScaleMode::Fit => 0,
            ScaleMode::Integer => 1,
            ScaleMode::Stretch => 2,
        }
    }

    pub(super) fn from_u8(v: u8) -> Self {
        match v {
            1 => ScaleMode::Integer,
            2 => ScaleMode::Stretch,
            _ => ScaleMode::Fit,
        }
    }
}

/// Per-axis scale factors for a mode; `blit` clamps the result to the panel.
pub(super) fn scale_factors(
    mode: ScaleMode,
    cw: usize,
    ch: usize,
    fw: usize,
    fh: usize,
) -> (f32, f32) {
    let fit = f32::min(cw as f32 / fw as f32, ch as f32 / fh as f32);
    match mode {
        ScaleMode::Fit => (fit.max(0.01), fit.max(0.01)),
        ScaleMode::Integer => {
            let s = fit.floor().max(1.0);
            (s, s)
        }
        ScaleMode::Stretch => (cw as f32 / fw as f32, ch as f32 / fh as f32),
    }
}

/// Display preferences, persisted next to the volume: scale mode, panel
/// brightness and the warm (blue-reducing) tint.
#[derive(Clone, Copy)]
pub(super) struct DisplayPrefs {
    pub(super) theme: u8,
    pub(super) background: u8,
    pub(super) anim_bg: bool,
    pub(super) glow: bool,
    pub(super) scale: ScaleMode,
    pub(super) brightness: u32,
    pub(super) warmth: u32,
    pub(super) lid_sleep: bool,
    pub(super) shutdown_min: u32,
    pub(super) rumble: bool,
    pub(super) bat_shutdown: u32,
    pub(super) charge_limit: u32,
    pub(super) fps_counter: bool,
    pub(super) sysinfo: bool,
}

pub(super) fn load_display_prefs() -> DisplayPrefs {
    let v: serde_json::Value = std::fs::read_to_string(runtime_config_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let brightness = v
        .get("brightness")
        .and_then(|x| x.as_u64())
        .map(|x| x.min(100) as u32)
        // Never surprise the user: unsaved brightness is what the panel shows.
        .or_else(crate::util::brightness_now)
        .unwrap_or(crate::util::STOCK_BRIGHTNESS);
    DisplayPrefs {
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
        scale: ScaleMode::from_u8(v.get("scale_mode").and_then(|x| x.as_u64()).unwrap_or(0) as u8),
        brightness,
        warmth: v
            .get("warmth")
            .and_then(|x| x.as_u64())
            .map(|x| x.min(100) as u32)
            .unwrap_or(0),
        lid_sleep: v
            .get("lid_sleep")
            .and_then(|x| x.as_bool())
            .unwrap_or(Defaults::LID_SLEEP),
        rumble: v
            .get("rumble")
            .and_then(|x| x.as_bool())
            .unwrap_or(Defaults::RUMBLE),
        bat_shutdown: v
            .get("bat_shutdown")
            .and_then(|x| x.as_u64())
            .unwrap_or(Defaults::BAT_SHUTDOWN as u64) as u32,
        charge_limit: v
            .get("charge_limit")
            .and_then(|x| x.as_u64())
            .unwrap_or(Defaults::CHARGE_LIMIT as u64) as u32,
        shutdown_min: v
            .get("shutdown_min")
            .and_then(|x| x.as_u64())
            .map(|x| x as u32)
            .unwrap_or(Defaults::SHUTDOWN_MIN),
        fps_counter: v
            .get("fps_counter")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        sysinfo: v.get("sysinfo").and_then(|x| x.as_bool()).unwrap_or(false),
    }
}

pub(super) fn save_display_prefs(p: &DisplayPrefs) {
    let path = Path::new(runtime_config_path());
    let mut v: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    v["scale_mode"] = serde_json::json!(p.scale.as_u8());
    v["rumble"] = serde_json::json!(p.rumble);
    v["brightness"] = serde_json::json!(p.brightness);
    v["warmth"] = serde_json::json!(p.warmth);
    v["fps_counter"] = serde_json::json!(p.fps_counter);
    v["sysinfo"] = serde_json::json!(p.sysinfo);
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::create_dir_all("Config");
        if let Err(e) = util::write_file_fast(path, text.as_bytes()) {
            util::log(&format!("display: cannot save: {e}"));
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum AdjKind {
    Brightness,
    Warmth,
}

pub(super) struct AdjState {
    pub(super) kind: AdjKind,
    pub(super) dir: i32,
    pub(super) next: Instant,
}
