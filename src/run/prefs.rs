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
/// The shelf text-size setting, clamped to the available steps.
pub(super) fn load_text_scale() -> f32 {
    std::fs::read_to_string(runtime_config_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("text_scale").and_then(|x| x.as_f64()))
        .map(|v| (v as f32).clamp(1.0, 1.5))
        .unwrap_or(Defaults::TEXT_SCALE)
}

pub(super) fn load_shot_chord_pref() -> bool {
    std::fs::read_to_string(runtime_config_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("shot_chord").and_then(|x| x.as_bool()))
        .unwrap_or(Defaults::SHOT_CHORD)
}

/// Per-system A/B swap chosen in the shelf's SYSTEM tab.
pub(super) fn load_swap_ab(system: &str) -> bool {
    std::fs::read_to_string(runtime_config_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .map(|v| swap_ab_in(&v, system))
        .unwrap_or(false)
}

fn swap_ab_in(v: &serde_json::Value, system: &str) -> bool {
    v.get("swap_ab")
        .and_then(|m| m.get(system))
        .and_then(|x| x.as_bool())
        .unwrap_or(false)
}

/// Per-system L1/R1 swap chosen in the shelf's SYSTEM tab (Dreamcast).
pub(super) fn load_swap_l1r1(system: &str) -> bool {
    std::fs::read_to_string(runtime_config_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .map(|v| swap_l1r1_in(&v, system))
        .unwrap_or(false)
}

fn swap_l1r1_in(v: &serde_json::Value, system: &str) -> bool {
    v.get("swap_l1r1")
        .and_then(|m| m.get(system))
        .and_then(|x| x.as_bool())
        .unwrap_or(false)
}

/// D-pad as the left analog stick. The handheld has no stick, and Dreamcast
/// needs one for many games, so Dreamcast defaults on.
pub(super) fn load_analog_dpad(system: &str) -> bool {
    let fallback = system == "dc";
    std::fs::read_to_string(runtime_config_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .map(|v| analog_dpad_in(&v, system, fallback))
        .unwrap_or(fallback)
}

fn analog_dpad_in(v: &serde_json::Value, system: &str, fallback: bool) -> bool {
    v.get("analog_dpad")
        .and_then(|m| m.get(system))
        .and_then(|x| x.as_bool())
        .unwrap_or(fallback)
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

/// Screen shader applied to the game image by the GLES presenter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShaderMode {
    Off,
    GbaColor,
    GbcColor,
    Lcd3x,
    Dot,
    LcdAccurate,
}

impl ShaderMode {
    pub(super) fn next(self) -> Self {
        match self {
            ShaderMode::Off => ShaderMode::GbaColor,
            ShaderMode::GbaColor => ShaderMode::GbcColor,
            ShaderMode::GbcColor => ShaderMode::Lcd3x,
            ShaderMode::Lcd3x => ShaderMode::Dot,
            ShaderMode::Dot => ShaderMode::LcdAccurate,
            ShaderMode::LcdAccurate => ShaderMode::Off,
        }
    }

    pub(super) fn prev(self) -> Self {
        match self {
            ShaderMode::Off => ShaderMode::LcdAccurate,
            ShaderMode::GbaColor => ShaderMode::Off,
            ShaderMode::GbcColor => ShaderMode::GbaColor,
            ShaderMode::Lcd3x => ShaderMode::GbcColor,
            ShaderMode::Dot => ShaderMode::Lcd3x,
            ShaderMode::LcdAccurate => ShaderMode::Dot,
        }
    }

    pub(super) fn supported(self, system: &str) -> bool {
        self != ShaderMode::LcdAccurate || matches!(system, "gb" | "gbc" | "ngp" | "ngpc")
    }

    pub(super) fn next_for(self, system: &str) -> Self {
        let mut mode = self.next();
        while !mode.supported(system) {
            mode = mode.next();
        }
        mode
    }

    pub(super) fn prev_for(self, system: &str) -> Self {
        let mut mode = self.prev();
        while !mode.supported(system) {
            mode = mode.prev();
        }
        mode
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            ShaderMode::Off => "OFF",
            ShaderMode::GbaColor => "GBA COLOR",
            ShaderMode::GbcColor => "GBC COLOR",
            ShaderMode::Lcd3x => "LCD3X",
            ShaderMode::Dot => "DOT",
            ShaderMode::LcdAccurate => "LCD ACCURATE",
        }
    }

    pub(super) fn as_u8(self) -> u8 {
        match self {
            ShaderMode::Off => 0,
            ShaderMode::GbaColor => 1,
            ShaderMode::GbcColor => 2,
            ShaderMode::Lcd3x => 3,
            ShaderMode::Dot => 4,
            ShaderMode::LcdAccurate => 5,
        }
    }

    pub(super) fn from_u8(v: u8) -> Self {
        match v {
            1 => ShaderMode::GbaColor,
            2 => ShaderMode::GbcColor,
            3 => ShaderMode::Lcd3x,
            4 => ShaderMode::Dot,
            5 => ShaderMode::LcdAccurate,
            _ => ShaderMode::Off,
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
    pub(super) shader: ShaderMode,
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

fn load_scale(v: &serde_json::Value, system: &str) -> ScaleMode {
    let per_system = v
        .get("scales")
        .and_then(|s| s.get(system))
        .and_then(|x| x.as_u64());
    let global = v.get("scale_mode").and_then(|x| x.as_u64());
    let chosen = per_system.or(if system == "psx" { None } else { global });
    chosen
        .map(|x| ScaleMode::from_u8(x as u8))
        .unwrap_or(if system == "psx" {
            ScaleMode::Stretch
        } else {
            ScaleMode::Fit
        })
}

pub(super) fn load_display_prefs(system: &str) -> DisplayPrefs {
    let v: serde_json::Value = std::fs::read_to_string(runtime_config_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let brightness = v
        .get("brightness")
        .and_then(|x| x.as_u64())
        .map(|x| crate::util::clamp_brightness(x.min(100) as u32))
        // Never surprise the user: unsaved brightness is what the panel shows.
        .or_else(|| crate::util::brightness_now().map(crate::util::clamp_brightness))
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
        scale: load_scale(&v, system),
        shader: ShaderMode::from_u8(v.get("shader").and_then(|x| x.as_u64()).unwrap_or(0) as u8),
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

pub(super) fn save_display_prefs(system: &str, p: &DisplayPrefs) {
    let path = Path::new(runtime_config_path());
    let mut v: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if !v.get("scales").is_some_and(|s| s.is_object()) {
        v["scales"] = serde_json::json!({});
    }
    v["scales"][system] = serde_json::json!(p.scale.as_u8());
    if system != "psx" {
        v["scale_mode"] = serde_json::json!(p.scale.as_u8());
    }
    v["shader"] = serde_json::json!(p.shader.as_u8());
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

#[cfg(test)]
mod tests {
    use super::{load_scale, swap_ab_in, swap_l1r1_in, ScaleMode, ShaderMode};

    #[test]
    fn psx_scale_defaults_to_stretch() {
        let empty = serde_json::json!({});
        assert_eq!(load_scale(&empty, "psx"), ScaleMode::Stretch);
        assert_eq!(load_scale(&empty, "gb"), ScaleMode::Fit);

        let global = serde_json::json!({"scale_mode": 1});
        assert_eq!(load_scale(&global, "gb"), ScaleMode::Integer);
        assert_eq!(load_scale(&global, "psx"), ScaleMode::Stretch);

        let explicit = serde_json::json!({"scale_mode": 1, "scales": {"psx": 0}});
        assert_eq!(load_scale(&explicit, "psx"), ScaleMode::Fit);
    }

    #[test]
    fn swap_ab_is_per_system() {
        let v = serde_json::json!({"swap_ab": {"nes": true, "gb": false}});
        assert!(swap_ab_in(&v, "nes"));
        assert!(!swap_ab_in(&v, "gb"));
        assert!(!swap_ab_in(&v, "snes"));
        assert!(!swap_ab_in(&serde_json::json!({}), "nes"));
    }

    #[test]
    fn swap_l1r1_is_per_system() {
        let v = serde_json::json!({"swap_l1r1": {"dc": true, "psx": false}});
        assert!(swap_l1r1_in(&v, "dc"));
        assert!(!swap_l1r1_in(&v, "psx"));
        assert!(!swap_l1r1_in(&v, "snes"));
        assert!(!swap_l1r1_in(&serde_json::json!({}), "dc"));
    }

    #[test]
    fn shader_modes_round_trip() {
        for mode in [
            ShaderMode::Off,
            ShaderMode::GbaColor,
            ShaderMode::GbcColor,
            ShaderMode::Lcd3x,
            ShaderMode::Dot,
            ShaderMode::LcdAccurate,
        ] {
            assert_eq!(ShaderMode::from_u8(mode.as_u8()), mode);
        }
        assert_eq!(ShaderMode::from_u8(6), ShaderMode::Off);
    }

    #[test]
    fn shader_modes_step_both_ways() {
        let mut mode = ShaderMode::Off;
        for _ in 0..6 {
            mode = mode.next();
        }
        assert_eq!(mode, ShaderMode::Off);
        for _ in 0..6 {
            mode = mode.prev();
        }
        assert_eq!(mode, ShaderMode::Off);
        assert_eq!(ShaderMode::GbaColor.prev(), ShaderMode::Off);
        assert_eq!(ShaderMode::LcdAccurate.prev(), ShaderMode::Dot);
        assert_eq!(ShaderMode::Off.prev(), ShaderMode::LcdAccurate);
    }

    #[test]
    fn lcd_accurate_is_only_for_handhelds() {
        for system in ["gb", "gbc", "ngp", "ngpc"] {
            assert!(ShaderMode::LcdAccurate.supported(system), "{system}");
        }
        for system in ["gba", "nes", "snes", "genesis", "psx"] {
            assert!(!ShaderMode::LcdAccurate.supported(system), "{system}");
        }
        assert_eq!(ShaderMode::Dot.next_for("gba"), ShaderMode::Off);
        assert_eq!(ShaderMode::Off.prev_for("gba"), ShaderMode::Dot);
    }
}
