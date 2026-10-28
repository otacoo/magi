//! BALTHASAR — the libretro host. Spawned by the UI as `magi run`. It takes
//! the panel and the input devices, loads a core, runs a game, and exits
//! back to MELCHIOR. Video goes to the same presenters as the UI; audio goes
//! directly to ALSA (libasound via dlopen).

use std::collections::VecDeque;
use std::ffi::{c_void, CStr, CString};
use std::os::raw::{c_char, c_uint};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use libloading::Library;
use sdl2::controller::{Axis as PadAxis, Button as PadButton};
use sdl2::event::Event;

use crate::canvas::Canvas;
use crate::defaults::Defaults;
use crate::font::{self, Fonts};
use crate::input::{ev, joypad_bit, retro, RawEvent, Shortcut, Shortcuts};
use crate::netplay;
use crate::present::Presenter;
use crate::systems::Catalogue;
use crate::theme::pal;
use crate::util;

#[cfg(target_os = "linux")]
use crate::audio::AlsaOut;
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
const ENV_GET_SAVE_DIRECTORY: u32 = 31;
const ENV_SET_GEOMETRY: u32 = 37;
const ENV_GET_INPUT_BITMASKS: u32 = 51 | 0x1_0000;
const ENV_GET_CORE_OPTIONS_VERSION: u32 = 52;
const ENV_SET_DISK_CONTROL_INTERFACE: u32 = 13;

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
    ctx().set_rumble(strength > 0)
}

const PIXFMT_0RGB1555: u32 = 0;
const PIXFMT_XRGB8888: u32 = 1;
const PIXFMT_RGB565: u32 = 2;

/// Sink rate on the H700; cores reporting their own rate are resampled.
#[cfg(target_os = "linux")]
const DEVICE_HZ: u32 = 32_768;

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

/// Core defaults that differ from the core's first value (its own default).
/// A saved choice always wins.
fn default_option(core: &str, key: &str) -> Option<&'static str> {
    match (core, key) {
        // LCD ghosting: 50:50 with the previous frame, for games that
        // flicker sprites to fake transparency (F-Zero, Boktai, ZAS).
        ("mgba", "mgba_interframe_blending") => Some("mix"),
        ("gambatte", "gambatte_mix_frames") => Some("mix"),
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
fn chosen_index(core: &str, key: &str, values: &[String], saved: Option<&String>) -> usize {
    // Netplay owns the link mode at runtime. Never restore a saved server or
    // client mode, or a game would open a socket before the player asks.
    if core == "gambatte" && key == "gambatte_gb_link_mode" {
        if let Some(i) = values.iter().position(|v| v == "Not Connected") {
            return i;
        }
    }
    if let Some(i) = saved.and_then(|s| values.iter().position(|v| v == s)) {
        return i;
    }
    if let Some(d) = default_option(core, key) {
        if let Some(i) = values.iter().position(|v| v.eq_ignore_ascii_case(d)) {
            return i;
        }
    }
    0
}

fn options_path() -> &'static str {
    "Config/core-options.json"
}

/// Per-core saved choices: `{ "<core>": { "<option key>": "<label>" } }`.
fn load_saved_options(core: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Ok(text) = std::fs::read_to_string(options_path()) else {
        return map;
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&text) else {
        return map;
    };
    if let Some(obj) = root.get(core).and_then(|v| v.as_object()) {
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
    for o in &ctx.options {
        core.insert(o.key.clone(), serde_json::json!(o.values[o.chosen]));
    }
    root[&ctx.opt_key] = serde_json::Value::Object(core);
    if let Ok(text) = serde_json::to_string_pretty(&root) {
        let _ = std::fs::create_dir_all("Config");
        if let Err(e) = util::write_file_atomic(path, text.as_bytes()) {
            util::log(&format!("core options: cannot save: {e}"));
        }
    }
}

struct Ctx {
    pixfmt: u32,
    geometry: GameGeometry,
    frame: *const u8,
    /// Bumped by the video callback on real frames (nulls are dups).
    frame_seq: u64,
    pitch: usize,
    bits: u16,
    left: (i16, i16),
    right: (i16, i16),
    audio: Vec<i16>,
    save_dir: CString,
    system_dir: CString,
    shutdown: bool,
    sram_ptr: *mut u8,
    ser_size: unsafe extern "C" fn() -> usize,
    ser: unsafe extern "C" fn(*mut c_void, usize) -> bool,
    unser: unsafe extern "C" fn(*const c_void, usize) -> bool,
    options: Vec<CoreOption>,
    opt_key: String,
    opt_dirty: bool,
    saved_opts: std::collections::HashMap<String, String>,
    overlay_layers: OverlayLayers,
    overlay_dirty: bool,
    moto: Option<std::fs::File>,
    rumble_on: bool,
    rumble_ok: bool,
    /// The core's disk table while a multi-disc game is loaded.
    disk: Option<DiskControl>,
    system: String,
    rom: String,
    netplay: Option<netplay::Session>,
    netplay_linked: bool,
}

impl Ctx {
    /// The handheld's vibration motor: a write-only sysfs switch on the
    /// AXP2202. The state is cached so a core pulsing rumble at frame rate
    /// does not beat on the attribute.
    fn set_rumble(&mut self, on: bool) -> bool {
        let on = on && self.rumble_ok;
        if self.rumble_on == on {
            return true;
        }
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
                return true;
            }
        }
        false
    }
}

static CTX: AtomicPtr<Ctx> = AtomicPtr::new(std::ptr::null_mut());

fn ctx() -> &'static mut Ctx {
    unsafe { &mut *CTX.load(Ordering::Relaxed) }
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
                    let chosen =
                        chosen_index(&ctx.opt_key, &key, &values, ctx.saved_opts.get(&key));
                    let c_values = values
                        .iter()
                        .map(|v| CString::new(v.as_str()).unwrap_or_default())
                        .collect();
                    let visible = option_kept(&desc);
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
        // No GL, no logging interface: cores cope.
        ENV_SET_HW_RENDER => false,
        _ => false,
    }
}

extern "C" fn video_cb(data: *const c_void, width: c_uint, height: c_uint, pitch: usize) {
    if data.is_null() || data as usize == usize::MAX {
        return; // duplicate frame, or a HW buffer (we refuse HW render)
    }
    let ctx = ctx();
    ctx.frame = data as *const u8;
    ctx.frame_seq += 1;
    ctx.geometry.base_width = width;
    ctx.geometry.base_height = height;
    ctx.pitch = pitch;
}

extern "C" fn audio_sample_cb(left: i16, right: i16) {
    let ctx = ctx();
    ctx.audio.push(left);
    ctx.audio.push(right);
}

extern "C" fn audio_batch_cb(data: *const i16, frames: usize) -> usize {
    let ctx = ctx();
    if !data.is_null() {
        let slice = unsafe { std::slice::from_raw_parts(data, frames * 2) };
        ctx.audio.extend_from_slice(slice);
    }
    frames
}

extern "C" fn input_poll_cb() {}

extern "C" fn input_state_cb(port: c_uint, device: c_uint, index: c_uint, id: c_uint) -> i16 {
    if port != 0 {
        return 0;
    }
    if device == RETRO_DEVICE_ANALOG {
        let ctx = ctx();
        let v = match (index, id) {
            (0, ANALOG_X) => ctx.left.0,
            (0, ANALOG_Y) => ctx.left.1,
            (1, ANALOG_X) => ctx.right.0,
            (1, ANALOG_Y) => ctx.right.1,
            _ => 0,
        };
        return if v.abs() < STICK_DEADZONE { 0 } else { v };
    }
    if device != RETRO_DEVICE_JOYPAD {
        return 0;
    }
    let bits = ctx().bits;
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

/// One decoded overlay PNG (0xAARRGGBB), pre-scaled to the panel.
struct OverlayLayer {
    px: Vec<u32>,
}

/// Overlay paths by layer, in blend order: CRT, bezel.
type OverlayLayers = [Vec<String>; 2];

/// Which layer an overlay belongs to, from its path; colour-correction
/// files are ignored (that belongs in the core options).
fn overlay_kind(rel: &str) -> Option<usize> {
    let lower = rel.to_ascii_lowercase();
    let top = lower.split('/').next().unwrap_or("");
    if top.contains("crt") || lower.contains("_crt") {
        Some(0)
    } else if top.contains("color") || top.contains("colour") || top.contains("correction") {
        None
    } else {
        Some(1)
    }
}

/// Every PNG under Overlays/ (recursively), as relative paths, sorted.
fn overlay_scan() -> Vec<String> {
    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("png"))
            {
                if let Ok(rel) = p.strip_prefix(base) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    let base = std::path::Path::new("Overlays");
    let mut out = Vec::new();
    walk(base, base, &mut out);
    out.sort();
    out
}

/// The scanned overlays split into their two layer lists.
fn overlay_scan_layers() -> OverlayLayers {
    let mut out: OverlayLayers = std::array::from_fn(|_| Vec::new());
    for name in overlay_scan() {
        if let Some(kind) = overlay_kind(&name) {
            out[kind].push(name);
        }
    }
    out
}

/// Decode a PNG into the canvas' 0xAARRGGBB layout, nearest-scaled to the
/// panel (the overlay packs are authored at 640x480 already).
fn load_overlay_png(name: &str, w: u32, h: u32) -> Option<OverlayLayer> {
    let path = std::path::Path::new("Overlays").join(name);
    let img = crate::image::load_png(&path)?;
    let img = if img.w == w && img.h == h {
        img
    } else {
        img.scaled(w, h)
    };
    Some(OverlayLayer { px: img.px })
}

/// The layer choices, flattened into one image at load time so the
/// per-frame cost is a single blend pass no matter how many layers are on
/// (three passes were dropping frames on GBA/GBC cores).
struct Overlay {
    px: Vec<u32>,
    /// Contiguous runs of non-transparent pixels, when they save real work
    /// (bezel-style overlays are mostly clear in the middle). None means a
    /// plain full scan is cheaper.
    spans: Option<Vec<(u32, u32)>>,
    any: bool,
}

/// Runs of pixels that need blending; None when spans would not pay off
/// (near-full coverage or a very fragmented pattern, e.g. CRT scanlines).
fn transparent_spans(px: &[u32]) -> Option<Vec<(u32, u32)>> {
    let mut spans: Vec<(u32, u32)> = Vec::new();
    let mut start: Option<u32> = None;
    let mut covered = 0usize;
    for (i, &p) in px.iter().enumerate() {
        if p != 0xFF00_0000 {
            covered += 1;
            if start.is_none() {
                start = Some(i as u32);
            }
        } else if let Some(s) = start.take() {
            spans.push((s, i as u32));
            if spans.len() > 8192 {
                return None;
            }
        }
    }
    if let Some(s) = start {
        spans.push((s, px.len() as u32));
    }
    (spans.len() <= 8192 && covered <= px.len() * 9 / 10).then_some(spans)
}

/// One source-over blend step on premultiplied pixels.
#[inline]
fn blend_px(dst: &mut u32, src: u32) {
    if src == 0xFF00_0000 {
        return;
    }
    let inv = src >> 24;
    let d = *dst;
    let r = ((src >> 16) & 0xff) + (((d >> 16) & 0xff) * inv + 127) / 255;
    let g = ((src >> 8) & 0xff) + (((d >> 8) & 0xff) * inv + 127) / 255;
    let b = (src & 0xff) + ((d & 0xff) * inv + 127) / 255;
    *dst = 0xFF00_0000 | (r << 16) | (g << 8) | b;
}

/// Four pixels at a time on NEON. The overlay is premultiplied, so each
/// channel is `src + dst * inv / 255`; a fully transparent pixel (the
/// 0xFF000000 marker) makes this an identity.
#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn blend4_neon(dst: *mut u32, src: *const u32) {
    use std::arch::aarch64::*;
    let s = vld1q_u32(src);
    let d = vld1q_u32(dst as *const u32);
    let mask = vdupq_n_u32(0xff);
    let sh24 = vdupq_n_s32(-24);
    let sh16 = vdupq_n_s32(-16);
    let sh8 = vdupq_n_s32(-8);
    let sh23 = vdupq_n_s32(-23);
    let left16 = vdupq_n_s32(16);
    let left8 = vdupq_n_s32(8);
    let inv = vshlq_u32(s, sh24);
    // Alpha 255 (inv 0) is a copy; alpha 0 (the transparent marker) is a
    // no-op. CRT/bezel art is mostly one or the other outside the screen.
    if vmaxvq_u32(inv) == 0 {
        vst1q_u32(dst, vorrq_u32(s, vdupq_n_u32(0xFF00_0000)));
        return;
    }
    if vminvq_u32(inv) == 255 {
        return;
    }
    let sr = vandq_u32(vshlq_u32(s, sh16), mask);
    let sg = vandq_u32(vshlq_u32(s, sh8), mask);
    let sb = vandq_u32(s, mask);
    let dr = vandq_u32(vshlq_u32(d, sh16), mask);
    let dg = vandq_u32(vshlq_u32(d, sh8), mask);
    let db = vandq_u32(d, mask);
    let round = vdupq_n_u32(127);
    let magic = vdupq_n_u32(32897);
    let div255 = |x: uint32x4_t| vshlq_u32(vmulq_u32(x, magic), sh23);
    let r = vaddq_u32(sr, div255(vaddq_u32(vmulq_u32(dr, inv), round)));
    let g = vaddq_u32(sg, div255(vaddq_u32(vmulq_u32(dg, inv), round)));
    let b = vaddq_u32(sb, div255(vaddq_u32(vmulq_u32(db, inv), round)));
    let out = vorrq_u32(
        vorrq_u32(vshlq_u32(r, left16), vshlq_u32(g, left8)),
        vorrq_u32(b, vdupq_n_u32(0xFF00_0000)),
    );
    vst1q_u32(dst, out);
}

fn blend_slice(dst: &mut [u32], src: &[u32]) {
    let n = dst.len().min(src.len());
    let mut i = 0;
    #[cfg(target_arch = "aarch64")]
    unsafe {
        while i + 4 <= n {
            blend4_neon(dst.as_mut_ptr().add(i), src.as_ptr().add(i));
            i += 4;
        }
    }
    while i < n {
        blend_px(&mut dst[i], src[i]);
        i += 1;
    }
}

/// Source-over `src` onto `dst`, in place.
fn over(dst: &mut u32, src: u32) {
    let sa = src >> 24;
    if sa == 0 {
        return;
    }
    if sa == 255 {
        *dst = src;
        return;
    }
    let da = 255 - sa;
    let r = (((src >> 16) & 0xff) * sa + ((*dst >> 16) & 0xff) * da) / 255;
    let g = (((src >> 8) & 0xff) * sa + ((*dst >> 8) & 0xff) * da) / 255;
    let b = ((src & 0xff) * sa + (*dst & 0xff) * da) / 255;
    *dst = 0xFF00_0000 | (r << 16) | (g << 8) | b;
}

/// Source-over that also composites the alpha channel. `over` is for the
/// final opaque canvas; composites that stay transparent (the DE overlay)
/// need this one, or the second layer turns the whole image opaque.
fn over_alpha(dst: &mut u32, src: u32) {
    let sa = src >> 24;
    if sa == 0 {
        return;
    }
    if sa == 255 {
        *dst = src;
        return;
    }
    let da = *dst >> 24;
    let inv = 255 - sa;
    let ao = sa + da * inv / 255;
    if ao == 0 {
        *dst = 0;
        return;
    }
    let mix = |s: u32, d: u32| ((s * sa + d * da * inv / 255) / ao).min(255);
    let r = mix((src >> 16) & 0xff, (*dst >> 16) & 0xff);
    let g = mix((src >> 8) & 0xff, (*dst >> 8) & 0xff);
    let b = mix(src & 0xff, *dst & 0xff);
    *dst = (ao << 24) | (r << 16) | (g << 8) | b;
}

/// Straight-alpha composite of the layer choices (0xAARRGGBB); the hardware
/// layer uploads this as-is.
fn load_composite(names: &[String; 2], w: u32, h: u32) -> Option<Vec<u32>> {
    let mut iter = names.iter().filter(|n| !n.is_empty());
    let first = iter.next()?;
    let mut img = load_overlay_png(first, w, h)?;
    for name in iter {
        let Some(l) = load_overlay_png(name, w, h) else {
            continue;
        };
        if l.px.len() != img.px.len() {
            continue;
        }
        for (dst, &src) in img.px.iter_mut().zip(l.px.iter()) {
            over_alpha(dst, src);
        }
    }
    Some(img.px)
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

/// The bezel's screen region: the bounding box of its non-opaque pixels.
fn screen_area(px: &[u32], w: u32, h: u32) -> Option<(i32, i32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, -1i32, -1i32);
    for y in 0..h as usize {
        for x in 0..w as usize {
            if px[y * w as usize + x] >> 24 < 240 {
                x0 = x0.min(x as u32);
                y0 = y0.min(y as u32);
                x1 = x1.max(x as i32);
                y1 = y1.max(y as i32);
            }
        }
    }
    let (aw, ah) = ((x1 - x0 as i32 + 1).max(0), (y1 - y0 as i32 + 1).max(0));
    if aw < (w / 4) as i32 || ah < (h / 4) as i32 {
        return None;
    }
    Some((x0 as i32, y0 as i32, aw as u32, ah as u32))
}

/// Source-over a straight-alpha composite onto the canvas (screenshots when
/// the overlay lives in a DE layer instead).
fn blend_straight(cv: &mut Canvas, px: Option<&[u32]>) {
    let Some(px) = px else { return };
    if px.len() != cv.px.len() {
        return;
    }
    for (dst, &src) in cv.px.iter_mut().zip(px.iter()) {
        over(dst, src);
    }
}

/// The overlay DE layer's image: opaque backplate with a transparent hole at
/// the game rect, art on top. The backplate hides the fb layer underneath
/// and fills any gap between the game image and the bezel's screen.
#[cfg(target_os = "linux")]
fn upload_overlay(
    layer: &mut Option<crate::disp2::ImageLayer>,
    art: Option<&[u32]>,
    cw: usize,
    ch: usize,
    rect: (i32, i32, u32, u32),
) {
    let Some(layer) = layer.as_mut() else { return };
    let mut px = vec![0xFF03_0303u32; cw * ch];
    for y in rect.1.max(0)..(rect.1 + rect.3 as i32).min(ch as i32) {
        for x in rect.0.max(0)..(rect.0 + rect.2 as i32).min(cw as i32) {
            px[y as usize * cw + x as usize] = 0;
        }
    }
    if let Some(art) = art {
        for (d, &s) in px.iter_mut().zip(art.iter()) {
            over(d, s);
        }
    }
    layer.upload(&px);
}

/// The bezel's screen region, from its PNG alpha (None = full canvas).
fn load_bezel_area(names: &[String; 2], w: u32, h: u32) -> Option<(i32, i32, u32, u32)> {
    if names[1].is_empty() {
        return None;
    }
    let img = load_overlay_png(&names[1], w, h)?;
    screen_area(&img.px, w, h)
}

/// Software fallback: premultiply for the frame blend (RGB carries src*a,
/// the top byte is 255-a, so per frame it is one multiply per channel
/// instead of two; 0xFF00_0000 marks a fully transparent pixel).
fn premultiplied(px: &[u32]) -> Overlay {
    let mut px = px.to_vec();
    let mut any = false;
    for p in &mut px {
        let a = *p >> 24;
        if a == 0 {
            *p = 0xFF00_0000;
            continue;
        }
        any = true;
        let inv = 255 - a;
        let r = ((*p >> 16) & 0xff) * a / 255;
        let g = ((*p >> 8) & 0xff) * a / 255;
        let b = (*p & 0xff) * a / 255;
        *p = (inv << 24) | (r << 16) | (g << 8) | b;
    }
    let spans = transparent_spans(&px);
    Overlay { px, spans, any }
}

/// Blend the pre-composited overlay over the finished canvas.
fn apply_overlays(cv: &mut Canvas, overlay: &Option<Overlay>) {
    let Some(o) = overlay else { return };
    if !o.any || o.px.len() != cv.px.len() {
        return;
    }
    match &o.spans {
        Some(spans) => {
            for &(s, e) in spans {
                let (s, e) = (s as usize, e as usize);
                blend_slice(&mut cv.px[s..e], &o.px[s..e]);
            }
        }
        None => {
            let mid = (cv.px.len() / 2) & !3;
            if mid >= 4096 {
                let (dst_a, dst_b) = cv.px.split_at_mut(mid);
                let (src_a, src_b) = o.px.split_at(mid);
                std::thread::scope(|scope| {
                    scope.spawn(|| blend_slice(dst_b, src_b));
                    blend_slice(dst_a, src_a);
                });
            } else {
                blend_slice(&mut cv.px, &o.px);
            }
        }
    }
}

/// Per-system overlay choices from Config/magi.json ("" = off). A missing
/// layer uses the shipped default; the legacy global keys seed the first
/// launch, and an explicit empty string turns it off. NGP/NGPC share one
/// system, so the ROM extension picks the bezel.
fn default_overlays(system: &str, rom: &str, wide: bool) -> [&'static str; 2] {
    let crt = if wide {
        "CRT/Perfect_CRT-720x480.png"
    } else {
        "CRT/Perfect_CRT.png"
    };
    let crt240 = if wide {
        "CRT/Perfect_CRT-240p-720x480.png"
    } else {
        "CRT/Perfect_CRT-240p.png"
    };
    let crt240_noframe = if wide {
        "CRT/Perfect_CRT-240p(noframe)-720x480.png"
    } else {
        "CRT/Perfect_CRT-240p(noframe).png"
    };
    match system {
        "psx" => [crt, ""],
        "snes" | "pce" | "nes" | "neocd" => [crt240, ""],
        "genesis" => [crt240_noframe, ""],
        "gba" => {
            if wide {
                // 720x480 is exactly 3x GBA; a bezel would only shrink it.
                ["", ""]
            } else {
                ["", "GBA/mugwomp93/Perfect_GBA.png"]
            }
        }
        "gbc" => [
            "",
            if wide {
                "GBC/mugwomp93/Perfect_GBC-720x480.png"
            } else {
                "GBC/mugwomp93/Perfect_GBC.png"
            },
        ],
        "gb" => [
            "",
            if wide {
                "GB/mugwomp93/Perfect_GBP-EX-720x480.png"
            } else {
                "GB/mugwomp93/Perfect_GBP-EX.png"
            },
        ],
        "ngp" => {
            // NGP and NGPC share one catalogue entry and folder.
            let ngp = rom.to_ascii_lowercase().ends_with(".ngp");
            match (wide, ngp) {
                (true, true) => ["", "NGP/mugwomp93/ngp-720x480.png"],
                (true, false) => ["", "NGPC/mugwomp93/ngpc-720x480.png"],
                (false, true) => ["", "NGP/mugwomp93/ngp-640x480.png"],
                (false, false) => ["", "NGPC/mugwomp93/ngpc-640x480.png"],
            }
        }
        _ => ["", ""],
    }
}

fn overlay_prefs_from(v: &serde_json::Value, system: &str, rom: &str, wide: bool) -> [String; 2] {
    let has_map = v.get("overlays").is_some_and(|o| o.is_object());
    let per = v.get("overlays").and_then(|o| o.get(system));
    let get = |layer: usize, key: &str| -> String {
        let explicit = if has_map {
            per.and_then(|p| p.get(key)).and_then(|x| x.as_str())
        } else {
            v.get(format!("overlay_{key}")).and_then(|x| x.as_str())
        };
        match explicit {
            Some(value) => value.to_string(),
            None => default_overlays(system, rom, wide)[layer].to_string(),
        }
    };
    [get(0, "crt"), get(1, "bezel")]
}

fn load_overlay_prefs(system: &str, rom: &str, wide: bool) -> [String; 2] {
    let v: serde_json::Value = std::fs::read_to_string(runtime_config_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    overlay_prefs_from(&v, system, rom, wide)
}

fn save_overlay_prefs(system: &str, names: &[String; 2]) {
    let path = Path::new(runtime_config_path());
    let mut v: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if !v.get("overlays").is_some_and(|o| o.is_object()) {
        v["overlays"] = serde_json::json!({});
    }
    v["overlays"][system] = serde_json::json!({
        "crt": names[0],
        "bezel": names[1],
    });
    // Keep the old top-level keys current for older builds and downgrades.
    v["overlay_crt"] = serde_json::json!(names[0]);
    v["overlay_bezel"] = serde_json::json!(names[1]);
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::create_dir_all("Config");
        let _ = util::write_file_atomic(path, text.as_bytes());
    }
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
        "pce" => &[
            "syscard3.pce",
            "syscard3u.pce",
            "syscard2.pce",
            "syscard2u.pce",
            "syscard1.pce",
            "gexpress.pce",
        ],
        _ => &[],
    };
    if names.is_empty() && system_id != "neocd" {
        return;
    }
    let mut found: Vec<String> = names
        .iter()
        .filter(|name| dir.join(name).is_file())
        .map(|name| (*name).to_string())
        .collect();
    if system_id == "neocd" {
        let sub = dir.join("neocd");
        if let Ok(entries) = std::fs::read_dir(&sub) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && path.extension().and_then(|s| s.to_str()).is_some_and(|e| {
                        e.eq_ignore_ascii_case("bin") || e.eq_ignore_ascii_case("rom")
                    })
                {
                    found.push(format!("neocd/{}", entry.file_name().to_string_lossy()));
                }
            }
        }
    }
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
    #[cfg(target_os = "linux")]
    util::hdmi_init();
    #[cfg(target_os = "linux")]
    let _governor = GovernorGuard::new(needs_performance(system_id));

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
        bits: 0,
        left: (0, 0),
        right: (0, 0),
        audio: Vec::new(),
        save_dir: CString::new(save_dir.as_str()).unwrap(),
        system_dir: CString::new(system_dir.as_str()).unwrap(),
        shutdown: false,
        sram_ptr: std::ptr::null_mut(),
        ser_size: serialize_size,
        ser: serialize,
        unser: unserialize,
        options: Vec::new(),
        opt_key: core_name.to_string(),
        opt_dirty: false,
        saved_opts: load_saved_options(core_name),
        overlay_layers: std::array::from_fn(|_| Vec::new()),
        overlay_dirty: false,
        moto: None,
        rumble_on: false,
        rumble_ok: true,
        disk: None,
        system: system_id.to_string(),
        rom: rom.to_string(),
        netplay: None,
        netplay_linked: false,
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
    util::log(&format!(
        "core: {} {}",
        cstr(info.library_name),
        cstr(info.library_version)
    ));

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

    #[cfg(target_os = "linux")]
    let mut evdev = Evdev::open();
    #[cfg(target_os = "linux")]
    let mut lid = crate::util::LidWatch::new();
    #[cfg(target_os = "linux")]
    let analog_pad = pads.has_stick() || evdev.has_stick();
    #[cfg(not(target_os = "linux"))]
    let analog_pad = pads.has_stick();
    // Beetle PCE only reads the joypad after the frontend selects it for
    // the port; it defaults to no device and otherwise ignores all input.
    if system_id == "pce" {
        unsafe { set_controller(0, RETRO_DEVICE_JOYPAD) };
    }
    if system_id == "psx" {
        // PCSX-ReARMed only accepts subclasses; a plain ANALOG (5) falls
        // through to NONE and disables the pad. Games in DualShock mode
        // expect a stick, so pads without one get the standard controller.
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
    let audio = AlsaOut::start(DEVICE_HZ, bt_audio);
    #[cfg(target_os = "linux")]
    let mut resampler = Resampler::new(sample_rate as f64, DEVICE_HZ as f64);
    #[cfg(target_os = "linux")]
    let mut audio_out: Vec<i16> = Vec::new();

    // Software volume: the mixer stays at 100% and the persisted percentage
    // scales the samples.
    let mut gain_pct: u8 = load_runtime_volume();
    let (ff_steps, ff_sound) = load_runtime_prefs();
    let mut disp = load_display_prefs();
    ctx().rumble_ok = disp.rumble;
    crate::theme::load_user_themes(std::path::Path::new(crate::systems::DIR_THEMES));
    crate::theme::set_theme_index(disp.theme);
    crate::theme::set_bg_style(disp.background);
    crate::theme::set_anim_bg(disp.anim_bg);
    crate::theme::set_glow(disp.glow);
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
    // move under SELECT (SELECT+L2 / SELECT+R2).
    let game_l2_r2 = needs_l2_r2(system_id);
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
    let mut rewind_ok = unsafe { (ctx().ser_size)() > 0 };
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
    let hdmi_at_launch = util::hdmi_active();
    #[cfg(target_os = "linux")]
    let mut scanout_active = false;
    #[cfg(target_os = "linux")]
    let mut fb_hide_warned = false;
    #[cfg(target_os = "linux")]
    let mut ui_was_active = false;
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
        overlay_sel: 0,
        overlay_idx,
        fps: disp.fps_counter,
        sysinfo: disp.sysinfo,
        hold: None,
        disc_sel: 0,
        netplay_page: NetplayPage::Menu,
        netplay_sel: 0,
        netplay_transport: netplay::Transport::Wifi,
        netplay_role: netplay::Role::Host,
        netplay_msg: None,
    };
    let mut shortcuts = Shortcuts::default();
    let mut take_shot = false;
    let mut osd = Osd::new();
    let mut pad = PadState::default();
    let analog_core = system_id == "psx" && analog_pad;
    pad.analog = analog_core;
    let mut last_sram_flush = Instant::now();
    let fps_dt = Duration::from_secs_f64(1.0 / fps);
    let mut deadline = Instant::now() + fps_dt;
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

    util::log("game loop up");
    let play_t0 = Instant::now();

    loop {
        if ctx().shutdown {
            util::log("core requested shutdown");
            break;
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
                            let _ = std::fs::remove_file(auto_state_path(&save_dir, &stem));
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
                            menu.open = true;
                            menu.page = MenuPage::Main;
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
                hud = Some((msg, Instant::now()));
                continue;
            }
            if pad.bits & (1 << retro::SELECT) != 0 {
                let scale_key = e.type_ == ev::EV_KEY
                    && e.value == 1
                    && (e.code == ev::BTN_NORTH || (e.code == ev::BTN_SELECT && !game_l2_r2));
                if scale_key {
                    disp.scale = disp.scale.next();
                    save_display_prefs(&disp);
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
                menu.handle_raw(e, &state_paths);
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
                        menu.open = true;
                        menu.page = MenuPage::Main;
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
                    match save_state_to(&state_paths[menu.slot]) {
                        Some(true) => util::log("state saved (menu hold)"),
                        Some(false) => util::log("state save failed (menu hold)"),
                        None => util::log("state save not supported by core (menu hold)"),
                    }
                    match save_state_to(&auto_state_path(&save_dir, &stem)) {
                        Some(true) => util::log("auto state saved"),
                        Some(false) => util::log("auto state save failed"),
                        None => util::log("auto state not supported by core"),
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
            disp.fps_counter = menu.fps;
            disp.sysinfo = menu.sysinfo;
            save_display_prefs(&disp);
            overlay_art = load_composite(&overlay_names, cw, ch);
            bezel_area = load_bezel_area(&overlay_names, cw, ch);
            overlays = overlay_art.as_deref().map(premultiplied);
            #[cfg(target_os = "linux")]
            {
                if let Some(gpu) = gpu.as_mut() {
                    gpu.set_overlay(overlay_art.as_deref(), cw, ch);
                }
                overlay_rect = None;
            }
        }

        ctx().bits = pad.bits;
        ctx().left = pad.left;
        ctx().right = pad.right;

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
                save_display_prefs(&disp);
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
                && !hdmi_at_launch;
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
            let gpu_menu = gpu.is_some() && !hdmi_at_launch && !ctx().frame.is_null();
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
                    MenuPage::Options => draw_options(menu_cv, &mut fonts, &menu, t),
                    MenuPage::Overlays => {
                        draw_overlay_menu(menu_cv, &mut fonts, &menu, &ctx().overlay_layers, t)
                    }
                    MenuPage::Disc => draw_disc(
                        menu_cv,
                        &mut fonts,
                        menu.disc_sel,
                        menu.msg.as_ref().map(|(m, _)| m.as_str()),
                        disc_count(),
                        disc_index(),
                        t,
                    ),
                    MenuPage::Netplay => draw_netplay(menu_cv, &mut fonts, &menu, t),
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
                    Some(compose_shot(
                        ctx(),
                        disp.scale,
                        game_area,
                        &overlays,
                        &layer,
                        cw,
                        ch,
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
                if !gpu_present(
                    &mut gpu,
                    &layer,
                    ctx.frame,
                    ctx.pitch,
                    fw,
                    fh,
                    ctx.pixfmt,
                    rect,
                    disp.warmth as u8,
                ) {
                    gpu = None;
                    util::log("gpu: presenter failed; software path");
                    crate::theme::apply_warmth(&mut cv);
                    presenter.present(&cv)?;
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
                    unsafe { retro_run() }; // re-render the restored frame
                    ctx().audio.clear(); // no scrub noise
                } else {
                    util::log("rewind: state rejected; buffer cleared");
                    rewind_ring.clear();
                    rewind_bytes = 0;
                    rewind_ok = false;
                }
            }
        } else {
            unsafe { retro_run() };
            if ff {
                // The extra frames run headless: no video (the last frame is
                // the one presented); audio keeps playing only if FF SOUND is on.
                for _ in 1..ff_steps {
                    if !ff_sound {
                        ctx().audio.clear();
                    }
                    unsafe { retro_run() };
                }
                if !ff_sound {
                    ctx().audio.clear();
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
        let gpu_frame = gpu.is_some() && !hdmi_at_launch && !ctx.frame.is_null();
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
                hud = Some((
                    take_screenshot(&compose_shot(
                        ctx, disp.scale, game_area, &overlays, &layer, cw, ch,
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
            let mut samples = std::mem::take(&mut ctx.audio);
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
        ctx.audio.clear();
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
                let ok = gpu_present(
                    &mut gpu,
                    &layer,
                    ctx.frame,
                    ctx.pitch,
                    fw,
                    fh,
                    ctx.pixfmt,
                    rect,
                    disp.warmth as u8,
                );
                if !ok {
                    gpu = None;
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
            #[cfg(not(target_os = "linux"))]
            presenter.present(&cv)?;
        } else {
            presenter.present(&cv)?;
        }
        let pres = t.elapsed();
        t_pres += pres;
        pres_ms = pres_ms * 0.9 + pres.as_secs_f64() * 1000.0;
        let work = started.elapsed();
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
                save_display_prefs(&disp);
            }
        }

        // Frame-clock pacing. When the pan blocks on vsync (~16 ms of
        // present), sleeping to a free-running deadline periodically lands
        // just past a vsync and drops a whole frame (the GBA/GBC stutter).
        // At ~60 fps the present is then the clock; PAL rates still sleep.
        let now = Instant::now();
        let vsync_paced = pres_ms > 8.0 && fps_dt < Duration::from_micros(17_240);
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
            let f = frames.max(1) as f64;
            let ms = |d: Duration| d.as_secs_f64() * 1000.0 / f;
            util::log(&format!(
                "fps {:.1}, frame {:.1} ms (worst {:.1}, wframe {:.1}) | core {:.1} blit {:.1} ovl {:.1} hud {:.1} aud {:.1} pres {:.1} rew {:.1} ring {:.2} late {}",
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
                late_frames
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
    save_display_prefs(&disp);
    ctx().set_rumble(false);
    save_runtime_volume(gain_pct);
    // Hand the mixer back to the user's level: the ports and the shelf play
    // through ALSA directly.
    #[cfg(target_os = "linux")]
    util::set_mixer_volume(gain_pct);
    unsafe {
        unload_game();
        retro_deinit();
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

fn runtime_config_path() -> &'static str {
    "Config/magi.json"
}

fn load_runtime_volume() -> u8 {
    std::fs::read_to_string(runtime_config_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("volume").and_then(|x| x.as_u64()))
        .map(|v| v.min(100) as u8)
        .unwrap_or(100)
}

/// Fast-forward preferences: steps per presented frame and whether audio
/// keeps playing while fast-forwarding.
fn load_runtime_prefs() -> (u32, bool) {
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
fn load_shot_chord_pref() -> bool {
    std::fs::read_to_string(runtime_config_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("shot_chord").and_then(|x| x.as_bool()))
        .unwrap_or(Defaults::SHOT_CHORD)
}

fn save_runtime_volume(pct: u8) {
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
const DOUBLE_TAP: Duration = Duration::from_millis(400);
/// SELECT + d-pad: first repeat after this, then this often.
const COMBO_FIRST_REPEAT: Duration = Duration::from_millis(350);
const COMBO_REPEAT: Duration = Duration::from_millis(110);
/// Brightness and warmth steps, in percent.
const COMBO_STEP: i32 = 5;
/// Analog triggers read as pressed past this (Xbox axes span 0..1023).
const TRIGGER_ON: i32 = 300;

/// How the game image fills the panel.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ScaleMode {
    /// Fractional, aspect preserved (the historic default).
    Fit,
    /// Whole-number pixels, aspect preserved.
    Integer,
    /// Fill the panel, aspect ignored.
    Stretch,
}

impl ScaleMode {
    fn next(self) -> Self {
        match self {
            ScaleMode::Fit => ScaleMode::Integer,
            ScaleMode::Integer => ScaleMode::Stretch,
            ScaleMode::Stretch => ScaleMode::Fit,
        }
    }

    fn label(self) -> &'static str {
        match self {
            ScaleMode::Fit => "SCALE FIT",
            ScaleMode::Integer => "SCALE INTEGER",
            ScaleMode::Stretch => "SCALE STRETCH",
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            ScaleMode::Fit => 0,
            ScaleMode::Integer => 1,
            ScaleMode::Stretch => 2,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => ScaleMode::Integer,
            2 => ScaleMode::Stretch,
            _ => ScaleMode::Fit,
        }
    }
}

/// Per-axis scale factors for a mode; `blit` clamps the result to the panel.
fn scale_factors(mode: ScaleMode, cw: usize, ch: usize, fw: usize, fh: usize) -> (f32, f32) {
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
struct DisplayPrefs {
    theme: u8,
    background: u8,
    anim_bg: bool,
    glow: bool,
    scale: ScaleMode,
    brightness: u32,
    warmth: u32,
    lid_sleep: bool,
    shutdown_min: u32,
    rumble: bool,
    bat_shutdown: u32,
    charge_limit: u32,
    fps_counter: bool,
    sysinfo: bool,
}

fn load_display_prefs() -> DisplayPrefs {
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

fn save_display_prefs(p: &DisplayPrefs) {
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
enum AdjKind {
    Brightness,
    Warmth,
}

struct AdjState {
    kind: AdjKind,
    dir: i32,
    next: Instant,
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

/// Only these core options are surfaced in the UI — matched loosely against
/// the label (cores append things like "(Restart Required)"); everything else
/// stays at the core's default (the owner's list for the handheld cores).
const KEEP_OPTIONS: [&str; 39] = [
    "color correction",
    "color correction mode",
    "dark filter level (%)",
    "interframe blending",
    "emulated hardware",
    "rumble strength",
    "region",
    // GBA (mGBA)
    "game boy model",
    "game boy palette",
    "super game boy borders",
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
    "genesis fm",
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

/// True when the game owns the shoulder triggers and the frontend's
/// rewind/FF live under SELECT instead.
fn needs_l2_r2(system_id: &str) -> bool {
    system_id == "psx"
}

/// Cores that still need the performance governor after GPU presentation.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
/// PS1 and Neo CD keep the performance governor; Genesis and PCE run on
/// schedutil and ramp only when they need to.
fn needs_performance(system_id: &str) -> bool {
    matches!(system_id, "psx" | "neocd")
}

/// D-pad and left-stick state for a Bluetooth/USB pad, combined into the
/// hat events the input pipeline already understands.
#[derive(Default)]
struct PadHat {
    dpad: (i32, i32),
    stick: (i32, i32),
    last: (i32, i32),
}

/// SDL controller events as vendor-space raw events, so the shelf's action
/// table, chords, hold-exit and libretro bits need no second copy.
fn pump_pads(
    pads: &mut crate::pads::Pads,
    events: &mut sdl2::EventPump,
    hat: &mut PadHat,
    stick_as_dpad: bool,
) -> (Vec<RawEvent>, bool) {
    let mut out = Vec::new();
    let mut changed = false;
    let mut removed = false;
    for ev in events.poll_iter() {
        match ev {
            Event::ControllerButtonDown { button, .. } => match button {
                PadButton::DPadUp => hat.dpad.1 = -1,
                PadButton::DPadDown => hat.dpad.1 = 1,
                PadButton::DPadLeft => hat.dpad.0 = -1,
                PadButton::DPadRight => hat.dpad.0 = 1,
                _ => {
                    if let Some(code) = pad_button_code(button) {
                        out.push(pad_key(code, 1));
                    }
                }
            },
            Event::ControllerButtonUp { button, .. } => match button {
                PadButton::DPadUp | PadButton::DPadDown => hat.dpad.1 = 0,
                PadButton::DPadLeft | PadButton::DPadRight => hat.dpad.0 = 0,
                _ => {
                    if let Some(code) = pad_button_code(button) {
                        out.push(pad_key(code, 0));
                    }
                }
            },
            Event::ControllerAxisMotion { axis, value, .. } => match axis {
                PadAxis::TriggerLeft => out.push(pad_axis(ev::ABS_Z, pad_trigger(value))),
                PadAxis::TriggerRight => out.push(pad_axis(ev::ABS_RZ, pad_trigger(value))),
                PadAxis::LeftX => {
                    hat.stick.0 = pad_stick(value);
                    out.push(pad_axis(ev::ABS_X, i32::from(value)));
                }
                PadAxis::LeftY => {
                    hat.stick.1 = pad_stick(value);
                    out.push(pad_axis(ev::ABS_Y, i32::from(value)));
                }
                PadAxis::RightX => out.push(pad_axis(ev::ABS_RX, i32::from(value))),
                PadAxis::RightY => out.push(pad_axis(ev::ABS_RY, i32::from(value))),
            },
            Event::ControllerDeviceAdded { which, .. } => {
                pads.added(which);
                changed = true;
            }
            Event::ControllerDeviceRemoved { which, .. } => {
                pads.removed(which);
                changed = true;
                removed = true;
            }
            _ => {}
        }
    }
    if removed {
        hat.dpad = (0, 0);
        hat.stick = (0, 0);
    }
    let x = if hat.dpad.0 != 0 {
        hat.dpad.0
    } else if stick_as_dpad {
        hat.stick.0
    } else {
        0
    };
    let y = if hat.dpad.1 != 0 {
        hat.dpad.1
    } else if stick_as_dpad {
        hat.stick.1
    } else {
        0
    };
    if (x, y) != hat.last {
        out.push(pad_axis(ev::ABS_HAT0X, x));
        out.push(pad_axis(ev::ABS_HAT0Y, y));
        hat.last = (x, y);
    }
    (out, changed)
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

fn pad_key(code: u16, value: i32) -> RawEvent {
    RawEvent {
        dev: 0,
        type_: ev::EV_KEY,
        code,
        value,
    }
}

fn pad_axis(code: u16, value: i32) -> RawEvent {
    RawEvent {
        dev: 0,
        type_: ev::EV_ABS,
        code,
        value,
    }
}

fn pad_button_code(b: PadButton) -> Option<u16> {
    use ev::*;
    match b {
        PadButton::A => Some(BTN_SOUTH),
        PadButton::B => Some(BTN_EAST),
        PadButton::X => Some(BTN_NORTH),
        PadButton::Y => Some(BTN_C),
        PadButton::LeftShoulder => Some(BTN_WEST),
        PadButton::RightShoulder => Some(BTN_Z),
        PadButton::Back => Some(BTN_TL),
        PadButton::Start => Some(BTN_TR),
        PadButton::Guide => Some(BTN_TL2),
        _ => None,
    }
}

fn pad_stick(value: i16) -> i32 {
    const DEADZONE: i16 = 16_000;
    match value {
        v if v <= -DEADZONE => -1,
        v if v >= DEADZONE => 1,
        _ => 0,
    }
}

/// SDL trigger axes span 0..32767; the pipeline reads 0..1023.
fn pad_trigger(value: i16) -> i32 {
    (value.max(0) as i32 * 1023) / 32767
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

/// Keep holding A this long after the first frame to skip the resume point;
/// a normal launch tap has been released by then.
const AUTO_HOLD: Duration = Duration::from_millis(600);

fn restore_auto_state(path: &Path, hud: &mut Option<(String, Instant)>) {
    match load_state_from(path) {
        Ok(true) => {
            util::log(&format!("auto state restored: {}", path.display()));
            let _ = std::fs::remove_file(path);
            *hud = Some(("STATE RESTORED".into(), Instant::now()));
        }
        Ok(false) => util::log("auto state rejected by core"),
        Err(e) => util::log(&format!("auto state load failed: {e}")),
    }
}

/// Serialize into a file; `None` means the core cannot save at all.
fn save_state_to(path: &Path) -> Option<bool> {
    let size = unsafe { (ctx().ser_size)() };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size];
    let ok = unsafe { (ctx().ser)(buf.as_mut_ptr() as *mut c_void, size) };
    if ok && util::write_file_atomic(path, &buf).is_ok() {
        util::log(&format!("state saved: {}", path.display()));
        Some(true)
    } else {
        Some(false)
    }
}

fn load_state_from(path: &Path) -> std::io::Result<bool> {
    let buf = std::fs::read(path)?;
    Ok(unsafe { (ctx().unser)(buf.as_ptr() as *const c_void, buf.len()) })
}

/// Push a fresh snapshot into the rewind ring, dropping the oldest states
/// once the memory budget is exceeded.
fn push_rewind(ring: &mut VecDeque<Vec<u8>>, bytes: &mut usize) {
    let size = unsafe { (ctx().ser_size)() };
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

#[derive(Default)]
struct PadState {
    bits: u16,
    hat_x: i8,
    hat_y: i8,
    left: (i16, i16),
    right: (i16, i16),
    analog: bool,
}

impl PadState {
    fn apply(&mut self, e: &RawEvent) {
        if e.type_ == ev::EV_ABS {
            match e.code {
                ev::ABS_X => {
                    self.left.0 = e.value as i16;
                    self.stick_bits();
                    return;
                }
                ev::ABS_Y => {
                    self.left.1 = e.value as i16;
                    self.stick_bits();
                    return;
                }
                ev::ABS_RX => {
                    self.right.0 = e.value as i16;
                    return;
                }
                ev::ABS_RY => {
                    self.right.1 = e.value as i16;
                    return;
                }
                ev::ABS_HAT0X => self.hat_x = e.value.clamp(-1, 1) as i8,
                ev::ABS_HAT0Y => self.hat_y = e.value.clamp(-1, 1) as i8,
                ev::ABS_Z => {
                    self.set_bit(retro::L2, e.value > TRIGGER_ON);
                    return;
                }
                ev::ABS_RZ => {
                    self.set_bit(retro::R2, e.value > TRIGGER_ON);
                    return;
                }
                _ => return,
            }
            self.set_bit(retro::LEFT, self.hat_x < 0);
            self.set_bit(retro::RIGHT, self.hat_x > 0);
            self.set_bit(retro::UP, self.hat_y < 0);
            self.set_bit(retro::DOWN, self.hat_y > 0);
            return;
        }
        if e.type_ != ev::EV_KEY {
            return;
        }
        if let Some(bit) = joypad_bit(e.code) {
            self.set_bit(bit, e.value != 0);
            return;
        }
        let dir = match e.code {
            ev::KEY_UP | ev::BTN_DPAD_UP => Some(retro::UP),
            ev::KEY_DOWN | ev::BTN_DPAD_DOWN => Some(retro::DOWN),
            ev::KEY_LEFT | ev::BTN_DPAD_LEFT => Some(retro::LEFT),
            ev::KEY_RIGHT | ev::BTN_DPAD_RIGHT => Some(retro::RIGHT),
            _ => None,
        };
        if let Some(bit) = dir {
            self.set_bit(bit, e.value != 0);
        }
    }

    /// JOYPAD-only cores get the left stick as a d-pad.
    fn stick_bits(&mut self) {
        if self.analog {
            return;
        }
        self.set_bit(retro::LEFT, self.left.0 < -STICK_DEADZONE);
        self.set_bit(retro::RIGHT, self.left.0 > STICK_DEADZONE);
        self.set_bit(retro::UP, self.left.1 < -STICK_DEADZONE);
        self.set_bit(retro::DOWN, self.left.1 > STICK_DEADZONE);
    }

    fn set_bit(&mut self, bit: u16, on: bool) {
        if on {
            self.bits |= 1 << bit;
        } else {
            self.bits &= !(1 << bit);
        }
    }

    fn reset(&mut self) {
        self.bits = 0;
        self.hat_x = 0;
        self.hat_y = 0;
        self.left = (0, 0);
        self.right = (0, 0);
    }
}

#[derive(PartialEq)]
enum MenuPage {
    Main,
    Options,
    Overlays,
    Disc,
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

struct GameMenu {
    open: bool,
    sel: usize,
    msg: Option<(String, Instant)>,
    exit_requested: bool,
    page: MenuPage,
    slot: usize,
    opt_sel: usize,
    opt_top: usize,
    overlay_sel: usize,
    overlay_idx: [usize; 2],
    /// On-screen counters (persisted as display prefs).
    fps: bool,
    sysinfo: bool,
    /// Progress of the held-MENU save-and-exit gesture, when underway.
    hold: Option<f32>,
    /// The pending image on the DISC page.
    disc_sel: usize,
    netplay_page: NetplayPage,
    netplay_sel: usize,
    netplay_transport: netplay::Transport,
    netplay_role: netplay::Role,
    netplay_msg: Option<String>,
}

const OPT_ROWS: usize = 7;

fn menu_items() -> Vec<&'static str> {
    let linked = ctx().netplay_linked;
    let mut v = vec!["RESUME"];
    if !linked {
        v.push("SAVE STATE");
        v.push("LOAD STATE");
    }
    if netplay_supported() {
        v.push("NETPLAY");
    }
    if !linked && ctx().options.iter().any(|o| o.visible) {
        v.push("CORE OPTIONS");
    }
    v.push("OVERLAYS");
    if disc_count() > 1 {
        v.push("SWAP DISC");
    }
    v.push("EXIT GAME");
    v
}

fn netplay_supported() -> bool {
    matches!(ctx().system.as_str(), "gb" | "gbc")
        && ctx().opt_key == "gambatte"
        && ctx()
            .options
            .iter()
            .any(|o| o.key == "gambatte_gb_link_mode")
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

fn cycled(cur: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (((cur as isize + delta) % len as isize + len as isize) % len as isize) as usize
}

impl GameMenu {
    fn handle_raw(&mut self, e: &RawEvent, state_paths: &[std::path::PathBuf]) {
        if e.value == 0 {
            return; // act on presses only; hat directions arrive as ┬▒1
        }
        let up = e.code == ev::KEY_UP
            || e.code == ev::BTN_DPAD_UP
            || (e.type_ == ev::EV_ABS && e.code == ev::ABS_HAT0Y && e.value < 0);
        let down = e.code == ev::KEY_DOWN
            || e.code == ev::BTN_DPAD_DOWN
            || (e.type_ == ev::EV_ABS && e.code == ev::ABS_HAT0Y && e.value > 0);
        let left = e.code == ev::KEY_LEFT
            || e.code == ev::BTN_DPAD_LEFT
            || (e.type_ == ev::EV_ABS && e.code == ev::ABS_HAT0X && e.value < 0);
        let right = e.code == ev::KEY_RIGHT
            || e.code == ev::BTN_DPAD_RIGHT
            || (e.type_ == ev::EV_ABS && e.code == ev::ABS_HAT0X && e.value > 0);
        let a = e.type_ == ev::EV_KEY && e.code == ev::BTN_SOUTH;
        let b = e.type_ == ev::EV_KEY && e.code == ev::BTN_EAST;

        if self.page == MenuPage::Disc {
            let count = disc_count();
            if count < 2 {
                self.page = MenuPage::Main;
                return;
            }
            if up {
                self.disc_sel = cycled(self.disc_sel, count, -1);
            } else if down {
                self.disc_sel = cycled(self.disc_sel, count, 1);
            } else if a {
                self.msg = Some(match disc_swap(self.disc_sel) {
                    true => (
                        format!("DISC {} INSERTED", self.disc_sel + 1),
                        Instant::now(),
                    ),
                    false => ("DISC SWAP FAILED".into(), Instant::now()),
                });
            } else if b {
                self.page = MenuPage::Main;
            }
            return;
        }

        if self.page == MenuPage::Options {
            let count = visible_options(ctx()).len();
            if up {
                self.opt_sel = cycled(self.opt_sel, count, -1);
                self.scroll_options(count);
            } else if down {
                self.opt_sel = cycled(self.opt_sel, count, 1);
                self.scroll_options(count);
            } else if left {
                self.change_option(-1);
            } else if right || a {
                self.change_option(1);
            } else if b {
                self.page = MenuPage::Main;
            }
            return;
        }

        if self.page == MenuPage::Overlays {
            const ROWS: usize = 4;
            if up {
                self.overlay_sel = (self.overlay_sel + ROWS - 1) % ROWS;
            } else if down {
                self.overlay_sel = (self.overlay_sel + 1) % ROWS;
            } else if left || right || a {
                if self.overlay_sel == 0 {
                    self.fps = !self.fps;
                } else if self.overlay_sel == 1 {
                    self.sysinfo = !self.sysinfo;
                } else {
                    let layer = self.overlay_sel - 2;
                    let n = ctx().overlay_layers[layer].len() as i32 + 1;
                    let delta = if left { -1 } else { 1 };
                    let cur = &mut self.overlay_idx[layer];
                    *cur = (((*cur as i32 + delta) % n) + n) as usize % n as usize;
                }
                ctx().overlay_dirty = true;
            } else if b {
                self.page = MenuPage::Main;
            }
            return;
        }

        if self.page == MenuPage::Netplay {
            self.handle_netplay(up, down, a, b);
            return;
        }

        let items = menu_items();
        if self.sel >= items.len() {
            self.sel = 0;
        }
        if up {
            self.sel = (self.sel + items.len() - 1) % items.len();
        } else if down {
            self.sel = (self.sel + 1) % items.len();
        } else if left {
            self.slot = cycled(self.slot, STATE_SLOTS, -1);
        } else if right {
            self.slot = cycled(self.slot, STATE_SLOTS, 1);
        } else if a {
            self.invoke(state_paths);
        } else if b {
            self.page = MenuPage::Main;
            self.open = false;
        }
    }

    fn handle_netplay(&mut self, up: bool, down: bool, a: bool, b: bool) {
        if ctx().netplay.is_none()
            && !matches!(
                self.netplay_page,
                NetplayPage::Menu | NetplayPage::HostTransport | NetplayPage::JoinTransport
            )
        {
            self.netplay_page = NetplayPage::Menu;
            self.netplay_sel = 0;
        }

        match self.netplay_page {
            NetplayPage::Menu => {
                if up {
                    self.netplay_sel = cycled(self.netplay_sel, 3, -1);
                } else if down {
                    self.netplay_sel = cycled(self.netplay_sel, 3, 1);
                } else if a {
                    self.netplay_msg = None;
                    match self.netplay_sel {
                        0 => {
                            self.netplay_page = NetplayPage::HostTransport;
                            self.netplay_sel = 0;
                        }
                        1 => {
                            self.netplay_page = NetplayPage::JoinTransport;
                            self.netplay_sel = 0;
                        }
                        _ => self.page = MenuPage::Main,
                    }
                } else if b {
                    self.page = MenuPage::Main;
                }
            }
            NetplayPage::HostTransport => {
                if up {
                    self.netplay_sel = cycled(self.netplay_sel, 3, -1);
                } else if down {
                    self.netplay_sel = cycled(self.netplay_sel, 3, 1);
                } else if a {
                    match self.netplay_sel {
                        0 => self.start_host(netplay::Transport::Hotspot),
                        1 => self.start_host(netplay::Transport::Wifi),
                        _ => self.netplay_page = NetplayPage::Menu,
                    }
                } else if b {
                    self.netplay_page = NetplayPage::Menu;
                }
            }
            NetplayPage::JoinTransport => {
                if up {
                    self.netplay_sel = cycled(self.netplay_sel, 3, -1);
                } else if down {
                    self.netplay_sel = cycled(self.netplay_sel, 3, 1);
                } else if a {
                    match self.netplay_sel {
                        0 => self.start_join(netplay::Transport::Hotspot),
                        1 => self.start_join(netplay::Transport::Wifi),
                        _ => self.netplay_page = NetplayPage::Menu,
                    }
                } else if b {
                    self.netplay_page = NetplayPage::Menu;
                }
            }
            NetplayPage::HostWait => {
                if b {
                    netplay_stop();
                    self.netplay_page = NetplayPage::Menu;
                    self.netplay_sel = 0;
                }
            }
            NetplayPage::JoinScan => {
                let status = ctx().netplay.as_ref().map(|s| s.status());
                match status {
                    Some(netplay::Status::Found) => {
                        let hosts = ctx()
                            .netplay
                            .as_ref()
                            .map(|s| s.hosts())
                            .unwrap_or_default();
                        if hosts.is_empty() {
                            if a {
                                self.start_join(self.netplay_transport);
                            } else if b {
                                netplay_stop();
                                self.netplay_page = NetplayPage::Menu;
                                self.netplay_sel = 0;
                            }
                        } else {
                            let n = hosts.len().min(5);
                            if up {
                                self.netplay_sel = cycled(self.netplay_sel, n, -1);
                            } else if down {
                                self.netplay_sel = cycled(self.netplay_sel, n, 1);
                            } else if a {
                                self.connect_selected();
                            } else if b {
                                netplay_stop();
                                self.netplay_page = NetplayPage::Menu;
                                self.netplay_sel = 0;
                            }
                        }
                    }
                    Some(netplay::Status::Error(_)) | Some(netplay::Status::Lost) => {
                        if a {
                            self.retry_transport();
                        } else if b {
                            netplay_stop();
                            self.netplay_page = NetplayPage::Menu;
                            self.netplay_sel = 0;
                        }
                    }
                    Some(netplay::Status::Connecting) => {
                        if b {
                            netplay_stop();
                            self.netplay_page = NetplayPage::Menu;
                            self.netplay_sel = 0;
                        }
                    }
                    _ => {
                        if b {
                            netplay_stop();
                            self.netplay_page = NetplayPage::Menu;
                            self.netplay_sel = 0;
                        }
                    }
                }
            }
            NetplayPage::Linked => {
                if up {
                    self.netplay_sel = cycled(self.netplay_sel, 2, -1);
                } else if down {
                    self.netplay_sel = cycled(self.netplay_sel, 2, 1);
                } else if a {
                    if self.netplay_sel == 0 {
                        netplay_stop();
                        self.netplay_page = NetplayPage::Menu;
                        self.netplay_sel = 0;
                    } else {
                        self.page = MenuPage::Main;
                    }
                } else if b {
                    self.page = MenuPage::Main;
                }
            }
            NetplayPage::Error => {
                if a {
                    self.retry_transport();
                } else if b {
                    netplay_stop();
                    self.netplay_page = NetplayPage::Menu;
                    self.netplay_sel = 0;
                }
            }
        }
    }

    fn start_host(&mut self, transport: netplay::Transport) {
        netplay_stop();
        let game = netplay::Game::from_rom(&ctx().system, &ctx().rom);
        ctx().netplay = Some(netplay::Session::host(transport, game));
        self.netplay_transport = transport;
        self.netplay_role = netplay::Role::Host;
        self.netplay_page = NetplayPage::HostWait;
        self.netplay_sel = 0;
        self.netplay_msg = None;
    }

    fn start_join(&mut self, transport: netplay::Transport) {
        netplay_stop();
        let game = netplay::Game::from_rom(&ctx().system, &ctx().rom);
        ctx().netplay = Some(netplay::Session::join(transport, game));
        self.netplay_transport = transport;
        self.netplay_role = netplay::Role::Join;
        self.netplay_page = NetplayPage::JoinScan;
        self.netplay_sel = 0;
        self.netplay_msg = None;
    }

    fn connect_selected(&mut self) {
        let Some(session) = ctx().netplay.as_mut() else {
            return;
        };
        let hosts = session.hosts();
        if hosts.is_empty() {
            return;
        }
        let n = hosts.len().min(5);
        if self.netplay_sel >= n {
            self.netplay_sel = 0;
        }
        session.connect(hosts[self.netplay_sel].clone());
        self.netplay_msg = None;
    }

    fn retry_transport(&mut self) {
        netplay_stop();
        self.netplay_page = match self.netplay_role {
            netplay::Role::Host => NetplayPage::HostTransport,
            netplay::Role::Join => NetplayPage::JoinTransport,
        };
        self.netplay_sel = 0;
        self.netplay_msg = None;
    }

    fn scroll_options(&mut self, count: usize) {
        if self.opt_sel < self.opt_top {
            self.opt_top = self.opt_sel;
        }
        if self.opt_sel >= self.opt_top + OPT_ROWS {
            self.opt_top = self.opt_sel + 1 - OPT_ROWS;
        }
        let _ = count;
    }

    fn change_option(&mut self, delta: isize) {
        let ctx = ctx();
        let ids = visible_options(ctx);
        if self.opt_sel >= ids.len() {
            return;
        }
        let o = &mut ctx.options[ids[self.opt_sel]];
        o.chosen = cycled(o.chosen, o.values.len(), delta);
        ctx.opt_dirty = true; // cores that support live updates re-read it
        save_core_options(ctx);
    }

    fn invoke(&mut self, state_paths: &[std::path::PathBuf]) {
        let state_path = &state_paths[self.slot];
        match menu_items()[self.sel] {
            "RESUME" => self.open = false,
            "SAVE STATE" => {
                let saved = save_state_to(state_path);
                if saved == Some(true) {
                    // An explicit save is also the resume point for the next
                    // launch; holding A at launch still skips it.
                    let auto = state_path.with_extension("stateauto");
                    match save_state_to(&auto) {
                        Some(true) => util::log("auto state saved (menu)"),
                        Some(false) => util::log("auto state save failed (menu)"),
                        None => {}
                    }
                }
                self.msg = Some(match saved {
                    Some(true) => ("STATE SAVED".into(), Instant::now()),
                    Some(false) => ("SAVE FAILED".into(), Instant::now()),
                    None => ("SAVES NOT SUPPORTED".into(), Instant::now()),
                });
            }
            "LOAD STATE" => match load_state_from(state_path) {
                Ok(true) => self.msg = Some(("STATE LOADED".into(), Instant::now())),
                Ok(false) => self.msg = Some(("LOAD FAILED".into(), Instant::now())),
                Err(_) => self.msg = Some(("NO STATE FILE".into(), Instant::now())),
            },
            "CORE OPTIONS" => {
                self.page = MenuPage::Options;
                self.opt_sel = 0;
                self.opt_top = 0;
            }
            "OVERLAYS" => {
                self.page = MenuPage::Overlays;
                self.overlay_sel = 0;
            }
            "SWAP DISC" => {
                let count = disc_count();
                self.page = MenuPage::Disc;
                self.disc_sel = disc_index().min(count.saturating_sub(1));
            }
            "NETPLAY" => {
                self.page = MenuPage::Netplay;
                self.netplay_sel = 0;
                self.netplay_msg = None;
                self.netplay_page = match ctx().netplay.as_ref().map(|s| s.status()) {
                    Some(netplay::Status::Linked) => NetplayPage::Linked,
                    Some(netplay::Status::Waiting) | Some(netplay::Status::Starting) => {
                        NetplayPage::HostWait
                    }
                    Some(netplay::Status::Scanning)
                    | Some(netplay::Status::Found)
                    | Some(netplay::Status::Connecting) => NetplayPage::JoinScan,
                    Some(netplay::Status::Lost) => {
                        self.netplay_msg = Some("THE OTHER PLAYER LEFT".into());
                        NetplayPage::Error
                    }
                    Some(netplay::Status::Error(e)) => {
                        self.netplay_msg = Some(e);
                        NetplayPage::Error
                    }
                    _ => NetplayPage::Menu,
                };
                if let Some(session) = ctx().netplay.as_ref() {
                    self.netplay_role = session.role();
                    self.netplay_transport = session.transport();
                }
            }
            _ => {
                netplay_stop();
                self.exit_requested = true;
            }
        }
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
fn draw_menu(cv: &mut Canvas, fonts: &mut Fonts, menu: &GameMenu, rom: &str, t: f32) {
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let items = menu_items();
    let ww = p(400.0).min(w - p(24.0));
    let header = p(104.0);
    let footer = if menu.msg.is_some() { p(84.0) } else { p(44.0) };
    let pitch = ((h - p(12.0) - header - footer) / items.len() as i32)
        .min(p(40.0))
        .max(p(26.0));
    let hh = header + items.len() as i32 * pitch + footer;
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 216);
    crate::theme::glow_border(
        cv,
        x0,
        y0,
        ww as u32,
        hh as u32,
        pal().border,
        pal().glow,
        t,
        0.0,
    );
    let title = "MAGI // PAUSED";
    fonts.draw_center(cv, w / 2, y0 + p(26.0), title, 16.0, pal().title);
    let rom_title = Path::new(rom)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(56.0),
        clip(rom_title, 40).as_str(),
        10.0,
        pal().text_faint,
    );
    let discs = disc_count();
    let header_line = if discs > 1 {
        format!(
            "SLOT {}/{}   DISC {}/{}",
            menu.slot + 1,
            STATE_SLOTS,
            disc_index().min(discs - 1) + 1,
            discs
        )
    } else {
        format!("SLOT {}/{}", menu.slot + 1, STATE_SLOTS)
    };
    fonts.draw_center(cv, w / 2, y0 + p(80.0), &header_line, 11.0, pal().text_dim);

    for (i, item) in items.iter().enumerate() {
        let c = y0 + header + i as i32 * pitch + pitch / 2;
        let y = c + p(5.5);
        let bh = (pitch - p(10.0)).min(p(36.0));
        if i == menu.sel {
            cv.fill_rect(
                x0 + p(28.0),
                c - bh / 2,
                (ww - p(56.0)) as u32,
                bh as u32,
                pal().bg3,
            );
            crate::theme::glow_border(
                cv,
                x0 + p(28.0),
                c - bh / 2,
                (ww - p(56.0)) as u32,
                bh as u32,
                pal().glow,
                crate::theme::mix(pal().glow, pal().accent, 0.35),
                t,
                0.4,
            );
            fonts.draw_center(cv, w / 2, y, item, 15.0, pal().accent);
        } else {
            fonts.draw_center(cv, w / 2, y, item, 15.0, pal().text_dim);
        }
    }

    if let Some((msg, _)) = &menu.msg {
        fonts.draw_center(cv, w / 2, y0 + hh - p(66.0), msg, 10.0, pal().signal);
    }
    match menu.hold {
        Some(f) => {
            let bw = p(260.0);
            let x = (w - bw) / 2;
            let y = y0 + hh - p(24.0);
            cv.rect_outline(x, y, bw as u32, p(10.0) as u32, pal().border);
            let fw = ((bw - 2) as f32 * f.clamp(0.0, 1.0)) as u32;
            cv.fill_rect(
                x + 1,
                y + 1,
                fw,
                (p(10.0) as u32).saturating_sub(2),
                pal().signal,
            );
            fonts.draw_center(
                cv,
                w / 2,
                y0 + hh - p(34.0),
                "HOLD MENU: SAVE + EXIT",
                10.0,
                pal().signal,
            );
        }
        None => {
            fonts.draw_center(
                cv,
                w / 2,
                y0 + hh - p(26.0),
                "MENU CLOSE   A SELECT   B RESUME   L/R SLOT",
                9.0,
                pal().text_faint,
            );
        }
    }
}

/// The netplay page family. The session lives in `Ctx`; this only draws the
/// current step.
fn draw_netplay(cv: &mut Canvas, fonts: &mut Fonts, menu: &GameMenu, t: f32) {
    let session = ctx().netplay.as_ref();
    let status = session.map(|s| s.status());
    match menu.netplay_page {
        NetplayPage::Menu => netplay_panel(
            cv,
            fonts,
            "MAGI // NETPLAY",
            &clip(&game_title(), 38),
            &["HOST GAME", "JOIN GAME", "BACK"],
            menu.netplay_sel,
            "A SELECT   B BACK",
            t,
        ),
        NetplayPage::HostTransport => netplay_panel(
            cv,
            fonts,
            "HOST GAME",
            "PICK A CONNECTION",
            &["HOTSPOT", "WIFI", "BACK"],
            menu.netplay_sel,
            "A SELECT   B BACK",
            t,
        ),
        NetplayPage::JoinTransport => netplay_panel(
            cv,
            fonts,
            "JOIN GAME",
            "PICK A CONNECTION",
            &["HOTSPOT", "WIFI", "BACK"],
            menu.netplay_sel,
            "A SELECT   B BACK",
            t,
        ),
        NetplayPage::HostWait => draw_netplay_wait(cv, fonts, menu, t),
        NetplayPage::JoinScan => draw_netplay_scan(cv, fonts, menu, status, t),
        NetplayPage::Linked => netplay_panel(
            cv,
            fonts,
            "MAGI // NETPLAY",
            &format!("LINKED {}", session.map(|s| s.code()).unwrap_or("----")),
            &["END NETPLAY", "BACK"],
            menu.netplay_sel,
            "A SELECT   B BACK",
            t,
        ),
        NetplayPage::Error => draw_netplay_error(cv, fonts, menu, t),
    }
}

fn game_title() -> String {
    Path::new(&ctx().rom)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("GAME")
        .to_string()
}

fn netplay_panel(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    title: &str,
    subtitle: &str,
    items: &[&str],
    sel: usize,
    footer: &str,
    t: f32,
) {
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let ww = p(400.0).min(w - p(24.0));
    let header = p(104.0);
    let pitch = ((h - p(12.0) - header - p(44.0)) / items.len().max(1) as i32)
        .min(p(40.0))
        .max(p(26.0));
    let hh = header + items.len().max(1) as i32 * pitch + p(44.0);
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 216);
    crate::theme::glow_border(
        cv,
        x0,
        y0,
        ww as u32,
        hh as u32,
        pal().border,
        pal().glow,
        t,
        0.0,
    );
    fonts.draw_center(cv, w / 2, y0 + p(26.0), title, 16.0, pal().title);
    fonts.draw_center(cv, w / 2, y0 + p(56.0), subtitle, 10.0, pal().text_faint);
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(80.0),
        "GAME BOY NETPLAY",
        9.0,
        pal().text_dim,
    );

    for (i, item) in items.iter().enumerate() {
        let c = y0 + header + i as i32 * pitch + pitch / 2;
        let y = c + p(5.5);
        let bh = (pitch - p(10.0)).min(p(36.0));
        let picked = i == sel.min(items.len().saturating_sub(1));
        if picked {
            cv.fill_rect(
                x0 + p(28.0),
                c - bh / 2,
                (ww - p(56.0)) as u32,
                bh as u32,
                pal().bg3,
            );
            crate::theme::glow_border(
                cv,
                x0 + p(28.0),
                c - bh / 2,
                (ww - p(56.0)) as u32,
                bh as u32,
                pal().glow,
                crate::theme::mix(pal().glow, pal().accent, 0.35),
                t,
                0.4,
            );
        }
        fonts.draw_center(
            cv,
            w / 2,
            y,
            item,
            15.0,
            if picked { pal().accent } else { pal().text_dim },
        );
    }
    fonts.draw_center(cv, w / 2, y0 + hh - p(26.0), footer, 9.0, pal().text_faint);
}

fn draw_netplay_wait(cv: &mut Canvas, fonts: &mut Fonts, menu: &GameMenu, t: f32) {
    let session = ctx().netplay.as_ref();
    let code = session.map(|s| s.code()).unwrap_or("----");
    let transport = session
        .map(|s| s.transport())
        .unwrap_or(menu.netplay_transport);
    let status = session
        .map(|s| s.status())
        .unwrap_or(netplay::Status::Stopped);
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let ww = p(400.0);
    let hh = p(300.0);
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 216);
    crate::theme::glow_border(
        cv,
        x0,
        y0,
        ww as u32,
        hh as u32,
        pal().border,
        pal().glow,
        t,
        0.0,
    );
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(26.0),
        "MAGI // NETPLAY",
        16.0,
        pal().title,
    );
    fonts.draw_center(cv, w / 2, y0 + p(56.0), "HOST GAME", 10.0, pal().text_faint);
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(80.0),
        &format!("{} CONNECTION", transport.label()),
        9.0,
        pal().text_dim,
    );

    let pill_w = p(190.0);
    let pill_h = p(62.0);
    let px = w / 2 - pill_w / 2;
    let py = y0 + p(104.0);
    cv.fill_rect(px, py, pill_w as u32, pill_h as u32, pal().bg3);
    crate::theme::glow_border(
        cv,
        px,
        py,
        pill_w as u32,
        pill_h as u32,
        pal().glow,
        crate::theme::mix(pal().glow, pal().accent, 0.35),
        t,
        0.4,
    );
    fonts.draw_center(cv, w / 2, py + p(42.0), code, 34.0, pal().accent);
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(192.0),
        "SELECT THIS CODE ON THE OTHER DEVICE",
        10.0,
        pal().text_dim,
    );

    let line = match &status {
        netplay::Status::Starting => format!("STARTING{}", netplay_dots(t)),
        netplay::Status::Waiting => format!("WAITING FOR CONNECTION{}", netplay_dots(t)),
        netplay::Status::Linked => "LINKED".into(),
        netplay::Status::Lost => "THE OTHER PLAYER LEFT".into(),
        netplay::Status::Error(e) => clip(e, 44),
        _ => "STOPPED".into(),
    };
    fonts.draw_center(cv, w / 2, y0 + p(226.0), &line, 11.0, pal().signal);
    let hint = match transport {
        netplay::Transport::Hotspot => format!("HOTSPOT MAGI-{code}"),
        netplay::Transport::Wifi => "SAME WI-FI NETWORK".into(),
    };
    fonts.draw_center(cv, w / 2, y0 + p(256.0), &hint, 9.0, pal().text_faint);
    fonts.draw_center(
        cv,
        w / 2,
        y0 + hh - p(22.0),
        "B CANCEL",
        9.0,
        pal().text_faint,
    );
}

fn draw_netplay_scan(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    menu: &GameMenu,
    status: Option<netplay::Status>,
    t: f32,
) {
    match &status {
        Some(netplay::Status::Error(e)) => {
            netplay_message(
                cv,
                fonts,
                "LINK FAILED",
                &clip(e, 44),
                "A RETRY   B BACK",
                t,
            );
        }
        Some(netplay::Status::Lost) => netplay_message(
            cv,
            fonts,
            "LINK ENDED",
            "THE OTHER PLAYER LEFT",
            "A RETRY   B BACK",
            t,
        ),
        Some(netplay::Status::Connecting) => {
            let code = ctx().netplay.as_ref().map(|s| s.code()).unwrap_or("----");
            netplay_message(
                cv,
                fonts,
                "JOIN GAME",
                &format!("CONNECTING TO {code}{}", netplay_dots(t)),
                "B CANCEL",
                t,
            );
        }
        Some(netplay::Status::Found) => {
            let hosts = ctx()
                .netplay
                .as_ref()
                .map(|s| s.hosts())
                .unwrap_or_default();
            if hosts.is_empty() {
                netplay_message(
                    cv,
                    fonts,
                    "JOIN GAME",
                    "NO HOSTS FOUND",
                    "A RESCAN   B BACK",
                    t,
                );
            } else {
                let n = hosts.len().min(5);
                let mut rows: Vec<String> = hosts
                    .iter()
                    .take(n)
                    .map(|h| {
                        if h.hotspot {
                            h.code.clone()
                        } else {
                            format!("{}  {}", h.code, clip(&h.title, 24))
                        }
                    })
                    .collect();
                while rows.len() < n {
                    rows.push(String::new());
                }
                let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
                netplay_panel(
                    cv,
                    fonts,
                    "JOIN GAME",
                    "PICK A HOST",
                    &refs,
                    menu.netplay_sel.min(n.saturating_sub(1)),
                    "A JOIN   B BACK",
                    t,
                );
            }
        }
        _ => {
            let transport = ctx()
                .netplay
                .as_ref()
                .map(|s| s.transport())
                .unwrap_or(menu.netplay_transport);
            let label = match transport {
                netplay::Transport::Hotspot => "SCANNING FOR HOTSPOTS",
                netplay::Transport::Wifi => "SCANNING FOR NETPLAY HOSTS",
            };
            netplay_message(
                cv,
                fonts,
                "JOIN GAME",
                &format!("{label}{}", netplay_dots(t)),
                "B CANCEL",
                t,
            );
        }
    }
}

fn draw_netplay_error(cv: &mut Canvas, fonts: &mut Fonts, menu: &GameMenu, t: f32) {
    let msg = menu.netplay_msg.clone().unwrap_or_else(|| {
        ctx()
            .netplay
            .as_ref()
            .map(|s| match s.status() {
                netplay::Status::Error(e) => e,
                netplay::Status::Lost => "THE OTHER PLAYER LEFT".into(),
                _ => "LINK FAILED".into(),
            })
            .unwrap_or_else(|| "LINK FAILED".into())
    });
    netplay_message(
        cv,
        fonts,
        "MAGI // NETPLAY",
        &clip(&msg, 44),
        "A RETRY   B BACK",
        t,
    );
}

fn netplay_message(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    title: &str,
    message: &str,
    footer: &str,
    t: f32,
) {
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let ww = p(400.0);
    let hh = p(190.0);
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 216);
    crate::theme::glow_border(
        cv,
        x0,
        y0,
        ww as u32,
        hh as u32,
        pal().border,
        pal().glow,
        t,
        0.0,
    );
    fonts.draw_center(cv, w / 2, y0 + p(30.0), title, 16.0, pal().title);
    fonts.draw_center(cv, w / 2, y0 + p(86.0), message, 13.0, pal().signal);
    fonts.draw_center(cv, w / 2, y0 + hh - p(26.0), footer, 9.0, pal().text_faint);
}

fn netplay_dots(t: f32) -> &'static str {
    ["", ".", "..", "..."][(t * 2.0) as usize % 4]
}

/// The DISC page: the images the core reported, current one marked.
pub(crate) fn draw_disc(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    sel: usize,
    msg: Option<&str>,
    count: usize,
    current: usize,
    t: f32,
) {
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let count = count.max(1);
    let current = current.min(count - 1);
    let ww = p(400.0).min(w - p(24.0));
    let header = p(104.0);
    let footer = if msg.is_some() { p(80.0) } else { p(44.0) };
    let pitch = ((h - p(12.0) - header - footer) / count as i32)
        .min(p(36.0))
        .max(p(24.0));
    let hh = header + count as i32 * pitch + footer;
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 216);
    crate::theme::glow_border(
        cv,
        x0,
        y0,
        ww as u32,
        hh as u32,
        pal().border,
        pal().glow,
        t,
        0.3,
    );
    fonts.draw_center(cv, w / 2, y0 + p(28.0), "MAGI // DISC", 16.0, pal().title);
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(56.0),
        &format!("DISC {} OF {}", current + 1, count),
        11.0,
        pal().text_dim,
    );
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(80.0),
        "PICK A DISC, THEN INSERT",
        9.0,
        pal().text_faint,
    );

    for i in 0..count {
        let c = y0 + header + i as i32 * pitch + pitch / 2;
        let y = c + p(5.5);
        let bh = (pitch - p(10.0)).min(p(34.0));
        let picked = i == sel.min(count - 1);
        if picked {
            cv.fill_rect(
                x0 + p(28.0),
                c - bh / 2,
                (ww - p(56.0)) as u32,
                bh as u32,
                pal().bg3,
            );
            crate::theme::glow_border(
                cv,
                x0 + p(28.0),
                c - bh / 2,
                (ww - p(56.0)) as u32,
                bh as u32,
                pal().glow,
                crate::theme::mix(pal().glow, pal().accent, 0.35),
                t,
                0.4,
            );
        }
        let label = if i == current {
            format!("DISC {}   (IN)", i + 1)
        } else {
            format!("DISC {}", i + 1)
        };
        fonts.draw_center(
            cv,
            w / 2,
            y,
            &label,
            15.0,
            if picked { pal().accent } else { pal().text_dim },
        );
    }

    if let Some(msg) = msg {
        fonts.draw_center(cv, w / 2, y0 + hh - p(58.0), msg, 10.0, pal().signal);
    }
    fonts.draw_center(
        cv,
        w / 2,
        y0 + hh - p(24.0),
        "U/D PICK   A INSERT   B BACK",
        9.0,
        pal().text_faint,
    );
}

fn draw_options(cv: &mut Canvas, fonts: &mut Fonts, menu: &GameMenu, t: f32) {
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let options = &ctx().options;
    let ids = visible_options(ctx());
    let count = ids.len();
    let ww = p(500.0).min(w - p(24.0));
    let header = p(88.0);
    let pitch = ((h - p(12.0) - header - p(28.0)) / OPT_ROWS as i32)
        .min(p(36.0))
        .max(p(26.0));
    let hh = header + OPT_ROWS as i32 * pitch + p(28.0);
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 216);
    crate::theme::glow_border(
        cv,
        x0,
        y0,
        ww as u32,
        hh as u32,
        pal().border,
        pal().glow,
        t,
        0.2,
    );

    let title = "MAGI // CORE OPTIONS";
    fonts.draw_center(cv, w / 2, y0 + p(28.0), title, 16.0, pal().title);
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(52.0),
        &format!("{}/{}", (menu.opt_sel + 1).min(count), count),
        11.0,
        pal().text_faint,
    );

    for (row, vi) in ids.iter().skip(menu.opt_top).take(OPT_ROWS).enumerate() {
        let o = &options[*vi];
        let c = y0 + header + row as i32 * pitch + pitch / 2;
        let y = c + p(4.9);
        let bh = (pitch - p(10.0)).min(p(30.0));
        let sel = menu.opt_top + row == menu.opt_sel;
        if sel {
            cv.fill_rect(
                x0 + p(14.0),
                c - bh / 2,
                (ww - p(28.0)) as u32,
                bh as u32,
                pal().bg3,
            );
            crate::theme::glow_border(
                cv,
                x0 + p(14.0),
                c - bh / 2,
                (ww - p(28.0)) as u32,
                bh as u32,
                pal().glow,
                pal().accent,
                t,
                0.4,
            );
        }
        let text = if sel { pal().accent } else { pal().text_dim };
        fonts.draw(cv, x0 + p(24.0), y, clip(&o.desc, 30).as_str(), 13.5, text);
        let value = format!("< {} >", clip(&o.values[o.chosen], 18));
        let vw = fonts.text_width(&value, 13.5);
        fonts.draw(
            cv,
            x0 + ww - p(24.0) - vw,
            y,
            &value,
            13.5,
            if sel { pal().signal } else { text },
        );
    }

    fonts.draw_center(
        cv,
        w / 2,
        y0 + hh - p(22.0),
        "UP/DOWN PICK   LEFT/RIGHT CHANGE   B BACK   MENU CLOSE",
        9.0,
        pal().text_faint,
    );
}

fn draw_overlay_menu(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    menu: &GameMenu,
    files: &OverlayLayers,
    t: f32,
) {
    use crate::theme::pal;
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let ww = p(500.0).min(w - p(24.0));
    let hh = p(106.0) + 4 * p(36.0) + p(28.0);
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 216);
    crate::theme::glow_border(
        cv,
        x0,
        y0,
        ww as u32,
        hh as u32,
        pal().border,
        pal().glow,
        t,
        0.2,
    );
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(28.0),
        "MAGI // OVERLAYS",
        16.0,
        pal().title,
    );
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(52.0),
        "OSD, THEN LOWEST LAYER FIRST",
        11.0,
        pal().text_faint,
    );

    // Same row geometry as the pause menu: 36 px pitch, 28 px inset, the
    // text baseline 5.5 px under the row centre.
    let rows = ["FPS", "SYS INFO", "CRT", "BEZEL"];
    let pitch = p(36.0);
    for (row, label) in rows.iter().enumerate() {
        let c = y0 + p(88.0) + row as i32 * pitch + pitch / 2;
        let baseline = c + p(5.5);
        let bh = (pitch - p(10.0)).min(p(36.0));
        let sel = row == menu.overlay_sel;
        if sel {
            cv.fill_rect(
                x0 + p(28.0),
                c - bh / 2,
                (ww - p(56.0)) as u32,
                bh as u32,
                pal().bg3,
            );
            crate::theme::glow_border(
                cv,
                x0 + p(28.0),
                c - bh / 2,
                (ww - p(56.0)) as u32,
                bh as u32,
                pal().glow,
                crate::theme::mix(pal().glow, pal().accent, 0.35),
                t,
                0.4,
            );
        }
        let text = if sel { pal().accent } else { pal().text_dim };
        let label_w = fonts.text_width(label, 13.5);
        fonts.draw(cv, x0 + p(38.0), baseline, label, 13.5, text);
        let cell = |v: bool| if v { "ON" } else { "OFF" }.to_string();
        let max_w = (ww - p(38.0) * 2 - label_w - p(12.0)).max(p(40.0));
        let value = match row {
            0 => cell(menu.fps),
            1 => cell(menu.sysinfo),
            _ => match menu.overlay_idx[row - 2] {
                0 => "OFF".to_string(),
                k => {
                    let name = files
                        .get(row - 2)
                        .and_then(|l| l.get(k - 1))
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    fonts.elide(name, 13.5, max_w)
                }
            },
        };
        let vw = fonts.text_width(&value, 13.5);
        fonts.draw(
            cv,
            x0 + ww - p(38.0) - vw,
            baseline,
            &value,
            13.5,
            if sel { pal().signal } else { text },
        );
    }

    fonts.draw_center(
        cv,
        w / 2,
        y0 + hh - p(22.0),
        "UP/DOWN PICK   LEFT/RIGHT CHANGE   B BACK   MENU CLOSE",
        9.0,
        pal().text_faint,
    );
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(2)).collect();
        t.push_str("..");
        t
    }
}

/// Render the overlays page with a given selection, for the headless check.
pub(crate) fn draw_overlay_menu_preview(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    sel: usize,
    idx: [usize; 2],
    files: &OverlayLayers,
    t: f32,
) {
    let menu = GameMenu {
        open: true,
        sel: 0,
        msg: None,
        exit_requested: false,
        page: MenuPage::Overlays,
        slot: 0,
        opt_sel: 0,
        opt_top: 0,
        overlay_sel: sel,
        overlay_idx: idx,
        fps: false,
        sysinfo: false,
        hold: None,
        disc_sel: 0,
        netplay_page: NetplayPage::Menu,
        netplay_sel: 0,
        netplay_transport: netplay::Transport::Wifi,
        netplay_role: netplay::Role::Host,
        netplay_msg: None,
    };
    draw_overlay_menu(cv, fonts, &menu, files, t);
}

fn apply_adjust(disp: &mut DisplayPrefs, kind: AdjKind, dir: i32) {
    match kind {
        AdjKind::Brightness => {
            disp.brightness = (disp.brightness as i32 + dir * COMBO_STEP).clamp(0, 100) as u32;
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
/// on the CPU (the GPU keeps its frame in texture memory).
fn compose_shot(
    ctx: &Ctx,
    scale: ScaleMode,
    game_area: (i32, i32, u32, u32),
    overlays: &Option<Overlay>,
    layer: &Canvas,
    cw: u32,
    ch: u32,
) -> Canvas {
    let mut shot = Canvas::new(cw, ch);
    if ctx.frame.is_null() {
        shot.clear(pal().bg0);
    } else {
        blit(ctx, &mut shot, scale, game_area);
        apply_overlays(&mut shot, overlays);
    }
    blend_layer(&mut shot, layer);
    shot
}

/// Hand a frame to the GPU: the game, the bezel overlay and the UI layer
/// (the overlays and the menu) in one pass.
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
    let Some(g) = gpu.as_mut() else {
        return false;
    };
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
        return false;
    }
    g.present(frame, pitch, fw, fh, fmt, rect, warmth)
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

    // The scaler is the largest software cost in the frame; split the
    // output rows over two cores while the emulator core is idle.
    let split = dh / 2;
    if split >= 32 {
        let split_row = y0 + split;
        let (top, bottom) = cv.px.split_at_mut(split_row * cw);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                blit_rows(
                    &src, &sx_map, bottom, split_row, cw, x0, y0, dw, fw, fh, dh, split, dh,
                );
            });
            blit_rows(&src, &sx_map, top, 0, cw, x0, y0, dw, fw, fh, dh, 0, split);
        });
    } else {
        blit_rows(
            &src, &sx_map, &mut cv.px, 0, cw, x0, y0, dw, fw, fh, dh, 0, dh,
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
    (((r << 3) | (r >> 2)) << 16) | (((g << 2) | (g >> 4)) << 8) | ((b << 3) | (b >> 2))
}

fn rgb1555(v: u32) -> u32 {
    let r = (v >> 10) & 0x1f;
    let g = (v >> 5) & 0x1f;
    let b = v & 0x1f;
    (((r << 3) | (r >> 2)) << 16) | (((g << 3) | (g >> 2)) << 8) | ((b << 3) | (b >> 2))
}

#[cfg(test)]
mod option_tests {
    use super::{ensure_gba_bios, parse_option, pce_bios_default_in};

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
}

#[cfg(test)]
mod key_mode_tests {
    use super::needs_performance;

    #[test]
    fn light_systems_stay_on_schedutil() {
        for system in ["gb", "gbc", "gba", "ngp", "snes", "pce", "genesis"] {
            assert!(!needs_performance(system), "{system}");
        }
        for system in ["psx", "neocd"] {
            assert!(needs_performance(system), "{system}");
        }
    }
}

#[cfg(test)]
mod pad_tests {
    use super::{ev, retro, PadState, RawEvent};

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
    fn psx_dualshock_is_analog_subclass_one() {
        assert_eq!(
            super::RETRO_DEVICE_PSE_DUALSHOCK,
            super::retro_subclass(super::RETRO_DEVICE_ANALOG, 1)
        );
        assert_eq!(super::RETRO_DEVICE_PSE_DUALSHOCK, 0x205);
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
        assert!(option_kept("Game Boy Model (Restart)"));
        assert!(option_kept("Hardware Preset Game Boy Palettes (Restart)"));
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
        assert!(!option_kept("Frameskip"));
        assert!(!option_kept("Game Boy Player Rumble (Restart)"));
        assert!(!option_kept("Color"));
        assert!(option_kept("Solar Sensor Level"));
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
    use super::{game_rect, scale_factors, screen_area, ScaleMode};

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

#[cfg(test)]
mod image_overlay_tests {
    use super::*;

    #[test]
    fn overlay_kind_sorts_pack_folders() {
        assert_eq!(overlay_kind("CRT/Perfect_CRT.png"), Some(0));
        assert_eq!(overlay_kind("CRT/Perfect_CRT-240p(noframe).png"), Some(0));
        assert_eq!(overlay_kind("GB/drkhrse/Perfect_DMG-EX.png"), Some(1));
        assert_eq!(overlay_kind("misc/Perfect_GBA-SP.png"), Some(1));
        // Colour correction belongs in the core options, not a layer.
        assert_eq!(overlay_kind("COLOR/scanlines.png"), None);
        assert_eq!(overlay_kind("colour/tint.png"), None);
    }

    #[test]
    fn overlay_choices_are_per_system() {
        let v = serde_json::json!({
            "overlays": {"gba": {"crt": "CRT/a.png", "bezel": "GBA/b.png"}}
        });
        assert_eq!(
            overlay_prefs_from(&v, "gba", "", false),
            ["CRT/a.png".to_string(), "GBA/b.png".to_string()]
        );
        assert_eq!(
            overlay_prefs_from(&v, "genesis", "", false),
            [
                "CRT/Perfect_CRT-240p(noframe).png".to_string(),
                String::new()
            ],
            "the global keys do not leak across systems"
        );
        let legacy = serde_json::json!({
            "overlay_crt": "CRT/x.png",
            "overlay_bezel": ""
        });
        assert_eq!(
            overlay_prefs_from(&legacy, "snes", "", false),
            ["CRT/x.png".to_string(), String::new()],
            "the legacy globals seed the first launch"
        );
    }

    #[test]
    fn fresh_configs_get_the_shipped_overlays() {
        let v = serde_json::json!({});
        assert_eq!(
            overlay_prefs_from(&v, "psx", "", false),
            ["CRT/Perfect_CRT.png".to_string(), String::new()]
        );
        for system in ["snes", "pce", "nes", "neocd"] {
            assert_eq!(
                overlay_prefs_from(&v, system, "", false),
                ["CRT/Perfect_CRT-240p.png".to_string(), String::new()],
                "{system}"
            );
            assert_eq!(
                overlay_prefs_from(&v, system, "", true),
                [
                    "CRT/Perfect_CRT-240p-720x480.png".to_string(),
                    String::new()
                ],
                "{system} wide"
            );
        }
        assert_eq!(
            overlay_prefs_from(&v, "genesis", "", false),
            [
                "CRT/Perfect_CRT-240p(noframe).png".to_string(),
                String::new()
            ]
        );
        assert_eq!(
            overlay_prefs_from(&v, "genesis", "", true),
            [
                "CRT/Perfect_CRT-240p(noframe)-720x480.png".to_string(),
                String::new()
            ]
        );
        assert_eq!(
            overlay_prefs_from(&v, "gba", "", false),
            [String::new(), "GBA/mugwomp93/Perfect_GBA.png".to_string()]
        );
        assert_eq!(
            overlay_prefs_from(&v, "gba", "", true),
            [String::new(), String::new()],
            "720x480 is exactly 3x GBA"
        );
        assert_eq!(
            overlay_prefs_from(&v, "gbc", "", false),
            [String::new(), "GBC/mugwomp93/Perfect_GBC.png".to_string()]
        );
        assert_eq!(
            overlay_prefs_from(&v, "gb", "", false),
            [String::new(), "GB/mugwomp93/Perfect_GBP-EX.png".to_string()]
        );
        assert_eq!(
            overlay_prefs_from(&v, "gb", "", true),
            [
                String::new(),
                "GB/mugwomp93/Perfect_GBP-EX-720x480.png".to_string()
            ]
        );
        assert_eq!(
            overlay_prefs_from(&v, "ngp", "Games/NGPC/Game.ngp", false),
            ["".to_string(), "NGP/mugwomp93/ngp-640x480.png".to_string()]
        );
        assert_eq!(
            overlay_prefs_from(&v, "ngp", "Games/NGPC/Game.ngc", false),
            [
                "".to_string(),
                "NGPC/mugwomp93/ngpc-640x480.png".to_string()
            ]
        );
        assert_eq!(
            overlay_prefs_from(&v, "ngp", "Games/NGPC/Game.ngp", true),
            ["".to_string(), "NGP/mugwomp93/ngp-720x480.png".to_string()]
        );
        assert_eq!(
            overlay_prefs_from(&v, "ngp", "Games/NGPC/Game.ngc", true),
            [
                "".to_string(),
                "NGPC/mugwomp93/ngpc-720x480.png".to_string()
            ]
        );
    }
}
