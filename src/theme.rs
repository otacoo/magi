//! SEELE — the palette. Roles are named after the element they paint:
//! surfaces, text levels, borders and glow, then the individual bits of chrome
//! (clock, battery, scrollbars, signal lamps). A theme is a pure data change.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use crate::font::Fonts;

use crate::canvas::Canvas;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// 0xAARRGGBB, the layout SDL's ARGB8888 pixel format expects.
    pub const fn to_u32(self) -> u32 {
        0xFF00_0000 | ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }
}

/// A colour palette. Every theme fills one of these; the painters below and
/// every screen read it through [`pal`], so a theme is a pure data change.
///
/// Roles are named after what they paint, not after a hue, so a theme can move
/// a colour without the name lying about it.
#[derive(Clone, Copy)]
pub struct Palette {
    // Surfaces, darkest first.
    pub bg0: Color,
    pub bg1: Color,
    pub bg2: Color,
    pub bg3: Color,

    // Type.
    /// Page, panel and shelf titles.
    pub title: Color,
    /// Body text, values and status messages.
    pub text: Color,
    /// Secondary rows and labels.
    pub text_dim: Color,
    /// Hints, captions and disabled text.
    pub text_faint: Color,

    // Chrome and state.
    /// Selection text, active tab, ratios and hazard bars.
    pub accent: Color,
    /// Static frame and divider lines.
    pub border: Color,
    /// The breathing glow on borders and the selected row.
    pub glow: Color,
    /// Errors, warnings and low-battery state.
    pub alert: Color,

    // Individual elements.
    /// The seven-segment clock and the clock wizard.
    pub clock: Color,
    /// A healthy battery fill (and its percentage).
    pub battery: Color,
    /// The battery case outline and terminal.
    pub battery_shell: Color,
    /// Scrollbar thumb.
    pub scrollbar: Color,
    /// Scrollbar track.
    pub scrollbar_track: Color,
    /// "On" states: wifi, favorite star, spinner, connected rows.
    pub signal: Color,
    /// The Bluetooth/radio icon.
    pub link: Color,

    /// The heat ramp for the volume/brightness/temperature bars.
    pub heat: [Color; 5],
}

/// A named theme: the palette today, fonts or chrome rules tomorrow.
pub struct Theme {
    pub name: &'static str,
    pub palette: Palette,
}

/// NERV orange-on-black (the original MAGI look).
pub static NERV: Theme = Theme {
    name: "NERV",
    palette: Palette {
        bg0: Color::rgb(3, 3, 3),
        bg1: Color::rgb(9, 8, 5),
        bg2: Color::rgb(15, 13, 8),
        bg3: Color::rgb(23, 19, 12),
        title: Color::rgb(255, 122, 0),
        text: Color::rgb(255, 176, 59),
        text_dim: Color::rgb(214, 138, 44),
        text_faint: Color::rgb(148, 96, 30),
        accent: Color::rgb(255, 122, 0),
        border: Color::rgb(148, 96, 30),
        glow: Color::rgb(0, 255, 136),
        alert: Color::rgb(255, 42, 26),
        clock: Color::rgb(255, 176, 0),
        battery: Color::rgb(0, 255, 136),
        battery_shell: Color::rgb(148, 96, 30),
        scrollbar: Color::rgb(255, 176, 59),
        scrollbar_track: Color::rgb(15, 13, 8),
        signal: Color::rgb(0, 255, 136),
        link: Color::rgb(90, 150, 255),
        heat: [
            Color::rgb(0x2a, 0x5c, 0xff),
            Color::rgb(0x00, 0xc8, 0x96),
            Color::rgb(0x7a, 0xd6, 0x3a),
            Color::rgb(0xff, 0xb0, 0x00),
            Color::rgb(0xff, 0x2a, 0x1a),
        ],
    },
};

/// EVA-01: purple chassis, unit-green signals.
pub static UNIT01: Theme = Theme {
    name: "UNIT-01",
    palette: Palette {
        bg0: Color::rgb(4, 3, 6),
        bg1: Color::rgb(10, 8, 14),
        bg2: Color::rgb(16, 12, 22),
        bg3: Color::rgb(24, 18, 34),
        title: Color::rgb(122, 77, 255),
        text: Color::rgb(176, 140, 255),
        text_dim: Color::rgb(138, 99, 214),
        text_faint: Color::rgb(92, 63, 148),
        accent: Color::rgb(122, 77, 255),
        border: Color::rgb(92, 63, 148),
        glow: Color::rgb(108, 255, 77),
        alert: Color::rgb(255, 42, 26),
        clock: Color::rgb(108, 255, 77),
        battery: Color::rgb(108, 255, 77),
        battery_shell: Color::rgb(92, 63, 148),
        scrollbar: Color::rgb(176, 140, 255),
        scrollbar_track: Color::rgb(16, 12, 22),
        signal: Color::rgb(108, 255, 77),
        link: Color::rgb(90, 150, 255),
        heat: [
            Color::rgb(0x2a, 0x5c, 0xff),
            Color::rgb(0x00, 0xc8, 0x96),
            Color::rgb(0x7a, 0xd6, 0x3a),
            Color::rgb(0xff, 0xb0, 0x00),
            Color::rgb(0xff, 0x2a, 0x1a),
        ],
    },
};

pub static BUILTIN: [&Theme; 2] = [&NERV, &UNIT01];

/// Built-in themes first, then user themes from `Themes/*.ini` (file-name
/// order). Initialised once, before the first `set_theme_index` call.
static THEMES: std::sync::OnceLock<&'static [&'static Theme]> = std::sync::OnceLock::new();

static THEME: AtomicPtr<Theme> = AtomicPtr::new(std::ptr::null_mut());

/// The active theme (NERV until one is selected).
pub fn theme() -> &'static Theme {
    let p = THEME.load(Ordering::Relaxed);
    if p.is_null() {
        &NERV
    } else {
        unsafe { &*p }
    }
}

/// The active palette; painters and screens read colours from here.
pub fn pal() -> &'static Palette {
    &theme().palette
}

pub fn set_theme(t: &'static Theme) {
    THEME.store(t as *const Theme as *mut Theme, Ordering::Relaxed);
}

/// Select by index (config persistence uses the index).
pub fn set_theme_index(i: u8) {
    let themes = themes();
    set_theme(themes[(i as usize).min(themes.len() - 1)]);
}

pub fn themes() -> &'static [&'static Theme] {
    THEMES.get_or_init(|| &BUILTIN)
}

// ----- user themes ---------------------------------------------------------

/// The template written to `Themes/_template.ini` on first start.
pub const THEME_TEMPLATE: &str = include_str!("../assets/themes/_template.ini");
/// How many user themes a card may add; further files are ignored.
const MAX_USER_THEMES: usize = 8;
/// Refuse anything much larger than the template.
const MAX_THEME_BYTES: usize = 16 * 1024;

/// Create `Themes/` and its template when missing. The template is skipped by
/// the loader because its name starts with `_`.
pub fn ensure_user_themes(dir: &Path) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        crate::util::log(&format!("theme: cannot create {}: {e}", dir.display()));
        return;
    }
    let path = dir.join("_template.ini");
    if !path.exists() {
        if let Err(e) = crate::util::write_file_atomic(&path, THEME_TEMPLATE.as_bytes()) {
            crate::util::log(&format!("theme: cannot write {}: {e}", path.display()));
        }
    }
}

/// Load `Themes/*.ini` after the built-ins, in file-name order. Call once at
/// start-up, before the saved theme index is applied.
pub fn load_user_themes(dir: &Path) {
    static LOADED: OnceLock<()> = OnceLock::new();
    if LOADED.set(()).is_err() {
        return;
    }
    let mut list: Vec<&'static Theme> = BUILTIN.to_vec();
    let builtin = list.len();
    for path in theme_files(dir) {
        if list.len() - builtin >= MAX_USER_THEMES {
            crate::util::log(&format!(
                "theme: more than {MAX_USER_THEMES} files in {}; ignored",
                dir.display()
            ));
            break;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) if text.len() <= MAX_THEME_BYTES => text,
            Ok(_) => {
                crate::util::log(&format!("theme: {} is too large; ignored", path.display()));
                continue;
            }
            Err(e) => {
                crate::util::log(&format!("theme: cannot read {}: {e}", path.display()));
                continue;
            }
        };
        let theme: &'static Theme = Box::leak(Box::new(parse_theme(&text, stem)));
        crate::util::log(&format!(
            "theme: \"{}\" from {}",
            theme.name,
            path.display()
        ));
        list.push(theme);
    }
    if list.len() > builtin && THEMES.set(Box::leak(list.into_boxed_slice())).is_err() {
        crate::util::log("theme: user themes ignored (list already built)");
    }
}

/// `Themes/*.ini`, sorted; the template and dotfiles stay out.
fn theme_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("ini"))
        })
        .filter(|p| {
            !p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('_') || n.starts_with('.'))
        })
        .collect();
    files.sort();
    files
}

/// One `key = value` per line. Every key left out keeps the NERV colour, so a
/// theme file may set two colours or all of them.
fn parse_theme(text: &str, stem: &str) -> Theme {
    let mut palette = NERV.palette;
    let mut name: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let mut value = value.trim();
        for marker in [" #", " ;"] {
            if let Some(i) = value.find(marker) {
                value = &value[..i];
            }
        }
        let value = value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "name" => {
                if !value.is_empty() {
                    name = Some(value.chars().take(20).collect());
                }
            }
            "bg0" => set_color(&mut palette.bg0, value),
            "bg1" => set_color(&mut palette.bg1, value),
            "bg2" => set_color(&mut palette.bg2, value),
            "bg3" => set_color(&mut palette.bg3, value),
            "title" => set_color(&mut palette.title, value),
            "text" => set_color(&mut palette.text, value),
            "text_dim" => set_color(&mut palette.text_dim, value),
            "text_faint" => set_color(&mut palette.text_faint, value),
            "accent" => set_color(&mut palette.accent, value),
            "border" => set_color(&mut palette.border, value),
            "glow" => set_color(&mut palette.glow, value),
            "alert" => set_color(&mut palette.alert, value),
            "clock" => set_color(&mut palette.clock, value),
            "battery" => set_color(&mut palette.battery, value),
            "battery_shell" => set_color(&mut palette.battery_shell, value),
            "scrollbar" => set_color(&mut palette.scrollbar, value),
            "scrollbar_track" => set_color(&mut palette.scrollbar_track, value),
            "signal" => set_color(&mut palette.signal, value),
            "link" => set_color(&mut palette.link, value),
            "heat" => {
                if let Some(heat) = parse_heat(value) {
                    palette.heat = heat;
                }
            }
            _ => {}
        }
    }
    let name = name.unwrap_or_else(|| stem.to_string());
    Theme {
        name: Box::leak(name.into_boxed_str()),
        palette,
    }
}

fn set_color(slot: &mut Color, value: &str) {
    if let Some(c) = parse_color(value) {
        *slot = c;
    }
}

/// `RRGGBB`, `RGB`, with or without a leading `#`.
fn parse_color(value: &str) -> Option<Color> {
    let v = value.trim().trim_start_matches('#');
    if !v.is_ascii() {
        return None;
    }
    let hex = |s: &str| u8::from_str_radix(s, 16).ok();
    match v.len() {
        3 => {
            let d = |i: usize| hex(&v[i..i + 1]).map(|x| x * 17);
            Some(Color::rgb(d(0)?, d(1)?, d(2)?))
        }
        6 => Some(Color::rgb(hex(&v[0..2])?, hex(&v[2..4])?, hex(&v[4..6])?)),
        _ => None,
    }
}

/// Five comma-separated colours; a bad ramp keeps the default.
fn parse_heat(value: &str) -> Option<[Color; 5]> {
    let parts: Vec<&str> = value.split(',').map(str::trim).collect();
    if parts.len() != 5 {
        return None;
    }
    let mut heat = NERV.palette.heat;
    for (i, part) in parts.iter().enumerate() {
        heat[i] = parse_color(part)?;
    }
    Some(heat)
}

/// UI text scale, set by the TEXT SIZE setting (1.0 = normal). Applied
/// inside the font layer, so every draw call honours it.
static UI_SCALE: AtomicU32 = AtomicU32::new(1000);

pub fn ui_scale() -> f32 {
    UI_SCALE.load(Ordering::Relaxed) as f32 / 1000.0
}

pub fn set_ui_scale(scale: f32) {
    UI_SCALE.store((scale.clamp(0.5, 2.0) * 1000.0) as u32, Ordering::Relaxed);
}

/// THEME FX: animated backgrounds (the scanner lattice and friends) and
/// pulsing glow on borders. Both can be switched off in settings.
static ANIM_BG: AtomicBool = AtomicBool::new(true);
static GLOW: AtomicBool = AtomicBool::new(true);

pub fn anim_bg() -> bool {
    ANIM_BG.load(Ordering::Relaxed)
}

pub fn set_anim_bg(on: bool) {
    ANIM_BG.store(on, Ordering::Relaxed);
}

pub fn glow() -> bool {
    GLOW.load(Ordering::Relaxed)
}

pub fn set_glow(on: bool) {
    GLOW.store(on, Ordering::Relaxed);
}

/// BACKGROUND style: 0 grid, 1 honeycomb (the classic scanner), 2 topo.
const BG_GRID: u8 = 0;
const BG_HONEYCOMB: u8 = 1;
const BG_NOISE: u8 = 2;
static BG_STYLE: AtomicU8 = AtomicU8::new(BG_HONEYCOMB);
static SHELF_BG: AtomicBool = AtomicBool::new(false);

pub fn bg_style() -> u8 {
    BG_STYLE.load(Ordering::Relaxed)
}

pub fn set_bg_style(style: u8) {
    BG_STYLE.store(style.min(BG_NOISE), Ordering::Relaxed);
}

/// Whether the shelf (the main panel) also shows the backdrop.
pub fn shelf_bg() -> bool {
    SHELF_BG.load(Ordering::Relaxed)
}

pub fn set_shelf_bg(on: bool) {
    SHELF_BG.store(on, Ordering::Relaxed);
}

pub fn bg_styles() -> &'static [&'static str] {
    &["GRID", "HONEYCOMB", "TOPO"]
}

/// Warm (blue-reducing) tint for the whole screen, adjusted in-game with
/// SELECT+LEFT/RIGHT. 0 is the identity.
static WARMTH: AtomicU8 = AtomicU8::new(0);

pub fn warmth() -> u8 {
    WARMTH.load(Ordering::Relaxed)
}

pub fn set_warmth(w: u8) {
    WARMTH.store(w.min(100), Ordering::Relaxed);
}

/// The tint as three channel tables: red lifted, blue cut, green untouched.
pub fn warmth_tables(w: u8) -> ([u8; 256], [u8; 256], [u8; 256]) {
    let f = w.min(100) as f32 / 100.0;
    let (rs, bs) = (1.0 + 0.12 * f, 1.0 - 0.40 * f);
    let mut r = [0u8; 256];
    let mut g = [0u8; 256];
    let mut b = [0u8; 256];
    for i in 0..256usize {
        r[i] = ((i as f32) * rs).min(255.0) as u8;
        g[i] = i as u8;
        b[i] = ((i as f32) * bs) as u8;
    }
    (r, g, b)
}

/// Apply the warm tint to a whole canvas, just before it is presented, so
/// the shelf, menus and games all share the same screen hue.
pub fn apply_warmth(cv: &mut Canvas) {
    let w = warmth();
    if w == 0 {
        return;
    }
    let (rt, gt, bt) = warmth_tables(w);
    for p in cv.px.iter_mut() {
        let c = *p;
        *p = 0xFF00_0000
            | ((rt[((c >> 16) & 0xff) as usize] as u32) << 16)
            | ((gt[((c >> 8) & 0xff) as usize] as u32) << 8)
            | (bt[(c & 0xff) as usize] as u32);
    }
}

/// The backdrop for a region: whichever pattern BACKGROUND selects. The
/// motion stops (calm, static pattern) when ANIM BACKGROUND is off.
pub fn background(cv: &mut Canvas, x: i32, y: i32, w: u32, h: u32, t: f32) {
    match bg_style() {
        BG_GRID => grid_pattern(cv, x, y, w, h, 40, pal().bg2, t),
        BG_NOISE => noise_pattern(cv, x, y, w, h, t),
        _ => hex_grid(cv, x, y, w, h, 34, pal().bg2, t),
    }
}

/// A technician's square grid with a few intersections idling on and off.
fn grid_pattern(cv: &mut Canvas, x: i32, y: i32, w: u32, h: u32, cell: i32, tint: Color, t: f32) {
    if cell < 6 || w == 0 || h == 0 {
        return;
    }
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w as i32).min(cv.w);
    let y1 = (y + h as i32).min(cv.h);
    let mut gx = (x0 + cell - 1) / cell * cell;
    while gx < x1 {
        cv.vline(gx, y0, (y1 - y0) as u32, tint);
        gx += cell;
    }
    let mut gy = (y0 + cell - 1) / cell * cell;
    while gy < y1 {
        cv.hline(x0, gy, (x1 - x0) as u32, tint);
        gy += cell;
    }
    if !anim_bg() {
        return;
    }
    let mut py = (y0 + cell - 1) / cell * cell;
    while py < y1 {
        let mut px = (x0 + cell - 1) / cell * cell;
        while px < x1 {
            let hsh = hash2(px, py);
            let p = pulse01(t, 28.0 + (hsh % 6) as f32 * 6.0, (hsh % 89) as f32 / 89.0);
            if p > 0.93 {
                cv.fill_rect(px - 1, py - 1, 3, 3, mix(tint, pal().alert, 0.5));
            }
            px += cell;
        }
        py += cell;
    }
}

/// A topographic contour map: level lines of a few drifting sine planes
/// filling the whole region (the TOPO backdrop).
fn noise_pattern(cv: &mut Canvas, x: i32, y: i32, w: u32, h: u32, t: f32) {
    if w == 0 || h == 0 {
        return;
    }
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w as i32).min(cv.w);
    let y1 = (y + h as i32).min(cv.h);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let tt = if anim_bg() { t } else { 0.0 };
    let dim = mix(pal().bg0, pal().glow, 0.16);
    let lit = mix(pal().bg0, pal().glow, 0.42);
    let mut py = y0;
    while py < y1 {
        let yf = py as f32;
        let mut px = x0;
        while px < x1 {
            let xf = px as f32;
            let v = (xf * 0.011 + tt * 0.50).sin()
                + (yf * 0.013 - tt * 0.38).sin()
                + ((xf + yf) * 0.007 + tt * 0.62).sin();
            let band = (v * 1.3).rem_euclid(1.0);
            if band < 0.14 {
                let c = if band < 0.06 { lit } else { dim };
                cv.put(px, py, c);
                cv.put(px + 1, py, c);
            }
            px += 2;
        }
        py += 2;
    }
}

/// The EVA-style shutdown countdown (both hosts draw it): bordered panel,
/// "TIME REMAINING TO SHUTDOWN", a draining bar, SS,mmm in the clock's
/// seven-segment digits and the warning line below.
pub fn draw_shutdown(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    remaining: Duration,
    total: Duration,
    t: f32,
) {
    let (w, h) = (cv.w, cv.h);
    // 90% of the selected theme backdrop over the frozen frame.
    cv.fill_rect_a(0, 0, w as u32, h as u32, pal().bg0, 230);
    background(cv, 0, 0, w as u32, h as u32, t);

    // Warning title outside the panel, like the settings pages but red.
    fonts.draw_center(cv, w / 2, 42, "WARNING: SHUTTING DOWN", 15.0, pal().alert);
    hazard_bar_colored(cv, w / 2 - 150, 56, 300, 5, t, pal().alert);

    let pw = (w as f32 * 0.65) as i32;
    let ph = (h as f32 * 0.33) as i32;
    let x0 = (w - pw) / 2;
    let y0 = (h - ph) / 2 + 10;
    cv.fill_rect_a(x0, y0, pw as u32, ph as u32, pal().bg0, 238);
    // Solid, slightly thick border.
    cv.rect_outline(x0, y0, pw as u32, ph as u32, pal().text);
    cv.rect_outline(x0 + 1, y0 + 1, (pw - 2) as u32, (ph - 2) as u32, pal().text);
    cv.rect_outline(x0 + 2, y0 + 2, (pw - 4) as u32, (ph - 4) as u32, pal().text);

    fonts.draw_center(
        cv,
        w / 2,
        y0 + 22,
        "TIME REMAINING TO SHUTDOWN",
        11.0,
        pal().text,
    );
    // The rule under the title doubles as the draining bar.
    let bw = pw - 48;
    let bx = x0 + 24;
    let by = y0 + 32;
    cv.fill_rect(bx, by, bw as u32, 4, pal().bg2);
    let frac = (remaining.as_secs_f32() / total.as_secs_f32()).clamp(0.0, 1.0);
    cv.fill_rect(bx, by, (bw as f32 * frac) as u32, 4, pal().text);

    // The countdown: seconds and milliseconds, seven-segment like the clock.
    let ms = remaining.as_millis() as u64;
    let secs = format!("{:03}", (ms / 1000).min(999));
    let msec = format!("{:03}", ms % 1000);
    let dh = (ph as f32 * 0.42) as i32;
    let gap = dh / 5;
    let ws = seven_segment_width(&secs, dh);
    let wm = seven_segment_width(&msec, dh);
    let sec_w = fonts.text_width("sec.", 12.0);
    let total_w = ws + gap + wm + gap + sec_w;
    let sx = (w - total_w) / 2;
    let dy = y0 + (ph - dh) / 2 + 6;
    let adv = seven_segment(cv, sx, dy, dh, &secs, pal().text);
    let comma_w = (dh / 8).max(3);
    cv.fill_rect(
        adv + (gap - comma_w) / 2,
        dy + dh - dh / 5,
        comma_w as u32,
        (dh / 5) as u32,
        pal().text,
    );
    seven_segment(cv, sx + ws + gap, dy, dh, &msec, pal().text);
    fonts.draw(
        cv,
        sx + ws + gap + wm + gap,
        dy + dh - 4,
        "sec.",
        12.0,
        pal().text,
    );
}

/// A 0..1 breathing wave.
pub fn pulse01(t: f32, period: f32, phase: f32) -> f32 {
    let x = (t / period + phase) * std::f32::consts::TAU;
    (x.sin() + 1.0) * 0.5
}

/// Linear blend of two colours.
pub fn mix(a: Color, b: Color, f: f32) -> Color {
    let f = f.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f).round() as u8;
    Color::rgb(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

/// A border colour that breathes between `base` and `peak` (static when
/// GLOW FX is off).
pub fn glow_color(base: Color, peak: Color, t: f32, phase: f32) -> Color {
    if glow() {
        mix(base, peak, pulse01(t, 5.0, phase))
    } else {
        base
    }
}

/// A border that breathes between `base` and `peak` (static when GLOW FX is
/// off).
pub fn glow_border(
    cv: &mut Canvas,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    base: Color,
    peak: Color,
    t: f32,
    phase: f32,
) {
    cv.rect_outline(x, y, w, h, glow_color(base, peak, t, phase));
}

/// Diagonal hazard stripes, `stripe` on dark, sliding with `t`.
pub fn hazard_bar_colored(cv: &mut Canvas, x: i32, y: i32, w: u32, h: u32, t: f32, stripe: Color) {
    let off = if anim_bg() { (t * 8.0) as i32 } else { 0 };
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w as i32).min(cv.w);
    let y1 = (y + h as i32).min(cv.h);
    let (on_c, off_c) = (stripe.to_u32(), pal().bg1.to_u32());
    for py in y0..y1 {
        for px in x0..x1 {
            let on = (px + py + off).rem_euclid(12) < 6;
            cv.px[(py * cv.w + px) as usize] = if on { on_c } else { off_c };
        }
    }
}

/// Diagonal hazard stripes in accent orange (NERV signage).
pub fn hazard_bar(cv: &mut Canvas, x: i32, y: i32, w: u32, h: u32, t: f32) {
    hazard_bar_colored(cv, x, y, w, h, t, pal().accent);
}

/// A Wi-Fi fan whose wedges light with the signal strength (0-100).
pub fn wifi_icon(cv: &mut Canvas, x: i32, y: i32, color: Color, strength: u8) {
    let lit = ((strength.min(100) as i32 * 3 + 99) / 100).max(if strength > 0 { 1 } else { 0 });
    for (i, &(dy, w)) in [(0i32, 14i32), (4, 8), (8, 3)].iter().enumerate() {
        let c = if (i as i32) < lit { color } else { pal().bg3 };
        cv.fill_rect(x + (14 - w) / 2, y + dy, w as u32, 2, c);
    }
    let dot = if strength > 0 { color } else { pal().bg3 };
    cv.fill_rect(x + 6, y + 10, 2, 2, dot);
}

/// The Bluetooth rune, drawn as line segments.
pub fn bt_icon(cv: &mut Canvas, x: i32, y: i32, color: Color) {
    line(cv, x + 4, y, x + 4, y + 11, color);
    line(cv, x + 4, y, x + 8, y + 3, color);
    line(cv, x + 8, y + 3, x, y + 8, color);
    line(cv, x + 4, y + 11, x + 8, y + 8, color);
    line(cv, x + 8, y + 8, x, y + 3, color);
}

/// Wired headphones: a headband over two earcups.
pub fn headphone_icon(cv: &mut Canvas, x: i32, y: i32, color: Color) {
    cv.fill_rect(x + 2, y, 8, 2, color);
    cv.fill_rect(x + 1, y + 1, 1, 2, color);
    cv.fill_rect(x + 10, y + 1, 1, 2, color);
    cv.fill_rect(x, y + 3, 3, 6, color);
    cv.fill_rect(x + 9, y + 3, 3, 6, color);
}

/// A small rotating ring of dots, for a background job.
pub fn spinner(cv: &mut Canvas, x: i32, y: i32, t: f32, color: Color) {
    let head = t * 8.0;
    for i in 0..8 {
        let angle = head + i as f32 * std::f32::consts::TAU / 8.0;
        let px = x + (angle.cos() * 4.0).round() as i32;
        let py = y + (angle.sin() * 4.0).round() as i32;
        let alpha = 30 + (i as u16 * 200 / 7) as u8;
        cv.blend(px, py, color, alpha);
        cv.blend(px + 1, py, color, alpha);
    }
}

/// Four signal bars, lit to `pct` (0-100).
pub fn signal_bars(cv: &mut Canvas, x: i32, y: i32, pct: u8, color: Color) {
    let lit = (pct as i32 * 4 + 99) / 100;
    for i in 0..4i32 {
        let h = 3 + i * 2;
        let c = if i < lit { color } else { pal().bg3 };
        cv.fill_rect(x + i * 4, y + (10 - h), 3, h as u32, c);
    }
}

/// A tiny padlock: body and shackle.
pub fn lock_icon(cv: &mut Canvas, x: i32, y: i32, color: Color) {
    cv.rect_outline(x + 1, y + 2, 8, 7, color);
    cv.fill_rect(x + 2, y + 5, 6, 1, color);
    cv.put(x + 2, y, color);
    cv.put(x + 7, y, color);
    cv.put(x + 2, y + 1, color);
    cv.put(x + 7, y + 1, color);
}

/// Which symbol sits left of an adjustment bar.
#[derive(Clone, Copy, PartialEq)]
pub enum BarIcon {
    Brightness,
    Temperature,
    Volume,
}

/// The Evangelion-style heat ramp: blue at 0, green at half, red at full.
fn heat_color(f: f32) -> Color {
    const POS: [f32; 5] = [0.0, 0.35, 0.55, 0.78, 1.0];
    let f = f.clamp(0.0, 1.0);
    let stops = pal().heat;
    let mut i = 0;
    while i + 1 < POS.len() && f > POS[i + 1] {
        i += 1;
    }
    let (f0, c0) = (POS[i], stops[i]);
    let (f1, c1) = (POS[i + 1], stops[i + 1]);
    if f1 <= f0 {
        return c1;
    }
    mix(c0, c1, (f - f0) / (f1 - f0))
}

/// A sun: disc and rays.
fn sun_icon(cv: &mut Canvas, x: i32, y: i32, color: Color) {
    cv.fill_rect(x + 4, y + 4, 5, 5, color);
    for (dx, dy, w, h) in [(6, 0, 2, 3), (6, 11, 2, 3), (0, 6, 3, 2), (11, 6, 3, 2)] {
        cv.fill_rect(x + dx, y + dy, w as u32, h as u32, color);
    }
    for (dx, dy) in [(1, 1), (11, 1), (1, 11), (11, 11)] {
        cv.fill_rect(x + dx, y + dy, 2, 2, color);
    }
}

/// A thermometer whose mercury follows the ratio.
fn thermo_icon(cv: &mut Canvas, x: i32, y: i32, ratio: f32, color: Color) {
    cv.rect_outline(x + 5, y, 4, 10, color);
    let inner = ((ratio.clamp(0.0, 1.0)) * 7.0) as u32;
    cv.fill_rect(x + 6, y + 8 - inner as i32, 2, inner, color);
    cv.fill_rect(x + 3, y + 9, 8, 5, color);
}

/// A speaker: body, cone and two sound arcs.
fn speaker_icon(cv: &mut Canvas, x: i32, y: i32, color: Color) {
    cv.fill_rect(x + 1, y + 4, 3, 6, color);
    for i in 0..5 {
        cv.fill_rect(x + 4 + i, y + 4 - i, 1, (6 + 2 * i) as u32, color);
    }
    cv.fill_rect(x + 11, y + 5, 2, 4, color);
    cv.fill_rect(x + 14, y + 2, 2, 10, color);
}

/// A horizontal bar filled with the heat ramp: each column takes the
/// colour of its position along the full width (blue -> green -> red).
fn heat_bar(cv: &mut Canvas, x: i32, y: i32, w: i32, h: u32, ratio: f32) {
    let full = w.max(2);
    let fill_w = ((full as f32 * ratio.clamp(0.0, 1.0)).round() as i32).max(1);
    for i in 0..fill_w {
        let c = heat_color(i as f32 / (full - 1) as f32);
        cv.fill_rect(x + i, y, 1, h, c);
    }
}

/// The volume/brightness/temperature overlay: a heat bar at the top centre
/// on a soft translucent strip. The volume bar reads icon, level, bar.
pub fn bar_hud(cv: &mut Canvas, fonts: &mut Fonts, icon: BarIcon, ratio: f32) {
    let (bar_w, icon_w, gap) = (220i32, 14i32, 10i32);
    let ratio = ratio.clamp(0.0, 1.0);
    let pct = (icon == BarIcon::Volume).then(|| format!("{}%", (ratio * 100.0).round() as u32));
    let pct_w = pct.as_ref().map(|p| fonts.text_width(p, 10.0)).unwrap_or(0);
    let label = if pct.is_some() { pct_w + gap } else { 0 };
    let total = icon_w + gap + label + bar_w;
    let x0 = (cv.w - total) / 2;
    let y = 26;
    cv.fill_rect_a(x0 - 10, y - 9, (total + 20) as u32, 32, pal().bg0, 204);
    cv.rect_outline(x0 - 10, y - 9, (total + 20) as u32, 32, pal().border);
    let icon_c = heat_color(ratio);
    match icon {
        BarIcon::Brightness => sun_icon(cv, x0, y, icon_c),
        BarIcon::Temperature => thermo_icon(cv, x0, y, ratio, icon_c),
        BarIcon::Volume => speaker_icon(cv, x0, y, icon_c),
    }
    if let Some(pct) = pct {
        fonts.draw(cv, x0 + icon_w + gap, y + 11, &pct, 10.0, pal().text);
    }
    let x = x0 + icon_w + gap + label;
    cv.fill_rect(x - 3, y - 3, (bar_w + 6) as u32, 20, pal().bg2);
    cv.rect_outline(x - 3, y - 3, (bar_w + 6) as u32, 20, pal().border);
    heat_bar(cv, x, y, bar_w, 14, ratio);
}
/// The scanner pattern: a faint hexagonal lattice across a rect, with dim
/// red blips breathing in and out — slowly (the animation can be off).
pub fn hex_grid(cv: &mut Canvas, x: i32, y: i32, w: u32, h: u32, cell: i32, tint: Color, t: f32) {
    if cell < 6 || w == 0 || h == 0 {
        return;
    }
    let clip: Clip = (
        x.max(0),
        y.max(0),
        (x + w as i32).min(cv.w),
        (y + h as i32).min(cv.h),
    );
    if clip.2 <= clip.0 || clip.3 <= clip.1 {
        return;
    }
    let dx = cell * 2;
    let dy = (cell as f32 * 1.732) as i32;
    let animate = anim_bg();
    let rows = (h as i32 + cell) / dy.max(1) + 1;
    for row in 0..rows {
        let cy = y + row * dy;
        let mut cx = x + if row % 2 == 0 { 0 } else { dx / 2 };
        while cx < x + w as i32 + cell {
            hex_outline(cv, cx, cy, cell, tint, clip);
            if animate {
                let hsh = hash2(cx, cy);
                let period = 26.0 + (hsh % 5) as f32 * 7.0;
                let phase = (hsh % 97) as f32 / 97.0;
                let p = pulse01(t, period, phase);
                if p > 0.86 {
                    let strength = (p - 0.86) / 0.14;
                    hex_fill(
                        cv,
                        cx,
                        cy,
                        cell - 3,
                        mix(pal().bg0, pal().alert, strength * 0.5),
                        clip,
                    );
                }
            }
            cx += dx;
        }
    }
}

fn hash2(x: i32, y: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663);
    h ^= h >> 13;
    h = h.wrapping_mul(1_274_126_177);
    h ^ (h >> 16)
}

pub(crate) fn hex_fill(cv: &mut Canvas, cx: i32, cy: i32, r: i32, c: Color, clip: Clip) {
    if r < 2 {
        return;
    }
    let (x0, y0, x1, y1) = clip;
    for dy in -r..=r {
        let ys = cy + dy;
        if ys < y0 || ys >= y1 {
            continue;
        }
        let ady = dy.abs();
        let half = if ady <= r / 2 { r } else { 2 * (r - ady) };
        let left = (cx - half).max(x0);
        let right = (cx + half).min(x1);
        if right > left {
            cv.fill_rect(left, ys, (right - left) as u32, 1, c);
        }
    }
}

pub(crate) fn hex_outline(cv: &mut Canvas, cx: i32, cy: i32, r: i32, c: Color, clip: Clip) {
    let (mut px, mut py) = (cx, cy - r);
    for k in 1..=6 {
        let a = std::f32::consts::PI / 3.0 * k as f32 - std::f32::consts::PI / 2.0;
        let nx = cx + (r as f32 * a.cos()).round() as i32;
        let ny = cy + (r as f32 * a.sin()).round() as i32;
        line_clip(cv, px, py, nx, ny, c, clip);
        px = nx;
        py = ny;
    }
}

/// `(x0, y0, x1, y1)`, right/bottom exclusive.
pub(crate) type Clip = (i32, i32, i32, i32);

/// Bresenham with a scissor so hexes cannot spill over neighbouring panels.
fn line_clip(cv: &mut Canvas, x0: i32, y0: i32, x1: i32, y1: i32, c: Color, clip: Clip) {
    let (cx0, cy0, cx1, cy1) = clip;
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let (mut x, mut y) = (x0, y0);
    loop {
        if x >= cx0 && x < cx1 && y >= cy0 && y < cy1 {
            cv.put(x, y, c);
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

fn line(cv: &mut Canvas, x0: i32, y0: i32, x1: i32, y1: i32, c: Color) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let (mut x, mut y) = (x0, y0);
    loop {
        cv.put(x, y, c);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

const SEG_A: u8 = 0x01;
const SEG_B: u8 = 0x02;
const SEG_C: u8 = 0x04;
const SEG_D: u8 = 0x08;
const SEG_E: u8 = 0x10;
const SEG_F: u8 = 0x20;
const SEG_G: u8 = 0x40;

fn seven_segments(c: char) -> u8 {
    match c {
        '0' => SEG_A | SEG_B | SEG_C | SEG_D | SEG_E | SEG_F,
        '1' => SEG_B | SEG_C,
        '2' => SEG_A | SEG_B | SEG_G | SEG_E | SEG_D,
        '3' => SEG_A | SEG_B | SEG_G | SEG_C | SEG_D,
        '4' => SEG_F | SEG_G | SEG_B | SEG_C,
        '5' => SEG_A | SEG_F | SEG_G | SEG_C | SEG_D,
        '6' => SEG_A | SEG_F | SEG_G | SEG_E | SEG_C | SEG_D,
        '7' => SEG_A | SEG_B | SEG_C,
        '8' => SEG_A | SEG_B | SEG_C | SEG_D | SEG_E | SEG_F | SEG_G,
        '9' => SEG_A | SEG_B | SEG_F | SEG_G | SEG_C | SEG_D,
        '-' => SEG_G,
        _ => 0,
    }
}

/// Width [`seven_segment`] would use for `text` at height `h`.
pub fn seven_segment_width(text: &str, h: i32) -> i32 {
    let t = (h / 7).max(2);
    let w = (h / 2).max(8);
    let mut total = 0;
    for c in text.chars() {
        total += if c == ':' { t * 2 } else { w } + t * 2;
    }
    (total - t * 2).max(0)
}

/// Big blocky digits — the NERV timer look. Returns the advance.
pub fn seven_segment(cv: &mut Canvas, x: i32, y: i32, h: i32, text: &str, color: Color) -> i32 {
    let t = (h / 7).max(2);
    let w = (h / 2).max(8);
    let m = h / 2;
    let vlen = (m - t - t / 2).max(2) as u32;
    let mut pen = x;
    for c in text.chars() {
        if c == ':' {
            cv.fill_rect(pen + t / 2, y + h / 3 - t / 2, t as u32, t as u32, color);
            cv.fill_rect(
                pen + t / 2,
                y + 2 * h / 3 - t / 2,
                t as u32,
                t as u32,
                color,
            );
            pen += t * 4;
            continue;
        }
        let seg = seven_segments(c);
        if seg & SEG_A != 0 {
            cv.fill_rect(pen + t, y, (w - 2 * t) as u32, t as u32, color);
        }
        if seg & SEG_G != 0 {
            cv.fill_rect(pen + t, y + m - t / 2, (w - 2 * t) as u32, t as u32, color);
        }
        if seg & SEG_D != 0 {
            cv.fill_rect(pen + t, y + h - t, (w - 2 * t) as u32, t as u32, color);
        }
        if seg & SEG_F != 0 {
            cv.fill_rect(pen, y + t, t as u32, vlen, color);
        }
        if seg & SEG_B != 0 {
            cv.fill_rect(pen + w - t, y + t, t as u32, vlen, color);
        }
        if seg & SEG_E != 0 {
            cv.fill_rect(pen, y + m + t / 2, t as u32, vlen, color);
        }
        if seg & SEG_C != 0 {
            cv.fill_rect(pen + w - t, y + m + t / 2, t as u32, vlen, color);
        }
        pen += w + t * 2;
    }
    pen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_theme_parses_and_keeps_defaults() {
        let t = parse_theme("name = MY UNIT\nbg0 = #102030\naccent = f80\n", "my-unit");
        assert_eq!(t.name, "MY UNIT");
        assert_eq!(t.palette.bg0, Color::rgb(0x10, 0x20, 0x30));
        assert_eq!(t.palette.accent, Color::rgb(0xff, 0x88, 0x00));
        assert_eq!(t.palette.text, NERV.palette.text, "missing keys stay NERV");
        assert_eq!(t.palette.heat, NERV.palette.heat);
    }

    #[test]
    fn bad_user_theme_values_keep_the_defaults() {
        let t = parse_theme("bg0 = nope\nheat = 1, 2, 3\nunknown = x\n", "junk");
        assert_eq!(t.name, "junk");
        assert_eq!(t.palette.bg0, NERV.palette.bg0);
        assert_eq!(t.palette.heat, NERV.palette.heat);
    }

    #[test]
    fn the_theme_template_is_valid() {
        let t = parse_theme(THEME_TEMPLATE, "mine");
        assert_eq!(t.name, "MY THEME");
        assert_eq!(t.palette.bg0, NERV.palette.bg0);
        assert_eq!(t.palette.heat, NERV.palette.heat);
    }

    #[test]
    fn the_theme_folder_gets_a_template() {
        let dir = std::env::temp_dir().join(format!("magi_themes_ensure_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        ensure_user_themes(&dir);
        let path = dir.join("_template.ini");
        assert!(path.is_file(), "template written");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(parse_theme(&text, "t").palette.bg0, NERV.palette.bg0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn theme_files_skip_the_template_and_other_files() {
        let dir = std::env::temp_dir().join(format!("magi_themes_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["_template.ini", "b-theme.ini", "a-theme.INI", "notes.txt"] {
            std::fs::write(dir.join(name), "name = X\n").unwrap();
        }
        let files: Vec<String> = theme_files(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files, vec!["a-theme.INI", "b-theme.ini"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
