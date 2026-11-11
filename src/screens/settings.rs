//! The settings screen (MENU): grouped MAGI options with inline values.

use crate::canvas::Canvas;
use crate::defaults::Defaults;
use crate::font::Fonts;
use crate::theme::{self, pal};
use crate::util;

pub const MAIN_ROWS: [&str; 8] = [
    "FAST FORWARD",
    "FF SOUND",
    "RUMBLE",
    "SHELF",
    "THEME",
    "NETWORK",
    "POWER MANAGEMENT",
    "SYSTEM",
];

/// Group captions and the row index they start at (main page).
const MAIN_GROUPS: [(usize, &str); 2] = [(0, "CONFIGS"), (3, "GENERAL")];
const NO_GROUPS: [(usize, &str); 0] = [];

pub const SHELF_ROWS: [&str; 4] = [
    "INTERFACE",
    "AUTO DOWNLOAD ART",
    "ART TYPE",
    "ART BACKGROUND",
];
/// The art choices; SIMPLE has no covers, so only the interface stays.
const SHELF_ROWS_SIMPLE: [&str; 1] = ["INTERFACE"];

/// ART BACKGROUND values.
pub const ART_BG_LABELS: [&str; 2] = ["NONE", "CARD"];

pub const THEME_ROWS: [&str; 6] = [
    "COLOUR THEME",
    "TEXT SIZE",
    "BACKGROUND",
    "SHELF BACKGROUND",
    "ANIM BACKGROUND",
    "GLOW FX",
];
pub const SYSTEM_ROWS: [&str; 7] = [
    "LED",
    "SCREENSHOT",
    "SCREENSHOTS",
    "HDMI AUTO",
    "CLOCK",
    "CHECK FOR UPDATES",
    "ABOUT",
];
const SYSTEM_ROWS_NO_SHOTS: [&str; 6] = [
    "LED",
    "SCREENSHOT",
    "HDMI AUTO",
    "CLOCK",
    "CHECK FOR UPDATES",
    "ABOUT",
];

pub const POWER_ROWS: [&str; 6] = [
    "LID SLEEP",
    "LID SHUTDOWN AFTER",
    "IDLE SHUTDOWN",
    "SHOW BATTERY %",
    "BATTERY SHUTDOWN",
    "CHARGE LIMIT",
];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Page {
    Main,
    Shelf,
    Theme,
    System,
    Power,
}

impl Page {
    fn subtitle(self) -> &'static str {
        match self {
            Page::Main => "SYSTEM CONFIGURATION",
            Page::Shelf => "SHELF OPTIONS",
            Page::Theme => "THEME AND DISPLAY",
            Page::System => "SYSTEM OPTIONS",
            Page::Power => "POWER MANAGEMENT",
        }
    }
}

/// The persisted preferences, shared between the settings screen, the shelf
/// and the run host via `Config/magi.json`.
#[derive(Clone, Copy)]
pub struct Prefs {
    pub ff_steps: u32,
    /// Shelf list layout (see views::View): text rows or cover art.
    pub interface: u8,
    /// Fetch missing art when a system with wifi up is opened.
    pub auto_art: bool,
    /// ART TYPE: 0 = boxart, 1 = snap, 2 = title, 3 = logo.
    pub art_type: u8,
    /// ART BACKGROUND: 0 = none (drop shadow), 1 = card.
    pub art_bg: u8,
    pub ff_sound: bool,
    pub text_scale: f32,
    pub background: u8,
    pub shelf_bg: bool,
    pub anim_bg: bool,
    pub glow: bool,
    pub theme: u8,
    pub shot_chord: bool,
    /// Show the shelf clock and CLOCK page in 12-hour form.
    pub clock_12h: bool,
    pub battery_pct: bool,
    pub lid_sleep: bool,
    pub shutdown_min: u32,
    pub idle_min: u32,
    pub bat_shutdown: u32,
    pub charge_limit: u32,
    pub led_on: bool,
    pub rumble: bool,
    /// Poll the HDMI cable on the shelf and switch the output automatically.
    /// Off means nothing touches the display output beyond game launch.
    pub hdmi_auto: bool,
}

/// SHUTDOWN AFTER steps, minutes (0 = no timer).
pub const SHUTDOWN_MINS: [u32; 5] = [0, 1, 5, 15, 30];
pub const SHUTDOWN_LABELS: [&str; 5] = ["OFF", "1 MIN", "5 MIN", "15 MIN", "30 MIN"];

/// BATTERY SHUTDOWN steps, percent (0 = off).
pub const BATTERY_SHUTDOWN_PCTS: [u32; 5] = [0, 5, 10, 15, 20];
/// CHARGE LIMIT steps, percent (0 = off; 100 = once full, until unplug).
pub const CHARGE_LIMIT_PCTS: [u32; 8] = [0, 60, 70, 80, 85, 90, 95, 100];

fn pct_label(p: u32) -> String {
    if p == 0 {
        "OFF".into()
    } else {
        format!("{p}%")
    }
}

fn pct_idx(list: &[u32], p: u32) -> usize {
    list.iter().position(|&x| x == p).unwrap_or(0)
}

pub fn shutdown_idx(min: u32) -> usize {
    SHUTDOWN_MINS.iter().position(|&m| m == min).unwrap_or(2)
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            ff_steps: Defaults::FF_STEPS,
            interface: Defaults::INTERFACE,
            auto_art: Defaults::AUTO_ART,
            art_type: Defaults::ART_TYPE,
            art_bg: Defaults::ART_BG,
            ff_sound: Defaults::FF_SOUND,
            text_scale: Defaults::TEXT_SCALE,
            background: Defaults::BACKGROUND,
            shelf_bg: Defaults::SHELF_BG,
            anim_bg: Defaults::ANIM_BG,
            glow: Defaults::GLOW,
            theme: Defaults::THEME,
            shot_chord: Defaults::SHOT_CHORD,
            clock_12h: Defaults::CLOCK_12H,
            battery_pct: Defaults::BATTERY_PCT,
            lid_sleep: Defaults::LID_SLEEP,
            shutdown_min: Defaults::SHUTDOWN_MIN,
            idle_min: Defaults::IDLE_MIN,
            bat_shutdown: Defaults::BAT_SHUTDOWN,
            charge_limit: Defaults::CHARGE_LIMIT,
            led_on: Defaults::LED_ON,
            rumble: Defaults::RUMBLE,
            hdmi_auto: Defaults::HDMI_AUTO,
        }
    }
}

pub const FF_STEPS_MIN: u32 = 2;
pub const FF_STEPS_MAX: u32 = 6;
pub const TEXT_SCALES: [f32; 3] = [1.0, 1.3, 1.5];
const TEXT_LABELS: [&str; 3] = ["S", "M", "L"];

const FIELD_RANGES: [(i32, i32); 5] = [
    (2020, 2099), // year
    (1, 12),      // month
    (1, 31),      // day
    (0, 23),      // hour
    (0, 59),      // minute
];
const FIELD_LABELS: [&str; 5] = ["YEAR", "MONTH", "DAY", "HOUR", "MIN"];

#[derive(Clone, Copy)]
pub struct ClockEdit {
    pub v: [i32; 5],
    pub field: usize,
}

impl ClockEdit {
    pub fn from_now() -> Self {
        Self {
            v: util::full_date().unwrap_or([2026, 1, 1, 0, 0]),
            field: 0,
        }
    }

    pub fn adjust(&mut self, delta: i32) {
        let (lo, hi) = FIELD_RANGES[self.field];
        let mut val = self.v[self.field] + delta;
        if val < lo {
            val = hi;
        }
        if val > hi {
            val = lo;
        }
        self.v[self.field] = val;
    }

    pub fn next_field(&mut self, delta: i32) {
        self.field = (self.field as i32 + delta).rem_euclid(5) as usize;
    }

    fn field_text(&self, i: usize, clock_12h: bool) -> String {
        match i {
            0 => format!("{:04}", self.v[0]),
            3 if clock_12h => {
                let h = match self.v[3] % 12 {
                    0 => 12,
                    h => h,
                };
                format!("{h:02}")
            }
            _ => format!("{:02}", self.v[i]),
        }
    }

    pub fn stamp(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}  {:02}:{:02}",
            self.v[0], self.v[1], self.v[2], self.v[3], self.v[4]
        )
    }
}

pub struct SettingsState {
    pub sel: usize,
    pub page: Page,
    /// The main-page row to restore after leaving a sub-page.
    pub main_sel: usize,
    pub clock: Option<ClockEdit>,
    pub prefs: Prefs,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            sel: 0,
            page: Page::Main,
            main_sel: 0,
            clock: None,
            prefs: Prefs::default(),
        }
    }
}

fn nearest_scale_idx(s: f32) -> usize {
    let mut best = 0;
    let mut dist = f32::MAX;
    for (i, v) in TEXT_SCALES.iter().enumerate() {
        let d = (v - s).abs();
        if d < dist {
            dist = d;
            best = i;
        }
    }
    best
}

fn hint_lines(fonts: &mut Fonts, text: &str, px: f32, max_w: i32, max_lines: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut lines = Vec::new();
    let mut i = 0;
    while i < words.len() && lines.len() < max_lines {
        let mut line = words[i].to_string();
        i += 1;
        while i < words.len() {
            let candidate = format!("{line} {}", words[i]);
            if fonts.text_width(&candidate, px) > max_w {
                break;
            }
            line = candidate;
            i += 1;
        }
        lines.push(fonts.elide(&line, px, max_w));
    }
    if i < words.len() {
        if let Some(last) = lines.last_mut() {
            let rest = words[i..].join(" ");
            *last = fonts.elide(&format!("{last} {rest}"), px, max_w);
        }
    }
    lines
}

fn row_description(row: &str) -> &'static str {
    match row {
        "FAST FORWARD" => "Hold R2 to fast-forward. Double-tap R2 to latch it on.",
        "FF SOUND" => "Keep game audio while fast-forwarding.",
        "RUMBLE" => "Motor feedback for supported cores and games.",
        "COLOUR THEME" => "Interface colour palette.",
        "TEXT SIZE" => "Shelf and menu text scale.",
        "BACKGROUND" => "Backdrop pattern behind the interface.",
        "SHELF BACKGROUND" => "Show the backdrop behind the shelf.",
        "ANIM BACKGROUND" => "Animate the backdrop.",
        "GLOW FX" => "Neon glow on borders and selections.",
        "INTERFACE" => "Simple list or cover-art shelf.",
        "AUTO DOWNLOAD ART" => "Fetch missing covers when a system opens.",
        "ART TYPE" => "Boxart, snap, title or logo.",
        "ART BACKGROUND" => "Pick the type of art background.",
        "LID SLEEP" => "Suspend when the lid closes.",
        "LID SHUTDOWN AFTER" => "Power off after the lid has been closed.",
        "IDLE SHUTDOWN" => "Power off after no input for this long.",
        "SHOW BATTERY %" => "Show the battery percentage in the header.",
        "BATTERY SHUTDOWN" => "Power off at this battery level.",
        "CHARGE LIMIT" => "Stop charging at this percentage.",
        "LED" => "Show power LED while the console is awake.",
        "SCREENSHOT" => "Enable the SELECT+MENU+R1 screenshot hotkey.",
        "SCREENSHOTS" => "Browse and delete screenshots.",
        "HDMI AUTO" => "Enable HDMI out when cable is connected.",
        "CLOCK" => "Set the date and time.",
        "NETWORK" => "Wi-Fi, Bluetooth, SSH and NTP.",
        "CHECK FOR UPDATES" => "Check for a new MAGI payload.",
        "ABOUT" => "Version and build information.",
        "SHELF" => "Game list and artwork options.",
        "THEME" => "Colours, text and background options.",
        "POWER MANAGEMENT" => "Battery and sleep options.",
        "SYSTEM" => "Device, network and info options.",
        _ => "",
    }
}

impl SettingsState {
    /// The rows of the page currently shown.
    pub fn rows(&self) -> &'static [&'static str] {
        match self.page {
            Page::Main => &MAIN_ROWS,
            Page::Shelf => match crate::screens::views::View::from_pref(self.prefs.interface) {
                crate::screens::views::View::Art => &SHELF_ROWS,
                crate::screens::views::View::Simple => &SHELF_ROWS_SIMPLE,
            },
            Page::Theme => &THEME_ROWS,
            Page::System => {
                if self.prefs.shot_chord {
                    &SYSTEM_ROWS
                } else {
                    &SYSTEM_ROWS_NO_SHOTS
                }
            }
            Page::Power => &POWER_ROWS,
        }
    }

    /// Enter a sub-page, remembering the main row to restore.
    pub fn open_page(&mut self, page: Page) {
        if self.page == Page::Main {
            self.main_sel = self.sel;
        }
        self.page = page;
        self.sel = 0;
    }

    /// Return to the main page. Returns false when already there.
    pub fn back_to_main(&mut self) -> bool {
        if self.page == Page::Main {
            return false;
        }
        self.page = Page::Main;
        self.sel = self.main_sel;
        true
    }

    pub fn from_prefs(prefs: Prefs) -> Self {
        Self {
            prefs: Prefs {
                ff_steps: prefs.ff_steps.clamp(FF_STEPS_MIN, FF_STEPS_MAX),
                text_scale: TEXT_SCALES[nearest_scale_idx(prefs.text_scale)],
                ..prefs
            },
            ..Self::default()
        }
    }

    pub fn to_prefs(&self) -> Prefs {
        self.prefs
    }

    pub fn adjust_interface(&mut self, delta: i32) {
        self.prefs.interface = ((self.prefs.interface as i32 + delta)
            .rem_euclid(crate::screens::views::LABELS.len() as i32))
            as u8;
        // SIMPLE hides the art rows; keep the cursor on one that exists.
        self.sel = self.sel.min(self.rows().len() - 1);
    }

    pub fn toggle_auto_art(&mut self) {
        self.prefs.auto_art = !self.prefs.auto_art;
    }

    pub fn adjust_art_type(&mut self, delta: i32) {
        self.prefs.art_type = ((self.prefs.art_type as i32 + delta)
            .rem_euclid(crate::art::LABELS.len() as i32)) as u8;
    }

    pub fn adjust_art_bg(&mut self, delta: i32) {
        self.prefs.art_bg =
            ((self.prefs.art_bg as i32 + delta).rem_euclid(ART_BG_LABELS.len() as i32)) as u8;
    }

    pub fn adjust_ff_steps(&mut self, delta: i32) {
        let v = self.prefs.ff_steps as i32 + delta;
        self.prefs.ff_steps = if v < FF_STEPS_MIN as i32 {
            FF_STEPS_MAX
        } else if v > FF_STEPS_MAX as i32 {
            FF_STEPS_MIN
        } else {
            v as u32
        };
    }

    pub fn toggle_ff_sound(&mut self) {
        self.prefs.ff_sound = !self.prefs.ff_sound;
    }

    pub fn adjust_text_scale(&mut self, delta: i32) {
        let i = nearest_scale_idx(self.prefs.text_scale) as i32;
        let n = TEXT_SCALES.len() as i32;
        self.prefs.text_scale = TEXT_SCALES[(((i + delta) % n) + n) as usize % TEXT_SCALES.len()];
    }

    pub fn adjust_shutdown(&mut self, delta: i32) {
        let n = SHUTDOWN_MINS.len() as i32;
        let i = shutdown_idx(self.prefs.shutdown_min) as i32;
        self.prefs.shutdown_min =
            SHUTDOWN_MINS[(((i + delta) % n) + n) as usize % SHUTDOWN_MINS.len()];
    }

    pub fn adjust_idle(&mut self, delta: i32) {
        let n = SHUTDOWN_MINS.len() as i32;
        let i = shutdown_idx(self.prefs.idle_min) as i32;
        self.prefs.idle_min = SHUTDOWN_MINS[(((i + delta) % n) + n) as usize % SHUTDOWN_MINS.len()];
    }

    pub fn adjust_bat_shutdown(&mut self, delta: i32) {
        let n = BATTERY_SHUTDOWN_PCTS.len() as i32;
        let i = pct_idx(&BATTERY_SHUTDOWN_PCTS, self.prefs.bat_shutdown) as i32;
        self.prefs.bat_shutdown =
            BATTERY_SHUTDOWN_PCTS[(((i + delta) % n) + n) as usize % BATTERY_SHUTDOWN_PCTS.len()];
    }

    pub fn adjust_charge_limit(&mut self, delta: i32) {
        let n = CHARGE_LIMIT_PCTS.len() as i32;
        let i = pct_idx(&CHARGE_LIMIT_PCTS, self.prefs.charge_limit) as i32;
        self.prefs.charge_limit =
            CHARGE_LIMIT_PCTS[(((i + delta) % n) + n) as usize % CHARGE_LIMIT_PCTS.len()];
    }

    pub fn adjust_theme(&mut self, delta: i32) {
        let n = crate::theme::themes().len() as i32;
        self.prefs.theme = (((self.prefs.theme as i32 + delta) % n + n) % n) as u8;
        crate::theme::set_theme_index(self.prefs.theme);
    }

    pub fn adjust_background(&mut self, delta: i32) {
        let n = crate::theme::bg_styles().len() as i32;
        self.prefs.background = ((self.prefs.background as i32 + delta).rem_euclid(n)) as u8;
    }

    /// One line of help for the selected row.
    fn description(&self) -> &'static str {
        row_description(self.rows()[self.sel.min(self.rows().len() - 1)])
    }

    /// Right-hand value for a row: the text and whether L/R adjusts it.
    fn value_text(&self, row: &str, clock: &str) -> Option<(String, bool)> {
        let onoff = |b: bool| if b { "ON" } else { "OFF" };
        match row {
            "INTERFACE" => Some((
                format!(
                    "< {} >",
                    crate::screens::views::LABELS[self.prefs.interface.min(1) as usize]
                ),
                true,
            )),
            "AUTO DOWNLOAD ART" => Some((format!("< {} >", onoff(self.prefs.auto_art)), true)),
            "ART TYPE" => {
                let i = self.prefs.art_type.min(crate::art::LABELS.len() as u8 - 1) as usize;
                Some((format!("< {} >", crate::art::LABELS[i]), true))
            }
            "ART BACKGROUND" => Some((
                format!("< {} >", ART_BG_LABELS[self.prefs.art_bg.min(1) as usize]),
                true,
            )),
            "FAST FORWARD" => Some((format!("< {}x >", self.prefs.ff_steps), true)),
            "FF SOUND" => Some((format!("< {} >", onoff(self.prefs.ff_sound)), true)),
            "COLOUR THEME" => Some((
                format!(
                    "< {} >",
                    crate::theme::themes()
                        [self.prefs.theme as usize % crate::theme::themes().len()]
                    .name
                ),
                true,
            )),
            "TEXT SIZE" => Some((
                format!(
                    "< {} >",
                    TEXT_LABELS[nearest_scale_idx(self.prefs.text_scale)]
                ),
                true,
            )),
            "BACKGROUND" => Some((
                format!(
                    "< {} >",
                    crate::theme::bg_styles()
                        [self.prefs.background as usize % crate::theme::bg_styles().len()]
                ),
                true,
            )),
            "SHELF BACKGROUND" => Some((format!("< {} >", onoff(self.prefs.shelf_bg)), true)),
            "ANIM BACKGROUND" => Some((format!("< {} >", onoff(self.prefs.anim_bg)), true)),
            "GLOW FX" => Some((format!("< {} >", onoff(self.prefs.glow)), true)),
            "SCREENSHOT" => Some((format!("< {} >", onoff(self.prefs.shot_chord)), true)),
            "SHOW BATTERY %" => Some((format!("< {} >", onoff(self.prefs.battery_pct)), true)),
            "RUMBLE" => Some((format!("< {} >", onoff(self.prefs.rumble)), true)),
            "LED" => Some((format!("< {} >", onoff(self.prefs.led_on)), true)),
            "HDMI AUTO" => Some((format!("< {} >", onoff(self.prefs.hdmi_auto)), true)),
            "LID SLEEP" => Some((format!("< {} >", onoff(self.prefs.lid_sleep)), true)),
            "LID SHUTDOWN AFTER" => Some((
                format!(
                    "< {} >",
                    SHUTDOWN_LABELS[shutdown_idx(self.prefs.shutdown_min)]
                ),
                true,
            )),
            "IDLE SHUTDOWN" => Some((
                format!("< {} >", SHUTDOWN_LABELS[shutdown_idx(self.prefs.idle_min)]),
                true,
            )),
            "BATTERY SHUTDOWN" => {
                Some((format!("< {} >", pct_label(self.prefs.bat_shutdown)), true))
            }
            "CHARGE LIMIT" => Some((format!("< {} >", pct_label(self.prefs.charge_limit)), true)),
            "CLOCK" => Some((clock.to_string(), false)),
            "THEME" | "SHELF" | "SYSTEM" | "NETWORK" | "ABOUT" | "POWER MANAGEMENT"
            | "SCREENSHOTS" | "CHECK FOR UPDATES" => Some((">".into(), false)),
            _ => None,
        }
    }
}

pub fn draw(cv: &mut Canvas, fonts: &mut Fonts, st: &SettingsState, t: f32) {
    let (w, h) = (cv.w, cv.h);
    let s = theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    cv.clear(pal().bg0);
    theme::background(
        cv,
        p(10.0),
        p(10.0),
        (w - p(20.0)).max(1) as u32,
        (h - p(20.0)).max(1) as u32,
        t,
    );
    theme::glow_border(
        cv,
        p(8.0),
        p(8.0),
        (w - p(16.0)).max(1) as u32,
        (h - p(16.0)).max(1) as u32,
        pal().border,
        pal().glow,
        t,
        0.0,
    );

    let title = match st.page {
        Page::Main => "MAGI // SETTINGS",
        Page::Shelf => "MAGI // SHELF",
        Page::Theme => "MAGI // THEME",
        Page::System => "MAGI // SYSTEM",
        Page::Power => "MAGI // POWER",
    };
    let left = 24;
    let right = w - 24;
    let title_base = 30;
    fonts.draw_bold(cv, left, title_base, title, 15.0, pal().title);
    fonts.draw(
        cv,
        left,
        title_base + 18,
        st.page.subtitle(),
        9.0,
        pal().text_faint,
    );
    let div_y = 60;
    cv.hline(left, div_y, (right - left).max(1) as u32, pal().border);
    cv.fill_rect(left, div_y, 160, 3, pal().accent);

    let clock = util::full_date()
        .map(|v| {
            format!(
                "{:04}-{:02}-{:02}  {:02}:{:02}",
                v[0], v[1], v[2], v[3], v[4]
            )
        })
        .unwrap_or_else(|| "--".into());

    let x0 = 38;
    let row_w = (w - 76).max(400);
    let rows = st.rows();
    let groups: &[(usize, &str)] = match st.page {
        Page::Main => &MAIN_GROUPS,
        _ => &NO_GROUPS,
    };
    let mut y = match st.page {
        Page::Main => 96,
        _ => 108,
    };
    for (i, label) in rows.iter().enumerate() {
        for &(grow, gname) in groups {
            if grow == i {
                if i > 0 {
                    y += 14;
                }
                fonts.draw(cv, x0, y - 2, gname, 9.0, pal().text_faint);
                y += 18;
            }
        }
        let selected = i == st.sel;
        if selected {
            cv.fill_rect(x0 - 8, y - 16, row_w as u32 + 16, 24, pal().bg3);
            theme::glow_border(
                cv,
                x0 - 8,
                y - 16,
                row_w as u32 + 16,
                24,
                pal().glow,
                theme::mix(pal().glow, pal().accent, 0.35),
                t,
                0.4,
            );
        }
        fonts.draw(
            cv,
            x0 + 6,
            y,
            label,
            12.0,
            if selected {
                pal().signal
            } else {
                pal().text_dim
            },
        );
        if let Some((value, adjustable)) = st.value_text(label, &clock) {
            let tw = fonts.text_width(&value, 11.0);
            let color = if selected && adjustable {
                pal().signal
            } else {
                pal().text
            };
            fonts.draw(cv, x0 + row_w - 10 - tw, y, &value, 11.0, color);
        }
        y += 28;
    }

    let notice_w = (w - 280).clamp(320, 460);
    let notice_x = w / 2 - notice_w / 2;
    let notice_y = h - 100;
    let notice_h = 42;
    cv.fill_rect_a(
        notice_x,
        notice_y,
        notice_w as u32,
        notice_h as u32,
        pal().bg2,
        238,
    );
    cv.rect_outline(
        notice_x,
        notice_y,
        notice_w as u32,
        notice_h as u32,
        theme::mix(pal().border, pal().accent, 0.45),
    );
    cv.fill_rect(notice_x, notice_y, 3, notice_h as u32, pal().accent);
    let lines = hint_lines(fonts, st.description(), 9.0, notice_w - 20, 2);
    let line_h = p(11.5);
    let start = notice_y + (notice_h - line_h * lines.len() as i32) / 2 + p(9.0);
    for (i, line) in lines.iter().enumerate() {
        fonts.draw(
            cv,
            notice_x + 10,
            start + i as i32 * line_h,
            line,
            9.0,
            pal().text_dim,
        );
    }

    crate::status::draw_center(cv, fonts, h - 44);
    fonts.draw_center(
        cv,
        w / 2,
        h - 20,
        "U/D PICK   L/R CHANGE   A SELECT   B BACK",
        9.0,
        pal().text_faint,
    );
}
pub fn draw_clock(cv: &mut Canvas, fonts: &mut Fonts, ce: &ClockEdit, clock_12h: bool, t: f32) {
    let (w, h) = (cv.w, cv.h);
    cv.clear(pal().bg0);
    theme::background(cv, 12, 64, (w - 32) as u32, (h - 124) as u32, t);
    theme::glow_border(
        cv,
        8,
        8,
        (w - 16) as u32,
        (h - 16) as u32,
        pal().border,
        pal().glow,
        t,
        0.0,
    );
    fonts.draw_center(
        cv,
        w / 2,
        46,
        "CLOCK // SET DATE AND TIME",
        14.0,
        pal().title,
    );
    let widths = [104, 66, 66, 66, 66];
    let gap = 16;
    let suffix_w = if clock_12h { 30 } else { 0 };
    let total: i32 = widths.iter().sum::<i32>() + gap * 4 + suffix_w;
    let mut x = w / 2 - total / 2;
    let y = h / 2 - 30;
    let mut min_x = 0;

    for (i, fw) in widths.iter().enumerate() {
        let selected = i == ce.field;
        cv.rect_outline(x, y, *fw as u32, 60, pal().bg2);
        if selected {
            theme::glow_border(
                cv,
                x,
                y,
                *fw as u32,
                60,
                pal().glow,
                pal().accent,
                t,
                0.2 * i as f32,
            );
        } else {
            cv.rect_outline(x, y, *fw as u32, 60, pal().border);
        }
        // The digits in NERV timer style.
        let digits = ce.field_text(i, clock_12h);
        let dw = theme::seven_segment_width(&digits, 30);
        theme::seven_segment(cv, x + (fw - dw) / 2, y + 16, 30, &digits, pal().clock);
        fonts.draw_center(
            cv,
            x + fw / 2,
            y - 10,
            FIELD_LABELS[i],
            9.0,
            pal().text_faint,
        );
        if i == 4 {
            min_x = x;
        }
        x += fw + gap;
    }

    if clock_12h {
        let pm = ce.v[3] >= 12;
        let sx = min_x + widths[4] + 8;
        let (am, pm_col) = if pm {
            (pal().text_faint, pal().clock)
        } else {
            (pal().clock, pal().text_faint)
        };
        fonts.draw(cv, sx, y + 26, "AM", 9.0, am);
        fonts.draw(cv, sx, y + 44, "PM", 9.0, pm_col);
    }

    crate::status::draw_center(cv, fonts, y + 100);
    fonts.draw_center(
        cv,
        w / 2,
        h - 26,
        "U/D ADJUST   A APPLY   X 12/24H   B CANCEL",
        9.0,
        pal().text_faint,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shelf_page_offers_the_interface() {
        assert!(
            SHELF_ROWS.contains(&"INTERFACE"),
            "no INTERFACE row to switch Simple/Art"
        );
        assert!(
            !THEME_ROWS.contains(&"INTERFACE"),
            "INTERFACE belongs on the SHELF page"
        );
    }

    #[test]
    fn shelf_rows_offer_values_and_the_action_is_not_adjustable() {
        let st = SettingsState::default();
        for row in SHELF_ROWS {
            let (_, adjustable) = st
                .value_text(row, "--")
                .unwrap_or_else(|| panic!("{row} has no value"));
            assert!(adjustable, "{row} adjustability");
        }
    }

    #[test]
    fn simple_hides_the_art_rows() {
        let mut st = SettingsState {
            page: Page::Shelf,
            ..Default::default()
        };
        assert_eq!(st.prefs.interface, 1, "ART is the default");
        assert_eq!(st.rows().len(), SHELF_ROWS.len());
        st.adjust_interface(1);
        assert_eq!(st.prefs.interface, 0);
        assert_eq!(st.rows().len(), 1);
        assert_eq!(st.rows()[0], "INTERFACE");
    }

    #[test]
    fn switching_to_simple_keeps_the_cursor_on_a_row() {
        let mut st = SettingsState {
            page: Page::Shelf,
            sel: SHELF_ROWS.len() - 1,
            ..Default::default()
        };
        st.adjust_interface(1);
        assert_eq!(st.prefs.interface, 0);
        assert_eq!(st.rows()[st.sel], "INTERFACE");
    }

    #[test]
    fn every_theme_row_has_a_value() {
        let st = SettingsState::default();
        for row in THEME_ROWS {
            assert!(st.value_text(row, "--").is_some(), "{row} has no value");
        }
    }

    #[test]
    fn every_settings_row_has_a_description() {
        let mut st = SettingsState::default();
        st.prefs.shot_chord = true;
        for page in [
            Page::Main,
            Page::Shelf,
            Page::Theme,
            Page::System,
            Page::Power,
        ] {
            st.page = page;
            for row in st.rows() {
                assert!(!row_description(row).is_empty(), "{row} has no description");
            }
        }
    }

    #[test]
    fn leaving_a_subpage_restores_the_main_row() {
        let mut st = SettingsState {
            sel: 5,
            ..Default::default()
        };
        st.open_page(Page::Shelf);
        assert_eq!(st.page, Page::Shelf);
        assert_eq!(st.sel, 0);
        assert!(st.back_to_main());
        assert_eq!(st.page, Page::Main);
        assert_eq!(st.sel, 5);
        assert!(!st.back_to_main(), "already on the main page");
    }

    #[test]
    fn screenshots_row_follows_the_shot_toggle() {
        let mut st = SettingsState {
            page: Page::System,
            ..Default::default()
        };
        st.prefs.shot_chord = false;
        assert!(!st.rows().contains(&"SCREENSHOTS"));
        st.prefs.shot_chord = true;
        let rows = st.rows();
        let shot = rows
            .iter()
            .position(|r| *r == "SCREENSHOT")
            .expect("SCREENSHOT row");
        assert_eq!(rows[shot + 1], "SCREENSHOTS");
    }
}
