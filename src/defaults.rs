//! Fresh-card defaults for every persisted option. `Config/magi.json` only
//! stores what the user changed; anything missing falls back here, and the
//! shelf, settings screen and run host all read the same values.

pub struct Defaults;

impl Defaults {
    pub const FF_STEPS: u32 = 4;
    /// Shelf interface: 0 = Simple, 1 = Art (cover preview).
    pub const INTERFACE: u8 = 1;
    /// Fetch missing art when a system with wifi up is opened.
    pub const AUTO_ART: bool = false;
    /// ART TYPE: 0 = boxart, 1 = snap, 2 = title, 3 = logo.
    pub const ART_TYPE: u8 = 0;
    /// ART BACKGROUND: 0 = none (drop shadow), 1 = card.
    pub const ART_BG: u8 = 0;
    pub const FF_SOUND: bool = true;
    pub const TEXT_SCALE: f32 = 1.3;
    /// NERV, index 0 of `theme::themes()`.
    pub const THEME: u8 = 0;
    /// HONEYCOMB, index 1 of `theme::bg_styles()`.
    pub const BACKGROUND: u8 = 1;
    pub const SHELF_BG: bool = true;
    pub const ANIM_BG: bool = false;
    pub const GLOW: bool = false;
    pub const SHOT_CHORD: bool = false;
    /// 24-hour clock by default; the CLOCK page toggles 12-hour.
    pub const CLOCK_12H: bool = false;
    pub const BATTERY_PCT: bool = false;
    pub const LID_SLEEP: bool = true;
    pub const SHUTDOWN_MIN: u32 = 5;
    pub const IDLE_MIN: u32 = 15;
    pub const BAT_SHUTDOWN: u32 = 5;
    pub const CHARGE_LIMIT: u32 = 85;
    pub const LED_ON: bool = true;
    pub const RUMBLE: bool = true;
    pub const HDMI_AUTO: bool = false;
    pub const WIFI: bool = false;
    pub const BT: bool = false;
    pub const SSH: bool = false;
    pub const NTP: bool = false;
}
