//! Input abstraction: one `Action` vocabulary for keyboard, joystick buttons,
//! hats and axes. Per-device button-index tables land here in M1; M0 uses the
//! standard Xbox-family layout the H700 pads present.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sdl2::keyboard::Keycode;

/// USB vendor/product pairs owned by SDL's game-controller layer. The evdev
/// reader skips these so a Bluetooth/USB pad never fires twice.
static SDL_OWNED: Mutex<Vec<(u16, u16)>> = Mutex::new(Vec::new());

pub fn set_sdl_owned(ids: Vec<(u16, u16)>) {
    if let Ok(mut v) = SDL_OWNED.lock() {
        *v = ids;
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn sdl_owns(vendor: u16, product: u16) -> bool {
    SDL_OWNED
        .lock()
        .map(|v| v.contains(&(vendor, product)))
        .unwrap_or(false)
}

/// The screenshot chord (SELECT+MENU+R1) can be switched off in MISC; with
/// it off, MENU and SELECT act at once with no chord arbitration.
static SHOT_CHORD: AtomicBool = AtomicBool::new(true);

pub fn set_shot_chord(on: bool) {
    SHOT_CHORD.store(on, Ordering::Relaxed);
}

pub fn shot_chord() -> bool {
    SHOT_CHORD.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Back,
    X,
    Y,
    L1,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    L2,
    R1,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    R2,
    Start,
    Select,
    Menu,
    Power,
    /// The POWER button went back up (the press/hold decision point).
    PowerRelease,
    /// The Hall sensor reported the lid closed (a sleep request).
    LidClosed,
    /// SELECT + d-pad: brightness and warmth, on the shelf and in game.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    BrightnessUp,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    BrightnessDown,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    WarmthUp,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    WarmthDown,
    VolumeUp,
    VolumeDown,
    Screenshot,
    /// SELECT+START: repair a read-only SD card.
    CardRepair,
    Quit,
}

/// Desktop dev mapping. On device the pad is a joystick, not a keyboard.
pub fn key_to_action(key: Keycode) -> Option<Action> {
    match key {
        Keycode::Up | Keycode::W => Some(Action::Up),
        Keycode::Down | Keycode::S => Some(Action::Down),
        Keycode::Left | Keycode::A => Some(Action::Left),
        Keycode::Right | Keycode::D => Some(Action::Right),
        Keycode::Z => Some(Action::Confirm),
        Keycode::X => Some(Action::Back),
        Keycode::C => Some(Action::X),
        Keycode::V => Some(Action::Y),
        Keycode::Q => Some(Action::L1),
        Keycode::E => Some(Action::R1),
        Keycode::Minus => Some(Action::VolumeDown),
        Keycode::Equals => Some(Action::VolumeUp),
        Keycode::F12 => Some(Action::Screenshot),
        Keycode::Return => Some(Action::Start),
        Keycode::RShift | Keycode::LShift => Some(Action::Select),
        Keycode::Tab => Some(Action::Menu),
        Keycode::P => Some(Action::Power),
        Keycode::Escape => Some(Action::Quit),
        _ => None,
    }
}

/// Linux input-event-codes.h values used by [`evdev_action`] and the
/// run host's joypad mapping.
pub mod ev {
    #[cfg(target_os = "linux")]
    pub const EV_SYN: u16 = 0x00;
    pub const EV_KEY: u16 = 0x01;
    pub const EV_ABS: u16 = 0x03;
    pub const EV_SW: u16 = 0x05;
    #[cfg(target_os = "linux")]
    pub const SYN_REPORT: u16 = 0x00;
    #[cfg(target_os = "linux")]
    pub const SYN_DROPPED: u16 = 0x03;
    pub const SW_LID: u16 = 0x00;
    pub const KEY_UP: u16 = 0x67;
    pub const KEY_LEFT: u16 = 0x69;
    pub const KEY_RIGHT: u16 = 0x6A;
    pub const KEY_DOWN: u16 = 0x6C;
    pub const KEY_POWER: u16 = 0x74;
    pub const KEY_VOLUMEUP: u16 = 0x73;
    pub const KEY_VOLUMEDOWN: u16 = 0x72;
    pub const KEY_GOTO: u16 = 0x162;
    pub const BTN_SOUTH: u16 = 0x130;
    pub const BTN_EAST: u16 = 0x131;
    pub const BTN_C: u16 = 0x132;
    pub const BTN_NORTH: u16 = 0x133;
    pub const BTN_WEST: u16 = 0x134;
    pub const BTN_Z: u16 = 0x135;
    pub const BTN_TL: u16 = 0x136;
    pub const BTN_TR: u16 = 0x137;
    pub const BTN_TL2: u16 = 0x138;
    #[allow(dead_code)]
    pub const BTN_TR2: u16 = 0x139;
    pub const BTN_SELECT: u16 = 0x13A;
    pub const BTN_START: u16 = 0x13B;
    #[allow(dead_code)]
    pub const BTN_MODE: u16 = 0x13C;
    pub const BTN_DPAD_UP: u16 = 0x220;
    pub const BTN_DPAD_DOWN: u16 = 0x221;
    pub const BTN_DPAD_LEFT: u16 = 0x222;
    pub const BTN_DPAD_RIGHT: u16 = 0x223;
    pub const ABS_Z: u16 = 0x02;
    pub const ABS_RZ: u16 = 0x05;
    pub const ABS_X: u16 = 0x00;
    pub const ABS_Y: u16 = 0x01;
    pub const ABS_RX: u16 = 0x03;
    pub const ABS_RY: u16 = 0x04;
    pub const ABS_HAT0X: u16 = 0x10;
    pub const ABS_HAT0Y: u16 = 0x11;
}

/// Apply one key event to the EVIOCGKEY-layout bitset (768 keys, 96 bytes):
/// the reader keeps it so a dropped event batch can be resynced.
#[cfg(target_os = "linux")]
pub fn track_key(keys: &mut [u8; 96], code: u16, value: i32) {
    let i = (code / 8) as usize;
    if i >= keys.len() {
        return;
    }
    if value != 0 {
        keys[i] |= 1 << (code % 8);
    } else {
        keys[i] &= !(1 << (code % 8));
    }
}

/// Map a standard gamepad (hid/xpad/xbox) key code onto the vendor pad's
/// scrambled codes, so one downstream table serves both. Standard pads are
/// recognised by name in the reader; their events are normalized here.
#[cfg(target_os = "linux")]
pub fn standard_code(code: u16) -> u16 {
    use ev::*;
    match code {
        BTN_WEST => BTN_C,     // Y
        BTN_TL => BTN_WEST,    // L1
        BTN_TR => BTN_Z,       // R1
        BTN_SELECT => BTN_TL,  // SHARE / VIEW
        BTN_START => BTN_TR,   // START / MENU
        BTN_MODE => BTN_TL2,   // GUIDE -> MAGI MENU
        BTN_TL2 => BTN_SELECT, // L2
        BTN_TR2 => BTN_START,  // R2
        _ => code,
    }
}

/// Map an Xbox/XInput pad onto the vendor's scrambled codes. HOGP pads follow
/// the generic HID button order: X is BTN_C, Y is BTN_NORTH, LB is BTN_WEST,
/// RB is BTN_Z, View is BTN_TL, Menu is BTN_TR and Guide is BTN_TL2 - only
/// X/Y need swapping. The stick clicks and Share arrive as SELECT/START/TR2
/// and are not face buttons, so they are silenced (code 0 is unmapped). The
/// analog triggers are axes (ABS_Z/ABS_RZ), handled by the run host.
#[cfg(target_os = "linux")]
pub fn xbox_code(code: u16) -> u16 {
    use ev::*;
    match code {
        BTN_C => BTN_NORTH,
        BTN_NORTH => BTN_C,
        BTN_MODE => BTN_TL2,
        BTN_SELECT | BTN_START | BTN_TR2 => 0,
        _ => code,
    }
}

/// Map a raw evdev (type, code, value) triple to an action. Direction keys
/// honour autorepeat (value 2); buttons fire once, on press (value 1).
/// The vendor scrambled the BTN_* names on the H700 ("ANBERNIC-keys" uses
/// SOUTH/EAST for A/B but C/NORTH for Y/X, WEST/Z for L1/R1, TL/TR for
/// SELECT/START, TL2 for MENU, SELECT/START for L2/R2) — this table matches
/// the layout the sibling frontends ship for these devices.
#[cfg(any(target_os = "linux", test))]
pub fn evdev_action(type_: u16, code: u16, value: i32) -> Option<Action> {
    use ev::*;
    match type_ {
        EV_KEY => {
            let dir = match code {
                KEY_UP | BTN_DPAD_UP => Some(Action::Up),
                KEY_DOWN | BTN_DPAD_DOWN => Some(Action::Down),
                KEY_LEFT | BTN_DPAD_LEFT => Some(Action::Left),
                KEY_RIGHT | BTN_DPAD_RIGHT => Some(Action::Right),
                _ => None,
            };
            if let Some(a) = dir {
                return if value >= 1 { Some(a) } else { None };
            }
            if code == KEY_POWER && value == 0 {
                return Some(Action::PowerRelease);
            }
            if value != 1 {
                return None;
            }
            match code {
                BTN_SOUTH => Some(Action::Confirm),
                BTN_EAST => Some(Action::Back),
                BTN_C => Some(Action::Y),
                BTN_NORTH => Some(Action::X),
                BTN_WEST => Some(Action::L1),
                BTN_Z => Some(Action::R1),
                BTN_TL => Some(Action::Select),
                BTN_TR => Some(Action::Start),
                BTN_TL2 | KEY_GOTO => Some(Action::Menu),
                BTN_SELECT => Some(Action::L2),
                BTN_START => Some(Action::R2),
                KEY_POWER => Some(Action::Power),
                KEY_VOLUMEUP => Some(Action::VolumeUp),
                KEY_VOLUMEDOWN => Some(Action::VolumeDown),
                _ => None,
            }
        }
        EV_ABS => match code {
            ABS_HAT0X => match value {
                -1 => Some(Action::Left),
                1 => Some(Action::Right),
                _ => None,
            },
            ABS_HAT0Y => match value {
                -1 => Some(Action::Up),
                1 => Some(Action::Down),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// One raw evdev-style event; device index included.
#[derive(Clone, Copy)]
pub struct RawEvent {
    /// Index into the reader's device list; used by the UI's action logging.
    #[allow(dead_code)]
    pub dev: usize,
    pub type_: u16,
    pub code: u16,
    pub value: i32,
}

/// libretro joypad button ids (RETRO_DEVICE_ID_JOYPAD_*).
pub mod retro {
    pub const B: u16 = 0;
    pub const Y: u16 = 1;
    pub const SELECT: u16 = 2;
    pub const START: u16 = 3;
    pub const UP: u16 = 4;
    pub const DOWN: u16 = 5;
    pub const LEFT: u16 = 6;
    pub const RIGHT: u16 = 7;
    pub const A: u16 = 8;
    pub const X: u16 = 9;
    pub const L: u16 = 10;
    pub const R: u16 = 11;
    pub const L2: u16 = 12;
    pub const R2: u16 = 13;
    pub const MASK: u16 = 256;
}

/// evdev key code -> libretro joypad bit (bit index == joypad id), using the
/// vendor's actual button layout (see [`evdev_action`]).
pub fn joypad_bit(code: u16) -> Option<u16> {
    use ev::*;
    match code {
        BTN_SOUTH => Some(retro::A),
        BTN_EAST => Some(retro::B),
        BTN_C => Some(retro::Y),
        BTN_NORTH => Some(retro::X),
        BTN_WEST => Some(retro::L),
        BTN_Z => Some(retro::R),
        BTN_TL => Some(retro::SELECT),
        BTN_TR => Some(retro::START),
        BTN_SELECT => Some(retro::L2),
        BTN_START => Some(retro::R2),
        _ => None,
    }
}

/// The selection cluster: SELECT, MENU and R1 share the screenshot chord.
pub const CODES_SELECT: [u16; 1] = [ev::BTN_TL];
pub const CODES_MENU: [u16; 2] = [ev::BTN_TL2, ev::KEY_GOTO];
pub const CODE_R1: u16 = ev::BTN_Z;

#[cfg(any(target_os = "linux", test))]
pub fn tracked_code(code: u16) -> bool {
    CODES_SELECT.contains(&code) || CODES_MENU.contains(&code) || code == CODE_R1
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    /// SELECT+MENU+R1 held together, or struck in quick succession.
    Screenshot,
    /// SELECT released without the chord.
    SelectTap,
    /// MENU released without the chord.
    MenuTap,
}

/// The chord fires when the three are held together, or when all three were
/// struck within this window (tapping SELECT, MENU, R1 in a row works).
pub const CHORD_WINDOW: Duration = Duration::from_millis(600);
/// Taps are emitted this late, so a chord can still claim the gesture and no
/// panel flashes behind a screenshot.
pub const TAP_GRACE: Duration = Duration::from_millis(350);
/// The vendor pad emits MENU twice per press (BTN_TL2, then the KEY_GOTO
/// alias right as the first releases); an alias press this soon after any
/// menu-code release is the same gesture, not a second tap.
const MENU_ALIAS: Duration = Duration::from_millis(150);

/// Watches the selection cluster: MENU/SELECT act on release, the screenshot
/// chord swallows their taps, and taps are grace-delayed.
#[derive(Default)]
pub struct Shortcuts {
    held: [bool; 3],
    struck: [Option<Instant>; 3],
    fired: bool,
    pending: Option<(Shortcut, Instant)>,
    /// MENU has two codes (BTN_TL2 and the KEY_GOTO alias); each release
    /// knows whether its press was already answered.
    menu_sent: [bool; 2],
    menu_released: Option<Instant>,
    /// The SELECT release after a combo must not fire its tap (CONTROLS).
    select_combo: bool,
}

impl Shortcuts {
    /// Called when a SELECT + d-pad combination was used, so the SELECT
    /// release stays silent.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn combo_used(&mut self) {
        self.select_combo = true;
    }

    fn slot(code: u16) -> Option<usize> {
        if CODES_SELECT.contains(&code) {
            Some(0)
        } else if CODES_MENU.contains(&code) {
            Some(1)
        } else if code == CODE_R1 {
            Some(2)
        } else {
            None
        }
    }

    fn menu_idx(code: u16) -> usize {
        if code == ev::BTN_TL2 {
            0
        } else {
            1
        }
    }

    pub fn feed(&mut self, e: &RawEvent) -> Option<Shortcut> {
        if e.type_ != ev::EV_KEY {
            return None;
        }
        let slot = Self::slot(e.code)?;
        let now = Instant::now();
        let idx = Self::menu_idx(e.code);
        let menu_was_held = self.held[1];

        if e.value >= 1 {
            self.held[slot] = true;
            self.struck[slot] = Some(now);
            let within =
                |t: Option<Instant>| t.is_some_and(|t| now.duration_since(t) <= CHORD_WINDOW);
            if shot_chord() && !self.fired {
                let succession = (0..3).all(|i| within(self.struck[i]));
                if succession || self.held.iter().all(|h| *h) {
                    self.fired = true;
                    self.pending = None;
                    self.menu_sent = [false; 2];
                    return Some(Shortcut::Screenshot);
                }
            }
            // MENU acts at once — waiting the tap grace made the menus feel
            // laggy — unless a chord could still be in flight (SELECT or R1
            // struck first): then the tap keeps its grace so a screenshot
            // never flashes a panel. The vendor's second code for the same
            // press (BTN_TL2 then KEY_GOTO, the alias right as TL2 releases
            // or while it is still down) is one gesture and is swallowed.
            if slot == 1 && !self.fired {
                let alias = menu_was_held
                    || self
                        .menu_released
                        .is_some_and(|t| now.duration_since(t) < MENU_ALIAS);
                let chord_possible = shot_chord()
                    && (self.held[0]
                        || self.held[2]
                        || within(self.struck[0])
                        || within(self.struck[2]));
                if alias {
                    self.menu_sent[idx] = true; // its release adds nothing
                } else if !chord_possible {
                    self.menu_sent[idx] = true;
                    return Some(Shortcut::MenuTap);
                }
            }
            None
        } else {
            self.held[slot] = false;
            if slot == 1 {
                self.menu_released = Some(now);
            }
            if !self.fired {
                if slot == 1 && std::mem::take(&mut self.menu_sent[idx]) {
                    // Already acted on the press; no delayed tap.
                } else if slot == 0 && std::mem::take(&mut self.select_combo) {
                    // A SELECT combination consumed the gesture.
                } else {
                    let tap = match slot {
                        0 => Shortcut::SelectTap,
                        1 => Shortcut::MenuTap,
                        _ => return None,
                    };
                    let grace = if shot_chord() {
                        TAP_GRACE
                    } else {
                        Duration::ZERO
                    };
                    self.pending = Some((tap, now + grace));
                }
            }
            if !self.held.iter().any(|h| *h) {
                self.fired = false;
            }
            None
        }
    }

    /// Emit a grace-delayed tap once its window passes.
    pub fn tick(&mut self, now: Instant) -> Option<Shortcut> {
        if let Some((sc, at)) = self.pending {
            if now >= at {
                self.pending = None;
                return Some(sc);
            }
        }
        None
    }
}

/// Left-stick-as-dpad with initial delay and auto-repeat, per axis.
pub struct AxisRepeat {
    dirs: [Option<Action>; 2],
    next: [Instant; 2],
}

const AXIS_DEADZONE: i16 = 16_000;
const AXIS_FIRST_DELAY: Duration = Duration::from_millis(320);
const AXIS_REPEAT: Duration = Duration::from_millis(110);

impl AxisRepeat {
    pub fn new() -> Self {
        Self {
            dirs: [None, None],
            next: [Instant::now(), Instant::now()],
        }
    }

    /// Feed axis motion; returns an action on a fresh push.
    pub fn on_axis(&mut self, axis: u8, value: i16) -> Option<Action> {
        if axis > 1 {
            return None;
        }
        let i = axis as usize;
        let new_dir = if value <= -AXIS_DEADZONE {
            Some(if axis == 0 { Action::Left } else { Action::Up })
        } else if value >= AXIS_DEADZONE {
            Some(if axis == 0 {
                Action::Right
            } else {
                Action::Down
            })
        } else {
            None
        };
        if new_dir == self.dirs[i] {
            return None;
        }
        self.dirs[i] = new_dir;
        self.next[i] = Instant::now() + AXIS_FIRST_DELAY;
        new_dir
    }

    /// Call each frame; returns a repeat while an axis stays deflected.
    pub fn tick(&mut self, now: Instant) -> Option<Action> {
        for i in 0..2 {
            if let Some(dir) = self.dirs[i] {
                if now >= self.next[i] {
                    self.next[i] = now + AXIS_REPEAT;
                    return Some(dir);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_has_essentials() {
        assert_eq!(key_to_action(Keycode::Z), Some(Action::Confirm));
        assert_eq!(key_to_action(Keycode::Up), Some(Action::Up));
        assert_eq!(key_to_action(Keycode::F1), None);
    }

    #[test]
    fn evdev_maps_dpad_and_buttons() {
        use ev::*;
        assert_eq!(evdev_action(EV_KEY, KEY_UP, 1), Some(Action::Up));
        assert_eq!(evdev_action(EV_KEY, KEY_UP, 2), Some(Action::Up), "repeat");
        assert_eq!(evdev_action(EV_KEY, KEY_UP, 0), None);
        assert_eq!(evdev_action(EV_KEY, BTN_SOUTH, 1), Some(Action::Confirm));
        assert_eq!(evdev_action(EV_KEY, BTN_SOUTH, 2), None, "no button repeat");
        assert_eq!(evdev_action(EV_KEY, BTN_EAST, 1), Some(Action::Back));
        // The vendor's scrambled layout (see evdev_action docs).
        assert_eq!(evdev_action(EV_KEY, BTN_C, 1), Some(Action::Y));
        assert_eq!(evdev_action(EV_KEY, BTN_NORTH, 1), Some(Action::X));
        assert_eq!(evdev_action(EV_KEY, BTN_WEST, 1), Some(Action::L1));
        assert_eq!(evdev_action(EV_KEY, BTN_Z, 1), Some(Action::R1));
        assert_eq!(evdev_action(EV_KEY, BTN_TL, 1), Some(Action::Select));
        assert_eq!(evdev_action(EV_KEY, BTN_TR, 1), Some(Action::Start));
        assert_eq!(evdev_action(EV_KEY, BTN_TL2, 1), Some(Action::Menu));
        assert_eq!(evdev_action(EV_KEY, BTN_SELECT, 1), Some(Action::L2));
        assert_eq!(evdev_action(EV_KEY, BTN_START, 1), Some(Action::R2));
        assert_eq!(
            evdev_action(EV_KEY, KEY_GOTO, 1),
            Some(Action::Menu),
            "the MENU alias"
        );
        assert_eq!(
            evdev_action(EV_KEY, KEY_VOLUMEUP, 1),
            Some(Action::VolumeUp)
        );
        assert_eq!(
            evdev_action(EV_KEY, KEY_VOLUMEDOWN, 1),
            Some(Action::VolumeDown)
        );
        assert_eq!(evdev_action(EV_KEY, KEY_POWER, 1), Some(Action::Power));
        assert_eq!(evdev_action(EV_KEY, KEY_POWER, 2), None, "no power repeat");
        assert_eq!(evdev_action(EV_ABS, ABS_HAT0Y, -1), Some(Action::Up));
        assert_eq!(evdev_action(EV_ABS, ABS_HAT0Y, 1), Some(Action::Down));
        assert_eq!(evdev_action(EV_ABS, ABS_HAT0X, 0), None);
        assert_eq!(evdev_action(EV_KEY, 0xF1, 1), None, "unknown code");
    }

    #[test]
    fn joypad_bits_are_retro_ids() {
        use ev::*;
        assert_eq!(joypad_bit(BTN_SOUTH), Some(retro::A));
        assert_eq!(joypad_bit(BTN_EAST), Some(retro::B));
        assert_eq!(joypad_bit(BTN_C), Some(retro::Y));
        assert_eq!(joypad_bit(BTN_NORTH), Some(retro::X));
        assert_eq!(joypad_bit(BTN_WEST), Some(retro::L));
        assert_eq!(joypad_bit(BTN_Z), Some(retro::R));
        assert_eq!(joypad_bit(BTN_TL), Some(retro::SELECT));
        assert_eq!(joypad_bit(BTN_TR), Some(retro::START));
        assert_eq!(joypad_bit(BTN_SELECT), Some(retro::L2));
        assert_eq!(joypad_bit(BTN_START), Some(retro::R2));
        assert_eq!(joypad_bit(BTN_TL2), None, "MENU is not a joypad button");
    }

    fn key(code: u16, value: i32) -> RawEvent {
        RawEvent {
            dev: 0,
            type_: ev::EV_KEY,
            code,
            value,
        }
    }

    #[test]
    fn tracked_codes_stay_consistent() {
        assert!(tracked_code(ev::BTN_TL), "the SP's SELECT");
        assert!(tracked_code(ev::BTN_TL2), "the SP's MENU");
        assert!(tracked_code(ev::KEY_GOTO), "the MENU alias");
        assert!(tracked_code(ev::BTN_Z), "R1");
        assert!(!tracked_code(ev::BTN_SOUTH));
    }

    #[test]
    fn menu_waits_when_a_chord_may_be_coming() {
        let mut s = Shortcuts::default();
        assert_eq!(s.feed(&key(0x136, 1)), None, "select first");
        assert_eq!(s.feed(&key(0x138, 1)), None, "menu holds back");
        assert_eq!(
            s.feed(&key(CODE_R1, 1)),
            Some(Shortcut::Screenshot),
            "the chord claims it"
        );
    }

    #[test]
    fn shortcuts_sequence_taps_fire_the_chord() {
        let mut s = Shortcuts::default();
        assert_eq!(s.feed(&key(0x136, 1)), None, "select");
        assert_eq!(s.feed(&key(0x136, 0)), None);
        assert_eq!(s.feed(&key(0x138, 1)), None, "menu");
        assert_eq!(s.feed(&key(0x138, 0)), None);
        assert_eq!(
            s.feed(&key(CODE_R1, 1)),
            Some(Shortcut::Screenshot),
            "three quick taps fire the chord"
        );
        assert_eq!(s.feed(&key(CODE_R1, 0)), None);
        let later = Instant::now() + TAP_GRACE + Duration::from_millis(1);
        assert_eq!(s.tick(later), None, "pending taps were swallowed");
    }
}
