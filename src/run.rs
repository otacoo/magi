//! BALTHASAR — the libretro host. Spawned by the UI as `magi run`. It takes
//! the panel and the input devices, loads a core, runs a game, and exits
//! back to MELCHIOR. Video goes to the same presenters as the UI; audio goes
//! directly to ALSA (libasound via dlopen).
//!
//! This file owns the host loop and core plumbing; the pause menu, overlays,
//! pad mapping and runtime preferences live in the `run/` submodules.

use std::collections::VecDeque;
use std::ffi::{c_void, CStr, CString};
use std::os::raw::{c_char, c_uint};
use std::path::Path;
#[cfg(target_os = "linux")]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{
    AtomicBool, AtomicI16, AtomicPtr, AtomicU16, AtomicU32, AtomicU64, Ordering,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use libloading::Library;
use sdl2::controller::Button as PadButton;

use crate::canvas::Canvas;
use crate::font::{self, Fonts};
use crate::image::Image;
use crate::input::{ev, retro, RawEvent, Shortcut, Shortcuts};
use crate::netplay;
use crate::present::Presenter;
use crate::systems::Catalogue;
use crate::theme::pal;
use crate::util;

mod menu;
mod overlay;
mod pad;
mod prefs;

use menu::{draw_hold_panel, draw_menu, draw_netplay, draw_overlay_menu};
pub(crate) use menu::{draw_menu_preview, draw_overlay_menu_preview};
#[cfg(target_os = "linux")]
use overlay::upload_overlay;
use overlay::{
    apply_overlays, blend_straight, load_bezel_area, load_composite, load_overlay_prefs,
    overlay_scan_layers, premultiplied, save_overlay_prefs, Overlay, OverlayLayers,
};
use pad::{pump_pads, PadHat, PadState};
use prefs::{
    load_analog_dpad, load_display_prefs, load_runtime_prefs, load_runtime_volume,
    load_shot_chord_pref, load_swap_ab, load_swap_l1r1, load_text_scale, save_display_prefs,
    save_runtime_volume, scale_factors, AdjKind, AdjState, DisplayPrefs, ScaleMode, ShaderMode,
    COMBO_FIRST_REPEAT, COMBO_REPEAT, COMBO_STEP, DOUBLE_TAP, TRIGGER_ON,
};

#[cfg(target_os = "linux")]
use crate::audio::{AlsaOut, OUTPUT_HZ};
#[cfg(target_os = "linux")]
use crate::evdev::Evdev;
#[cfg(target_os = "linux")]
use crate::resample::Resampler;

const RETRO_DEVICE_JOYPAD: u32 = 1;
const RETRO_DEVICE_ANALOG: u32 = 5;
/// libretro subcontracts pack `(id + 1) << 8 | base`.
const fn retro_subclass(base: u32, id: u32) -> u32 {
    ((id + 1) << 8) | base
}
const RETRO_DEVICE_PSE_DUALSHOCK: u32 = retro_subclass(RETRO_DEVICE_ANALOG, 1);
const ANALOG_X: u32 = 0;
const ANALOG_Y: u32 = 1;
const STICK_DEADZONE: i16 = 4000;
/// Rumble is a spike effect. Ignore sustained low-level Purupuru values,
/// and give a held or repeated request one short pulse instead of a motor
/// that runs continuously.
const RUMBLE_MIN_STRENGTH: u16 = 0x6000;
const RUMBLE_ON_GAP: Duration = Duration::from_millis(250);
const RUMBLE_ON_MAX: Duration = Duration::from_millis(160);
/// The request must fall below the threshold for this long before the same
/// effect can fire again.
const RUMBLE_REARM: Duration = Duration::from_millis(250);
/// A jump this large while the pack is already high is a new spike.
const RUMBLE_RISE: u16 = 0x2000;
/// Let the emulation lead the audio clock by this much before the audio
/// callback blocks. RetroArch gets this pacing from its blocking audio
/// driver; MAGI's writer does not block the core.
const AUDIO_LEAD: f64 = 0.050;

const ENV_GET_CAN_DUPE: u32 = 3;
const ENV_SET_MESSAGE: u32 = 6;
const ENV_SHUTDOWN: u32 = 7;
const ENV_GET_SYSTEM_DIRECTORY: u32 = 9;
const ENV_SET_PIXEL_FORMAT: u32 = 10;
const ENV_SET_HW_RENDER: u32 = 14;
const ENV_GET_VARIABLE: u32 = 15;
const ENV_SET_VARIABLES: u32 = 16;
const ENV_GET_VARIABLE_UPDATE: u32 = 17;
const ENV_GET_RUMBLE_INTERFACE: u32 = 23;
const ENV_GET_LOG_INTERFACE: u32 = 27;
const ENV_GET_SAVE_DIRECTORY: u32 = 31;
const ENV_SET_SYSTEM_AV_INFO: u32 = 32;
const ENV_SET_GEOMETRY: u32 = 37;
const ENV_GET_INPUT_BITMASKS: u32 = 51 | 0x1_0000;
const ENV_GET_CORE_OPTIONS_VERSION: u32 = 52;
const ENV_GET_FASTFORWARDING: u32 = 49 | 0x1_0000;
/// RetroArch extension the 2021 Flycast uses before waiting for its emu
/// thread to leave on unload.
const ENV_GET_CLEAR_ALL_THREAD_WAITS_CB: u32 = 3 | 0x80_0000;
const ENV_SET_DISK_CONTROL_INTERFACE: u32 = 13;

#[cfg(target_os = "linux")]
const RETRO_HW_CONTEXT_OPENGLES2: u32 = 2;
const RETRO_HW_FRAME_BUFFER_VALID: usize = usize::MAX;

type RumbleFn = unsafe extern "C" fn(c_uint, c_uint, u16) -> bool;

#[repr(C)]
struct RumbleInterface {
    set_rumble_state: Option<RumbleFn>,
}

/// The legacy disk control table pcsx_rearmed offers for multi-disc games.
#[repr(C)]
#[derive(Clone, Copy)]
struct DiskControl {
    set_eject_state: unsafe extern "C" fn(bool) -> bool,
    get_eject_state: unsafe extern "C" fn() -> bool,
    get_image_index: unsafe extern "C" fn() -> c_uint,
    set_image_index: unsafe extern "C" fn(c_uint) -> bool,
    get_num_images: unsafe extern "C" fn() -> c_uint,
    replace_image_index: unsafe extern "C" fn(c_uint, *const GameInfo) -> bool,
    add_image_index: unsafe extern "C" fn() -> bool,
}

extern "C" fn rumble_cb(_port: c_uint, _effect: c_uint, strength: u16) -> bool {
    ctx().set_rumble(strength)
}

const PIXFMT_0RGB1555: u32 = 0;
const PIXFMT_XRGB8888: u32 = 1;
const PIXFMT_RGB565: u32 = 2;

#[repr(C)]
struct SystemInfo {
    library_name: *const c_char,
    library_version: *const c_char,
    valid_extensions: *const c_char,
    need_fullpath: bool,
    block_extract: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct GameGeometry {
    base_width: c_uint,
    base_height: c_uint,
    max_width: c_uint,
    max_height: c_uint,
    aspect_ratio: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SystemTiming {
    fps: f64,
    sample_rate: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SystemAvInfo {
    geometry: GameGeometry,
    timing: SystemTiming,
}

#[repr(C)]
struct GameInfo {
    path: *const c_char,
    data: *const c_void,
    size: usize,
    meta: *const c_char,
}

/// A game launch request from the UI.
pub struct Launch {
    pub system: String,
    pub core: String,
    pub rom: String,
}

/// One core option (libretro v1 vocabulary): value strings the core accepts,
/// index 0 is the core's default.
struct CoreOption {
    key: String,
    desc: String,
    values: Vec<String>,
    c_values: Vec<CString>,
    chosen: usize,
    /// Shown in the CORE OPTIONS page; the rest stay at their defaults but
    /// must still answer the core's GET_VARIABLE calls.
    visible: bool,
}

#[repr(C)]
struct RetroVariable {
    key: *const c_char,
    value: *const c_char,
}

#[repr(C)]
struct RetroLogCallback {
    log: *mut c_void,
}

/// The libretro hardware-render request. The core fills the context type and
/// its own callbacks; the frontend fills in the framebuffer and symbol
/// lookups.
#[cfg(target_os = "linux")]
#[repr(C)]
struct RetroHwRender {
    context_type: u32,
    context_reset: Option<unsafe extern "C" fn()>,
    get_current_framebuffer: Option<extern "C" fn() -> usize>,
    get_proc_address: Option<extern "C" fn(*const c_char) -> *mut c_void>,
    depth: bool,
    stencil: bool,
    bottom_left_origin: bool,
    version_major: u32,
    version_minor: u32,
    cache_context: bool,
    context_destroy: Option<unsafe extern "C" fn()>,
    debug_context: bool,
}

/// The core's half of an accepted hardware context, kept for the lifetime of
/// the game. The FBO id is published separately for the emu thread.
#[cfg(target_os = "linux")]
struct HwRender {
    reset: Option<unsafe extern "C" fn()>,
    destroy: Option<unsafe extern "C" fn()>,
    bottom_left: bool,
    stencil: bool,
}

#[cfg(target_os = "linux")]
static HW_GPU: AtomicPtr<crate::present::gles::Gpu> = AtomicPtr::new(std::ptr::null_mut());
/// The FBO a hardware core must bind. Read from the core's emu thread.
#[cfg(target_os = "linux")]
static HW_FBO: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_os = "linux")]
fn hw_available() -> bool {
    !HW_GPU.load(Ordering::Relaxed).is_null()
}

/// "Descriptor; value1|value2|value3" (the legacy SET_VARIABLES format).
fn parse_option(raw: &str) -> (String, Vec<String>) {
    let (desc, list) = match raw.split_once(';') {
        Some((d, l)) => (d.trim(), l),
        None => (raw.trim(), ""),
    };
    let values = list
        .split('|')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    (desc.to_string(), values)
}

/// Core defaults MAGI pins instead of the core's own default. A saved choice
/// always wins.
fn default_option(core: &str, system: &str, key: &str) -> Option<&'static str> {
    match (core, key) {
        // LCD ghosting: 50:50 with the previous frame, for games that
        // flicker sprites to fake transparency (F-Zero, Boktai, ZAS).
        ("mgba", "mgba_interframe_blending") => Some("mix"),
        // GBA rumble: mGBA only reports it once the game sees a Game Boy
        // Player. Some games may stutter with it on, so the option stays
        // visible and a saved choice wins.
        ("mgba", "mgba_force_gbp") => Some("ON"),
        ("gambatte", "gambatte_mix_frames") => Some("mix"),
        ("gambatte", "gambatte_gbc_color_correction") => Some(if system == "gbc" {
            "GBC only"
        } else {
            "disabled"
        }),
        ("gambatte", "gambatte_gbc_color_correction_mode") => Some("accurate"),
        ("gambatte", "gambatte_gbc_frontlight_position") => Some("central"),
        ("gambatte", "gambatte_dark_filter_level") => Some("0"),
        ("gambatte", "gambatte_gb_hwmode") => Some("Auto"),
        ("gambatte", "gambatte_rumble_level") => Some("8"),
        ("gambatte", "gambatte_gb_colorization") => {
            Some(if system == "gbc" { "disabled" } else { "auto" })
        }
        ("gambatte", "gambatte_gb_internal_palette") => Some("GB - DMG"),
        // The H700's L1/R1 are digital buttons; Flycast then treats the
        // L2/R2 bits they are moved to as full-press analog triggers.
        ("flycast", "reicast_digital_triggers") => Some("enabled"),
        // The 2021 flycastvl build has no LOW_END defaults. Pin the fast
        // options it would otherwise leave off.
        ("flycast", "reicast_internal_resolution") => Some("640x480"),
        ("flycast", "reicast_enable_dsp") => Some("disabled"),
        // Threaded rendering runs ahead of the presenter without this.
        ("flycast", "reicast_synchronous_rendering") => Some("enabled"),
        ("flycast", "reicast_gdrom_fast_loading") => Some("enabled"),
        ("flycast", "reicast_alpha_sorting") => Some("per-strip (fast, least accurate)"),
        ("flycast", "reicast_cable_type") => Some("VGA (RGB)"),
        ("flycast", "reicast_anisotropic_filtering") => Some("disabled"),
        // The 2021 core drives the pack from a continuous Purupuru value;
        // leave it off unless the player turns it back on in the CORE menu.
        ("flycast", "reicast_enable_purupuru") => Some("disabled"),
        // PokeMini: 6x puts the 96x64 LCD at 576x384, close to the 640/720
        // panels, so the internal dot matrix survives the final scale.
        ("pokemini", "pokemini_video_scale") => Some("6x"),
        // The H700 panel runs at 60Hz; the core's 72Hz native rate would tear.
        ("pokemini", "pokemini_60hz_mode") => Some("enabled"),
        // PS1 (PCSX-ReARMed)
        ("pcsx_rearmed", "pcsx_rearmed_show_bios_bootlogo") => Some("disabled"),
        ("pcsx_rearmed", "pcsx_rearmed_dithering") => Some("disabled"),
        ("pcsx_rearmed", "pcsx_rearmed_frameskip_type") => Some("auto_threshold"),
        ("pcsx_rearmed", "pcsx_rearmed_frameskip_threshold") => Some("15"),
        ("pcsx_rearmed", "pcsx_rearmed_frameskip_interval") => Some("3"),
        ("pcsx_rearmed", "pcsx_rearmed_neon_enhancement_enable") => Some("disabled"),
        ("pcsx_rearmed", "pcsx_rearmed_neon_enhancement_tex_adj_v2") => Some("enabled"),
        ("pcsx_rearmed", "pcsx_rearmed_neon_enhancement_no_main") => Some("enabled"),
        ("pcsx_rearmed", "pcsx_rearmed_spu_reverb") => Some("enabled"),
        ("pcsx_rearmed", "pcsx_rearmed_spu_interpolation") => Some("simple"),
        ("pcsx_rearmed", "pcsx_rearmed_vibration") => Some("enabled"),
        // Beetle PCE defaults to the Japanese System Card 3. Follow the
        // BIOS that is actually on the card when it is a different one.
        ("mednafen_pce", "pce_cdbios") => pce_bios_default(),
        _ => None,
    }
}

fn pce_bios_default() -> Option<&'static str> {
    pce_bios_default_in(Path::new("BIOS"))
}

fn pce_bios_default_in(bios: &Path) -> Option<&'static str> {
    if bios.join("syscard3.pce").is_file() {
        return None;
    }
    if bios.join("syscard3u.pce").is_file() {
        return Some("System Card 3 US");
    }
    if bios.join("syscard2.pce").is_file() {
        return Some("System Card 2");
    }
    if bios.join("syscard2u.pce").is_file() {
        return Some("System Card 2 US");
    }
    if bios.join("syscard1.pce").is_file() {
        return Some("System Card 1");
    }
    if bios.join("gexpress.pce").is_file() {
        return Some("Games Express");
    }
    None
}

/// The stored label wins when the core still offers it, then our per-core
/// default, else the core's own default (index 0).
fn chosen_index(
    core: &str,
    system: &str,
    key: &str,
    values: &[String],
    saved: Option<&String>,
    reflective: bool,
) -> usize {
    // Netplay owns the link mode at runtime. Never restore a saved server or
    // client mode, or a game would open a socket before the player asks.
    if core == "gambatte" && key == "gambatte_gb_link_mode" {
        if let Some(i) = values.iter().position(|v| v == "Not Connected") {
            return i;
        }
    }
    if core == "gambatte" && key == "gambatte_gb_colorization" && reflective {
        if let Some(i) = values
            .iter()
            .position(|v| v.eq_ignore_ascii_case("disabled"))
        {
            return i;
        }
    }
    // Flycast renders at native size on the H700: the frontend FBO is fixed
    // at the load-time geometry, so higher values only cost frames. Ignore
    // a stale saved choice; both rows are hidden from the CORE menu. The
    // 2021 core spells native as 640x480, the current one as 1x.
    if core == "flycast" && key == "reicast_internal_resolution" {
        for native in ["1x", "640x480"] {
            if let Some(i) = values.iter().position(|v| v.eq_ignore_ascii_case(native)) {
                return i;
            }
        }
    }
    if core == "flycast" && key == "reicast_render_to_texture_upscaling" {
        if let Some(i) = values.iter().position(|v| v.eq_ignore_ascii_case("1x")) {
            return i;
        }
    }

    // Fast loading is part of the VL speed profile; ignore a stale
    // "disabled" left by a previous core.
    if core == "flycast" && key == "reicast_gdrom_fast_loading" {
        if let Some(i) = values
            .iter()
            .position(|v| v.eq_ignore_ascii_case("enabled"))
        {
            return i;
        }
    }
    // The 2021 core's threaded mode runs the emulation ahead of the renderer
    // when this is off, which looks like permanent fast-forward.
    if core == "flycast" && key == "reicast_synchronous_rendering" {
        if let Some(i) = values
            .iter()
            .position(|v| v.eq_ignore_ascii_case("enabled"))
        {
            return i;
        }
    }
    if let Some(i) = saved.and_then(|s| values.iter().position(|v| v == s)) {
        return i;
    }
    if let Some(d) = default_option(core, system, key) {
        if let Some(i) = values.iter().position(|v| v.eq_ignore_ascii_case(d)) {
            return i;
        }
    }
    0
}

fn options_path() -> &'static str {
    "Config/core-options.json"
}

fn options_key(core: &str, system: &str) -> String {
    if core == "gambatte" {
        format!("gambatte:{system}")
    } else {
        core.to_string()
    }
}

/// Per-core saved choices: `{ "<core>": { "<option key>": "<label>" } }`.
/// Gambatte is split per system because GB and GBC share the core but not
/// their palettes.
fn load_saved_options(core: &str, system: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Ok(text) = std::fs::read_to_string(options_path()) else {
        return map;
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&text) else {
        return map;
    };
    let key = options_key(core, system);
    if let Some(obj) = root.get(&key).and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                map.insert(k.clone(), s.to_string());
            }
        }
    }
    map
}

fn save_core_options(ctx: &Ctx) {
    let path = Path::new(options_path());
    let mut root: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    let mut core = serde_json::Map::new();
    let key = options_key(&ctx.opt_key, &ctx.system);
    let previous = root.get(&key).cloned();
    for o in &ctx.options {
        if ctx.reflective && o.key == "gambatte_gb_colorization" {
            if let Some(v) = previous.as_ref().and_then(|c| c.get(&o.key)) {
                core.insert(o.key.clone(), v.clone());
            }
            continue;
        }
        core.insert(o.key.clone(), serde_json::json!(o.values[o.chosen]));
    }
    root[&key] = serde_json::Value::Object(core);
    if let Ok(text) = serde_json::to_string_pretty(&root) {
        let _ = std::fs::create_dir_all("Config");
        match util::write_file_atomic(path, text.as_bytes()) {
            Ok(()) => util::log(&format!("core options: saved {key}")),
            Err(e) => util::log(&format!("core options: cannot save: {e}")),
        }
    }
}

struct AudioPacer {
    start: Instant,
    produced: u64,
}

struct Ctx {
    pixfmt: u32,
    geometry: GameGeometry,
    frame: *const u8,
    /// Bumped by the video callback on real frames (nulls are dups).
    frame_seq: u64,
    pitch: usize,
    bits: AtomicU16,
    analog_dpad: AtomicBool,
    left: (AtomicI16, AtomicI16),
    right: (AtomicI16, AtomicI16),
    /// The core may deliver audio from its emulation thread while threaded
    /// rendering is on (the 2021 Flycast does): this buffer is shared.
    audio: Mutex<Vec<i16>>,
    audio_rate: AtomicU32,
    fps_milli: AtomicU32,
    audio_pacer: Mutex<AudioPacer>,
    audio_samples: AtomicU64,
    audio_sleep_us: AtomicU64,
    save_dir: CString,
    system_dir: CString,
    shutdown: bool,
    sram_ptr: *mut u8,
    ser_size: unsafe extern "C" fn() -> usize,
    ser: unsafe extern "C" fn(*mut c_void, usize) -> bool,
    unser: unsafe extern "C" fn(*const c_void, usize) -> bool,
    /// Cached once, before the game loop: `retro_serialize_size` is not
    /// thread-safe in cores with threaded rendering.
    state_size: usize,
    /// Flycast VL cannot serialize while its emulation thread is running.
    states_blocked: bool,
    options: Vec<CoreOption>,
    opt_key: String,
    opt_dirty: bool,
    saved_opts: std::collections::HashMap<String, String>,
    overlay_layers: OverlayLayers,
    overlay_dirty: bool,
    moto: Option<std::fs::File>,
    rumble_on: bool,
    rumble_at: Option<Instant>,
    rumble_high: bool,
    rumble_peak: u16,
    rumble_low_at: Option<Instant>,
    rumble_armed: bool,
    rumble_ok: bool,
    /// The core's disk table while a multi-disc game is loaded.
    disk: Option<DiskControl>,
    system: String,
    rom: String,
    core_version: String,
    netplay: Option<netplay::Session>,
    netplay_linked: bool,
    /// The LCD ACCURATE shader is selected.
    lcd_accurate: bool,
    /// GB is drawn by the LCD-accurate palette, so the core must stay greyscale.
    reflective: bool,
    /// The user's GB colourisation, restored when LCD ACCURATE is switched off.
    gb_color_backup: Option<String>,
    /// Set when the core accepted a libretro hardware GLES context.
    #[cfg(target_os = "linux")]
    hw_render: Option<HwRender>,
}

impl Ctx {
    /// The handheld's vibration motor: a write-only sysfs switch on the
    /// AXP2202. The state is cached so a core pulsing rumble at frame rate
    /// does not beat on the attribute.
    ///
    /// A held Purupuru value is not a rumble event. Fire one short pulse,
    /// then wait for the request to drop (or rise sharply) before firing
    /// again.
    fn set_rumble(&mut self, strength: u16) -> bool {
        if !self.rumble_ok {
            if self.rumble_on {
                return self.write_motor(false);
            }
            return true;
        }
        let now = Instant::now();
        let high = strength >= RUMBLE_MIN_STRENGTH;
        if !high {
            if self.rumble_high {
                self.rumble_low_at = Some(now);
            }
            self.rumble_high = false;
            if self
                .rumble_low_at
                .is_some_and(|t| now.saturating_duration_since(t) >= RUMBLE_REARM)
            {
                self.rumble_armed = true;
                self.rumble_peak = 0;
            }
            if self.rumble_on {
                return self.write_motor(false);
            }
            return true;
        }

        self.rumble_high = true;
        self.rumble_low_at = None;
        if !self.rumble_armed
            && strength > self.rumble_peak.saturating_add(RUMBLE_RISE)
            && self
                .rumble_at
                .is_none_or(|t| now.saturating_duration_since(t) >= RUMBLE_ON_GAP)
        {
            self.rumble_armed = true;
        }
        self.rumble_peak = self.rumble_peak.max(strength);

        if self.rumble_on {
            if self
                .rumble_at
                .is_some_and(|t| now.saturating_duration_since(t) >= RUMBLE_ON_MAX)
            {
                return self.write_motor(false);
            }
            return true;
        }
        if !self.rumble_armed {
            return true;
        }
        // Starting is debounced: a game that pulses the pack every frame must
        // not block the emu thread on the sysfs attribute.
        if self
            .rumble_at
            .is_some_and(|t| now.saturating_duration_since(t) < RUMBLE_ON_GAP)
        {
            return true;
        }
        self.rumble_armed = false;
        self.write_motor(true)
    }

    /// The AXP2202 keeps the motor state across process restarts. Clear it
    /// at launch so a killed run cannot leave the console buzzing.
    fn reset_motor(&mut self) {
        self.write_motor(false);
    }

    fn write_motor(&mut self, on: bool) -> bool {
        if self.moto.is_none() {
            self.moto = std::fs::OpenOptions::new()
                .write(true)
                .open("/sys/class/power_supply/axp2202-battery/moto")
                .ok();
        }
        if let Some(f) = &mut self.moto {
            use std::io::Write;
            if f.write_all(if on { b"1" } else { b"0" }).is_ok() {
                self.rumble_on = on;
                self.rumble_at = Some(Instant::now());
                return true;
            }
        }
        false
    }

    /// RetroArch's audio driver blocks when its buffer is full, which is what
    /// paces the emulator thread. MAGI's ALSA writer never blocks, so the
    /// 2021 Flycast can run the emulated machine faster than real time. Block
    /// the audio producer once it gets a few frames ahead of wall time.
    fn pace_audio(&self, frames: usize) {
        if frames == 0 {
            return;
        }
        let rate = f64::from(self.audio_rate.load(Ordering::Relaxed).max(1));
        let mut pacer = self.audio_pacer.lock().unwrap();
        pacer.produced = pacer.produced.saturating_add(frames as u64);
        let due = pacer.produced as f64 / rate;
        let elapsed = pacer.start.elapsed().as_secs_f64();
        // A menu or a paused core leaves the clock far behind. Re-anchor
        // instead of letting the next frame produce a catch-up burst.
        if elapsed > due + AUDIO_LEAD {
            pacer.start = Instant::now();
            pacer.produced = frames as u64;
            return;
        }
        let ahead = due - elapsed;
        if ahead > AUDIO_LEAD {
            let sleep = Duration::from_secs_f64((ahead - AUDIO_LEAD).min(0.100));
            self.audio_sleep_us
                .fetch_add(sleep.as_micros() as u64, Ordering::Relaxed);
            drop(pacer);
            std::thread::sleep(sleep);
        }
    }

    fn reset_audio_pacer(&self) {
        let mut pacer = self.audio_pacer.lock().unwrap();
        pacer.start = Instant::now();
        pacer.produced = 0;
    }
}

static CTX: AtomicPtr<Ctx> = AtomicPtr::new(std::ptr::null_mut());
/// The old Flycast's threaded path only sends audio and applies its own
/// frame limiter when the frontend reports that it is not fast-forwarding.
static FASTFORWARD: AtomicBool = AtomicBool::new(false);

fn ctx() -> &'static mut Ctx {
    unsafe { &mut *CTX.load(Ordering::Relaxed) }
}

/// The 2021 Flycast ARM64 dynarec saves only the low halves of d8-d15. When
/// it runs on the caller's thread (threaded rendering off), a live f64 in
/// one of those registers can come back as garbage. Save the full registers
/// around the call.
#[cfg(target_arch = "aarch64")]
#[inline(never)]
unsafe fn core_run(run: unsafe extern "C" fn()) {
    unsafe {
        core::arch::asm!(
            "stp d8, d9, [sp, #-16]!",
            "stp d10, d11, [sp, #-16]!",
            "stp d12, d13, [sp, #-16]!",
            "stp d14, d15, [sp, #-16]!",
            "blr {run}",
            "ldp d14, d15, [sp], #16",
            "ldp d12, d13, [sp], #16",
            "ldp d10, d11, [sp], #16",
            "ldp d8, d9, [sp], #16",
            run = in(reg) run,
            clobber_abi("C"),
        );
    }
}

#[cfg(not(target_arch = "aarch64"))]
#[inline(always)]
unsafe fn core_run(run: unsafe extern "C" fn()) {
    run();
}

/// How many disc images the core offers (0 for single-disc content).
fn disc_count() -> usize {
    ctx()
        .disk
        .map(|d| unsafe { (d.get_num_images)() } as usize)
        .unwrap_or(0)
}

/// The image currently in the tray.
fn disc_index() -> usize {
    ctx()
        .disk
        .map(|d| unsafe { (d.get_image_index)() } as usize)
        .unwrap_or(0)
}

/// Eject, insert `index`, close. The core loads the image itself.
fn disc_swap(index: usize) -> bool {
    let Some(d) = ctx().disk else {
        return false;
    };
    let count = disc_count();
    if index >= count {
        return false;
    }
    let ok = unsafe { (d.set_eject_state)(true) && (d.set_image_index)(index as c_uint) };
    let closed = unsafe { (d.set_eject_state)(false) };
    let done = ok && closed;
    util::log(&format!(
        "disc: insert {} ({}/{}) {}",
        index + 1,
        index + 1,
        count,
        if done { "ok" } else { "failed" }
    ));
    done
}

/// The core's variadic logger. Only the two string arguments used by the
/// core's own callback are read; other format strings are logged as-is.
unsafe extern "C" fn core_log(
    level: c_uint,
    fmt: *const c_char,
    a: *const c_char,
    b: *const c_char,
) {
    if fmt.is_null() {
        return;
    }
    let fmt = CStr::from_ptr(fmt).to_string_lossy();
    if fmt == "[%s] %s" && !a.is_null() && !b.is_null() {
        let tag = CStr::from_ptr(a).to_string_lossy();
        let msg = CStr::from_ptr(b).to_string_lossy();
        util::log(&format!("core {level} [{tag}] {msg}"));
    } else if fmt == "%s" && !a.is_null() {
        let msg = CStr::from_ptr(a).to_string_lossy();
        util::log(&format!("core {level}: {}", msg.trim_end()));
    } else {
        util::log(&format!("core {level}: {fmt}"));
    }
}

/// The FBO a hardware core must bind. The core reads this from its emu
/// thread, so it lives outside `Ctx`.
#[cfg(target_os = "linux")]
extern "C" fn hw_current_framebuffer() -> usize {
    HW_FBO.load(Ordering::Relaxed)
}

#[cfg(target_os = "linux")]
extern "C" fn hw_proc_address(name: *const c_char) -> *mut c_void {
    let gpu = HW_GPU.load(Ordering::Relaxed);
    if gpu.is_null() || name.is_null() {
        return std::ptr::null_mut();
    }
    let name = unsafe { CStr::from_ptr(name) };
    unsafe { (*gpu).proc_address(name) }
}

extern "C" fn env_cb(cmd: c_uint, data: *mut c_void) -> bool {
    let ctx = ctx();
    match cmd {
        ENV_SET_PIXEL_FORMAT => {
            if !data.is_null() {
                ctx.pixfmt = unsafe { *(data as *const u32) };
                return true;
            }
            false
        }
        ENV_SET_SYSTEM_AV_INFO => {
            if !data.is_null() {
                let av = unsafe { &*(data as *const SystemAvInfo) };
                ctx.geometry = av.geometry;
                if av.timing.fps > 1.0 {
                    let milli = (av.timing.fps * 1000.0).round() as u32;
                    if ctx.fps_milli.swap(milli, Ordering::Relaxed) != milli {
                        util::log(&format!("video: timing {:.2} fps", av.timing.fps));
                    }
                }
                if av.timing.sample_rate >= 8000.0 {
                    ctx.audio_rate
                        .store(av.timing.sample_rate as u32, Ordering::Relaxed);
                }
                return true;
            }
            false
        }
        ENV_SET_GEOMETRY => {
            if !data.is_null() {
                ctx.geometry = unsafe { *(data as *const GameGeometry) };
                return true;
            }
            false
        }
        ENV_GET_CAN_DUPE => {
            if !data.is_null() {
                unsafe { *(data as *mut bool) = true };
                return true;
            }
            false
        }
        ENV_GET_SYSTEM_DIRECTORY => {
            if !data.is_null() {
                unsafe { *(data as *mut *const c_char) = ctx.system_dir.as_ptr() };
                return true;
            }
            false
        }
        ENV_GET_SAVE_DIRECTORY => {
            if !data.is_null() {
                unsafe { *(data as *mut *const c_char) = ctx.save_dir.as_ptr() };
                return true;
            }
            false
        }
        ENV_GET_INPUT_BITMASKS => true,
        ENV_GET_FASTFORWARDING => {
            if !data.is_null() {
                unsafe { *(data as *mut bool) = FASTFORWARD.load(Ordering::Relaxed) };
                return true;
            }
            false
        }
        ENV_GET_CLEAR_ALL_THREAD_WAITS_CB => {
            if data.is_null() {
                return false;
            }
            type ClearWaitsFn = extern "C" fn(c_uint, *mut c_void) -> bool;
            unsafe {
                *(data as *mut Option<ClearWaitsFn>) = Some(clear_thread_waits_cb);
            }
            true
        }
        ENV_GET_LOG_INTERFACE => {
            if data.is_null() {
                return false;
            }
            unsafe {
                (*(data as *mut RetroLogCallback)).log = core_log as *const () as *mut c_void;
            }
            true
        }
        ENV_GET_RUMBLE_INTERFACE => {
            if data.is_null() {
                return false;
            }
            let iface = data as *mut RumbleInterface;
            unsafe { (*iface).set_rumble_state = Some(rumble_cb) };
            util::log("rumble: interface offered");
            true
        }
        ENV_GET_CORE_OPTIONS_VERSION => {
            // Legacy (v0) frontend: cores keep to SET_VARIABLES and cope.
            if !data.is_null() {
                unsafe { *(data as *mut u32) = 0 };
                return true;
            }
            false
        }
        ENV_SET_DISK_CONTROL_INTERFACE => {
            if data.is_null() {
                ctx.disk = None;
                return true;
            }
            ctx.disk = Some(unsafe { *(data as *const DiskControl) });
            util::log("disc: core offers disk control");
            true
        }
        ENV_SET_VARIABLES => {
            if data.is_null() {
                return false;
            }
            let mut p = data as *const RetroVariable;
            let mut options = Vec::new();
            let mut total = 0usize;
            while !unsafe { (*p).key }.is_null() {
                let key = cstr(unsafe { (*p).key });
                let raw = cstr(unsafe { (*p).value });
                let (desc, values) = parse_option(&raw);
                total += 1;
                if !values.is_empty() {
                    let chosen = chosen_index(
                        &ctx.opt_key,
                        &ctx.system,
                        &key,
                        &values,
                        ctx.saved_opts.get(&key),
                        ctx.reflective,
                    );
                    let c_values = values
                        .iter()
                        .map(|v| CString::new(v.as_str()).unwrap_or_default())
                        .collect();
                    let hidden_palette = ctx.lcd_accurate
                        && ctx.opt_key == "gambatte"
                        && (key == "gambatte_gb_colorization"
                            || key == "gambatte_gb_internal_palette"
                            || (ctx.system == "gb" && key == "gambatte_dark_filter_level"));
                    let visible = (option_kept(&desc)
                        || (ctx.opt_key == "pokemini" && pokemini_option_kept(&desc))
                        || (ctx.opt_key == "flycast" && flycast_option_kept(&desc)))
                        && !hidden_palette
                        && !(ctx.system == "genesis" && genesis_option_hidden(&desc))
                        && !(ctx.opt_key == "mgba" && mgba_option_hidden(&desc));
                    if key.to_ascii_lowercase().contains("bios") {
                        util::log(&format!("core option {key}={}", values[chosen]));
                    }
                    options.push(CoreOption {
                        key,
                        desc,
                        values,
                        c_values,
                        chosen,
                        visible,
                    });
                }
                p = unsafe { p.add(1) };
            }
            let labels: Vec<&str> = options
                .iter()
                .filter(|o| o.visible)
                .map(|o| o.desc.as_str())
                .collect();
            util::log(&format!(
                "core options: {} shown of {total} ({})",
                labels.len(),
                labels.join(", ")
            ));
            ctx.options = options;
            if ctx.opt_key == "flycast" {
                let picked: Vec<String> = ctx
                    .options
                    .iter()
                    .filter(|o| {
                        matches!(
                            o.key.as_str(),
                            "reicast_threaded_rendering"
                                | "reicast_internal_resolution"
                                | "reicast_render_to_texture_upscaling"
                                | "reicast_alpha_sorting"
                                | "reicast_frame_skipping"
                                | "reicast_gdrom_fast_loading"
                                | "reicast_synchronous_rendering"
                                | "reicast_enable_purupuru"
                        )
                    })
                    .map(|o| format!("{}={}", o.key, o.values[o.chosen]))
                    .collect();
                if !picked.is_empty() {
                    util::log(&format!("core options: {}", picked.join(", ")));
                }
            }
            true
        }
        ENV_GET_VARIABLE => {
            if data.is_null() {
                return false;
            }
            let rv = data as *mut RetroVariable;
            let key = cstr(unsafe { (*rv).key });
            let value = ctx
                .options
                .iter()
                .find(|o| o.key == key)
                .map(|o| o.c_values[o.chosen].as_ptr())
                .unwrap_or(std::ptr::null());
            unsafe { (*rv).value = value };
            true
        }
        ENV_GET_VARIABLE_UPDATE => {
            if !data.is_null() {
                let changed = ctx.opt_dirty;
                ctx.opt_dirty = false;
                unsafe { *(data as *mut bool) = changed };
                return true;
            }
            false
        }
        ENV_SET_MESSAGE => {
            if !data.is_null() {
                #[repr(C)]
                struct Message {
                    msg: *const c_char,
                    frames: c_uint,
                }
                let m = unsafe { &*(data as *const Message) };
                if !m.msg.is_null() {
                    let s = unsafe { CStr::from_ptr(m.msg) }.to_string_lossy();
                    util::log(&format!("core message: {s}"));
                }
            }
            true
        }
        ENV_SHUTDOWN => {
            ctx.shutdown = true;
            true
        }
        ENV_SET_HW_RENDER => {
            #[cfg(target_os = "linux")]
            {
                if data.is_null() || !hw_available() {
                    return false;
                }
                let hw = unsafe { &mut *(data as *mut RetroHwRender) };
                if hw.context_type != RETRO_HW_CONTEXT_OPENGLES2 || hw.context_reset.is_none() {
                    util::log(&format!("hw: cannot provide context {}", hw.context_type));
                    return false;
                }
                // Probe the FBO now: a failure here lets the core fall back
                // to its software context instead of failing after load.
                let (fw, fh) = (
                    if ctx.geometry.base_width > 0 {
                        ctx.geometry.base_width
                    } else {
                        640
                    },
                    if ctx.geometry.base_height > 0 {
                        ctx.geometry.base_height
                    } else {
                        480
                    },
                );
                let gpu = HW_GPU.load(Ordering::Relaxed);
                if gpu.is_null() {
                    return false;
                }
                let fbo = match unsafe { (&mut *gpu).hw_framebuffer(fw, fh, hw.stencil) } {
                    Ok(fbo) => fbo,
                    Err(e) => {
                        util::log(&format!("hw: {e}; software fallback"));
                        return false;
                    }
                };
                HW_FBO.store(fbo as usize, Ordering::Relaxed);
                hw.get_current_framebuffer = Some(hw_current_framebuffer);
                hw.get_proc_address = Some(hw_proc_address);
                ctx.hw_render = Some(HwRender {
                    reset: hw.context_reset,
                    destroy: hw.context_destroy,
                    bottom_left: hw.bottom_left_origin,
                    stencil: hw.stencil,
                });
                util::log(&format!(
                    "hw: GLES2 context accepted (depth {}, bottom-left {})",
                    hw.depth, hw.bottom_left_origin
                ));
                true
            }
            #[cfg(not(target_os = "linux"))]
            {
                false
            }
        }
        _ => false,
    }
}

extern "C" fn video_cb(data: *const c_void, width: c_uint, height: c_uint, pitch: usize) {
    if data.is_null() {
        return; // duplicate frame
    }
    let ctx = ctx();
    if data as usize == RETRO_HW_FRAME_BUFFER_VALID {
        // The core drew into MAGI's FBO; only the geometry changes here.
        ctx.frame_seq += 1;
        ctx.geometry.base_width = width;
        ctx.geometry.base_height = height;
        ctx.pitch = 0;
        return;
    }
    ctx.frame = data as *const u8;
    ctx.frame_seq += 1;
    ctx.geometry.base_width = width;
    ctx.geometry.base_height = height;
    ctx.pitch = pitch;
}

extern "C" fn audio_sample_cb(left: i16, right: i16) {
    let ctx = ctx();
    let mut audio = ctx.audio.lock().unwrap();
    audio.push(left);
    audio.push(right);
}

extern "C" fn audio_batch_cb(data: *const i16, frames: usize) -> usize {
    let ctx = ctx();
    if !data.is_null() {
        let slice = unsafe { std::slice::from_raw_parts(data, frames * 2) };
        ctx.audio.lock().unwrap().extend_from_slice(slice);
        ctx.audio_samples
            .fetch_add(frames as u64, Ordering::Relaxed);
        if !FASTFORWARD.load(Ordering::Relaxed) {
            ctx.pace_audio(frames);
        }
    }
    frames
}

extern "C" fn clear_thread_waits_cb(_cmd: c_uint, _data: *mut c_void) -> bool {
    // MAGI has no frontend-side thread waits; the core's own cancel is enough.
    true
}

/// A d-pad pair as a full-scale analog axis: negative is the first bit,
/// positive the second, both or neither is centred.
fn dpad_axis(bits: u16, lo: u16, hi: u16) -> i16 {
    let lo = bits & (1 << lo) != 0;
    let hi = bits & (1 << hi) != 0;
    if lo == hi {
        0
    } else if lo {
        -0x7fff
    } else {
        0x7fff
    }
}

extern "C" fn input_poll_cb() {}

extern "C" fn input_state_cb(port: c_uint, device: c_uint, index: c_uint, id: c_uint) -> i16 {
    if port != 0 {
        return 0;
    }
    if device == RETRO_DEVICE_ANALOG {
        let ctx = ctx();
        let (lx, ly) = (
            ctx.left.0.load(Ordering::Relaxed),
            ctx.left.1.load(Ordering::Relaxed),
        );
        let (dx, dy) = if ctx.analog_dpad.load(Ordering::Relaxed) {
            let bits = ctx.bits.load(Ordering::Relaxed);
            (
                dpad_axis(bits, retro::LEFT, retro::RIGHT),
                dpad_axis(bits, retro::UP, retro::DOWN),
            )
        } else {
            (0, 0)
        };
        let v = match (index, id) {
            (0, ANALOG_X) => {
                if lx != 0 {
                    lx
                } else {
                    dx
                }
            }
            (0, ANALOG_Y) => {
                if ly != 0 {
                    ly
                } else {
                    dy
                }
            }
            (1, ANALOG_X) => ctx.right.0.load(Ordering::Relaxed),
            (1, ANALOG_Y) => ctx.right.1.load(Ordering::Relaxed),
            _ => 0,
        };
        return if v.abs() < STICK_DEADZONE { 0 } else { v };
    }
    if device != RETRO_DEVICE_JOYPAD {
        return 0;
    }
    let bits = ctx().bits.load(Ordering::Relaxed);
    if id == u32::from(retro::MASK) {
        // RETRO_DEVICE_ID_JOYPAD_MASK
        return bits as i16;
    }
    if id < 16 && bits & (1 << id) != 0 {
        1
    } else {
        0
    }
}

/// DE pixel format and bytes-per-pixel for a libretro pixel format, when
/// the engine can scan it out directly.
#[cfg(target_os = "linux")]
fn game_layer_format(pixfmt: u32) -> Option<(i32, usize)> {
    match pixfmt {
        PIXFMT_RGB565 => Some((crate::disp2::DISP_FORMAT_RGB_565, 2)),
        PIXFMT_XRGB8888 => Some((crate::disp2::DISP_FORMAT_XRGB_8888, 4)),
        _ => None,
    }
}

/// The game rectangle for a scale mode inside `area` (a bezel's screen
/// region, or the whole canvas when none is selected).
fn game_rect(
    mode: ScaleMode,
    fw: usize,
    fh: usize,
    area: (i32, i32, u32, u32),
) -> (i32, i32, u32, u32) {
    let (ax, ay, aw, ah) = area;
    let (aw, ah) = (aw.max(1) as usize, ah.max(1) as usize);
    let (sx, sy) = scale_factors(mode, aw, ah, fw, fh);
    let dw = ((fw as f32 * sx) as usize).clamp(1, aw);
    let dh = ((fh as f32 * sy) as usize).clamp(1, ah);
    (
        ax + ((aw - dw) / 2) as i32,
        ay + ((ah - dh) / 2) as i32,
        dw as u32,
        dh as u32,
    )
}

pub fn main(args: &[String]) -> Result<()> {
    let mut system = None;
    let mut core = None;
    let mut rom = None;
    let mut it = args.iter().skip(2);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--system" => system = it.next().cloned(),
            "--core" => core = it.next().cloned(),
            "--rom" => rom = it.next().cloned(),
            other => bail!("magi run: unknown argument {other}"),
        }
    }
    let (Some(system), Some(core), Some(rom)) = (system, core, rom) else {
        bail!("usage: magi run --system <id> --core <name> --rom <path>");
    };
    play(&system, &core, &rom)
}

fn ensure_gba_bios(dir: &Path) {
    let dst = dir.join("gba_bios.bin");
    if dst.is_file() {
        return;
    }
    let src = dir.join("gba_bios.gba");
    let Ok(bytes) = std::fs::read(&src) else {
        return;
    };
    match util::write_file_atomic(&dst, &bytes) {
        Ok(()) => util::log("bios: copied gba_bios.gba -> gba_bios.bin"),
        Err(e) => util::log(&format!("bios: cannot create {}: {e}", dst.display())),
    }
}

fn log_bios(system_id: &str, dir: &Path) {
    let names: &[&str] = match system_id {
        "gba" => &["gba_bios.bin", "gba_bios.gba"],
        "psx" => &["scph5500.bin", "scph5501.bin", "scph5502.bin"],
        "dc" => &["dc/dc_boot.bin", "dc/dc_flash.bin", "dc/dc_nvmem.bin"],
        "pce" => &[
            "syscard3.pce",
            "syscard3u.pce",
            "syscard2.pce",
            "syscard2u.pce",
            "syscard1.pce",
            "gexpress.pce",
        ],
        "pokemini" => &["bios.min"],
        _ => &[],
    };
    if names.is_empty() {
        return;
    }
    let found: Vec<String> = names
        .iter()
        .filter(|name| dir.join(name).is_file())
        .map(|name| (*name).to_string())
        .collect();
    util::log(&format!(
        "bios: {system_id} dir={} found=[{}]",
        dir.display(),
        found.join(", ")
    ));
}

#[cfg(target_os = "linux")]
struct GovernorGuard;

#[cfg(target_os = "linux")]
impl GovernorGuard {
    fn new(performance: bool) -> Self {
        util::set_cpu_governor(if performance {
            "performance"
        } else {
            "schedutil"
        });
        Self
    }
}

#[cfg(target_os = "linux")]
impl Drop for GovernorGuard {
    fn drop(&mut self) {
        util::set_cpu_governor("schedutil");
    }
}

/// The GPU devfreq minimum (H700: 420 MHz idle, 648 MHz top) for the
/// hardware-rendered core.
#[cfg(target_os = "linux")]
struct GpuGovernorGuard(Option<String>);

#[cfg(target_os = "linux")]
impl GpuGovernorGuard {
    fn new(performance: bool) -> Self {
        Self(if performance {
            util::set_gpu_min_freq_max()
        } else {
            None
        })
    }
}

#[cfg(target_os = "linux")]
impl Drop for GpuGovernorGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.0.take() {
            util::restore_gpu_min_freq(&previous);
        }
    }
}

/// Next frame deadline and how long to wait first. A short overrun keeps the
/// clock so the following frames catch up; a real stall re-anchors instead of
/// replaying a burst of frames.
fn pace(now: Instant, deadline: Instant, fps_dt: Duration) -> (Instant, Option<Duration>) {
    if now < deadline {
        (deadline + fps_dt, Some(deadline - now))
    } else if now.saturating_duration_since(deadline) < fps_dt * 3 {
        (deadline + fps_dt, None)
    } else {
        (now + fps_dt, None)
    }
}

fn play(system_id: &str, core_name: &str, rom: &str) -> Result<()> {
    util::log(&format!(
        "launch: system={system_id} core={core_name} rom={rom}"
    ));
    // Read the shader before the core registers its options, so the
    // LCD-accurate GB choice can force the grey palette from the first frame.
    let mut disp = load_display_prefs(system_id);
    let swap_ab = load_swap_ab(system_id);
    if swap_ab {
        util::log("input: A/B swapped");
    }
    let swap_l1r1 = load_swap_l1r1(system_id);
    if swap_l1r1 {
        util::log("input: L1/R1 swapped");
    }
    let analog_dpad = (system_id == "psx" || system_id == "dc") && load_analog_dpad(system_id);
    if analog_dpad {
        util::log("input: d-pad as left analog stick");
    }
    if !disp.shader.supported(system_id) {
        disp.shader = ShaderMode::Off;
    }
    if disp.shader == ShaderMode::LcdAccurate {
        disp.scale = ScaleMode::Integer;
    }
    let lcd_accurate = disp.shader == ShaderMode::LcdAccurate;
    let reflective = system_id == "gb" && lcd_accurate;
    #[cfg(target_os = "linux")]
    util::hdmi_init();
    #[cfg(target_os = "linux")]
    let _governor = GovernorGuard::new(needs_performance(system_id));
    #[cfg(target_os = "linux")]
    let _gpu_governor = GpuGovernorGuard::new(system_id == "dc");

    let catalogue = Catalogue::load(Path::new("."));
    let folder = catalogue
        .systems
        .iter()
        .find(|s| s.id == system_id)
        .map(|s| s.folder())
        .unwrap_or_else(|| system_id.to_uppercase());
    let save_dir = format!("Saves/{folder}");
    std::fs::create_dir_all(&save_dir).with_context(|| format!("create {save_dir}"))?;
    let bios_dir = std::env::current_dir()
        .map(|cwd| cwd.join("BIOS"))
        .unwrap_or_else(|_| Path::new("BIOS").to_path_buf());
    if system_id == "gba" {
        ensure_gba_bios(&bios_dir);
    }
    if system_id == "dc" {
        let _ = std::fs::create_dir_all(bios_dir.join("dc"));
    }
    log_bios(system_id, &bios_dir);
    let system_dir = bios_dir.to_string_lossy().into_owned();

    let stem = Path::new(rom)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("game")
        .to_string();
    let srm_path = Path::new(&save_dir).join(format!("{stem}.srm"));
    let state_paths: Vec<std::path::PathBuf> = (0..STATE_SLOTS)
        .map(|s| state_slot_path(&save_dir, &stem, s))
        .collect();

    let candidates = [
        format!("System/magi/cores/{core_name}_libretro.so"),
        format!("System/magi/cores/{core_name}.so"),
    ];
    let core_path = candidates
        .iter()
        .find(|p| Path::new(p).is_file())
        .with_context(|| format!("no core found: {}", candidates.join(", ")))?;
    util::log(&format!("core: {core_path}"));

    let lib = unsafe { Library::new(core_path) }
        .with_context(|| format!("cannot load core {core_path}"))?;

    macro_rules! sym {
        ($ty:ty, $name:literal) => {{
            let s = unsafe { lib.get::<$ty>($name) }
                .with_context(|| format!("core missing {}", stringify!($name)))?;
            *s
        }};
    }

    let api_version: unsafe extern "C" fn() -> c_uint =
        sym!(unsafe extern "C" fn() -> c_uint, b"retro_api_version\0");
    let set_environment: unsafe extern "C" fn(extern "C" fn(c_uint, *mut c_void) -> bool) = sym!(
        unsafe extern "C" fn(extern "C" fn(c_uint, *mut c_void) -> bool),
        b"retro_set_environment\0"
    );
    let set_video: unsafe extern "C" fn(extern "C" fn(*const c_void, c_uint, c_uint, usize)) = sym!(
        unsafe extern "C" fn(extern "C" fn(*const c_void, c_uint, c_uint, usize)),
        b"retro_set_video_refresh\0"
    );
    let set_sample: unsafe extern "C" fn(extern "C" fn(i16, i16)) = sym!(
        unsafe extern "C" fn(extern "C" fn(i16, i16)),
        b"retro_set_audio_sample\0"
    );
    let set_batch: unsafe extern "C" fn(extern "C" fn(*const i16, usize) -> usize) = sym!(
        unsafe extern "C" fn(extern "C" fn(*const i16, usize) -> usize),
        b"retro_set_audio_sample_batch\0"
    );
    let set_input_poll: unsafe extern "C" fn(extern "C" fn()) = sym!(
        unsafe extern "C" fn(extern "C" fn()),
        b"retro_set_input_poll\0"
    );
    let set_input_state: unsafe extern "C" fn(
        extern "C" fn(c_uint, c_uint, c_uint, c_uint) -> i16,
    ) = sym!(
        unsafe extern "C" fn(extern "C" fn(c_uint, c_uint, c_uint, c_uint) -> i16),
        b"retro_set_input_state\0"
    );
    let retro_init: unsafe extern "C" fn() = sym!(unsafe extern "C" fn(), b"retro_init\0");
    let retro_deinit: unsafe extern "C" fn() = sym!(unsafe extern "C" fn(), b"retro_deinit\0");
    let get_system_info: unsafe extern "C" fn(*mut SystemInfo) = sym!(
        unsafe extern "C" fn(*mut SystemInfo),
        b"retro_get_system_info\0"
    );
    let get_av_info: unsafe extern "C" fn(*mut SystemAvInfo) = sym!(
        unsafe extern "C" fn(*mut SystemAvInfo),
        b"retro_get_system_av_info\0"
    );
    let load_game: unsafe extern "C" fn(*const GameInfo) -> bool = sym!(
        unsafe extern "C" fn(*const GameInfo) -> bool,
        b"retro_load_game\0"
    );
    let set_controller: unsafe extern "C" fn(c_uint, c_uint) = sym!(
        unsafe extern "C" fn(c_uint, c_uint),
        b"retro_set_controller_port_device\0"
    );
    let unload_game: unsafe extern "C" fn() = sym!(unsafe extern "C" fn(), b"retro_unload_game\0");
    let retro_run: unsafe extern "C" fn() = sym!(unsafe extern "C" fn(), b"retro_run\0");
    let serialize_size: unsafe extern "C" fn() -> usize =
        sym!(unsafe extern "C" fn() -> usize, b"retro_serialize_size\0");
    let serialize: unsafe extern "C" fn(*mut c_void, usize) -> bool = sym!(
        unsafe extern "C" fn(*mut c_void, usize) -> bool,
        b"retro_serialize\0"
    );
    let unserialize: unsafe extern "C" fn(*const c_void, usize) -> bool = sym!(
        unsafe extern "C" fn(*const c_void, usize) -> bool,
        b"retro_unserialize\0"
    );
    let mem_data: unsafe extern "C" fn(c_uint) -> *mut c_void = sym!(
        unsafe extern "C" fn(c_uint) -> *mut c_void,
        b"retro_get_memory_data\0"
    );
    let mem_size: unsafe extern "C" fn(c_uint) -> usize = sym!(
        unsafe extern "C" fn(c_uint) -> usize,
        b"retro_get_memory_size\0"
    );

    if unsafe { api_version() } != 1 {
        bail!("core reports unsupported api version");
    }

    let mut ctx_box = Box::new(Ctx {
        pixfmt: PIXFMT_0RGB1555,
        geometry: GameGeometry::default(),
        frame: std::ptr::null(),
        frame_seq: 0,
        pitch: 0,
        bits: AtomicU16::new(0),
        analog_dpad: AtomicBool::new(false),
        left: (AtomicI16::new(0), AtomicI16::new(0)),
        right: (AtomicI16::new(0), AtomicI16::new(0)),
        audio: Mutex::new(Vec::new()),
        audio_rate: AtomicU32::new(44100),
        fps_milli: AtomicU32::new(0),
        audio_pacer: Mutex::new(AudioPacer {
            start: Instant::now(),
            produced: 0,
        }),
        audio_samples: AtomicU64::new(0),
        audio_sleep_us: AtomicU64::new(0),
        save_dir: CString::new(save_dir.as_str()).unwrap(),
        system_dir: CString::new(system_dir.as_str()).unwrap(),
        shutdown: false,
        sram_ptr: std::ptr::null_mut(),
        ser_size: serialize_size,
        ser: serialize,
        unser: unserialize,
        state_size: 0,
        states_blocked: false,
        options: Vec::new(),
        opt_key: core_name.to_string(),
        opt_dirty: false,
        saved_opts: load_saved_options(core_name, system_id),
        overlay_layers: std::array::from_fn(|_| Vec::new()),
        overlay_dirty: false,
        moto: None,
        rumble_on: false,
        rumble_at: None,
        rumble_high: false,
        rumble_peak: 0,
        rumble_low_at: None,
        rumble_armed: true,
        rumble_ok: true,
        disk: None,
        system: system_id.to_string(),
        rom: rom.to_string(),
        core_version: String::new(),
        netplay: None,
        netplay_linked: false,
        lcd_accurate,
        reflective,
        gb_color_backup: None,
        #[cfg(target_os = "linux")]
        hw_render: None,
    });
    CTX.store(&mut *ctx_box, Ordering::Relaxed);

    unsafe {
        set_environment(env_cb);
        set_video(video_cb);
        set_sample(audio_sample_cb);
        set_batch(audio_batch_cb);
        set_input_poll(input_poll_cb);
        set_input_state(input_state_cb);
        retro_init();
    }

    let mut info = SystemInfo {
        library_name: std::ptr::null(),
        library_version: std::ptr::null(),
        valid_extensions: std::ptr::null(),
        need_fullpath: false,
        block_extract: false,
    };
    unsafe { get_system_info(&mut info) };
    let library_version = cstr(info.library_version).to_string();
    util::log(&format!(
        "core: {} {}",
        cstr(info.library_name),
        library_version
    ));
    ctx().core_version = library_version;

    // Cores that do not need a full path read the ROM from memory; handing
    // them a null buffer makes them refuse the game (gambatte does).
    let rom_bytes = if info.need_fullpath {
        None
    } else {
        Some(std::fs::read(rom).with_context(|| format!("cannot read rom {rom}"))?)
    };
    let (data, size) = match &rom_bytes {
        Some(b) => (b.as_ptr() as *const c_void, b.len()),
        None => (std::ptr::null(), 0),
    };

    let sdl = sdl2::init().map_err(anyhow::Error::msg)?;
    // SDL owns Bluetooth/USB pads (its controller database maps them); the
    // stock pad stays on evdev.
    let mut pads = crate::pads::Pads::new(&sdl);
    let mut sdl_events = sdl.event_pump().map_err(anyhow::Error::msg)?;
    let dev = crate::device::detect();
    let mut presenter = Presenter::open(&sdl, dev, crate::device::window_scale())?;
    let (cw, ch) = presenter.canvas_size();
    let mut cv = Canvas::new(cw, ch);
    // The GPU composites this layer over the game frame: the in-game
    // overlays, the OSD and the pause menu are drawn here, with a real
    // alpha, so the panel is never handed back to the software presenter
    // (two presenters fighting over the framebuffer pages leave a stale
    // frame on screen).
    let mut layer = Canvas::transparent(cw, ch);
    let mut fonts = Fonts::new(font::FONT_DATA)?;
    #[cfg(target_os = "linux")]
    let hdmi_at_launch = util::hdmi_active();
    #[cfg(target_os = "linux")]
    let mut gpu = if std::env::var("MAGI_GPU").map_or(true, |v| v != "0") {
        match crate::present::gles::Gpu::new(cw, ch) {
            Ok(gpu) => {
                util::log("gpu: EGL/GLES presenter up");
                Some(gpu)
            }
            Err(e) => {
                util::log(&format!("gpu: {e}; software presenter"));
                None
            }
        }
    } else {
        util::log("gpu: disabled by MAGI_GPU=0");
        None
    };
    // The core may only use the hardware path while the EGL context can be
    // composited; HDMI keeps the software presenter.
    #[cfg(target_os = "linux")]
    {
        let ptr = if hdmi_at_launch {
            std::ptr::null_mut()
        } else {
            gpu.as_mut().map_or(std::ptr::null_mut(), |g| g as *mut _)
        };
        HW_GPU.store(ptr, Ordering::Relaxed);
        if ptr.is_null() {
            util::log("hw: no GLES context; cores run software");
        }
    }

    // Input sticks must be known before the core loads: Flycast registers its
    // option list there, and the analog deadzones only appear when a real
    // stick is attached.
    #[cfg(target_os = "linux")]
    let mut evdev = Evdev::open();
    #[cfg(target_os = "linux")]
    let analog_pad = pads.has_stick() || evdev.has_stick() || analog_dpad;
    #[cfg(not(target_os = "linux"))]
    let analog_pad = pads.has_stick() || analog_dpad;
    if analog_pad {
        reveal_analog_options();
    }

    let rom_c = CString::new(rom).unwrap();
    let game = GameInfo {
        path: rom_c.as_ptr(),
        data,
        size,
        meta: std::ptr::null(),
    };
    if !unsafe { load_game(&game) } {
        bail!("core refused to load {rom}");
    }
    if ctx().reflective {
        let color = ctx()
            .saved_opts
            .get("gambatte_gb_colorization")
            .cloned()
            .unwrap_or_else(|| "auto".into());
        ctx().gb_color_backup = Some(color);
    }
    let discs = disc_count();
    if discs > 1 {
        util::log(&format!("disc: {discs} images"));
    }

    let mut av = SystemAvInfo::default();
    unsafe { get_av_info(&mut av) };
    let fps = if av.timing.fps > 1.0 {
        av.timing.fps
    } else {
        60.0
    };
    let sample_rate = av.timing.sample_rate.max(8000.0) as u32;
    util::log(&format!(
        "video: {}x{} @ {:.2} fps; audio {} Hz",
        av.geometry.base_width, av.geometry.base_height, fps, sample_rate
    ));
    ctx().audio_rate.store(sample_rate, Ordering::Relaxed);
    ctx()
        .fps_milli
        .store((fps * 1000.0).round() as u32, Ordering::Relaxed);
    ctx().reset_audio_pacer();

    // A core that took the hardware path draws into an FBO the size of its
    // base geometry. `context_reset` is only safe after `retro_load_game`
    // returns: the core's context object exists by then.
    #[cfg(target_os = "linux")]
    let hw_render = ctx().hw_render.is_some();
    #[cfg(target_os = "linux")]
    let hw_flip = ctx().hw_render.as_ref().is_some_and(|hw| hw.bottom_left);
    #[cfg(target_os = "linux")]
    let hw_stencil = ctx().hw_render.as_ref().is_some_and(|hw| hw.stencil);
    #[cfg(not(target_os = "linux"))]
    let hw_render = false;
    #[cfg(target_os = "linux")]
    if hw_render {
        let (fw, fh) = (
            av.geometry.base_width.max(1),
            av.geometry.base_height.max(1),
        );
        let Some(g) = gpu.as_mut() else {
            bail!("hardware render without a GL context");
        };
        let fbo = match g.hw_framebuffer(fw, fh, hw_stencil) {
            Ok(fbo) => fbo,
            Err(e) => {
                util::log(&format!("hw: {e}"));
                bail!("hardware framebuffer setup failed");
            }
        };
        HW_FBO.store(fbo as usize, Ordering::Relaxed);
        util::log(&format!("hw: framebuffer {fbo} {fw}x{fh}"));
        let reset = ctx().hw_render.as_ref().and_then(|hw| hw.reset);
        if let Some(reset) = reset {
            unsafe { reset() };
            util::log("hw: context reset");
        }
    }

    // SRAM.
    let sram_size = unsafe { mem_size(0) };
    let sram_ptr = unsafe { mem_data(0) } as *mut u8;
    ctx().sram_ptr = sram_ptr;
    if sram_size > 0 && !sram_ptr.is_null() {
        if let Ok(bytes) = std::fs::read(&srm_path) {
            let n = bytes.len().min(sram_size);
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), sram_ptr, n) };
            util::log(&format!("sram: loaded {n} bytes"));
        }
    }

    #[cfg(target_os = "linux")]
    let mut lid = crate::util::LidWatch::new();
    // Beetle PCE only reads the joypad after the frontend selects it for
    // the port; it defaults to no device and otherwise ignores all input.
    if system_id == "pce" {
        unsafe { set_controller(0, RETRO_DEVICE_JOYPAD) };
    }
    if system_id == "psx" {
        // PCSX-ReARMed only accepts subclasses; a plain ANALOG (5) falls
        // through to NONE and disables the pad. Only DualShock exposes
        // rumble, but a pad with no stick must keep the standard type or
        // the d-pad stops working.
        let device = if analog_pad {
            RETRO_DEVICE_PSE_DUALSHOCK
        } else {
            RETRO_DEVICE_JOYPAD
        };
        unsafe { set_controller(0, device) };
        util::log(&format!(
            "psx: controller {}",
            if analog_pad { "dualshock" } else { "standard" }
        ));
    }
    // Cores reporting their own rate are resampled in software.
    #[cfg(target_os = "linux")]
    let bt_audio = crate::util::bt_audio_device();
    #[cfg(target_os = "linux")]
    if let Some(device) = &bt_audio {
        util::log(&format!("audio: bluetooth output {device}"));
    }
    #[cfg(target_os = "linux")]
    let mut audio = AlsaOut::start(OUTPUT_HZ, bt_audio);
    #[cfg(target_os = "linux")]
    let mut resampler = Resampler::new(sample_rate as f64, OUTPUT_HZ as f64);
    #[cfg(target_os = "linux")]
    let mut audio_out: Vec<i16> = Vec::new();

    // Software volume: the mixer stays at 100% and the persisted percentage
    // scales the samples.
    let mut gain_pct: u8 = load_runtime_volume();
    let (ff_steps, ff_sound) = load_runtime_prefs();
    #[cfg(target_os = "linux")]
    if let Some(gpu) = gpu.as_mut() {
        gpu.set_system(system_id);
        gpu.set_shader(disp.shader.as_u8());
    }
    ctx().rumble_ok = disp.rumble;
    ctx().analog_dpad.store(analog_dpad, Ordering::Relaxed);
    ctx().reset_motor();
    crate::theme::load_user_themes(std::path::Path::new(crate::systems::DIR_THEMES));
    crate::theme::set_theme_index(disp.theme);
    crate::theme::set_bg_style(disp.background);
    crate::theme::set_anim_bg(disp.anim_bg);
    crate::theme::set_glow(disp.glow);
    crate::theme::set_ui_scale(load_text_scale());
    crate::theme::set_warmth(disp.warmth as u8);
    crate::input::set_shot_chord(load_shot_chord_pref());
    // The panel keeps its level across reboots only through us.
    util::set_brightness(disp.brightness);
    // Volume is software gain here; keep the ALSA mixer out of the way.
    util::set_mixer_volume(100);
    let mut adj_state: Option<AdjState> = None;
    // The adjustments are written back a moment after they happen: a reboot
    // in the middle of a game must not lose them.
    let mut prefs_save_at: Option<Instant> = None;
    let mut combo_hat = (0i32, 0i32);
    let mut pad_hat = PadHat::default();
    let mut bar_hud: Option<(crate::theme::BarIcon, f32, Instant)> = None;
    let mut hud: Option<(String, Instant)> = None;
    let mut vol_hud: Option<(f32, Instant)> = None;
    let mut ff_latch = false;
    let mut last_r2: Option<Instant> = None;
    let mut r2_prev = false;
    let mut select_down = false;
    let mut r1_down = false;
    let mut r2_down = false;
    let mut l2_down = false;
    // PS1 games need L2/R2 themselves; the frontend's rewind and FF then
    // move under SELECT (SELECT+L2 / SELECT+R2). Dreamcast's core reads its
    // triggers from L2/R2, so the handheld puts them on L1/R1 instead.
    let game_l2_r2 = needs_l2_r2(system_id);
    let triggers_l1r1 = system_id == "dc";
    let mut menu_hold_t0: Option<Instant> = None;
    let mut menu_close_pending = false;
    let mut menu_tl2_down = false;
    let mut menu_goto_down = false;
    let mut menu_release_guard: Option<Instant> = None;
    let mut power_down: Option<Instant> = None;
    let mut shutdown_t0: Option<Instant> = None;
    let mut shutdown_sent = false;
    let mut power_guard: Option<Instant> = None;
    let mut auto_pending = true;
    let mut auto_hold: Option<(std::path::PathBuf, Instant)> = None;
    let mut a_down = false;
    let threaded =
        core_option_value(ctx(), "reicast_threaded_rendering").as_deref() != Some("disabled");
    ctx().states_blocked = flycast_states_blocked(core_name, threaded);
    if ctx().states_blocked {
        util::log("states: off while Flycast threaded rendering is on");
    }
    let ser_size = unsafe { (ctx().ser_size)() };
    ctx().state_size = ser_size;
    let mut rewind_ok = rewind_allowed(system_id, ser_size);
    if !rewind_ok {
        if system_id == "psx" {
            util::log(&format!("rewind: off for {system_id}"));
        } else if ser_size > REWIND_MAX_STATE {
            util::log(&format!("rewind: off ({ser_size} byte states)"));
        }
    }
    let mut rewind_ring: VecDeque<Vec<u8>> = VecDeque::new();
    let mut rewind_bytes = 0usize;
    let mut rewind_cost_ms = 0.0f64;
    let mut rewind_next = 0u64;

    // Overlay layers: scan the folder into per-layer lists, then resolve
    // the saved choices (a file now living in another layer reads as off).
    ctx().overlay_layers = overlay_scan_layers();
    let wide = (cw as u64) * 3 > (ch as u64) * 4;
    let mut overlay_names = load_overlay_prefs(system_id, rom, wide);
    let mut overlay_idx: [usize; 2] = [0; 2];
    for i in 0..2 {
        if overlay_names[i].is_empty() {
            continue;
        }
        match ctx().overlay_layers[i]
            .iter()
            .position(|f| f == &overlay_names[i])
        {
            Some(pos) => overlay_idx[i] = pos + 1,
            None => overlay_names[i].clear(),
        }
    }
    let mut overlay_art = load_composite(&overlay_names, cw, ch);
    let mut bezel_area = load_bezel_area(&overlay_names, cw, ch);
    // The software composite stays loaded even when the overlay runs on the
    // GPU, so the fallback paths never miss it.
    let mut overlays = overlay_art.as_deref().map(premultiplied);
    #[cfg(target_os = "linux")]
    if let Some(gpu) = gpu.as_mut() {
        gpu.set_overlay(overlay_art.as_deref(), cw, ch);
    }

    // Hardware scanout: the core frame goes to the DE scaler layer (ch1
    // lyr0), the UI canvas and the overlay to pixel-alpha layers above it
    // (lyr1/lyr2, 1:1 - DE33 only scales the VI layer). Softer setups
    // (0RGB1555, warmth, HDMI) keep the software blit.
    #[cfg(target_os = "linux")]
    let mut hw_game = {
        let (fw, fh) = (
            ctx().geometry.base_width.max(1),
            ctx().geometry.base_height.max(1),
        );
        let layer = game_layer_format(ctx().pixfmt).and_then(|(fmt, bpp)| {
            crate::disp2::GameLayer::create(cw.max(fw), ch.max(fh), fmt, bpp)
        });
        if let Some(_) = layer.as_ref() {
            crate::disp2::set_background(0x0003_0303);
        } else {
            util::log("scanout: no hardware game layer; software blit only");
        }
        layer
    };
    #[cfg(target_os = "linux")]
    let mut hw_overlay = crate::disp2::ImageLayer::create(0, 1, 2, cw, ch);
    #[cfg(target_os = "linux")]
    let mut hw_ui = crate::disp2::ImageLayer::create(0, 2, 3, cw, ch);
    // The DE layer's pixel alpha does not blend on this display engine (the
    // layer covers the game); overlays stay in the software composite.
    // The DE stack starts disabled: the software present path owns the
    // panel until `SCANOUT_ENABLED` is on, and leftover enabled layers from
    // a crashed host must not cover the fb.
    #[cfg(target_os = "linux")]
    {
        if let Some(g) = hw_game.as_mut() {
            g.set_visible(false);
        }
        if let Some(o) = hw_overlay.as_mut() {
            o.set_visible(false);
        }
        if let Some(u) = hw_ui.as_mut() {
            u.set_visible(false);
        }
        crate::disp2::fb_restore();
    }
    #[cfg(target_os = "linux")]
    let mut overlay_rect: Option<(i32, i32, u32, u32)> = None;
    #[cfg(target_os = "linux")]
    let mut scanout_active = false;
    #[cfg(target_os = "linux")]
    let mut fb_hide_warned = false;
    #[cfg(target_os = "linux")]
    let mut ui_was_active = false;
    #[cfg(target_os = "linux")]
    let mut hw_present_warned = false;
    #[cfg(target_os = "linux")]
    let mut last_seq = 0u64;

    let mut menu = GameMenu {
        open: false,
        sel: 0,
        msg: None,
        exit_requested: false,
        page: MenuPage::Main,
        slot: 0,
        opt_sel: 0,
        opt_top: 0,
        opt_since: Instant::now(),
        overlay_sel: 0,
        overlay_idx,
        shader: disp.shader,
        fps: disp.fps_counter,
        sysinfo: disp.sysinfo,
        hold: None,
        disc_sel: 0,
        netplay_page: NetplayPage::Menu,
        netplay_sel: 0,
        netplay_transport: netplay::Transport::Wifi,
        netplay_role: netplay::Role::Host,
        netplay_msg: None,
        focus_pane: false,
        states: Vec::new(),
        state_paths: state_paths.clone(),
        auto_path: auto_state_path(&save_dir, &stem),
        delete_armed: None,
    };
    let mut shortcuts = Shortcuts::default();
    let mut take_shot = false;
    let mut osd = Osd::new();
    let mut pad = PadState::default();
    let analog_core = (system_id == "psx" || system_id == "dc") && analog_pad;
    pad.analog = analog_core;
    let mut last_sram_flush = Instant::now();
    let mut fps_dt = Duration::from_secs_f64(1.0 / fps);
    let mut loop_fps = fps;
    let mut deadline = Instant::now() + fps_dt;
    FASTFORWARD.store(false, Ordering::Relaxed);
    let mut frames = 0u32;
    let mut core_frames = 0u64;
    let mut work_total = Duration::ZERO;
    let mut pres_ms = 0f64;
    let mut t_core = Duration::ZERO;
    let mut t_blit = Duration::ZERO;
    let mut t_ovl = Duration::ZERO;
    let mut t_hud = Duration::ZERO;
    let mut t_aud = Duration::ZERO;
    let mut t_pres = Duration::ZERO;
    let mut t_rew = Duration::ZERO;
    let mut last_report = Instant::now();
    let mut last_slow_log = Instant::now();
    let mut last_start = Instant::now();
    let mut worst_gap = Duration::ZERO;
    let mut worst_work = Duration::ZERO;
    let mut late_frames = 0u32;
    let mut min_level = 1.0f32;

    // Battery policy on a worker: the PMIC reads can block and must never
    // stall the frame that feeds the audio ring.
    let power_off = Arc::new(AtomicBool::new(false));
    {
        let flag = power_off.clone();
        let (shutdown_pct, charge_limit) = (disp.bat_shutdown, disp.charge_limit);
        std::thread::Builder::new()
            .name("magi-battery".into())
            .spawn(move || {
                let mut latch = false;
                loop {
                    std::thread::sleep(Duration::from_secs(10));
                    if util::battery_policy(shutdown_pct, charge_limit, &mut latch) {
                        flag.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            })
            .ok();
    }

    ctx().reset_audio_pacer();
    util::log("game loop up");
    let play_t0 = Instant::now();

    loop {
        if ctx().shutdown {
            util::log("core requested shutdown");
            break;
        }
        // The core re-reports the real refresh rate after the game sets its
        // video mode. A PAL title is 50 Hz, and pacing it at the load-time
        // 59.83 Hz leaves the audio ring fighting the frame clock.
        let core_fps = f64::from(ctx().fps_milli.load(Ordering::Relaxed)) / 1000.0;
        if core_fps > 1.0 && (core_fps - loop_fps).abs() > 0.005 {
            loop_fps = core_fps;
            fps_dt = Duration::from_secs_f64(1.0 / core_fps);
            deadline = Instant::now() + fps_dt;
            util::log(&format!("video: pacing {core_fps:.2} fps"));
        }

        // The lid has no input events on this kernel; its hallkey attribute
        // is polled instead. SRAM first, then suspend.
        #[cfg(target_os = "linux")]
        if lid.poll() {
            if disp.lid_sleep {
                util::log("lid closed (hallkey); sleeping");
                if sram_size > 0 && !sram_ptr.is_null() {
                    save_sram(&srm_path, sram_ptr, sram_size);
                }
                util::sleep_now(disp.shutdown_min * 60);
                pad.reset();
            } else {
                util::log("lid closed (hallkey); lid sleep is off");
            }
        }

        // Battery policy ran on its worker; act on the verdict here.
        if power_off.load(Ordering::Relaxed) {
            util::log("battery: policy requested power off");
            if sram_size > 0 && !sram_ptr.is_null() {
                save_sram(&srm_path, sram_ptr, sram_size);
            }
            if save_state_to(&auto_state_path(&save_dir, &stem)) == Some(true) {
                util::log("battery: auto state saved");
            }
            #[cfg(target_os = "linux")]
            audio.close();
            util::set_brightness(0);
            util::spawn_detached("poweroff");
            break;
        }

        // ---- input ----
        let (pad_raw, pads_changed) =
            pump_pads(&mut pads, &mut sdl_events, &mut pad_hat, !analog_core);
        #[cfg(target_os = "linux")]
        if pads_changed {
            evdev.rescan();
        }
        #[cfg(not(target_os = "linux"))]
        let _ = pads_changed;
        #[cfg(target_os = "linux")]
        let mut raw = evdev.drain_raw();
        #[cfg(not(target_os = "linux"))]
        let mut raw: Vec<RawEvent> = Vec::new();
        raw.extend(pad_raw);

        for e in raw.iter() {
            // Held-state tracking for the gestures below: SELECT, R1, L2/R2
            // and MENU (both of its codes); A down over the boot means
            // "start fresh", not "resume".
            if e.type_ == ev::EV_KEY {
                match e.code {
                    ev::BTN_TL => select_down = e.value != 0,
                    ev::BTN_Z => r1_down = e.value != 0,
                    ev::BTN_SELECT => l2_down = e.value != 0,
                    ev::BTN_START => r2_down = e.value != 0,
                    ev::BTN_SOUTH => a_down = e.value != 0,
                    ev::BTN_TL2 | ev::KEY_GOTO => {
                        // MENU has two codes (TL2 and the KEY_GOTO alias);
                        // treat them as one button so the alias release can
                        // never cut a held press short.
                        if e.code == ev::BTN_TL2 {
                            menu_tl2_down = e.value != 0;
                        } else {
                            menu_goto_down = e.value != 0;
                        }
                        if menu_tl2_down || menu_goto_down {
                            menu_release_guard = None;
                            menu_hold_t0.get_or_insert(Instant::now());
                        } else {
                            menu_release_guard = Some(Instant::now() + MENU_ALIAS);
                        }
                    }
                    _ => {}
                }
            }
            // The lid: closing suspends (SRAM first), opening wakes in the
            // kernel before we ever run again.
            if e.type_ == ev::EV_SW && e.code == ev::SW_LID {
                if e.value == 1 {
                    if disp.lid_sleep {
                        util::log("lid closed; sleeping");
                        if sram_size > 0 && !sram_ptr.is_null() {
                            save_sram(&srm_path, sram_ptr, sram_size);
                        }
                        util::sleep_now(disp.shutdown_min * 60);
                        pad.reset();
                    } else {
                        util::log("lid closed; lid sleep is off");
                    }
                } else {
                    util::log("lid opened");
                }
                continue;
            }
            // POWER: a tap sleeps (a restore point is left behind for the
            // auto-off timer case); held, the EVA shutdown countdown runs.
            // The press that woke us from sleep is ignored for a moment so
            // waking cannot immediately sleep again.
            if e.type_ == ev::EV_KEY && e.code == ev::KEY_POWER {
                if e.value == 1 {
                    if !power_guard.is_some_and(|t| Instant::now() < t) {
                        power_down = Some(Instant::now());
                    }
                } else if e.value == 0 {
                    if power_guard.is_some_and(|t| Instant::now() < t) {
                        power_down = None;
                    } else if shutdown_t0.take().is_some() {
                        util::log("power: shutdown aborted");
                        power_down = None;
                    } else if let Some(t0) = power_down.take() {
                        if t0.elapsed() < POWER_HOLD {
                            util::log("power tap: sleep");
                            if sram_size > 0 && !sram_ptr.is_null() {
                                save_sram(&srm_path, sram_ptr, sram_size);
                            }
                            match save_state_to(&auto_state_path(&save_dir, &stem)) {
                                Some(true) => util::log("power tap: auto state saved"),
                                Some(false) => util::log("power tap: auto state save failed"),
                                None => {}
                            }
                            util::sleep_now(disp.shutdown_min * 60);
                            // We woke mid-session (RAM intact), so the
                            // restore point is stale now.
                            let auto = auto_state_path(&save_dir, &stem);
                            let _ = std::fs::remove_file(&auto);
                            let _ = std::fs::remove_file(state_thumb_path(&auto));
                            power_guard = Some(Instant::now() + Duration::from_millis(1200));
                            pad.reset();
                        }
                    }
                }
                continue;
            }
            if let Some(sc) = shortcuts.feed(e) {
                match sc {
                    Shortcut::Screenshot => take_shot = true,
                    Shortcut::MenuTap => {
                        // A press while open may be the start of the
                        // hold-to-save gesture: close on release only.
                        if menu.open {
                            // A press while open may be the start of the
                            // hold-to-save gesture: close on release only.
                            menu_close_pending = true;
                        } else {
                            menu.open_menu();
                            pad.reset(); // don't leak held buttons into the menu
                        }
                    }
                    Shortcut::SelectTap => {}
                }
            }
            if e.type_ == ev::EV_KEY
                && e.value == 1
                && (e.code == ev::KEY_VOLUMEUP || e.code == ev::KEY_VOLUMEDOWN)
            {
                let delta = if e.code == ev::KEY_VOLUMEUP { 5 } else { -5 };
                gain_pct = (gain_pct as i32 + delta).clamp(0, 100) as u8;
                vol_hud = Some((gain_pct as f32 / 100.0, Instant::now()));
                // Keep the new level even if the console goes down soon.
                prefs_save_at = Some(Instant::now() + Duration::from_millis(400));
                continue;
            }
            if e.type_ == ev::EV_ABS {
                match e.code {
                    ev::ABS_HAT0X => combo_hat.0 = e.value,
                    ev::ABS_HAT0Y => combo_hat.1 = e.value,
                    ev::ABS_Z => l2_down = e.value > TRIGGER_ON,
                    ev::ABS_RZ => r2_down = e.value > TRIGGER_ON,
                    _ => {}
                }
            }
            // SELECT combinations are pointers, not game input: with X the
            // scale mode cycles (SELECT+L2 too, except on PS1 where L2
            // rewinds); with the d-pad brightness and warmth adjust; with
            // R1/L1 the current save slot is written or read. They work
            // while the pause menu is open too (the gesture is swallowed, so
            // the menu cursor never moves).
            if select_down
                && menu_hold_t0.is_none()
                && !ctx().netplay_linked
                && e.type_ == ev::EV_KEY
                && e.value == 1
                && (e.code == ev::BTN_Z || e.code == ev::BTN_WEST)
            {
                let slot = menu.slot;
                if ctx().states_blocked {
                    hud = Some(("STATES NEED THREADED OFF".into(), Instant::now()));
                    continue;
                }
                let msg = if e.code == ev::BTN_Z {
                    let saved = save_state_to(&state_paths[slot]);
                    if saved == Some(true) {
                        match save_state_to(&auto_state_path(&save_dir, &stem)) {
                            Some(true) => util::log("auto state saved (quick save)"),
                            Some(false) => util::log("auto state save failed (quick save)"),
                            None => {}
                        }
                    }
                    match saved {
                        Some(true) => format!("STATE SAVED {}/{}", slot + 1, STATE_SLOTS),
                        Some(false) if util::card_read_only() => "CARD READ-ONLY".into(),
                        Some(false) => "SAVE FAILED".into(),
                        None => "SAVES NOT SUPPORTED".into(),
                    }
                } else {
                    match load_state_from(&state_paths[slot]) {
                        Ok(true) => format!("STATE LOADED {}/{}", slot + 1, STATE_SLOTS),
                        Ok(false) => "LOAD FAILED".into(),
                        Err(_) => format!("NO STATE {}/{}", slot + 1, STATE_SLOTS),
                    }
                };
                if menu.open && e.code == ev::BTN_Z {
                    menu.refresh_states();
                }
                hud = Some((msg, Instant::now()));
                continue;
            }
            if pad.bits & (1 << retro::SELECT) != 0 {
                let scale_key = e.type_ == ev::EV_KEY
                    && e.value == 1
                    && (e.code == ev::BTN_NORTH || (e.code == ev::BTN_SELECT && !game_l2_r2));
                if scale_key {
                    disp.scale = disp.scale.next();
                    sync_display_menu(&mut disp, &menu);
                    save_display_prefs(system_id, &disp);
                    hud = Some((disp.scale.label().into(), Instant::now()));
                    continue;
                }
                if e.type_ == ev::EV_ABS
                    && (e.code == ev::ABS_HAT0X || e.code == ev::ABS_HAT0Y)
                    && e.value != 0
                {
                    let kind = if e.code == ev::ABS_HAT0Y {
                        AdjKind::Brightness
                    } else {
                        AdjKind::Warmth
                    };
                    // Up means brighter (the hold-repeat below stays in
                    // step with the same mapping).
                    let dir = match kind {
                        AdjKind::Brightness => -e.value.signum(),
                        _ => e.value.signum(),
                    };
                    apply_adjust(&mut disp, kind, dir);
                    bar_hud = Some(bar_label(&disp, kind));
                    prefs_save_at = Some(Instant::now() + Duration::from_millis(400));
                    adj_state = Some(AdjState {
                        kind,
                        dir,
                        next: Instant::now() + COMBO_FIRST_REPEAT,
                    });
                    continue;
                }
            }
            if menu.open {
                menu.handle_raw(e);
            } else {
                pad.apply(e);
            }
        }
        if menu.open {
            pad.reset();
            if menu.exit_requested {
                // An explicit exit is not a resume point; only SAVE STATE
                // and the hold-MENU gesture create one.
                if sram_size > 0 && !sram_ptr.is_null() {
                    save_sram(&srm_path, sram_ptr, sram_size);
                }
                netplay_stop();
                break;
            }
        }

        netplay_tick(&mut menu, &mut hud);
        if let Some(sc) = shortcuts.tick(Instant::now()) {
            match sc {
                Shortcut::MenuTap => {
                    if menu.open {
                        menu_close_pending = true;
                    } else {
                        menu.open_menu();
                        pad.reset();
                    }
                }
                Shortcut::SelectTap => {}
                Shortcut::Screenshot => take_shot = true,
            }
        }

        // MENU held ~1 s: flush SRAM, leave an auto state and return to the
        // shelf. The hold starts at the press that opened the pause menu, so
        // a tap never triggers it; held SELECT/R1 suppress it.
        // POWER held: EVA countdown; release aborts. At zero: flush, restore
        // point, power off.
        if let (Some(t0), None, false) = (power_down, shutdown_t0, shutdown_sent) {
            if t0.elapsed() >= POWER_HOLD {
                shutdown_t0 = Some(Instant::now());
                util::log("power: shutdown countdown");
            }
        }
        if let Some(t0) = shutdown_t0 {
            if !shutdown_sent && t0.elapsed() >= SHUTDOWN_TOTAL {
                shutdown_sent = true;
                util::log("power: shutting down");
                if sram_size > 0 && !sram_ptr.is_null() {
                    save_sram(&srm_path, sram_ptr, sram_size);
                }
                if save_state_to(&auto_state_path(&save_dir, &stem)) == Some(true) {
                    util::log("power: auto state saved");
                }
                #[cfg(target_os = "linux")]
                audio.close();
                util::set_brightness(0);
                util::spawn_detached("poweroff");
            }
        }
        if menu_release_guard.is_some_and(|t| Instant::now() >= t) {
            menu_release_guard = None;
            if !menu_tl2_down && !menu_goto_down {
                menu_hold_t0 = None;
                if menu_close_pending {
                    menu_close_pending = false;
                    if menu.open {
                        menu.open = false;
                    }
                }
            }
        }
        menu.hold = None;
        if let Some(t0) = menu_hold_t0 {
            if !select_down && !r1_down {
                let held = Instant::now().duration_since(t0);
                if held >= MENU_HOLD_EXIT {
                    util::log("menu held: state + auto state + exit");
                    netplay_stop();
                    if sram_size > 0 && !sram_ptr.is_null() {
                        save_sram(&srm_path, sram_ptr, sram_size);
                    }
                    if ctx().states_blocked {
                        util::log("state save skipped (menu hold): threaded Flycast");
                    } else {
                        match save_state_to(&state_paths[menu.slot]) {
                            Some(true) => util::log("state saved (menu hold)"),
                            Some(false) if util::card_read_only() => {
                                util::log("state save failed (menu hold): card read-only")
                            }
                            Some(false) => util::log("state save failed (menu hold)"),
                            None => util::log("state save not supported by core (menu hold)"),
                        }
                        match save_state_to(&auto_state_path(&save_dir, &stem)) {
                            Some(true) => util::log("auto state saved"),
                            Some(false) => util::log("auto state save failed"),
                            None => util::log("auto state not supported by core"),
                        }
                    }
                    break;
                }
                if held >= MENU_HOLD_HINT {
                    menu.hold = Some(
                        ((held - MENU_HOLD_HINT).as_secs_f32()
                            / (MENU_HOLD_EXIT - MENU_HOLD_HINT).as_secs_f32())
                        .min(1.0),
                    );
                }
            }
        }

        if ctx().overlay_dirty {
            ctx().overlay_dirty = false;
            for (i, idx) in menu.overlay_idx.iter().enumerate() {
                overlay_names[i] = if *idx == 0 {
                    String::new()
                } else {
                    ctx()
                        .overlay_layers
                        .get(i)
                        .and_then(|l| l.get(idx - 1))
                        .cloned()
                        .unwrap_or_default()
                };
            }
            save_overlay_prefs(system_id, &overlay_names);
            sync_display_menu(&mut disp, &menu);
            if disp.shader == ShaderMode::LcdAccurate {
                disp.scale = ScaleMode::Integer;
            }
            let lcd_accurate = disp.shader == ShaderMode::LcdAccurate;
            let reflective = system_id == "gb" && lcd_accurate;
            ctx().lcd_accurate = lcd_accurate;
            set_lcd_accurate(lcd_accurate);
            ctx().reflective = reflective;
            save_display_prefs(system_id, &disp);
            overlay_art = load_composite(&overlay_names, cw, ch);
            bezel_area = load_bezel_area(&overlay_names, cw, ch);
            overlays = overlay_art.as_deref().map(premultiplied);
            #[cfg(target_os = "linux")]
            {
                if let Some(gpu) = gpu.as_mut() {
                    gpu.set_shader(disp.shader.as_u8());
                    gpu.set_overlay(overlay_art.as_deref(), cw, ch);
                }
                overlay_rect = None;
            }
        }

        ctx().bits.store(
            core_pad_bits(pad.bits, game_l2_r2, swap_ab, triggers_l1r1, swap_l1r1),
            Ordering::Relaxed,
        );
        ctx().left.0.store(pad.left.0, Ordering::Relaxed);
        ctx().left.1.store(pad.left.1, Ordering::Relaxed);
        ctx().right.0.store(pad.right.0, Ordering::Relaxed);
        ctx().right.1.store(pad.right.1, Ordering::Relaxed);

        // A held SELECT + d-pad keeps repeating its adjustment; letting go
        // of either ends the gesture and persists the new level.
        if let Some(mut st) = adj_state.take() {
            let select_held = pad.bits & (1 << retro::SELECT) != 0;
            let axis_val = match st.kind {
                AdjKind::Brightness => combo_hat.1,
                AdjKind::Warmth => combo_hat.0,
            };
            if select_held && axis_val != 0 {
                let mapped = match st.kind {
                    AdjKind::Brightness => -axis_val.signum(),
                    _ => axis_val.signum(),
                };
                if mapped != st.dir {
                    st.dir = mapped;
                }
                if Instant::now() >= st.next {
                    apply_adjust(&mut disp, st.kind, st.dir);
                    bar_hud = Some(bar_label(&disp, st.kind));
                    prefs_save_at = Some(Instant::now() + Duration::from_millis(400));
                    st.next = Instant::now() + COMBO_REPEAT;
                }
                adj_state = Some(st);
            } else {
                sync_display_menu(&mut disp, &menu);
                save_display_prefs(system_id, &disp);
                prefs_save_at = None;
            }
        }

        // R2: holding it fast-forwards; two quick presses latch it on or off.
        // PS1 games keep R2; there the gesture is SELECT+R2.
        let ff_key = !menu.open
            && if game_l2_r2 {
                select_down && r2_down
            } else {
                r2_down
            };
        if ff_key && !r2_prev && double_tap(&mut last_r2, Instant::now(), DOUBLE_TAP) {
            ff_latch = !ff_latch;
            util::log(&format!(
                "fast forward {}",
                if ff_latch { "on" } else { "off" }
            ));
        }
        r2_prev = ff_key;

        // Scanout mode: the DE owns the panel through channel 0; the fb
        // layer (channel 1) mixes above it, so it must be covered while the
        // game runs. Pixel alpha keeps its pan/vsync clock alive.
        let game_area = bezel_area.unwrap_or((0, 0, cw, ch));
        #[cfg(target_os = "linux")]
        {
            // ch0 carries the stack: lyr0 game/scaler, lyr1 overlay, lyr2 UI.
            // Off for now: the DE pan (the only vsync source) re-enables the
            // fb layer every frame, so it cannot be hidden reliably; the
            // software present path is the working default.
            const SCANOUT_ENABLED: bool = false;
            let mut want = SCANOUT_ENABLED
                && hw_game.is_some()
                && crate::theme::warmth() == 0
                && !hdmi_at_launch
                && !hw_render;
            if want && !scanout_active {
                // Channel 1 mixes above channel 0: disable the fb layer or
                // the frozen shelf frame hides the game.
                presenter.wipe();
                if !crate::disp2::fb_layer_hide() {
                    crate::disp2::fb_restore();
                    util::log("scanout: cannot hide the fb layer; software path");
                    want = false;
                }
            }
            if want != scanout_active {
                if want {
                    if let Some(g) = hw_game.as_mut() {
                        g.set_visible(true);
                    }
                    ui_was_active = false;
                } else {
                    if let Some(g) = hw_game.as_mut() {
                        g.set_visible(false);
                    }
                    if let Some(ui) = hw_ui.as_mut() {
                        ui.set_visible(false);
                    }
                    crate::disp2::fb_restore();
                    ui_was_active = false;
                }
                scanout_active = want;
            }
            if let Some(hw) = hw_overlay.as_mut() {
                hw.set_visible(scanout_active);
            }
            if scanout_active {
                let (fw, fh) = (
                    ctx().geometry.base_width.max(1) as usize,
                    ctx().geometry.base_height.max(1) as usize,
                );
                let win = game_rect(disp.scale, fw, fh, game_area);
                if overlay_rect != Some(win) {
                    upload_overlay(
                        &mut hw_overlay,
                        overlay_art.as_deref(),
                        cw as usize,
                        ch as usize,
                        win,
                    );
                    overlay_rect = Some(win);
                }
            }
        }
        #[cfg(target_os = "linux")]
        let hw_scanout = scanout_active;
        #[cfg(not(target_os = "linux"))]
        let hw_scanout = false;

        if menu.open {
            // With the GPU up the menu is a layer over the frozen game; the
            // software path still paints it on the canvas.
            #[cfg(target_os = "linux")]
            let gpu_menu =
                gpu.is_some() && !hdmi_at_launch && (hw_render || !ctx().frame.is_null());
            #[cfg(not(target_os = "linux"))]
            let gpu_menu = false;
            if hw_scanout {
                // The DE layers show the frozen frame and the overlay; the
                // canvas is composed in software so the menu's translucent
                // panels blend over the game as before.
                {
                    let ctxv = ctx();
                    if !ctxv.frame.is_null() {
                        blit(ctxv, &mut cv, disp.scale, game_area);
                    } else {
                        cv.clear(pal().bg0);
                    }
                    blend_straight(&mut cv, overlay_art.as_deref());
                }
            } else if !gpu_menu {
                let ctx = ctx();
                if !ctx.frame.is_null() {
                    blit(ctx, &mut cv, disp.scale, game_area);
                } else {
                    cv.clear(pal().bg0);
                }
                apply_overlays(&mut cv, &overlays);
            }
            let t = play_t0.elapsed().as_secs_f32();
            if gpu_menu {
                layer.clear_layer();
            }
            {
                let menu_cv = if gpu_menu { &mut layer } else { &mut cv };
                match menu.page {
                    MenuPage::Main => draw_menu(menu_cv, &mut fonts, &menu, rom, t),
                    MenuPage::Overlays => {
                        draw_overlay_menu(menu_cv, &mut fonts, &menu, &ctx().overlay_layers, t)
                    }
                    MenuPage::Netplay => draw_netplay(menu_cv, &mut fonts, &menu, t),
                }
                if menu.hold.is_some() {
                    draw_hold_panel(menu_cv, &mut fonts, &menu, t, menu_cv.w, menu_cv.h);
                }
                // Overlays and screenshots work while paused too.
                draw_overlays(
                    menu_cv,
                    &mut fonts,
                    &mut vol_hud,
                    &mut bar_hud,
                    ff_latch,
                    false,
                    &mut hud,
                );
                draw_shutdown_overlay(
                    menu_cv,
                    &mut fonts,
                    shutdown_t0,
                    shutdown_sent,
                    play_t0.elapsed().as_secs_f32(),
                );
            }
            if take_shot {
                take_shot = false;
                let shot = if gpu_menu {
                    #[cfg(target_os = "linux")]
                    let hw = if hw_render { hw_capture(hw_flip) } else { None };
                    #[cfg(not(target_os = "linux"))]
                    let hw: Option<Canvas> = None;
                    Some(compose_shot(
                        ctx(),
                        disp.scale,
                        game_area,
                        &overlays,
                        &layer,
                        cw,
                        ch,
                        hw,
                    ))
                } else {
                    None
                };
                let msg = match shot {
                    Some(shot) => take_screenshot(&shot),
                    None => take_screenshot(&cv),
                };
                hud = Some((msg, Instant::now()));
            }
            #[cfg(target_os = "linux")]
            if hw_scanout {
                if let Some(ui) = hw_ui.as_mut() {
                    ui.upload(&cv.px);
                    ui.set_visible(true);
                }
                ui_was_active = true;
            } else if gpu_menu {
                let ctx = ctx();
                let (fw, fh) = (
                    ctx.geometry.base_width.max(1),
                    ctx.geometry.base_height.max(1),
                );
                let rect = game_rect(disp.scale, fw as usize, fh as usize, game_area);
                let ok = if hw_render {
                    gpu_present_hw(&mut gpu, &layer, rect, disp.warmth as u8, hw_flip)
                } else {
                    gpu_present(
                        &mut gpu,
                        &layer,
                        ctx.frame,
                        ctx.pitch,
                        fw,
                        fh,
                        ctx.pixfmt,
                        rect,
                        disp.warmth as u8,
                    )
                };
                if !ok {
                    if hw_render {
                        if !hw_present_warned {
                            util::log("gpu: hardware present failed");
                            hw_present_warned = true;
                        }
                    } else {
                        drop_gpu(&mut gpu);
                        util::log("gpu: presenter failed; software path");
                        crate::theme::apply_warmth(&mut cv);
                        presenter.present(&cv)?;
                    }
                }
            } else {
                crate::theme::apply_warmth(&mut cv);
                presenter.present(&cv)?;
            }
            #[cfg(not(target_os = "linux"))]
            {
                crate::theme::apply_warmth(&mut cv);
                presenter.present(&cv)?;
            }
            if let Some((_, t)) = &menu.msg {
                if t.elapsed().as_millis() > 1500 {
                    menu.msg = None;
                }
            }
            let now = Instant::now();
            if now < deadline {
                std::thread::sleep(deadline - now);
            }
            deadline = Instant::now() + fps_dt;
            // A paused game does not count as one long frame in `worst_gap`.
            last_start = Instant::now();
            continue;
        }

        // ---- run a frame (several while FAST FORWARD is on), or step the
        // state back a snapshot while L2 is held (the rewind ring) ----
        let started = Instant::now();
        // Start-to-start is the real frame period: it catches stalls from
        // input, logging and the card as well as the core itself.
        worst_gap = worst_gap.max(started.saturating_duration_since(last_start));
        last_start = started;
        let mut rew_dt = Duration::ZERO;
        let ff = !ctx().netplay_linked && (ff_latch || ff_key);
        FASTFORWARD.store(ff, Ordering::Relaxed);
        let rewinding = !ctx().netplay_linked
            && rewind_ok
            && !ff
            && if game_l2_r2 {
                select_down && l2_down
            } else {
                l2_down && !select_down
            };
        if rewinding {
            if let Some(snap) = rewind_ring.pop_back() {
                if unsafe { (ctx().unser)(snap.as_ptr() as *const c_void, snap.len()) } {
                    unsafe { core_run(retro_run) }; // re-render the restored frame
                    ctx().audio.lock().unwrap().clear(); // no scrub noise
                } else {
                    util::log("rewind: state rejected; buffer cleared");
                    rewind_ring.clear();
                    rewind_bytes = 0;
                    rewind_ok = false;
                }
            }
        } else {
            unsafe { core_run(retro_run) };
            if ff {
                // The extra frames run headless: no video (the last frame is
                // the one presented); audio keeps playing only if FF SOUND is on.
                for _ in 1..ff_steps {
                    if !ff_sound {
                        ctx().audio.lock().unwrap().clear();
                    }
                    unsafe { core_run(retro_run) };
                }
                if !ff_sound {
                    ctx().audio.lock().unwrap().clear();
                }
            }
            if rewind_ok && !ff && core_frames >= rewind_next {
                let t = Instant::now();
                push_rewind(&mut rewind_ring, &mut rewind_bytes);
                rew_dt = t.elapsed();
                t_rew += rew_dt;
                // Serializing a big core state every few frames is the
                // frame-time hog on GBA-class cores: budget the ring at
                // ~3% of a 60 fps frame instead of a fixed cadence.
                let cost = rew_dt.as_secs_f64() * 1000.0;
                rewind_cost_ms = rewind_cost_ms * 0.8 + cost * 0.2;
                let every = ((rewind_cost_ms / 0.5).ceil() as u64).clamp(REWIND_EVERY, 90);
                rewind_next = core_frames + every;
            }
        }

        // With a frame up, restore the hold-MENU auto state if one was
        // left for us; it is consumed on a successful read. A held when the
        // game starts means "start fresh", but the press that launched the
        // game is usually still down - wait for the release before deciding.
        if auto_pending {
            auto_pending = false;
            let path = auto_state_path(&save_dir, &stem);
            if path.exists() {
                let held = {
                    #[cfg(target_os = "linux")]
                    {
                        evdev.key_down(ev::BTN_SOUTH)
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        false
                    }
                } || a_down
                    || pads.button_down(PadButton::A);
                if held {
                    util::log("auto state: A held; waiting for release");
                    auto_hold = Some((path, Instant::now() + AUTO_HOLD));
                } else {
                    restore_auto_state(&path, &mut hud);
                }
            }
        }
        if let Some((path, deadline)) = auto_hold.clone() {
            let held = {
                #[cfg(target_os = "linux")]
                {
                    evdev.key_down(ev::BTN_SOUTH)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    false
                }
            } || a_down
                || pads.button_down(PadButton::A);
            if !held {
                restore_auto_state(&path, &mut hud);
                auto_hold = None;
            } else if Instant::now() >= deadline {
                util::log("auto state skipped: A held at launch (kept)");
                auto_hold = None;
            }
        }

        osd.frame();
        if disp.sysinfo {
            osd.sys();
        }

        let ctx = ctx();
        #[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
        let ui_active = vol_hud.is_some()
            || bar_hud.is_some()
            || hud.is_some()
            || take_shot
            || ff
            || rewinding
            || shutdown_t0.is_some()
            || shutdown_sent;
        // The GPU presenter owns the panel while it is up: the overlays, the
        // OSD and the menu are composited on the GPU, so the framebuffer
        // pages never change hands (the software presenter and the EGL
        // surface each flip a page, and mixing them leaves a stale frame on
        // screen).
        #[cfg(target_os = "linux")]
        let gpu_frame = gpu.is_some() && !hdmi_at_launch && (hw_render || !ctx.frame.is_null());
        #[cfg(not(target_os = "linux"))]
        let gpu_frame = false;

        let t = Instant::now();
        if hw_scanout {
            // Push the new frame into the scanout buffer and flip: no blit,
            // no per-frame fb copy while the UI is quiet.
            #[cfg(target_os = "linux")]
            if let Some(g) = hw_game.as_mut() {
                if ctx.frame_seq != last_seq && !ctx.frame.is_null() {
                    last_seq = ctx.frame_seq;
                    let (iw, ih) = (
                        ctx.geometry.base_width.max(1),
                        ctx.geometry.base_height.max(1),
                    );
                    if g.upload(ctx.frame, ctx.pitch, iw, ih) {
                        let (x, y, w, h) =
                            game_rect(disp.scale, iw as usize, ih as usize, game_area);
                        g.set_window(x, y, w, h);
                        g.flip();
                    }
                }
            }
        } else if !gpu_frame && !ctx.frame.is_null() {
            blit(ctx, &mut cv, disp.scale, game_area);
        }
        t_blit += t.elapsed();

        let t = Instant::now();
        if !hw_scanout && !gpu_frame {
            apply_overlays(&mut cv, &overlays);
        }
        t_ovl += t.elapsed();

        let t = Instant::now();
        if hw_scanout {
            #[cfg(target_os = "linux")]
            {
                if ui_active {
                    // Compose in software so translucent HUD panels blend
                    // over the game (the canvas is then opaque and safe to
                    // push as the UI layer).
                    if !ctx.frame.is_null() {
                        blit(ctx, &mut cv, disp.scale, game_area);
                    }
                    blend_straight(&mut cv, overlay_art.as_deref());
                    draw_overlays(
                        &mut cv,
                        &mut fonts,
                        &mut vol_hud,
                        &mut bar_hud,
                        ff,
                        rewinding,
                        &mut hud,
                    );
                    draw_shutdown_overlay(
                        &mut cv,
                        &mut fonts,
                        shutdown_t0,
                        shutdown_sent,
                        play_t0.elapsed().as_secs_f32(),
                    );
                    if let Some(ui) = hw_ui.as_mut() {
                        ui.upload(&cv.px);
                        ui.set_visible(true);
                    }
                    ui_was_active = true;
                } else if ui_was_active {
                    if let Some(ui) = hw_ui.as_mut() {
                        ui.set_visible(false);
                    }
                    ui_was_active = false;
                }
            }
        } else if gpu_frame {
            // The overlays ride on the GPU: they are drawn into a layer with
            // a real alpha and the GPU blends them over the game.
            layer.clear_layer();
            draw_overlays(
                &mut layer,
                &mut fonts,
                &mut vol_hud,
                &mut bar_hud,
                ff,
                rewinding,
                &mut hud,
            );
            if shutdown_t0.is_none() && (disp.fps_counter || disp.sysinfo) {
                let (fw, fh) = (
                    ctx.geometry.base_width.max(1) as usize,
                    ctx.geometry.base_height.max(1) as usize,
                );
                let (gx, gy, _, _) = game_rect(disp.scale, fw, fh, game_area);
                draw_osd(
                    &mut layer,
                    &mut fonts,
                    &osd,
                    disp.fps_counter,
                    disp.sysinfo,
                    gx + 8,
                    gy + 8,
                );
            }
            draw_shutdown_overlay(
                &mut layer,
                &mut fonts,
                shutdown_t0,
                shutdown_sent,
                play_t0.elapsed().as_secs_f32(),
            );
            if take_shot {
                take_shot = false;
                #[cfg(target_os = "linux")]
                let hw = if hw_render { hw_capture(hw_flip) } else { None };
                #[cfg(not(target_os = "linux"))]
                let hw: Option<Canvas> = None;
                hud = Some((
                    take_screenshot(&compose_shot(
                        ctx, disp.scale, game_area, &overlays, &layer, cw, ch, hw,
                    )),
                    Instant::now(),
                ));
            }
        } else {
            if take_shot {
                take_shot = false;
                hud = Some((take_screenshot(&cv), Instant::now()));
            }
            draw_overlays(
                &mut cv,
                &mut fonts,
                &mut vol_hud,
                &mut bar_hud,
                ff,
                rewinding,
                &mut hud,
            );
            if shutdown_t0.is_none() && (disp.fps_counter || disp.sysinfo) {
                let (fw, fh) = (
                    ctx.geometry.base_width.max(1) as usize,
                    ctx.geometry.base_height.max(1) as usize,
                );
                let (gx, gy, _, _) = game_rect(disp.scale, fw, fh, game_area);
                draw_osd(
                    &mut cv,
                    &mut fonts,
                    &osd,
                    disp.fps_counter,
                    disp.sysinfo,
                    gx + 8,
                    gy + 8,
                );
            }
            draw_shutdown_overlay(
                &mut cv,
                &mut fonts,
                shutdown_t0,
                shutdown_sent,
                play_t0.elapsed().as_secs_f32(),
            );
            crate::theme::apply_warmth(&mut cv);
        }
        t_hud += t.elapsed();
        let t = Instant::now();
        #[cfg(target_os = "linux")]
        {
            let mut samples = std::mem::take(&mut *ctx.audio.lock().unwrap());
            apply_gain(&mut samples, gain_pct);
            resampler.process(&samples, &mut audio_out);
            let level = audio.push(&audio_out);
            min_level = min_level.min(level);
            // Keep the ring near half full with a bounded pitch nudge.
            // ±3% covers cores running at ~58–60 fps; ±0.5% could not follow
            // a PS1 core at 59.5 fps and the ring drained every ~15 s.
            let err = f64::from(((level - 0.5) * 2.0).clamp(-1.0, 1.0));
            resampler.set_ratio(1.0 - 0.03 * err);
        }
        #[cfg(not(target_os = "linux"))]
        ctx.audio.lock().unwrap().clear();
        t_aud += t.elapsed();

        let t = Instant::now();
        if hw_scanout {
            #[cfg(target_os = "linux")]
            {
                // Pan first (vsync when it still blocks), then re-hide in
                // case a pan restored the driver's layer.
                presenter.tick();
                if !crate::disp2::fb_layer_hide() && !fb_hide_warned {
                    util::log("scanout: fb layer hide lost");
                    fb_hide_warned = true;
                }
            }
        } else if gpu_frame {
            #[cfg(target_os = "linux")]
            {
                let (fw, fh) = (
                    ctx.geometry.base_width.max(1),
                    ctx.geometry.base_height.max(1),
                );
                let rect = game_rect(disp.scale, fw as usize, fh as usize, game_area);
                let ok = if hw_render {
                    gpu_present_hw(&mut gpu, &layer, rect, disp.warmth as u8, hw_flip)
                } else {
                    gpu_present(
                        &mut gpu,
                        &layer,
                        ctx.frame,
                        ctx.pitch,
                        fw,
                        fh,
                        ctx.pixfmt,
                        rect,
                        disp.warmth as u8,
                    )
                };
                if !ok {
                    if hw_render {
                        if !hw_present_warned {
                            util::log("gpu: hardware present failed");
                            hw_present_warned = true;
                        }
                    } else {
                        drop_gpu(&mut gpu);
                        util::log("gpu: presenter failed; software path");
                        if !ctx.frame.is_null() {
                            blit(ctx, &mut cv, disp.scale, game_area);
                        }
                        apply_overlays(&mut cv, &overlays);
                        blend_layer(&mut cv, &layer);
                        crate::theme::apply_warmth(&mut cv);
                        presenter.present(&cv)?;
                    }
                }
            }
            #[cfg(not(target_os = "linux"))]
            presenter.present(&cv)?;
        } else {
            presenter.present(&cv)?;
        }
        let pres = t.elapsed();
        t_pres += pres;
        pres_ms = pres_ms * 0.9 + pres.as_secs_f64() * 1000.0;
        let work = started.elapsed();
        if hw_render
            && work > Duration::from_secs(1)
            && last_slow_log.elapsed() >= Duration::from_secs(5)
        {
            last_slow_log = Instant::now();
            util::log(&format!(
                "hw: slow frame {:.2} s (present {:.2} s, {}x{})",
                work.as_secs_f64(),
                pres.as_secs_f64(),
                ctx.geometry.base_width,
                ctx.geometry.base_height
            ));
        }
        t_core += work - rew_dt;
        frames += 1;
        core_frames += 1;
        work_total += work;
        worst_work = worst_work.max(work);
        if work > fps_dt + Duration::from_millis(2) {
            late_frames += 1;
        }

        // Write the adjusted volume and display levels back a moment after
        // they change: a reboot must not lose them.
        if let Some(t) = prefs_save_at {
            if Instant::now() >= t {
                prefs_save_at = None;
                save_runtime_volume(gain_pct);
                sync_display_menu(&mut disp, &menu);
                save_display_prefs(system_id, &disp);
            }
        }

        // Frame-clock pacing. When the present really blocks on vsync
        // (~one frame period), sleeping to a free-running deadline
        // periodically lands just past a vsync and drops a whole frame (the
        // GBA/GBC stutter). A present that merely waits for the GPU is much
        // shorter; treating that as vsync lets the loop run past 60 Hz and
        // makes threaded cores run fast. PAL rates still sleep.
        let now = Instant::now();
        let frame_ms = fps_dt.as_secs_f64() * 1000.0;
        let vsync_paced = fps_dt < Duration::from_micros(17_240)
            && pres_ms >= frame_ms * 0.95
            && pres.as_secs_f64() * 1000.0 >= frame_ms * 0.95;
        if vsync_paced {
            deadline = now + fps_dt;
        } else {
            let (next, sleep) = pace(now, deadline, fps_dt);
            if let Some(d) = sleep {
                std::thread::sleep(d);
            }
            deadline = next;
        }

        if last_report.elapsed() >= Duration::from_secs(5) {
            let secs = last_report.elapsed().as_secs_f64();
            let audio_frames = ctx.audio_samples.swap(0, Ordering::Relaxed);
            let audio_rate = f64::from(ctx.audio_rate.load(Ordering::Relaxed).max(1));
            let emu = audio_frames as f64 / audio_rate / secs.max(0.001);
            let paced_ms = ctx.audio_sleep_us.swap(0, Ordering::Relaxed) as f64 / 1000.0;
            let f = frames.max(1) as f64;
            let ms = |d: Duration| d.as_secs_f64() * 1000.0 / f;
            util::log(&format!(
                "fps {:.1}, frame {:.1} ms (worst {:.1}, wframe {:.1}) | core {:.1} blit {:.1} ovl {:.1} hud {:.1} aud {:.1} pres {:.1} rew {:.1} ring {:.2} late {} frames {} secs {:.2} emu {:.2}x paced {:.0}ms ff {}",
                frames as f64 / secs,
                work_total.as_secs_f64() * 1000.0 / f,
                worst_gap.as_secs_f64() * 1000.0,
                worst_work.as_secs_f64() * 1000.0,
                ms(t_core),
                ms(t_blit),
                ms(t_ovl),
                ms(t_hud),
                ms(t_aud),
                ms(t_pres),
                ms(t_rew),
                min_level,
                late_frames,
                frames,
                secs,
                emu,
                paced_ms,
                ff
            ));
            frames = 0;
            work_total = Duration::ZERO;
            worst_gap = Duration::ZERO;
            worst_work = Duration::ZERO;
            late_frames = 0;
            min_level = 1.0;
            t_core = Duration::ZERO;
            t_blit = Duration::ZERO;
            t_ovl = Duration::ZERO;
            t_hud = Duration::ZERO;
            t_aud = Duration::ZERO;
            t_pres = Duration::ZERO;
            t_rew = Duration::ZERO;
            last_report = Instant::now();
        }

        // ---- periodic SRAM flush ----
        if sram_size > 0
            && !sram_ptr.is_null()
            && last_sram_flush.elapsed() > Duration::from_secs(30)
        {
            save_sram(&srm_path, sram_ptr, sram_size);
            last_sram_flush = Instant::now();
        }
    }

    netplay_stop();

    if sram_size > 0 && !sram_ptr.is_null() {
        save_sram(&srm_path, sram_ptr, sram_size);
    }
    // Free the display-engine layers before the core unloads.
    #[cfg(target_os = "linux")]
    {
        drop(hw_ui.take());
        drop(hw_overlay.take());
        drop(hw_game.take());
    }
    sync_display_menu(&mut disp, &menu);
    save_display_prefs(system_id, &disp);
    ctx().set_rumble(0);
    save_runtime_volume(gain_pct);
    // Hand the mixer back to the user's level: the ports and the shelf play
    // through ALSA directly.
    #[cfg(target_os = "linux")]
    util::set_mixer_volume(gain_pct);
    // Tear the core's GL resources down while the EGL context is current and
    // the core is still loaded: the core's context_destroy needs both.
    #[cfg(target_os = "linux")]
    {
        let destroy = ctx().hw_render.as_ref().and_then(|hw| hw.destroy);
        if let Some(destroy) = destroy {
            unsafe { destroy() };
            util::log("hw: context destroyed");
        }
        HW_FBO.store(0, Ordering::Relaxed);
    }
    save_core_options(ctx());
    unsafe {
        unload_game();
        retro_deinit();
    }
    #[cfg(target_os = "linux")]
    {
        HW_GPU.store(std::ptr::null_mut(), Ordering::Relaxed);
        ctx().hw_render = None;
        drop(gpu.take());
    }
    util::log("game loop down");
    Ok(())
}

/// Scale a stereo batch by the software volume (quadratic: 50% is a quarter
/// of full).
#[cfg(target_os = "linux")]
pub fn apply_gain(samples: &mut [i16], pct: u8) {
    match pct.min(100) {
        100 => {}
        0 => samples.fill(0),
        p => {
            let g = p as f32 / 100.0;
            let g = g * g;
            for s in samples.iter_mut() {
                *s = (*s as f32 * g) as i16;
            }
        }
    }
}

/// Save-state slots per game.
const STATE_SLOTS: usize = 3;

/// Holding MENU this long saves the state and leaves the game; the
/// progress bar only appears past the hint delay, so a plain tap never
/// shows it.
const MENU_HOLD_EXIT: Duration = Duration::from_secs(2);
const MENU_HOLD_HINT: Duration = Duration::from_millis(800);
const MENU_ALIAS: Duration = Duration::from_millis(150);

/// Holding POWER this long starts the shutdown countdown (two and a half
/// seconds, abortable by releasing); a shorter press sleeps.
const POWER_HOLD: Duration = Duration::from_millis(800);
const SHUTDOWN_TOTAL: Duration = Duration::from_millis(2500);

/// The rewind ring: one state every this many frames, memory-bounded.
const REWIND_EVERY: u64 = 6;
const REWIND_MAX_BYTES: usize = 48 << 20;
/// Cores with states this large cannot snapshot often enough to rewind
/// without stalling the frame loop.
const REWIND_MAX_STATE: usize = 8 << 20;

/// Only these core options are surfaced in the UI — matched loosely against
/// the label (cores append things like "(Restart Required)"); everything else
/// stays at the core's default (the owner's list for the handheld cores).
const KEEP_OPTIONS: [&str; 39] = [
    // Game Boy / Color (Gambatte)
    "color correction",
    "color correction mode",
    "dark filter level (%)",
    "interframe blending",
    "emulated hardware",
    "rumble strength",
    "gb colorization",
    "internal palette",
    "region",
    // GBA (mGBA)
    "game boy player rumble",
    "solar sensor level",
    "use bios",
    "skip bios",
    // SNES (Snes9x 2005)
    "reduce slowdown",
    "reduce flickering",
    // NES (FCEUmm)
    "color palette",
    "ntsc filter",
    "crop overscan",
    "overclock",
    "master volume",
    // Genesis (Genesis Plus GX)
    "system hardware",
    "borders",
    "lcd ghosting filter",
    "sound output",
    "low-pass filter",
    "preamp level",
    "cd-da volume",
    "pcm volume",
    "cpu speed",
    // PS1 (PCSX-ReARMed)
    "bios selection",
    "show bios",
    "dithering pattern",
    "enhanced resolution",
    "sound interpolation",
    "reverb",
    // Generic labels used by several cores
    "frameskip",
    "rumble effects",
    // CD systems
    "cd bios",
    "bios select",
];

fn visible_options(ctx: &Ctx) -> Vec<usize> {
    ctx.options
        .iter()
        .enumerate()
        .filter(|(_, o)| o.visible)
        .map(|(i, _)| i)
        .collect()
}

fn option_kept(desc: &str) -> bool {
    let d = desc.to_ascii_lowercase();
    KEEP_OPTIONS.iter().any(|k| d.contains(k))
}

/// mGBA runs only GBA here, so its Game Boy/Color options are hidden.
fn mgba_option_hidden(desc: &str) -> bool {
    let d = desc.to_ascii_lowercase();
    d.contains("game boy model")
        || d.contains("game boy palette")
        || d.contains("super game boy border")
}

/// Genesis Plus GX options that are too detailed for the handheld menu.
fn genesis_option_hidden(desc: &str) -> bool {
    let d = desc.to_ascii_lowercase();
    d.contains("mega drive/genesis fm")
        || d.contains("preamp level")
        || d.contains("cd-da volume")
        || d.contains("pcm volume")
}

/// PokeMini's curated options; the audio filter, turbo period and 60Hz mode
/// stay at the core defaults.
fn pokemini_option_kept(desc: &str) -> bool {
    let d = desc.to_ascii_lowercase();
    [
        "video scale",
        "lcd filter",
        "lcd mode",
        "lcd contrast",
        "lcd brightness",
        "palette",
        "piezo filter",
        "screen shake level",
        "controller rumble level",
    ]
    .iter()
    .any(|k| d.contains(k))
}

/// Flycast's curated options. VMU, light gun, NAOMI, HDR, orientation and
/// texture-upscaling rows stay hidden. The two analog deadzones are added
/// after a stick is detected, since the option list is registered first.
fn flycast_option_kept(desc: &str) -> bool {
    let d = desc.to_ascii_lowercase();
    [
        "region",
        "language",
        "use real bios",
        "hle bios",
        "threaded rendering",
        "force windows ce mode",
        "cable type",
        "alpha sorting",
        "frame skipping",
        "digital triggers",
        "purupuru",
    ]
    .iter()
    .any(|k| d.contains(k))
}

/// The core registers its options before input devices are opened. If a real
/// stick is attached, put the two deadzone rows back into the CORE menu.
fn reveal_analog_options() {
    let ctx = ctx();
    if ctx.opt_key != "flycast" {
        return;
    }
    let mut shown = false;
    for o in &mut ctx.options {
        if o.key == "reicast_analog_stick_deadzone" || o.key == "reicast_trigger_deadzone" {
            o.visible = true;
            shown = true;
        }
    }
    if shown {
        util::log("core options: analog stick rows shown");
    }
}

/// Flycast VL's serializer is only safe when its emulation thread is not
/// running. With threaded rendering on, MAGI refuses states instead of
/// crashing the core.
fn flycast_states_blocked(core: &str, threaded: bool) -> bool {
    core == "flycast" && threaded
}

/// True when this press completes a double tap (and then starts fresh).
fn double_tap(last: &mut Option<Instant>, now: Instant, window: Duration) -> bool {
    match *last {
        Some(t) if now.duration_since(t) <= window => {
            *last = None;
            true
        }
        _ => {
            *last = Some(now);
            false
        }
    }
}

/// True when the game owns L2/R2 and the frontend's rewind/FF live under
/// SELECT instead. Dreamcast triggers are moved to L1/R1, so only PS1 keeps
/// L2/R2.
fn needs_l2_r2(system_id: &str) -> bool {
    system_id == "psx"
}

/// The in-game menu owns the live shader and OSD choices. Bring the saved
/// display prefs up to date before any write, including an exit that happens
/// before the next overlay pass.
fn sync_display_menu(disp: &mut DisplayPrefs, menu: &GameMenu) {
    disp.shader = menu.shader;
    disp.fps_counter = menu.fps;
    disp.sysinfo = menu.sysinfo;
}

/// L2/R2 are rewind and fast-forward unless the core owns those buttons.
/// Keep them out of the core's joypad mask there: mGBA maps them to Turbo
/// L/R, so pressing R2 to fast-forward would also pull the GBA's R button.
/// Dreamcast reports its triggers on L2/R2, so move the handheld's L1/R1
/// bits there and leave L2/R2 for the frontend. SWAP A/B exchanges the two
/// buttons the emulated console sees; the frontend's own A/B confirmation
/// stays as it is.
fn core_pad_bits(
    bits: u16,
    game_l2_r2: bool,
    swap_ab: bool,
    triggers_l1r1: bool,
    swap_l1r1: bool,
) -> u16 {
    let mut bits = if triggers_l1r1 {
        // Flycast files the left trigger under R2 and the right trigger under
        // L2. Keep that normal mapping when the swap option is off, and
        // exchange the two only when it is on.
        let physical_l = bits & (1 << retro::L) != 0;
        let physical_r = bits & (1 << retro::R) != 0;
        let (l2, r2) = if swap_l1r1 {
            (physical_l, physical_r)
        } else {
            (physical_r, physical_l)
        };
        let mut out =
            bits & !((1 << retro::L) | (1 << retro::R) | (1 << retro::L2) | (1 << retro::R2));
        if l2 {
            out |= 1 << retro::L2;
        }
        if r2 {
            out |= 1 << retro::R2;
        }
        out
    } else {
        let mut out = if game_l2_r2 {
            bits
        } else {
            bits & !((1 << retro::L2) | (1 << retro::R2))
        };
        if swap_l1r1 {
            let l = out & (1 << retro::L) != 0;
            let r = out & (1 << retro::R) != 0;
            if l != r {
                out ^= (1 << retro::L) | (1 << retro::R);
            }
        }
        out
    };
    if swap_ab {
        let a = bits & (1 << retro::A) != 0;
        let b = bits & (1 << retro::B) != 0;
        if a != b {
            bits ^= (1 << retro::A) | (1 << retro::B);
        }
    }
    bits
}

/// Cores that still need the performance governor after GPU presentation.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
/// PS1 and Dreamcast keep the performance governor; Genesis and PCE run on
/// schedutil and ramp only when they need to.
fn needs_performance(system_id: &str) -> bool {
    matches!(system_id, "psx" | "dc")
}

/// Rewind is off for the heavy cores and for any state too large to snapshot
/// without stalling the frame loop.
fn rewind_allowed(system_id: &str, ser_size: usize) -> bool {
    !matches!(system_id, "psx" | "dc") && ser_size > 0 && ser_size <= REWIND_MAX_STATE
}

/// Live on-screen counters: frames per second and a once-a-second system
/// snapshot (CPU load, hottest thermal zone, memory, GPU clock).
struct Osd {
    frames: u32,
    t0: Instant,
    fps: f32,
    next_sys: Instant,
    cpu_prev: Option<(u64, u64)>,
    cpu_pct: f32,
    temp_c: f32,
    mem_used: u64,
    mem_total: u64,
    gpu_mhz: u32,
}

impl Osd {
    fn new() -> Self {
        Self {
            frames: 0,
            t0: Instant::now(),
            fps: 0.0,
            next_sys: Instant::now(),
            cpu_prev: None,
            cpu_pct: 0.0,
            temp_c: 0.0,
            mem_used: 0,
            mem_total: 0,
            gpu_mhz: 0,
        }
    }

    fn frame(&mut self) {
        self.frames += 1;
        let elapsed = self.t0.elapsed();
        if elapsed >= Duration::from_millis(500) {
            self.fps = self.frames as f32 / elapsed.as_secs_f32();
            self.frames = 0;
            self.t0 = Instant::now();
        }
    }

    fn sys(&mut self) {
        let now = Instant::now();
        if now < self.next_sys {
            return;
        }
        self.next_sys = now + Duration::from_secs(1);
        if let Ok(stat) = std::fs::read_to_string("/proc/stat") {
            if let Some(line) = stat.lines().next() {
                let nums: Vec<u64> = line
                    .split_whitespace()
                    .skip(1)
                    .filter_map(|x| x.parse().ok())
                    .collect();
                if nums.len() >= 4 {
                    let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
                    let total: u64 = nums.iter().sum();
                    if let Some((pi, pt)) = self.cpu_prev {
                        let dt = total.saturating_sub(pt);
                        if dt > 0 {
                            let di = idle.saturating_sub(pi);
                            self.cpu_pct = 100.0 * (1.0 - di as f32 / dt as f32);
                        }
                    }
                    self.cpu_prev = Some((idle, total));
                }
            }
        }
        let mut hottest = 0.0f32;
        for zone in 0..8 {
            let path = format!("/sys/class/thermal/thermal_zone{zone}/temp");
            if let Ok(raw) = std::fs::read_to_string(&path) {
                if let Ok(v) = raw.trim().parse::<i32>() {
                    hottest = hottest.max(v as f32 / 1000.0);
                }
            }
        }
        self.temp_c = hottest;
        if let Ok(mem) = std::fs::read_to_string("/proc/meminfo") {
            let kb = |key: &str| {
                mem.lines()
                    .find(|l| l.starts_with(key))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0)
            };
            self.mem_total = kb("MemTotal:");
            self.mem_used = self.mem_total.saturating_sub(kb("MemAvailable:"));
        }
        if let Ok(raw) = std::fs::read_to_string("/sys/class/devfreq/gpu/cur_freq") {
            if let Ok(v) = raw.trim().parse::<u64>() {
                self.gpu_mhz = (v / 1_000_000) as u32;
            }
        }
    }
}

fn draw_osd(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    osd: &Osd,
    fps_on: bool,
    sys_on: bool,
    x: i32,
    y: i32,
) {
    use crate::theme::pal;
    let mut lines: Vec<String> = Vec::new();
    if fps_on {
        lines.push(format!("{:.1} FPS", osd.fps));
    }
    if sys_on {
        lines.push(format!(
            "CPU {:.0}%  {:.0}C  MEM {}/{}M  GPU {}MHz",
            osd.cpu_pct,
            osd.temp_c,
            osd.mem_used / 1024,
            osd.mem_total / 1024,
            osd.gpu_mhz
        ));
    }
    if lines.is_empty() {
        return;
    }
    let size = 10.0;
    let pad = 6;
    let line_h = 12;
    let text_w = lines
        .iter()
        .map(|l| fonts.text_width(l, size))
        .max()
        .unwrap_or(0);
    cv.fill_rect_a(
        x,
        y,
        (text_w + pad * 2) as u32,
        (line_h * lines.len() as i32 + pad * 2) as u32,
        pal().bg0,
        170,
    );
    for (i, line) in lines.iter().enumerate() {
        fonts.draw(
            cv,
            x + pad,
            y + pad + line_h * (i as i32 + 1) - 2,
            line,
            size,
            pal().signal,
        );
    }
}

/// Save-state files for the slots; slot 1 keeps the historic name.
fn state_slot_path(save_dir: &str, stem: &str, slot: usize) -> std::path::PathBuf {
    let name = match slot {
        1 => format!("{stem}.state2"),
        2 => format!("{stem}.state3"),
        _ => format!("{stem}.state"),
    };
    Path::new(save_dir).join(name)
}

/// The hold-MENU state: a one-shot file the next launch restores.
fn auto_state_path(save_dir: &str, stem: &str) -> std::path::PathBuf {
    Path::new(save_dir).join(format!("{stem}.stateauto"))
}

/// A state's preview, stored next to it as `<state>.png`.
fn state_thumb_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".png");
    std::path::PathBuf::from(name)
}

/// The core version that wrote a state. Cores serialize differently: loading
/// another core's state can crash the core, and does with the 2021 Flycast.
fn state_meta_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".core");
    std::path::PathBuf::from(name)
}

/// Capture the current game frame as a small PNG beside a state.
fn save_state_thumb(path: &Path) {
    let ctx = ctx();
    #[cfg(target_os = "linux")]
    let hw = if ctx.frame.is_null() {
        hw_capture(ctx.hw_render.as_ref().is_some_and(|h| h.bottom_left))
    } else {
        None
    };
    #[cfg(not(target_os = "linux"))]
    let hw: Option<Canvas> = None;
    if ctx.frame.is_null() && hw.is_none() {
        util::log("state thumb: no frame available");
        return;
    }
    let mut cv = Canvas::new(240, 180);
    if let Some(frame) = hw.as_ref() {
        blit_px(
            &frame.px,
            frame.w as usize,
            frame.h as usize,
            &mut cv,
            ScaleMode::Fit,
            (0, 0, 240, 180),
        );
    } else if ctx.pitch > 0 && ctx.geometry.base_width > 0 && ctx.geometry.base_height > 0 {
        blit(ctx, &mut cv, ScaleMode::Fit, (0, 0, 240, 180));
    } else {
        util::log("state thumb: no frame available");
        return;
    }
    for p in &mut cv.px {
        *p |= 0xFF00_0000;
    }
    let bytes = match crate::image::encode_png(cv.w as u32, cv.h as u32, &cv.px) {
        Ok(bytes) => bytes,
        Err(e) => {
            util::log(&format!("state thumb: cannot encode: {e}"));
            return;
        }
    };
    let thumb = state_thumb_path(path);
    match util::write_file_atomic(&thumb, &bytes) {
        Ok(()) => util::log(&format!(
            "state thumb saved: {} ({}x{})",
            thumb.display(),
            cv.w,
            cv.h
        )),
        Err(e) => util::log(&format!(
            "state thumb: cannot write {}: {e}",
            thumb.display()
        )),
    }
}

/// Keep holding A this long after the first frame to skip the resume point;
/// a normal launch tap has been released by then.
const AUTO_HOLD: Duration = Duration::from_millis(600);

fn restore_auto_state(path: &Path, hud: &mut Option<(String, Instant)>) {
    match load_state_from(path) {
        Ok(true) => {
            util::log(&format!("auto state restored: {}", path.display()));
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(state_thumb_path(path));
            *hud = Some(("STATE RESTORED".into(), Instant::now()));
        }
        Ok(false) => util::log("auto state rejected by core"),
        Err(e) => util::log(&format!("auto state load failed: {e}")),
    }
}

/// Prefix for LZ4-compressed state files; states stored raw lack it.
const STATE_MAGIC: &[u8; 8] = b"MAGIS1\0\0";

fn pack_state(buf: Vec<u8>) -> (Vec<u8>, bool) {
    let packed = lz4_flex::compress_prepend_size(&buf);
    if STATE_MAGIC.len() + packed.len() < buf.len() {
        let mut out = Vec::with_capacity(STATE_MAGIC.len() + packed.len());
        out.extend_from_slice(STATE_MAGIC);
        out.extend_from_slice(&packed);
        (out, true)
    } else {
        (buf, false)
    }
}

fn unpack_state(bytes: Vec<u8>) -> std::io::Result<Vec<u8>> {
    if bytes.starts_with(STATE_MAGIC) {
        lz4_flex::decompress_size_prepended(&bytes[STATE_MAGIC.len()..])
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    } else {
        Ok(bytes)
    }
}

/// Serialize into a file; `None` means the core cannot save at all.
fn save_state_to(path: &Path) -> Option<bool> {
    if ctx().states_blocked {
        return None;
    }
    let size = ctx().state_size;
    if size == 0 {
        return None;
    }
    let t0 = Instant::now();
    let mut buf = vec![0u8; size];
    util::log(&format!("state save: begin {} bytes", size));
    let ok = unsafe { (ctx().ser)(buf.as_mut_ptr() as *mut c_void, size) };
    let ser_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if !ok {
        util::log("state save: core refused to serialize");
        return Some(false);
    }
    let (data, compressed) = pack_state(buf);
    let pack_ms = t0.elapsed().as_secs_f64() * 1000.0 - ser_ms;
    let stored = data.len();
    let t1 = Instant::now();
    match util::write_file_atomic(path, &data) {
        Ok(()) => {
            let write_ms = t1.elapsed().as_secs_f64() * 1000.0;
            let meta = state_meta_path(path);
            if let Err(e) = util::write_file_atomic(&meta, ctx().core_version.as_bytes()) {
                util::log(&format!("state version tag: {}: {e}", meta.display()));
            }
            save_state_thumb(path);
            util::log(&format!(
                "state saved: {} ({} -> {} bytes{}, serialize {:.1} ms, pack {:.1} ms, write {:.1} ms)",
                path.display(),
                size,
                stored,
                if compressed { ", lz4" } else { "" },
                ser_ms,
                pack_ms,
                write_ms
            ));
            Some(true)
        }
        Err(e) => {
            util::log(&format!("state save failed: {}: {e}", path.display()));
            Some(false)
        }
    }
}

fn load_state_from(path: &Path) -> std::io::Result<bool> {
    if ctx().states_blocked {
        return Ok(false);
    }
    let meta = state_meta_path(path);
    match std::fs::read_to_string(&meta) {
        Ok(v) if v.trim() == ctx().core_version.as_str() => {}
        Ok(v) => {
            util::log(&format!(
                "state: {} written by core {}; this is {}",
                path.display(),
                v.trim(),
                ctx().core_version
            ));
            return Ok(false);
        }
        Err(_) => {
            util::log(&format!(
                "state: {} has no core tag; not loading",
                path.display()
            ));
            return Ok(false);
        }
    }
    let buf = unpack_state(std::fs::read(path)?)?;
    Ok(unsafe { (ctx().unser)(buf.as_ptr() as *const c_void, buf.len()) })
}

/// Push a fresh snapshot into the rewind ring, dropping the oldest states
/// once the memory budget is exceeded.
fn push_rewind(ring: &mut VecDeque<Vec<u8>>, bytes: &mut usize) {
    let size = ctx().state_size;
    if size == 0 {
        return;
    }
    let mut buf = vec![0u8; size];
    if !unsafe { (ctx().ser)(buf.as_mut_ptr() as *mut c_void, size) } {
        return;
    }
    *bytes += buf.len();
    ring.push_back(buf);
    while *bytes > REWIND_MAX_BYTES {
        match ring.pop_front() {
            Some(old) => *bytes -= old.len(),
            None => break,
        }
    }
}

fn save_sram(path: &Path, ptr: *mut u8, size: usize) {
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, size) };
    if util::write_file_atomic(path, bytes).is_err() {
        util::log(&format!("sram: cannot write {}", path.display()));
    }
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        "<unknown>".into()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

#[derive(PartialEq)]
enum MenuPage {
    Main,
    Overlays,
    Netplay,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NetplayPage {
    Menu,
    HostTransport,
    JoinTransport,
    HostWait,
    JoinScan,
    Linked,
    Error,
}

struct StateCard {
    path: std::path::PathBuf,
    label: String,
    age: String,
    thumb: Option<Image>,
    exists: bool,
}

struct GameMenu {
    open: bool,
    /// Selected folder tab.
    sel: usize,
    msg: Option<(String, Instant)>,
    exit_requested: bool,
    page: MenuPage,
    /// State slot shown by SAVE/LOAD and used by quick save.
    slot: usize,
    opt_sel: usize,
    opt_top: usize,
    /// When the highlighted core option last changed, for its marquee.
    opt_since: Instant,
    overlay_sel: usize,
    overlay_idx: [usize; 2],
    /// Game-image shader, persisted with the display prefs.
    shader: ShaderMode,
    /// On-screen counters (persisted as display prefs).
    fps: bool,
    sysinfo: bool,
    /// Progress of the held-MENU save-and-exit gesture, when underway.
    hold: Option<f32>,
    /// The selected image on the DISC tab.
    disc_sel: usize,
    netplay_page: NetplayPage,
    netplay_sel: usize,
    netplay_transport: netplay::Transport,
    netplay_role: netplay::Role,
    netplay_msg: Option<String>,
    /// CORE or VIDEO is focused for option changes.
    focus_pane: bool,
    states: Vec<StateCard>,
    state_paths: Vec<std::path::PathBuf>,
    auto_path: std::path::PathBuf,
    delete_armed: Option<(usize, Instant)>,
}

fn netplay_stop() {
    if let Some(mut session) = ctx().netplay.take() {
        session.stop();
    }
    if ctx().netplay_linked || ctx().opt_key == "gambatte" {
        ctx().netplay_linked = false;
        set_gambatte_link(false);
    }
}

fn set_core_option(ctx: &mut Ctx, key: &str, value: &str) -> bool {
    let Some(option) = ctx.options.iter_mut().find(|o| o.key == key) else {
        return false;
    };
    let Some(index) = option.values.iter().position(|v| v == value) else {
        return false;
    };
    if option.chosen != index {
        option.chosen = index;
        ctx.opt_dirty = true;
    }
    true
}

fn core_option_value(ctx: &Ctx, key: &str) -> Option<String> {
    ctx.options
        .iter()
        .find(|o| o.key == key)
        .map(|o| o.values[o.chosen].clone())
}

fn set_option_visible(ctx: &mut Ctx, key: &str, visible: bool) {
    if let Some(o) = ctx.options.iter_mut().find(|o| o.key == key) {
        o.visible = visible;
    }
}

/// The LCD-accurate GB palette needs the core's unfiltered grey. The user's
/// colourisation comes back when another shader is picked.
fn set_lcd_accurate(on: bool) {
    let ctx = ctx();
    if ctx.opt_key != "gambatte" {
        return;
    }
    if ctx.system == "gb" {
        if on {
            if ctx.gb_color_backup.is_none() {
                ctx.gb_color_backup = core_option_value(ctx, "gambatte_gb_colorization");
            }
            set_core_option(ctx, "gambatte_gb_colorization", "disabled");
        } else if let Some(value) = ctx.gb_color_backup.take() {
            set_core_option(ctx, "gambatte_gb_colorization", &value);
        }
        set_option_visible(ctx, "gambatte_dark_filter_level", !on);
    }
    let hide = on && (ctx.system == "gb" || ctx.system == "gbc");
    set_option_visible(ctx, "gambatte_gb_colorization", !hide);
    set_option_visible(ctx, "gambatte_gb_internal_palette", !hide);
}

/// Point gambatte's netlink client at the local proxy.
fn set_gambatte_link(on: bool) -> bool {
    let ctx = ctx();
    if ctx.opt_key != "gambatte" {
        return false;
    }
    if !on {
        let ok = set_core_option(ctx, "gambatte_gb_link_mode", "Not Connected");
        if ok {
            util::log("netplay: core link off");
        }
        return ok;
    }
    let mode = set_core_option(ctx, "gambatte_gb_link_mode", "Network Client");
    let port = set_core_option(
        ctx,
        "gambatte_gb_link_network_port",
        &netplay::LOCAL_PORT.to_string(),
    );
    let digits = ["1", "2", "7", "0", "0", "0", "0", "0", "0", "0", "0", "1"];
    let mut ip = true;
    for (i, digit) in digits.iter().enumerate() {
        ip &= set_core_option(
            ctx,
            &format!("gambatte_gb_link_network_server_ip_{}", i + 1),
            digit,
        );
    }
    if mode && port && ip {
        util::log(&format!(
            "netplay: core link on 127.0.0.1:{}",
            netplay::LOCAL_PORT
        ));
    }
    mode && port && ip
}

/// Poll the netplay session and apply its state changes.
fn netplay_tick(menu: &mut GameMenu, hud: &mut Option<(String, Instant)>) {
    let mut linked = false;
    let mut lost = false;
    let mut error = None;
    if let Some(session) = ctx().netplay.as_ref() {
        match session.status() {
            netplay::Status::Linked => linked = true,
            netplay::Status::Lost => lost = true,
            netplay::Status::Error(e) => error = Some(e),
            _ => {}
        }
    } else if ctx().netplay_linked {
        lost = true;
    }

    let mut abort = false;
    if linked && !ctx().netplay_linked {
        if set_gambatte_link(true) {
            ctx().netplay_linked = true;
            ctx().opt_dirty = true;
            util::log("netplay: linked");
            menu.open = false;
            menu.page = MenuPage::Main;
            menu.netplay_page = NetplayPage::Linked;
            menu.netplay_msg = None;
            *hud = Some(("NETPLAY LINKED".into(), Instant::now()));
        } else {
            error = Some("CORE LINK UNAVAILABLE".into());
            abort = true;
        }
    }

    if lost {
        if ctx().netplay_linked {
            ctx().netplay_linked = false;
            set_gambatte_link(false);
            ctx().opt_dirty = true;
            *hud = Some(("NETPLAY ENDED".into(), Instant::now()));
        }
        netplay_stop();
        menu.netplay_page = NetplayPage::Error;
        menu.netplay_msg = Some("THE OTHER PLAYER LEFT".into());
    } else if abort {
        netplay_stop();
        menu.netplay_page = NetplayPage::Error;
        menu.netplay_msg = error;
    } else if let Some(e) = error {
        menu.netplay_page = NetplayPage::Error;
        menu.netplay_msg = Some(e);
    }
}

/// The countdown overlay: drawn whenever the POWER hold is running (and once
/// more at zero while the kernel shuts down).
fn draw_shutdown_overlay(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    t0: Option<Instant>,
    sent: bool,
    t: f32,
) {
    if let Some(t0) = t0 {
        let remaining = SHUTDOWN_TOTAL.saturating_sub(t0.elapsed());
        crate::theme::draw_shutdown(cv, fonts, remaining, SHUTDOWN_TOTAL, t);
    } else if sent {
        crate::theme::draw_shutdown(cv, fonts, Duration::ZERO, SHUTDOWN_TOTAL, t);
    }
}
fn apply_adjust(disp: &mut DisplayPrefs, kind: AdjKind, dir: i32) {
    match kind {
        AdjKind::Brightness => {
            disp.brightness = crate::util::clamp_brightness(
                (disp.brightness as i32 + dir * COMBO_STEP).max(0) as u32,
            );
            util::set_brightness(disp.brightness);
        }
        AdjKind::Warmth => {
            disp.warmth = (disp.warmth as i32 + dir * COMBO_STEP).clamp(0, 100) as u32;
            crate::theme::set_warmth(disp.warmth as u8);
        }
    }
}

/// The overlay content for an adjustment: label, fill ratio and bar colour.
fn bar_label(disp: &DisplayPrefs, kind: AdjKind) -> (crate::theme::BarIcon, f32, Instant) {
    match kind {
        AdjKind::Brightness => (
            crate::theme::BarIcon::Brightness,
            disp.brightness as f32 / 100.0,
            Instant::now(),
        ),
        AdjKind::Warmth => (
            crate::theme::BarIcon::Temperature,
            disp.warmth as f32 / 100.0,
            Instant::now(),
        ),
    }
}

/// Draw the in-game overlays (volume, brightness/temperature bar, fast
/// forward chip, toasts); shared by the running game and the pause menu.
fn draw_overlays(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    vol_hud: &mut Option<(f32, Instant)>,
    bar_hud: &mut Option<(crate::theme::BarIcon, f32, Instant)>,
    ff: bool,
    rewind: bool,
    toast: &mut Option<(String, Instant)>,
) {
    if let Some((ratio, t0)) = vol_hud {
        if t0.elapsed() < Duration::from_millis(1200) {
            crate::theme::bar_hud(cv, fonts, crate::theme::BarIcon::Volume, *ratio);
        } else {
            *vol_hud = None;
        }
    }
    if ff {
        draw_hud_box(cv, fonts, "FAST FORWARD", HudSpot::TopRight);
    }
    if rewind {
        draw_hud_box(cv, fonts, "REWIND", HudSpot::TopRight);
    }
    if let Some((icon, ratio, t0)) = bar_hud {
        if t0.elapsed() < Duration::from_millis(1400) {
            crate::theme::bar_hud(cv, fonts, *icon, *ratio);
        } else {
            *bar_hud = None;
        }
    }
    if let Some((msg, t0)) = toast {
        if t0.elapsed() < Duration::from_millis(1200) {
            draw_hud_box(cv, fonts, msg, HudSpot::BottomCenter);
        } else {
            *toast = None;
        }
    }
}

/// Save the current frame as a BMP; returns the toast to show.
fn take_screenshot(cv: &Canvas) -> String {
    let dir = std::path::Path::new(crate::systems::DIR_SHOTS);
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(format!("magi_{}.bmp", util::timestamp()));
    match cv.save_bmp(&path) {
        Ok(()) => {
            util::log(&format!("screenshot saved: {}", path.display()));
            "SCREENSHOT SAVED".into()
        }
        Err(e) => {
            util::log(&format!("screenshot failed: {e}"));
            "SCREENSHOT FAILED".into()
        }
    }
}

/// Blend a transparent layer over an opaque canvas (the layer keeps its own
/// coverage, so this is the same result the GPU produces).
fn blend_layer(dst: &mut Canvas, layer: &Canvas) {
    if layer.w != dst.w || layer.h != dst.h {
        return;
    }
    for (d, &s) in dst.px.iter_mut().zip(layer.px.iter()) {
        let a = s >> 24;
        if a == 0 {
            continue;
        }
        if a == 255 {
            *d = s | 0xFF00_0000;
            continue;
        }
        let keep = 255 - a;
        let r = (((s >> 16) & 0xff) * a + ((*d >> 16) & 0xff) * keep) / 255;
        let g = (((s >> 8) & 0xff) * a + ((*d >> 8) & 0xff) * keep) / 255;
        let b = ((s & 0xff) * a + (*d & 0xff) * keep) / 255;
        *d = 0xFF00_0000 | (r << 16) | (g << 8) | b;
    }
}

/// The screenshot frame: the game with the bezel and the overlays, composed
/// on the CPU. A hardware core has no CPU frame, so its FBO is read back
/// before this call and passed as `hw`.
fn compose_shot(
    ctx: &Ctx,
    scale: ScaleMode,
    game_area: (i32, i32, u32, u32),
    overlays: &Option<Overlay>,
    layer: &Canvas,
    cw: u32,
    ch: u32,
    hw: Option<Canvas>,
) -> Canvas {
    let mut shot = Canvas::new(cw, ch);
    if let Some(frame) = hw.as_ref() {
        blit_px(
            &frame.px,
            frame.w as usize,
            frame.h as usize,
            &mut shot,
            scale,
            game_area,
        );
        apply_overlays(&mut shot, overlays);
    } else if ctx.frame.is_null() {
        shot.clear(pal().bg0);
    } else {
        blit(ctx, &mut shot, scale, game_area);
        apply_overlays(&mut shot, overlays);
    }
    blend_layer(&mut shot, layer);
    shot
}

/// Drop the GPU presenter and withdraw its pointer from the core callbacks.
#[cfg(target_os = "linux")]
fn drop_gpu(gpu: &mut Option<crate::present::gles::Gpu>) {
    HW_GPU.store(std::ptr::null_mut(), Ordering::Relaxed);
    *gpu = None;
}

/// Read the hardware framebuffer back for screenshots and state previews.
#[cfg(target_os = "linux")]
fn hw_capture(flip_v: bool) -> Option<Canvas> {
    let gpu = HW_GPU.load(Ordering::Relaxed);
    if gpu.is_null() {
        return None;
    }
    let (px, w, h) = unsafe { (*gpu).read_hw(flip_v)? };
    let mut cv = Canvas::new(w, h);
    cv.px = px;
    Some(cv)
}

/// Upload the UI layer once for either present path.
#[cfg(target_os = "linux")]
fn gpu_ui<'a>(
    gpu: &'a mut Option<crate::present::gles::Gpu>,
    layer: &Canvas,
) -> Option<&'a mut crate::present::gles::Gpu> {
    let g = gpu.as_mut()?;
    let uploaded = match layer.dirty_rect() {
        Some((x, y, w, h)) => g.set_ui(Some(&crate::present::gles::Layer {
            px: &layer.px,
            w: layer.w as u32,
            h: layer.h as u32,
            rect: (x, y, w, h),
        })),
        None => g.set_ui(None),
    };
    if !uploaded {
        util::log("gpu: UI layer upload failed");
        return None;
    }
    Some(g)
}

/// Hand a software frame to the GPU: the game, the bezel overlay and the UI
/// layer (the overlays and the menu) in one pass.
#[cfg(target_os = "linux")]
fn gpu_present(
    gpu: &mut Option<crate::present::gles::Gpu>,
    layer: &Canvas,
    frame: *const u8,
    pitch: usize,
    fw: u32,
    fh: u32,
    fmt: u32,
    rect: (i32, i32, u32, u32),
    warmth: u8,
) -> bool {
    let Some(g) = gpu_ui(gpu, layer) else {
        return false;
    };
    g.present(frame, pitch, fw, fh, fmt, rect, warmth)
}

/// Composite the FBO a hardware-rendered core drew into.
#[cfg(target_os = "linux")]
fn gpu_present_hw(
    gpu: &mut Option<crate::present::gles::Gpu>,
    layer: &Canvas,
    rect: (i32, i32, u32, u32),
    warmth: u8,
    flip_v: bool,
) -> bool {
    let Some(g) = gpu_ui(gpu, layer) else {
        return false;
    };
    g.present_hw(rect, warmth, flip_v)
}

enum HudSpot {
    TopRight,
    BottomCenter,
}

fn draw_hud_box(cv: &mut Canvas, fonts: &mut Fonts, msg: &str, spot: HudSpot) {
    let tw = fonts.text_width(msg, 11.0);
    let (bx, by) = match spot {
        HudSpot::TopRight => (cv.w - tw - 28, 36),
        HudSpot::BottomCenter => ((cv.w - tw) / 2, cv.h - 36),
    };
    cv.fill_rect_a(bx - 12, by - 16, (tw + 24) as u32, 24, pal().bg0, 204);
    cv.rect_outline(bx - 12, by - 16, (tw + 24) as u32, 24, pal().border);
    fonts.draw(cv, bx, by, msg, 11.0, pal().text);
}

fn blit(ctx: &Ctx, cv: &mut Canvas, mode: ScaleMode, area: (i32, i32, u32, u32)) {
    let fw = ctx.geometry.base_width as usize;
    let fh = ctx.geometry.base_height as usize;
    if fw == 0 || fh == 0 || ctx.pitch == 0 {
        return;
    }

    // Convert the source once; rows that scale to several output rows are
    // expanded once and copied, instead of converting every pixel again.
    let mut src = vec![0u32; fw * fh];
    match ctx.pixfmt {
        PIXFMT_XRGB8888 => {
            for y in 0..fh {
                let row = unsafe {
                    std::slice::from_raw_parts(ctx.frame.add(y * ctx.pitch) as *const u32, fw)
                };
                for (d, &v) in src[y * fw..(y + 1) * fw].iter_mut().zip(row) {
                    *d = v | 0xFF00_0000;
                }
            }
        }
        PIXFMT_RGB565 => {
            for y in 0..fh {
                let row = unsafe {
                    std::slice::from_raw_parts(ctx.frame.add(y * ctx.pitch) as *const u16, fw)
                };
                for (d, &v) in src[y * fw..(y + 1) * fw].iter_mut().zip(row) {
                    *d = rgb565(v as u32);
                }
            }
        }
        _ => {
            for y in 0..fh {
                let row = unsafe {
                    std::slice::from_raw_parts(ctx.frame.add(y * ctx.pitch) as *const u16, fw)
                };
                for (d, &v) in src[y * fw..(y + 1) * fw].iter_mut().zip(row) {
                    *d = rgb1555(v as u32);
                }
            }
        }
    }

    blit_px(&src, fw, fh, cv, mode, area);
}

/// Scale an already-converted top-down `0xAARRGGBB` frame into `cv`.
fn blit_px(
    src: &[u32],
    fw: usize,
    fh: usize,
    cv: &mut Canvas,
    mode: ScaleMode,
    area: (i32, i32, u32, u32),
) {
    if fw == 0 || fh == 0 || src.len() < fw * fh {
        return;
    }
    let cw = cv.w as usize;
    let (x0, y0, dw, dh) = game_rect(mode, fw, fh, area);
    let (dw, dh) = (dw as usize, dh as usize);
    let (x0, y0) = (x0 as usize, y0 as usize);

    let ch = cv.h as usize;
    let bg = pal().bg0.to_u32();
    if y0 > 0 {
        cv.px[..y0 * cw].fill(bg);
    }
    let below = (y0 + dh).min(ch) * cw;
    if below < cv.px.len() {
        cv.px[below..].fill(bg);
    }
    for row in y0..(y0 + dh).min(ch) {
        let base = row * cw;
        cv.px[base..base + x0.min(cw)].fill(bg);
        let right = (x0 + dw).min(cw);
        cv.px[base + right..base + cw].fill(bg);
    }

    // One division per destination column.
    let sx_map: Vec<usize> = (0..dw)
        .map(|dx| ((dx as f32 * fw as f32 / dw as f32) as usize).min(fw - 1))
        .collect();

    // The scaler is the largest software cost in the frame; split the
    // output rows over two cores while the emulator core is idle.
    let split = dh / 2;
    if split >= 32 {
        let split_row = y0 + split;
        let (top, bottom) = cv.px.split_at_mut(split_row * cw);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                blit_rows(
                    src, &sx_map, bottom, split_row, cw, x0, y0, dw, fw, fh, dh, split, dh,
                );
            });
            blit_rows(src, &sx_map, top, 0, cw, x0, y0, dw, fw, fh, dh, 0, split);
        });
    } else {
        blit_rows(
            src, &sx_map, &mut cv.px, 0, cw, x0, y0, dw, fw, fh, dh, 0, dh,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_rows(
    src: &[u32],
    sx_map: &[usize],
    dst: &mut [u32],
    dst_base_row: usize,
    cw: usize,
    x0: usize,
    y0: usize,
    dw: usize,
    fw: usize,
    fh: usize,
    dh: usize,
    from: usize,
    to: usize,
) {
    let mut scaled = vec![0u32; dw];
    let mut last_sy = usize::MAX;
    for dy in from..to {
        let sy = ((dy as f32 * fh as f32 / dh as f32) as usize).min(fh - 1);
        if sy != last_sy {
            last_sy = sy;
            let srow = &src[sy * fw..(sy + 1) * fw];
            for (d, &sx) in scaled.iter_mut().zip(sx_map) {
                *d = srow[sx];
            }
        }
        let row = (y0 + dy - dst_base_row) * cw + x0;
        dst[row..row + dw].copy_from_slice(&scaled);
    }
}

fn rgb565(v: u32) -> u32 {
    let r = (v >> 11) & 0x1f;
    let g = (v >> 5) & 0x3f;
    let b = v & 0x1f;
    0xFF00_0000
        | (((r << 3) | (r >> 2)) << 16)
        | (((g << 2) | (g >> 4)) << 8)
        | ((b << 3) | (b >> 2))
}

fn rgb1555(v: u32) -> u32 {
    let r = (v >> 10) & 0x1f;
    let g = (v >> 5) & 0x1f;
    let b = v & 0x1f;
    0xFF00_0000
        | (((r << 3) | (r >> 2)) << 16)
        | (((g << 3) | (g >> 2)) << 8)
        | ((b << 3) | (b >> 2))
}

#[cfg(test)]
mod option_tests {
    use super::{
        chosen_index, default_option, ensure_gba_bios, flycast_states_blocked, parse_option,
        pce_bios_default_in,
    };

    #[test]
    fn parses_legacy_value_strings() {
        let (desc, values) = parse_option("Colorization; GBC|SGB|GBA");
        assert_eq!(desc, "Colorization");
        assert_eq!(values, vec!["GBC", "SGB", "GBA"]);
        let (desc, values) = parse_option("RTC; Enabled|Disabled");
        assert_eq!(desc, "RTC");
        assert_eq!(values.len(), 2);
        let (desc, values) = parse_option("NoSeparator");
        assert_eq!(desc, "NoSeparator");
        assert!(values.is_empty());
    }

    #[test]
    fn mgba_defaults_game_boy_player_rumble_on() {
        let values: Vec<String> = ["OFF", "ON"].iter().map(|s| s.to_string()).collect();
        assert_eq!(default_option("mgba", "gba", "mgba_force_gbp"), Some("ON"));
        assert_eq!(
            chosen_index("mgba", "gba", "mgba_force_gbp", &values, None, false),
            1,
            "a fresh card enables GBA rumble"
        );
        assert_eq!(
            chosen_index(
                "mgba",
                "gba",
                "mgba_force_gbp",
                &values,
                Some(&"OFF".to_string()),
                false
            ),
            0,
            "a saved choice still wins"
        );
    }

    #[test]
    fn gambatte_options_are_per_system() {
        assert_eq!(super::options_key("gambatte", "gb"), "gambatte:gb");
        assert_eq!(super::options_key("gambatte", "gbc"), "gambatte:gbc");
        assert_eq!(super::options_key("mgba", "gba"), "mgba");
    }

    #[test]
    fn lcd_accurate_gb_forces_greyscale() {
        let values: Vec<String> = ["disabled", "auto", "internal"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let saved = Some("auto".to_string());
        assert_eq!(
            chosen_index(
                "gambatte",
                "gb",
                "gambatte_gb_colorization",
                &values,
                saved.as_ref(),
                true
            ),
            0,
            "the LCD-accurate palette needs grey input"
        );
        assert_eq!(
            chosen_index(
                "gambatte",
                "gb",
                "gambatte_gb_colorization",
                &values,
                saved.as_ref(),
                false
            ),
            1,
            "the saved colourisation is kept otherwise"
        );
    }

    #[test]
    fn gambatte_defaults_follow_the_console() {
        assert_eq!(
            default_option("gambatte", "gb", "gambatte_gbc_color_correction"),
            Some("disabled")
        );
        assert_eq!(
            default_option("gambatte", "gbc", "gambatte_gbc_color_correction"),
            Some("GBC only")
        );
        assert_eq!(
            default_option("gambatte", "gb", "gambatte_dark_filter_level"),
            Some("0")
        );
        assert_eq!(
            default_option("gambatte", "gbc", "gambatte_dark_filter_level"),
            Some("0")
        );
        assert_eq!(
            default_option("gambatte", "gb", "gambatte_gb_colorization"),
            Some("auto")
        );
        assert_eq!(
            default_option("gambatte", "gbc", "gambatte_gb_colorization"),
            Some("disabled")
        );
        assert_eq!(
            default_option("gambatte", "gb", "gambatte_gb_internal_palette"),
            Some("GB - DMG")
        );
        assert_eq!(
            default_option("gambatte", "gb", "gambatte_mix_frames"),
            Some("mix")
        );
        assert_eq!(
            default_option("gambatte", "gb", "gambatte_gbc_color_correction_mode"),
            Some("accurate")
        );
        assert_eq!(
            default_option("gambatte", "gb", "gambatte_gbc_frontlight_position"),
            Some("central")
        );
        assert_eq!(
            default_option("gambatte", "gb", "gambatte_gb_hwmode"),
            Some("Auto")
        );
        assert_eq!(
            default_option("gambatte", "gb", "gambatte_rumble_level"),
            Some("8")
        );

        let cc = ["GBC only", "always", "disabled"].map(String::from);
        assert_eq!(
            chosen_index(
                "gambatte",
                "gb",
                "gambatte_gbc_color_correction",
                &cc,
                None,
                false
            ),
            2
        );
        assert_eq!(
            chosen_index(
                "gambatte",
                "gbc",
                "gambatte_gbc_color_correction",
                &cc,
                None,
                false
            ),
            0
        );
        let dark = ["0", "5", "10", "15", "20"].map(String::from);
        assert_eq!(
            chosen_index(
                "gambatte",
                "gb",
                "gambatte_dark_filter_level",
                &dark,
                None,
                false
            ),
            0
        );
        assert_eq!(
            chosen_index(
                "gambatte",
                "gbc",
                "gambatte_dark_filter_level",
                &dark,
                None,
                false
            ),
            0
        );
    }

    #[test]
    fn psx_defaults_follow_the_handheld_setting() {
        for (key, value) in [
            ("pcsx_rearmed_show_bios_bootlogo", "disabled"),
            ("pcsx_rearmed_dithering", "disabled"),
            ("pcsx_rearmed_frameskip_type", "auto_threshold"),
            ("pcsx_rearmed_frameskip_threshold", "15"),
            ("pcsx_rearmed_frameskip_interval", "3"),
            ("pcsx_rearmed_neon_enhancement_enable", "disabled"),
            ("pcsx_rearmed_neon_enhancement_tex_adj_v2", "enabled"),
            ("pcsx_rearmed_neon_enhancement_no_main", "enabled"),
            ("pcsx_rearmed_spu_reverb", "enabled"),
            ("pcsx_rearmed_spu_interpolation", "simple"),
            ("pcsx_rearmed_vibration", "enabled"),
        ] {
            assert_eq!(
                default_option("pcsx_rearmed", "psx", key),
                Some(value),
                "{key}"
            );
        }
    }

    #[test]
    fn pce_bios_default_follows_the_card() {
        let dir = std::env::temp_dir().join(format!("magi_pce_bios_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        assert_eq!(pce_bios_default_in(&dir), None, "core default");
        std::fs::write(dir.join("syscard3u.pce"), b"bios").unwrap();
        assert_eq!(pce_bios_default_in(&dir), Some("System Card 3 US"));
        std::fs::write(dir.join("syscard3.pce"), b"bios").unwrap();
        assert_eq!(pce_bios_default_in(&dir), None, "System Card 3 wins");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gba_bios_alias_is_created() {
        let dir = std::env::temp_dir().join(format!("magi_gba_bios_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("gba_bios.gba"), b"gba bios").unwrap();
        ensure_gba_bios(&dir);
        assert_eq!(
            std::fs::read(dir.join("gba_bios.bin")).unwrap(),
            b"gba bios"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pokemini_ships_a_curated_option_set() {
        for label in [
            "Video Scale (Restart)",
            "LCD Filter",
            "LCD Mode",
            "LCD Contrast",
            "LCD Brightness",
            "Palette",
            "Piezo Filter",
            "Screen Shake Level",
            "Controller Rumble Level",
        ] {
            assert!(super::pokemini_option_kept(label), "{label}");
        }
        assert!(!super::pokemini_option_kept("60Hz Mode"));
        assert!(!super::pokemini_option_kept("Low Pass Filter"));
        assert!(!super::pokemini_option_kept("Low Pass Filter Level (%)"));
        assert!(!super::pokemini_option_kept("Turbo Button Period"));
        assert_eq!(
            default_option("pokemini", "pokemini", "pokemini_video_scale"),
            Some("6x")
        );
        assert_eq!(
            default_option("pokemini", "pokemini", "pokemini_60hz_mode"),
            Some("enabled")
        );
    }

    #[test]
    fn flycast_states_need_threading_off() {
        assert!(flycast_states_blocked("flycast", true));
        assert!(!flycast_states_blocked("flycast", false));
        assert!(!flycast_states_blocked("pcsx_rearmed", true));
    }

    #[test]
    fn flycast_ships_a_curated_option_set() {
        for label in [
            "Region",
            "Language",
            "Use Real BIOS",
            "HLE BIOS",
            "Threaded Rendering",
            "Force Windows CE Mode",
            "Cable Type",
            "Alpha Sorting",
            "Frame Skipping",
            "Digital Triggers",
            "Purupuru Pack/Vibration Pack",
        ] {
            assert!(super::flycast_option_kept(label), "{label}");
        }
        for label in [
            "Screen Orientation",
            "Internal Resolution",
            "GD-ROM Fast Loading",
            "GDROM Fast Loading (inaccurate)",
            "Per-Game VMUs",
            "VMU Screen 1 Display",
            "Analog Stick Deadzone",
            "Trigger Deadzone",
            "Gun Crosshair 1 Display",
            "Anisotropic Filtering",
            "Texture Upscaling (xBRZ)",
        ] {
            assert!(!super::flycast_option_kept(label), "{label}");
        }
        assert_eq!(
            default_option("flycast", "dc", "reicast_digital_triggers"),
            Some("enabled")
        );
        assert_eq!(
            default_option("flycast", "dc", "reicast_enable_dsp"),
            Some("disabled")
        );
        assert_eq!(
            default_option("flycast", "dc", "reicast_synchronous_rendering"),
            Some("enabled")
        );
        assert_eq!(
            default_option("flycast", "dc", "reicast_gdrom_fast_loading"),
            Some("enabled")
        );
        assert_eq!(
            default_option("flycast", "dc", "reicast_alpha_sorting"),
            Some("per-strip (fast, least accurate)")
        );
        assert_eq!(
            default_option("flycast", "dc", "reicast_enable_purupuru"),
            Some("disabled")
        );
    }

    #[test]
    fn flycast_stays_at_native_resolution() {
        let values: Vec<String> = ["1x", "2x", "4x", "8x"]
            .iter()
            .map(|v| (*v).to_string())
            .collect();
        let saved = "8x".to_string();
        assert_eq!(
            super::chosen_index(
                "flycast",
                "dc",
                "reicast_internal_resolution",
                &values,
                Some(&saved),
                false
            ),
            0
        );
        assert_eq!(
            super::chosen_index(
                "flycast",
                "dc",
                "reicast_render_to_texture_upscaling",
                &values,
                Some(&saved),
                false
            ),
            0
        );

        let fast = ["enabled", "disabled"].map(str::to_string);
        assert_eq!(
            super::chosen_index(
                "flycast",
                "dc",
                "reicast_gdrom_fast_loading",
                &fast,
                Some(&"disabled".to_string()),
                false
            ),
            0
        );

        let old: Vec<String> = ["320x240", "640x480", "800x600"]
            .iter()
            .map(|v| (*v).to_string())
            .collect();
        let old_saved = "800x600".to_string();
        assert_eq!(
            super::chosen_index(
                "flycast",
                "dc",
                "reicast_internal_resolution",
                &old,
                Some(&old_saved),
                false
            ),
            1
        );
        let sync = ["disabled", "enabled"].map(str::to_string);
        assert_eq!(
            super::chosen_index(
                "flycast",
                "dc",
                "reicast_synchronous_rendering",
                &sync,
                Some(&"disabled".to_string()),
                false
            ),
            1
        );
    }
}

#[cfg(test)]
mod key_mode_tests {
    use super::{needs_l2_r2, needs_performance, rewind_allowed, REWIND_MAX_STATE};

    #[test]
    fn light_systems_stay_on_schedutil() {
        for system in [
            "gb", "gbc", "gba", "ngp", "pokemini", "snes", "pce", "genesis",
        ] {
            assert!(!needs_performance(system), "{system}");
        }
        assert!(needs_performance("psx"));
        assert!(needs_performance("dc"));
    }

    #[test]
    fn rewind_is_off_for_psx_dreamcast_and_large_states() {
        assert!(!rewind_allowed("psx", 1 << 20));
        assert!(!rewind_allowed("dc", 1 << 20));
        assert!(rewind_allowed("gba", 1 << 20));
        assert!(!rewind_allowed("gba", REWIND_MAX_STATE + 1));
        assert!(!rewind_allowed("gba", 0));
    }

    #[test]
    fn only_psx_keeps_l2_r2_from_the_frontend() {
        assert!(needs_l2_r2("psx"));
        assert!(!needs_l2_r2("dc"));
        assert!(!needs_l2_r2("snes"));
    }
}

#[cfg(test)]
mod pad_tests {
    use super::pad::PadState;
    use super::{ev, retro, RawEvent};

    fn axis(code: u16, value: i32) -> RawEvent {
        RawEvent {
            dev: 0,
            type_: ev::EV_ABS,
            code,
            value,
        }
    }

    #[test]
    fn left_stick_is_dpad_only_for_digital_cores() {
        let mut pad = PadState::default();
        pad.apply(&axis(ev::ABS_X, -8000));
        assert_ne!(pad.bits & (1 << retro::LEFT), 0);

        pad.analog = true;
        pad.reset();
        pad.apply(&axis(ev::ABS_X, -8000));
        assert_eq!(pad.bits & (1 << retro::LEFT), 0);
    }

    #[test]
    fn dpad_becomes_a_full_analog_axis() {
        let left = 1 << retro::LEFT;
        let right = 1 << retro::RIGHT;
        let up = 1 << retro::UP;
        let down = 1 << retro::DOWN;
        assert_eq!(super::dpad_axis(left, retro::LEFT, retro::RIGHT), -0x7fff);
        assert_eq!(super::dpad_axis(right, retro::LEFT, retro::RIGHT), 0x7fff);
        assert_eq!(super::dpad_axis(left | right, retro::LEFT, retro::RIGHT), 0);
        assert_eq!(super::dpad_axis(up, retro::UP, retro::DOWN), -0x7fff);
        assert_eq!(super::dpad_axis(down, retro::UP, retro::DOWN), 0x7fff);
    }

    #[test]
    fn psx_dualshock_is_analog_subclass_one() {
        assert_eq!(
            super::RETRO_DEVICE_PSE_DUALSHOCK,
            super::retro_subclass(super::RETRO_DEVICE_ANALOG, 1)
        );
        assert_eq!(super::RETRO_DEVICE_PSE_DUALSHOCK, 0x205);
    }

    #[test]
    fn frontend_triggers_stay_out_of_the_core() {
        let triggers = (1 << retro::L2) | (1 << retro::R2);
        let bits = triggers | (1 << retro::A);
        assert_eq!(
            super::core_pad_bits(bits, false, false, false, false),
            1 << retro::A
        );
        assert_eq!(super::core_pad_bits(bits, true, false, false, false), bits);
    }

    #[test]
    fn dreamcast_triggers_swap_only_when_enabled() {
        let l1 = 1 << retro::L;
        let r1 = 1 << retro::R;
        let l2 = 1 << retro::L2;
        let r2 = 1 << retro::R2;

        // Off: physical L1 is the Dreamcast's left trigger.
        let left = super::core_pad_bits(l1, false, false, true, false);
        assert_ne!(left & r2, 0);
        assert_eq!(left & l2, 0);
        let right = super::core_pad_bits(r1, false, false, true, false);
        assert_ne!(right & l2, 0);
        assert_eq!(right & r2, 0);
        let both = super::core_pad_bits(l1 | r1, false, false, true, false);
        assert_ne!(both & l2, 0);
        assert_ne!(both & r2, 0);
        assert_eq!(both & (l1 | r1), 0);

        // On: the two triggers exchange places.
        let left = super::core_pad_bits(l1, false, false, true, true);
        assert_ne!(left & l2, 0);
        assert_eq!(left & r2, 0);
        let right = super::core_pad_bits(r1, false, false, true, true);
        assert_ne!(right & r2, 0);
        assert_eq!(right & l2, 0);

        // Physical L2/R2 belong to the frontend on Dreamcast.
        assert_eq!(super::core_pad_bits(l2 | r2, false, false, true, false), 0);
    }

    #[test]
    fn ab_swap_only_moves_the_pressed_button() {
        let a = 1 << retro::A;
        let b = 1 << retro::B;
        assert_eq!(super::core_pad_bits(a, false, true, false, false), b);
        assert_eq!(super::core_pad_bits(b, false, true, false, false), a);
        assert_eq!(
            super::core_pad_bits(a | b, false, true, false, false),
            a | b
        );
        assert_eq!(super::core_pad_bits(0, false, true, false, false), 0);
        assert_eq!(super::core_pad_bits(a, false, false, false, false), a);
    }
}

#[cfg(test)]
mod state_slot_tests {
    use std::path::Path;

    #[test]
    fn slots_have_distinct_names() {
        let dir = "Saves/GBC";
        assert_eq!(
            super::state_slot_path(dir, "Game", 0),
            Path::new(dir).join("Game.state")
        );
        assert_eq!(
            super::state_slot_path(dir, "Game", 1),
            Path::new(dir).join("Game.state2")
        );
        assert_eq!(
            super::state_slot_path(dir, "Game", 2),
            Path::new(dir).join("Game.state3")
        );
        assert_eq!(
            super::state_thumb_path(Path::new("Saves/GBC/Game.state")),
            Path::new("Saves/GBC/Game.state.png")
        );
    }

    #[test]
    fn state_compression_round_trips() {
        let raw = vec![0u8; 1 << 20];
        let (packed, compressed) = super::pack_state(raw.clone());
        assert!(compressed);
        assert!(packed.starts_with(super::STATE_MAGIC));
        assert_eq!(super::unpack_state(packed).unwrap(), raw);

        let raw = b"small raw state".to_vec();
        let (packed, compressed) = super::pack_state(raw.clone());
        assert!(!compressed);
        assert_eq!(super::unpack_state(packed).unwrap(), raw);
    }
}

#[cfg(test)]
mod overlay_tests {
    use super::{double_tap, option_kept, DOUBLE_TAP};
    use std::time::{Duration, Instant};

    #[test]
    fn option_whitelist_matches_loosely() {
        assert!(option_kept("Color Correction"));
        assert!(option_kept("Color Correction Mode"));
        assert!(option_kept("Color Correction - Frontlight Position"));
        assert!(option_kept("Dark Filter Level (%)"));
        assert!(option_kept("Interframe Blending"));
        assert!(option_kept("Emulated Hardware (Restart Required)"));
        assert!(option_kept("Controller Rumble Strength"));
        assert!(option_kept("GB Colorization"));
        assert!(option_kept("Internal Palette"));
        assert!(option_kept("Console Region (Restart)"));
        assert!(option_kept("Reduce Slowdown (Hack, Unsafe, Restart)"));
        assert!(option_kept("Reduce Flickering (Hack, Unsafe)"));
        assert!(option_kept("Color Palette"));
        assert!(option_kept("Blargg NTSC Filter"));
        assert!(option_kept("Crop Overscan"));
        assert!(option_kept("Borders"));
        assert!(option_kept("CPU Speed"));
        assert!(option_kept("BIOS Selection"));
        assert!(option_kept("BIOS Select"));
        assert!(option_kept("Show BIOS Name/Boot Logo"));
        assert!(option_kept("Use BIOS File if Found (Restart)"));
        assert!(option_kept("Skip BIOS Intro (Restart)"));
        assert!(option_kept("CD Bios (Restart Required)"));
        assert!(option_kept("Enhanced Resolution"));
        assert!(option_kept("Rumble Effects"));
        assert!(option_kept("Frameskip"));
        assert!(option_kept("Frameskip Type"));
        assert!(option_kept("Game Boy Player Rumble (Restart)"));
        assert!(!option_kept("Texture Scaling"));
        assert!(!option_kept("Game Boy Model (Restart)"));
        assert!(!option_kept("Hardware Preset Game Boy Palettes (Restart)"));
        assert!(!option_kept("Default Game Boy Palette"));
        assert!(!option_kept("Color"));
        assert!(option_kept("Solar Sensor Level"));
    }

    #[test]
    fn mgba_hides_game_boy_options() {
        assert!(super::mgba_option_hidden("Game Boy Model (Restart)"));
        assert!(super::mgba_option_hidden("Default Game Boy Palette"));
        assert!(super::mgba_option_hidden(
            "Hardware Preset Game Boy Palettes (Restart)"
        ));
        assert!(super::mgba_option_hidden(
            "Use Super Game Boy Borders (Restart)"
        ));
        assert!(!super::mgba_option_hidden("Solar Sensor Level"));
        assert!(!super::mgba_option_hidden("Borders"));
    }

    #[test]
    fn genesis_hides_the_audio_mixer_details() {
        assert!(super::genesis_option_hidden(
            "Mega Drive/Genesis FM Channel 0 Volume %"
        ));
        assert!(super::genesis_option_hidden(
            "Mega Drive/Genesis FM Channel 5 Volume %"
        ));
        assert!(super::genesis_option_hidden("FM Preamp Level"));
        assert!(super::genesis_option_hidden("PSG Preamp Level"));
        assert!(super::genesis_option_hidden("CD-DA Volume %"));
        assert!(super::genesis_option_hidden("PCM Volume %"));
        assert!(super::genesis_option_hidden("Mega Drive/Genesis FM"));
        assert!(!super::genesis_option_hidden("Sound Output"));
    }

    #[test]
    fn double_tap_needs_two_quick_presses() {
        let t0 = Instant::now();
        let mut last = None;
        assert!(!double_tap(&mut last, t0, DOUBLE_TAP), "first press");
        assert!(
            double_tap(&mut last, t0 + Duration::from_millis(150), DOUBLE_TAP),
            "second press within the window toggles"
        );
        assert!(
            !double_tap(&mut last, t0 + Duration::from_millis(300), DOUBLE_TAP),
            "the pair is consumed"
        );
        let mut last = None;
        assert!(!double_tap(&mut last, t0, DOUBLE_TAP));
        assert!(
            !double_tap(
                &mut last,
                t0 + DOUBLE_TAP + Duration::from_millis(10),
                DOUBLE_TAP
            ),
            "too slow"
        );
    }
}

#[cfg(test)]
mod display_tests {
    use super::overlay::screen_area;
    use super::{game_rect, scale_factors, ScaleMode};

    #[test]
    fn scale_modes_fit_integer_stretch() {
        let (fx, fy) = scale_factors(ScaleMode::Fit, 640, 480, 240, 160);
        assert!(
            (fx - 2.6667).abs() < 0.01 && (fx - fy).abs() < 0.001,
            "{fx} {fy}"
        );
        let (ix, iy) = scale_factors(ScaleMode::Integer, 640, 480, 240, 160);
        assert_eq!((ix, iy), (2.0, 2.0), "floor to whole pixels");
        let (sx, sy) = scale_factors(ScaleMode::Stretch, 640, 480, 240, 160);
        assert!((sx - 2.6667).abs() < 0.01 && (sy - 3.0).abs() < 0.001);
        assert_eq!(ScaleMode::Fit.next().next().next(), ScaleMode::Fit);
    }

    #[test]
    fn converted_core_pixels_are_opaque() {
        for v in [0u32, 0x7fff, 0xffff] {
            assert_eq!(super::rgb565(v) >> 24, 0xFF, "565 {v:#x}");
            assert_eq!(super::rgb1555(v) >> 24, 0xFF, "1555 {v:#x}");
        }
    }

    #[test]
    fn game_rect_stays_inside_the_bezel_area() {
        // A GBA bezel: screen 640x427 at the top of the panel.
        let area = (0, 0, 640, 427);
        assert_eq!(game_rect(ScaleMode::Fit, 240, 160, area), (0, 0, 640, 426));
        assert_eq!(
            game_rect(ScaleMode::Integer, 240, 160, area),
            (80, 53, 480, 320)
        );
        assert_eq!(
            game_rect(ScaleMode::Stretch, 240, 160, area),
            (0, 0, 640, 427)
        );
        // A GB bezel: 532x480 centred horizontally.
        let area = (53, 0, 532, 480);
        let (x, _, w, _) = game_rect(ScaleMode::Fit, 160, 144, area);
        assert_eq!((x, w), (53, 532));
    }

    #[test]
    fn screen_area_finds_the_low_alpha_window() {
        let (w, h) = (32u32, 32u32);
        let mut px = vec![0xFFFF_FFFFu32; (w * h) as usize];
        for y in 8..24 {
            for x in 6..26 {
                px[(y * w + x) as usize] = 0x55FF_FFFF;
            }
        }
        assert_eq!(screen_area(&px, w, h), Some((6, 8, 20, 16)));
        // A fully opaque image has no screen region.
        let solid = vec![0xFFFF_FFFFu32; (w * h) as usize];
        assert_eq!(screen_area(&solid, w, h), None);
    }
}
