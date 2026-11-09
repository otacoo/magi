//! The app shell: multicall UI loop over the presenter and SDL input.

use std::time::{Duration, Instant};

use anyhow::Result;
use sdl2::controller::{Axis, Button as PadButton};
use sdl2::event::Event;

use crate::canvas::Canvas;
use crate::defaults::Defaults;
use crate::device;
use crate::font::{self, Fonts};
use crate::input::{self, Action, AxisRepeat};
use crate::pads::Pads;
use crate::present::Presenter;
use crate::screens::keyboard::{self, KeyboardState};
use crate::screens::library::LibraryState;
use crate::screens::settings::{self, ClockEdit, SettingsState};
use crate::screens::{boot, card, info, library, naoko, net, shots, update as update_screen};
use crate::update;
use crate::util;

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Boot,
    Library,
    Settings,
    Clock,
    Info(InfoKind),
    Network,
    NetWifi,
    NetBt,
    NetSsh,
    NetNtp,
    Shots,
    Keyboard,
    Update,
}

#[derive(Clone, Copy, PartialEq)]
enum InfoKind {
    About,
}

/// The X-button game menu (INFO / GAME / SYSTEM / CLOSE) over the shelf.
#[derive(Clone)]
struct GameMenuUi {
    title: String,
    path: String,
    /// 0 = INFO, 1 = GAME, 2 = SYSTEM, 3 = CLOSE.
    tab: u8,
    sel: usize,
    /// True while the pane has the d-pad; false while the tab column does.
    focus: bool,
    confirm: bool,
}

struct Ui {
    cv: Canvas,
    fonts: Fonts,
    screen: Screen,
    boot_t0: Instant,
    started: Instant,
    library: LibraryState,
    settings: SettingsState,
    shots: shots::ShotsState,
    game_menu: Option<GameMenuUi>,
    /// The POWER press/hold decision: set on press, resolved on release.
    power_down: Option<Instant>,
    /// The shutdown countdown (started by a POWER hold) and the guard that
    /// swallows the press which woke the device from sleep.
    shutdown_t0: Option<Instant>,
    shutdown_sent: bool,
    power_guard: Option<Instant>,
    info_kind: InfoKind,
    info_sel: usize,
    info_lines: Vec<(String, String)>,
    info_refresh: Instant,
    naoko: Option<naoko::Naoko>,
    update: Option<update::Update>,
    brightness: u32,
    warmth: u32,
    bar_hud: Option<(crate::theme::BarIcon, f32, Instant)>,
    net_sel: usize,
    net_tick: Instant,
    wifi: Vec<util::WifiNet>,
    wifi_sel: usize,
    wifi_line: String,
    wifi_busy_until: Option<Instant>,
    wifi_tx: std::sync::mpsc::Sender<Vec<util::WifiNet>>,
    wifi_rx: std::sync::mpsc::Receiver<Vec<util::WifiNet>>,
    joining: Option<(String, Instant)>,
    bt: Vec<(String, String)>,
    bt_connected: Vec<String>,
    bt_sel: usize,
    ntp_sel: usize,
    bt_busy_until: Option<Instant>,
    kb: Option<KeyboardState>,
    /// The page the keyboard was opened from: CANCEL and BACK return there
    /// (the library for a game rename, the network page for wifi/hostname).
    kb_origin: Screen,
    should_quit: bool,
    pending_screenshot: bool,
    pending_launch: Option<crate::run::Launch>,
    card_read_only: bool,
    card_notice: bool,
    card_confirm: bool,
    card_repairing: bool,
    card_repair_ready: bool,
    card_dismissed: bool,
    card_alerted: bool,
    card_result: Option<&'static str>,
}

impl Ui {
    fn new(w: u32, h: u32) -> Result<Self> {
        let (wifi_tx, wifi_rx) = std::sync::mpsc::channel();
        let mut ui = Self {
            cv: Canvas::new(w, h),
            fonts: Fonts::new(font::FONT_DATA)?,
            screen: Screen::Boot,
            boot_t0: Instant::now(),
            started: Instant::now(),
            library: LibraryState::load(std::path::Path::new(".")),
            settings: SettingsState::default(),
            shots: shots::ShotsState::load(),
            game_menu: None,
            power_down: None,
            shutdown_t0: None,
            shutdown_sent: false,
            power_guard: None,
            info_kind: InfoKind::About,
            info_sel: 0,
            info_lines: Vec::new(),
            info_refresh: Instant::now(),
            naoko: None,
            update: None,
            brightness: util::STOCK_BRIGHTNESS,
            warmth: 0,
            bar_hud: None,
            net_sel: 0,
            net_tick: Instant::now(),
            wifi: Vec::new(),
            wifi_sel: 0,
            wifi_line: String::new(),
            wifi_busy_until: None,
            wifi_tx,
            wifi_rx,
            joining: None,
            bt: Vec::new(),
            bt_connected: Vec::new(),
            bt_sel: 0,
            ntp_sel: 0,
            bt_busy_until: None,
            kb: None,
            kb_origin: Screen::Network,
            should_quit: false,
            pending_screenshot: false,
            pending_launch: None,
            card_read_only: util::card_read_only(),
            card_notice: false,
            card_confirm: false,
            card_repairing: false,
            card_repair_ready: util::card_repair_available(),
            card_dismissed: false,
            card_alerted: false,
            card_result: util::take_repair_result(),
        };
        // Create the card tree and scan immediately: the walk then overlaps
        // the rest of start-up and the boot splash.
        ui.library.catalogue.ensure_layout();
        ui.library.start_scan();
        Ok(ui)
    }

    /// Something on screen changes on its own: the loop then renders at the
    /// panel rate and wakes at 60 Hz. A static shelf wakes at 4 Hz instead.
    fn animating(&self) -> bool {
        self.screen == Screen::Boot
            || self.screen == Screen::Update
            || self.naoko.is_some()
            || self.bar_hud.is_some()
            || self.shutdown_t0.is_some()
            || self.shutdown_sent
            || crate::status::is_active()
            || self.library.scan_pending()
            || self.library.is_fetching_art()
            || self.library.title_scrolling()
            || self.joining.is_some()
            || self.wifi_busy_until.is_some()
            || self.bt_busy_until.is_some()
            || self.settings.clock.is_some()
            || self.library.prefs.glow
            || self.library.prefs.anim_bg
            || self.card_notice
            || self.card_confirm
            || self.card_repairing
    }

    fn open_settings(&mut self) {
        self.settings = SettingsState::from_prefs(self.library.prefs);
        self.screen = Screen::Settings;
    }

    fn leave_settings(&mut self) {
        self.library.set_prefs(self.settings.to_prefs());
        self.library.flush();
        self.screen = Screen::Library;
    }

    fn repair_card(&mut self) {
        self.card_confirm = false;
        self.card_repairing = true;
        self.library.flush();
        #[cfg(target_os = "linux")]
        crate::disp2::clear_stale_layer();
        let e = util::start_card_repair();
        util::log(&format!("card repair: {e}"));
        self.card_repairing = false;
        crate::status::show("REPAIR FAILED TO START");
    }

    fn open_shots(&mut self) {
        self.shots = shots::ShotsState::load();
        self.screen = Screen::Shots;
    }

    /// The X-button game menu: INFO / GAME / SYSTEM / CLOSE.
    fn game_menu_action(&mut self, a: Action) {
        let Some(mut m) = self.game_menu.take() else {
            return;
        };
        if m.confirm {
            match a {
                Action::Confirm => {
                    match self.library.delete_game(&m.path) {
                        Ok(()) => crate::status::show("DELETED"),
                        Err(msg) => crate::status::show(&msg),
                    }
                    return;
                }
                Action::Back | Action::X | Action::Menu => m.confirm = false,
                _ => {}
            }
            self.game_menu = Some(m);
            return;
        }

        if m.focus {
            match a {
                Action::Up | Action::Down => {
                    let rows = self.game_menu_rows(&m);
                    m.sel = if a == Action::Up {
                        (m.sel + rows - 1) % rows
                    } else {
                        (m.sel + 1) % rows
                    };
                }
                Action::Left | Action::Right => {
                    let delta = if a == Action::Left { -1 } else { 1 };
                    let analog_row = self.game_menu_analog_row(&m.path);
                    match (m.tab, m.sel) {
                        (1, 0) => self.toggle_play_tracking(&m.path),
                        (2, 0) => self.library.cycle_game_art(&m.path, delta),
                        (2, 1) if self.library.game_swap_ab(&m.path).is_some() => {
                            self.toggle_swap_ab(&m.path)
                        }
                        (2, 2) if self.library.game_swap_l1r1(&m.path).is_some() => {
                            self.toggle_swap_l1r1(&m.path)
                        }
                        (2, n) if Some(n) == analog_row => self.toggle_analog_dpad(&m.path),
                        _ => {}
                    }
                }
                Action::Confirm => match (m.tab, m.sel) {
                    (1, 0) => self.toggle_play_tracking(&m.path),
                    (1, 1) => {
                        let mut kb = KeyboardState::new(
                            "RENAME GAME",
                            keyboard::Target::Rename(m.path.clone()),
                        );
                        kb.set_text(
                            std::path::Path::new(&m.path)
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        );
                        self.kb = Some(kb);
                        self.kb_origin = Screen::Library;
                        self.screen = Screen::Keyboard;
                        return;
                    }
                    (1, 2) => m.confirm = true,
                    (2, 0) => self.library.cycle_game_art(&m.path, 1),
                    (2, 1) if self.library.game_swap_ab(&m.path).is_some() => {
                        self.toggle_swap_ab(&m.path)
                    }
                    (2, 2) if self.library.game_swap_l1r1(&m.path).is_some() => {
                        self.toggle_swap_l1r1(&m.path)
                    }
                    (2, n) if Some(n) == self.game_menu_analog_row(&m.path) => {
                        self.toggle_analog_dpad(&m.path)
                    }
                    _ => {}
                },
                Action::Back => m.focus = false,
                Action::X | Action::Menu => return,
                _ => {}
            }
            self.game_menu = Some(m);
            return;
        }

        match a {
            Action::Up => {
                m.tab = (m.tab + 3) % 4;
                m.sel = 0;
            }
            Action::Down => {
                m.tab = (m.tab + 1) % 4;
                m.sel = 0;
            }
            Action::Right if m.tab == 1 || m.tab == 2 => m.focus = true,
            Action::Confirm if m.tab == 1 || m.tab == 2 => m.focus = true,
            Action::Confirm if m.tab == 3 => return,
            Action::Back | Action::X | Action::Menu => return,
            _ => {}
        }
        self.game_menu = Some(m);
    }

    fn game_menu_rows(&self, m: &GameMenuUi) -> usize {
        match m.tab {
            1 => 3,
            2 => {
                1 + usize::from(self.library.game_swap_ab(&m.path).is_some())
                    + usize::from(self.library.game_swap_l1r1(&m.path).is_some())
                    + usize::from(self.library.game_analog_dpad(&m.path).is_some())
            }
            _ => 1,
        }
    }

    fn game_menu_analog_row(&self, path: &str) -> Option<usize> {
        self.library.game_analog_dpad(path).map(|_| {
            1 + usize::from(self.library.game_swap_ab(path).is_some())
                + usize::from(self.library.game_swap_l1r1(path).is_some())
        })
    }

    fn toggle_play_tracking(&mut self, path: &str) {
        let on = self.library.toggle_play_tracking(path);
        crate::status::show(if on {
            "PLAY TIME TRACKING ON"
        } else {
            "PLAY TIME TRACKING OFF"
        });
    }

    fn toggle_swap_ab(&mut self, path: &str) {
        if let Some(on) = self.library.toggle_game_swap_ab(path) {
            crate::status::show(if on { "A/B SWAPPED" } else { "A/B NORMAL" });
        }
    }

    fn toggle_swap_l1r1(&mut self, path: &str) {
        if let Some(on) = self.library.toggle_game_swap_l1r1(path) {
            crate::status::show(if on { "L1/R1 SWAPPED" } else { "L1/R1 NORMAL" });
        }
    }

    fn toggle_analog_dpad(&mut self, path: &str) {
        if let Some(on) = self.library.toggle_game_analog_dpad(path) {
            crate::status::show(if on { "DPAD AS ANALOG" } else { "DPAD DIGITAL" });
        }
    }

    /// L/R (and A on adjustable rows) for the settings screen; changes are
    /// applied to the shared prefs and saved right away.
    fn settings_adjust(&mut self, delta: i32) {
        match self.settings.rows()[self.settings.sel] {
            "INTERFACE" => self.settings.adjust_interface(delta),
            "AUTO ART" => self.settings.toggle_auto_art(),
            "ART TYPE" => self.settings.adjust_art_type(delta),
            "ART BACKGROUND" => self.settings.adjust_art_bg(delta),
            "FAST FORWARD" => self.settings.adjust_ff_steps(delta),
            "FF SOUND" => self.settings.toggle_ff_sound(),
            "COLOUR THEME" => self.settings.adjust_theme(delta),
            "TEXT SIZE" => {
                self.settings.adjust_text_scale(delta);
                crate::theme::set_ui_scale(self.settings.prefs.text_scale);
            }
            "BACKGROUND" => {
                self.settings.adjust_background(delta);
                crate::theme::set_bg_style(self.settings.prefs.background);
            }
            "SHELF BACKGROUND" => {
                self.settings.prefs.shelf_bg = !self.settings.prefs.shelf_bg;
                crate::theme::set_shelf_bg(self.settings.prefs.shelf_bg);
            }
            "ANIM BACKGROUND" => {
                self.settings.prefs.anim_bg = !self.settings.prefs.anim_bg;
                crate::theme::set_anim_bg(self.settings.prefs.anim_bg);
            }
            "GLOW FX" => {
                self.settings.prefs.glow = !self.settings.prefs.glow;
                crate::theme::set_glow(self.settings.prefs.glow);
            }
            "SCREENSHOT" => {
                self.settings.prefs.shot_chord = !self.settings.prefs.shot_chord;
                input::set_shot_chord(self.settings.prefs.shot_chord);
            }
            "SHOW BATTERY %" => self.settings.prefs.battery_pct = !self.settings.prefs.battery_pct,
            "LED" => {
                self.settings.prefs.led_on = !self.settings.prefs.led_on;
                util::set_led(self.settings.prefs.led_on);
            }
            "HDMI AUTO" => self.settings.prefs.hdmi_auto = !self.settings.prefs.hdmi_auto,
            "LID SLEEP" => self.settings.prefs.lid_sleep = !self.settings.prefs.lid_sleep,
            "RUMBLE" => {
                self.settings.prefs.rumble = !self.settings.prefs.rumble;
                if self.settings.prefs.rumble {
                    util::rumble_pulse();
                }
            }
            "LID SHUTDOWN AFTER" => self.settings.adjust_shutdown(delta),
            "IDLE SHUTDOWN" => self.settings.adjust_idle(delta),
            "BATTERY SHUTDOWN" => self.settings.adjust_bat_shutdown(delta),
            "CHARGE LIMIT" => self.settings.adjust_charge_limit(delta),
            _ => return,
        }
        self.settings.sel = self
            .settings
            .sel
            .min(self.settings.rows().len().saturating_sub(1));
        self.library.set_prefs(self.settings.to_prefs());
        self.library.flush();
    }

    fn open_update(&mut self) {
        if !util::wifi_up() {
            crate::status::show("TURN WI-FI ON");
            return;
        }
        if util::card_read_only() {
            self.card_read_only = true;
            return;
        }
        if util::battery_percent().is_some_and(|p| p < 20) {
            crate::status::show("CHARGE TO 20%");
            return;
        }
        let panel = if self.cv.w >= 700 { "720" } else { "640" };
        self.update = Some(update::Update::start(panel, env!("CARGO_PKG_VERSION")));
        self.screen = Screen::Update;
    }

    fn open_info(&mut self, kind: InfoKind) {
        self.info_kind = kind;
        self.info_sel = 0;
        self.refresh_info();
        let naoko = if kind == InfoKind::About {
            naoko::Naoko::roll()
        } else {
            None
        };
        if naoko.is_some() {
            util::log("about: naoko");
            #[cfg(target_os = "linux")]
            for p in ["System/magi/assets/boot/naoko.wav", "assets/boot/naoko.wav"] {
                let path = std::path::Path::new(p);
                if path.exists() {
                    crate::audio::spawn_wav(path, self.library.volume_percent() as u8);
                    break;
                }
            }
        }
        self.naoko = naoko;
        self.screen = Screen::Info(kind);
    }

    fn open_network(&mut self) {
        self.net_sel = 0;
        self.screen = Screen::Network;
    }

    fn open_keyboard(&mut self, title: &str, target: keyboard::Target) {
        self.kb = Some(KeyboardState::new(title, target));
        self.kb_origin = self.screen;
        self.screen = Screen::Keyboard;
    }

    fn wifi_status_refresh(&mut self) {
        let (up, connected, ssid, ip) = util::wifi_state();
        self.library.wifi_on = up;
        self.library.wifi_signal = if up {
            util::wifi_signal_pct().unwrap_or(0)
        } else {
            0
        };
        self.wifi_line = if connected {
            format!("{ssid}  {ip}")
        } else if up {
            "NOT CONNECTED".into()
        } else {
            "OFF".into()
        };
        if connected {
            self.joining = None;
        }
    }

    fn wifi_rescan(&mut self) {
        self.wifi_status_refresh();
        if !self.library.wifi_on {
            self.wifi.clear();
            return;
        }
        // The scan takes seconds; it runs in the background so the page
        // opens instantly and fills in when the results land.
        crate::status::show("SCANNING WIFI..");
        self.wifi_busy_until = Some(Instant::now() + Duration::from_secs(10));
        let tx = self.wifi_tx.clone();
        std::thread::spawn(move || {
            let nets = util::wifi_scan();
            let _ = tx.send(nets);
        });
    }

    /// Bluetooth work happens off the UI thread (attach takes seconds) and
    /// the page refreshes itself while a job is in flight.
    fn bt_job(&mut self, label: &str, job: impl FnOnce() + Send + 'static) {
        crate::status::show(label);
        self.bt_busy_until = Some(Instant::now() + Duration::from_secs(15));
        std::thread::spawn(job);
    }

    fn bt_refresh(&mut self, scan: bool) {
        self.library.refresh_radio();
        if scan {
            crate::status::show("SCANNING BLUETOOTH..");
        }
        self.bt = if self.library.bt_on {
            util::bt_devices(scan)
        } else {
            Vec::new()
        };
        self.bt_connected = if self.library.bt_on {
            let macs: Vec<String> = self.bt.iter().map(|(m, _)| m.clone()).collect();
            util::bt_connected(&macs)
        } else {
            Vec::new()
        };
        self.bt_sel = self.bt_sel.min(self.bt.len());
    }

    /// The shelf volume: the saved percentage drives the mixer, so the ports
    /// and the system sounds play at the level the user picked.
    fn set_volume(&mut self, delta: i32) {
        self.library.volume(delta);
        #[cfg(target_os = "linux")]
        util::set_mixer_volume(self.library.volume_percent() as u8);
        self.bar_hud = Some((
            crate::theme::BarIcon::Volume,
            self.library.volume_percent() as f32 / 100.0,
            Instant::now(),
        ));
    }

    /// SELECT + d-pad on the shelf: brightness and warmth, with the bar.
    fn adjust_display(&mut self, brightness: bool, delta: i32) {
        let (icon, ratio) = if brightness {
            self.brightness =
                util::clamp_brightness((self.brightness as i32 + delta).max(0) as u32);
            util::set_brightness(self.brightness);
            (
                crate::theme::BarIcon::Brightness,
                self.brightness as f32 / 100.0,
            )
        } else {
            self.warmth = (self.warmth as i32 + delta).clamp(0, 100) as u32;
            crate::theme::set_warmth(self.warmth as u8);
            (
                crate::theme::BarIcon::Temperature,
                self.warmth as f32 / 100.0,
            )
        };
        self.bar_hud = Some((icon, ratio, Instant::now()));
        self.save_display_prefs();
    }

    /// The N64 supervisor can adjust brightness and warmth while the shelf
    /// is paused; pick its saved values up before the next frame.
    fn reload_display(&mut self) {
        let Ok(text) = std::fs::read_to_string("Config/magi.json") else {
            return;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        if let Some(pct) = v.get("brightness").and_then(|x| x.as_u64()) {
            self.brightness = util::clamp_brightness(pct.min(100) as u32);
            util::set_brightness(self.brightness);
        }
        if let Some(warmth) = v.get("warmth").and_then(|x| x.as_u64()) {
            self.warmth = warmth.min(100) as u32;
            crate::theme::set_warmth(self.warmth as u8);
        }
    }

    fn save_display_prefs(&self) {
        let path = std::path::Path::new("Config/magi.json");
        let mut v: serde_json::Value = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .filter(|v: &serde_json::Value| v.is_object())
            .unwrap_or_else(|| serde_json::json!({}));
        v["brightness"] = serde_json::json!(self.brightness);
        v["warmth"] = serde_json::json!(self.warmth);
        if let Ok(text) = serde_json::to_string_pretty(&v) {
            let _ = std::fs::create_dir_all("Config");
            let _ = util::write_file_atomic(path, text.as_bytes());
        }
    }

    fn kb_move(&mut self, f: impl FnOnce(&mut KeyboardState)) {
        if let Some(kb) = &mut self.kb {
            f(kb);
        }
    }

    fn kb_activate(&mut self) {
        let result = self.kb.as_mut().and_then(|kb| kb.activate());
        match result {
            None => {}
            Some(false) => {
                self.kb = None;
                self.screen = self.kb_origin;
            }
            Some(true) => {
                self.kb_commit();
            }
        }
    }

    fn kb_commit(&mut self) {
        let Some(kb) = self.kb.take() else { return };
        let text = kb.text().to_string();
        match kb.target {
            keyboard::Target::WifiPass(ssid) => {
                util::wifi_connect(&ssid, Some(&text));
                self.joining = Some((ssid, Instant::now()));
                self.screen = Screen::NetWifi;
            }
            keyboard::Target::Hostname => {
                let ok = util::set_hostname(&text);
                if ok {
                    util::save_string_pref("hostname", &text);
                }
                crate::status::show(if ok {
                    "HOSTNAME SET"
                } else {
                    "INVALID HOSTNAME"
                });
                self.screen = Screen::Network;
            }
            keyboard::Target::Rename(path) => {
                match self.library.rename_game(&path, &text) {
                    Ok(()) => crate::status::show("RENAMED"),
                    Err(msg) => crate::status::show(&msg),
                }
                self.screen = Screen::Library;
            }
            keyboard::Target::NtpServer => {
                let ok = util::set_ntp_server(&text);
                crate::status::show(if ok {
                    "NTP SERVER SET"
                } else {
                    "INVALID SERVER"
                });
                self.screen = Screen::NetNtp;
            }
        }
    }

    fn refresh_info(&mut self) {
        self.info_lines = util::system_lines();
        self.info_refresh = Instant::now();
    }

    fn action(&mut self, a: Action) {
        if a == Action::Quit {
            self.should_quit = true;
            return;
        }
        // Global: POWER taps sleep, a hold runs the shutdown countdown, a
        // closed lid sleeps. The press/hold decision happens on release, and
        // the press that woke us from sleep is ignored for a moment.
        if a == Action::Power {
            if self.power_guard.is_some_and(|t| Instant::now() < t) {
                return;
            }
            self.power_down = Some(Instant::now());
            return;
        }
        if a == Action::PowerRelease {
            if self.power_guard.is_some_and(|t| Instant::now() < t) {
                self.power_down = None;
                return;
            }
            if self.shutdown_t0.take().is_some() {
                util::log("power: shutdown aborted");
                self.power_down = None;
                return;
            }
            if let Some(t0) = self.power_down.take() {
                if t0.elapsed() < POWER_HOLD {
                    util::log("power tap; sleeping");
                    util::sleep_now(self.library.prefs.shutdown_min * 60);
                    // The press that woke the device must not read as a new
                    // tap; ignore POWER for a moment after every sleep.
                    self.power_guard = Some(Instant::now() + Duration::from_millis(1200));
                }
            }
            return;
        }
        if let Some((bright, delta)) = match a {
            Action::BrightnessUp => Some((true, 5)),
            Action::BrightnessDown => Some((true, -5)),
            Action::WarmthUp => Some((false, 5)),
            Action::WarmthDown => Some((false, -5)),
            _ => None,
        } {
            self.adjust_display(bright, delta);
            return;
        }
        if let Some(delta) = match a {
            Action::VolumeUp => Some(5),
            Action::VolumeDown => Some(-5),
            _ => None,
        } {
            self.set_volume(delta);
            return;
        }
        if a == Action::LidClosed {
            if self.library.prefs.lid_sleep {
                util::log("lid closed; sleeping");
                util::sleep_now(self.library.prefs.shutdown_min * 60);
            } else {
                util::log("lid closed; lid sleep is off");
            }
            return;
        }
        // The card alert owns input while it is open. SELECT+START (or A)
        // opens the repair question; B dismisses until the card is clean.
        if a == Action::CardRepair {
            self.card_dismissed = false;
            self.card_read_only = util::card_read_only();
            self.card_repair_ready = util::card_repair_available();
            if self.card_read_only {
                self.card_notice = true;
                if self.card_repair_ready {
                    self.card_confirm = true;
                }
            } else {
                crate::status::show("SD CARD OK");
            }
            return;
        }
        if self.card_repairing {
            return;
        }
        if self.card_confirm {
            match a {
                Action::Confirm => self.repair_card(),
                Action::Back => {
                    self.card_confirm = false;
                    self.card_repair_ready = util::card_repair_available();
                }
                _ => {}
            }
            return;
        }
        if self.card_notice {
            match a {
                Action::Confirm if self.card_repair_ready => self.card_confirm = true,
                Action::Back => {
                    self.card_notice = false;
                    self.card_dismissed = true;
                }
                _ => {}
            }
            return;
        }
        if self.naoko.is_some() {
            self.naoko = None;
            #[cfg(target_os = "linux")]
            crate::audio::stop_boot_audio();
            return;
        }
        match self.screen {
            Screen::Boot => {
                util::log("boot splash skipped");
                #[cfg(target_os = "linux")]
                crate::audio::stop_boot_audio();
                self.screen = Screen::Library;
            }
            Screen::Library => {
                // The game actions menu swallows input while it is open.
                if self.game_menu.is_some() {
                    self.game_menu_action(a);
                    return;
                }
                // The CONTROLS panel closes on any button but volume.
                if self.library.controls_open && !matches!(a, Action::VolumeUp | Action::VolumeDown)
                {
                    self.library.controls_open = false;
                    return;
                }
                match a {
                    Action::Up => self.library.nav_up(),
                    Action::Down => self.library.nav_down(),
                    Action::Left | Action::L2 => self.library.move_system(-1),
                    Action::Right | Action::R2 => self.library.move_system(1),
                    Action::L1 => self.library.move_game(-10),
                    Action::R1 => self.library.move_game(10),
                    Action::X => {
                        if let Some(g) = self.library.selected_game() {
                            self.game_menu = Some(GameMenuUi {
                                title: g.title,
                                path: g.path,
                                tab: 0,
                                sel: 0,
                                focus: false,
                                confirm: false,
                            });
                        }
                    }
                    Action::Confirm => {
                        if util::card_read_only() && !self.card_dismissed {
                            self.card_read_only = true;
                            self.card_repair_ready = util::card_repair_available();
                            self.card_notice = true;
                            return;
                        }
                        match self.library.selected_launch() {
                            Some((sys, game)) => match sys.core.filter(|c| !c.is_empty()) {
                                Some(core) => {
                                    self.pending_launch = Some(crate::run::Launch {
                                        system: sys.id.clone(),
                                        core,
                                        rom: game.path.clone(),
                                    })
                                }
                                None => crate::status::show("NO CORE SET FOR SYSTEM"),
                            },
                            None => crate::status::show("NO GAMES FOUND"),
                        }
                    }
                    Action::Select => self.library.toggle_controls(),
                    Action::Y => self.library.toggle_favorite(),
                    Action::Back => self.library.stop_art(),
                    Action::Menu => self.open_settings(),
                    Action::Screenshot => self.pending_screenshot = true,
                    _ => {}
                }
            }
            Screen::Settings => match a {
                Action::Up => {
                    let n = self.settings.rows().len();
                    self.settings.sel = (self.settings.sel + n - 1) % n;
                }
                Action::Down => {
                    let n = self.settings.rows().len();
                    self.settings.sel = (self.settings.sel + 1) % n;
                }
                Action::Left => self.settings_adjust(-1),
                Action::Right => self.settings_adjust(1),
                Action::Confirm => match self.settings.rows()[self.settings.sel] {
                    "INTERFACE" | "AUTO ART" | "ART TYPE" | "ART BACKGROUND" | "FAST FORWARD"
                    | "FF SOUND" | "RUMBLE" | "COLOUR THEME" | "TEXT SIZE" | "BACKGROUND"
                    | "SHELF BACKGROUND" | "ANIM BACKGROUND" | "GLOW FX" | "SCREENSHOT"
                    | "SHOW BATTERY %" | "LID SLEEP" | "LID SHUTDOWN AFTER" | "IDLE SHUTDOWN"
                    | "BATTERY SHUTDOWN" | "CHARGE LIMIT" | "LED" | "HDMI AUTO" => {
                        self.settings_adjust(1)
                    }
                    "SHELF" => {
                        self.settings.page = settings::Page::Shelf;
                        self.settings.sel = 0;
                    }
                    "DOWNLOAD ART" => {
                        self.leave_settings();
                        self.library.start_art_job();
                    }
                    "THEME" => {
                        self.settings.page = settings::Page::Theme;
                        self.settings.sel = 0;
                    }
                    "SYSTEM" => {
                        self.settings.page = settings::Page::System;
                        self.settings.sel = 0;
                    }
                    "POWER MANAGEMENT" => {
                        self.settings.page = settings::Page::Power;
                        self.settings.sel = 0;
                    }

                    "CLOCK" => {
                        self.settings.clock = Some(ClockEdit::from_now());
                        self.screen = Screen::Clock;
                    }
                    "SCREENSHOTS" => self.open_shots(),
                    "NETWORK" => self.open_network(),
                    "CHECK FOR UPDATES" => self.open_update(),
                    "ABOUT" => self.open_info(InfoKind::About),
                    _ => self.leave_settings(),
                },
                Action::Back => match self.settings.page {
                    settings::Page::Main => self.leave_settings(),
                    _ => {
                        self.settings.page = settings::Page::Main;
                        self.settings.sel = 0;
                    }
                },
                Action::Start | Action::Menu => self.leave_settings(),
                Action::Screenshot => self.pending_screenshot = true,
                _ => {}
            },
            Screen::Info(_) => match a {
                Action::Back | Action::Start | Action::Menu | Action::Confirm => {
                    // Info screens open from settings; go back there.
                    self.screen = Screen::Settings;
                }
                Action::Screenshot => self.pending_screenshot = true,
                _ => {}
            },
            Screen::Update => match a {
                Action::Confirm => {
                    if let Some(u) = &self.update {
                        match u.phase {
                            update::Phase::Ready => {
                                crate::util::log("update: restarting");
                                self.library.flush();
                                util::spawn_detached("reboot");
                            }
                            phase if !phase.active() => {
                                self.update = None;
                                self.screen = Screen::Settings;
                            }
                            _ => {}
                        }
                    }
                }
                Action::Back | Action::Start | Action::Menu => {
                    if self.update.as_ref().is_some_and(|u| !u.phase.active()) {
                        self.update = None;
                        self.screen = Screen::Settings;
                    }
                }
                Action::Screenshot => self.pending_screenshot = true,
                _ => {}
            },
            Screen::Shots => match a {
                Action::Up | Action::Left => self.shots.move_by(-1),
                Action::Down | Action::Right => self.shots.move_by(1),
                Action::Confirm => self.shots.toggle_zoom(),
                Action::Back => {
                    if self.shots.zoom() {
                        self.shots.toggle_zoom();
                    } else {
                        self.screen = Screen::Settings;
                    }
                }
                Action::Start | Action::Menu => {
                    if self.shots.zoom() {
                        self.shots.toggle_zoom();
                    } else {
                        self.screen = Screen::Settings;
                    }
                }
                Action::Y => {
                    if let Some(msg) = self.shots.delete_selected() {
                        crate::status::show(msg);
                    }
                }
                Action::Screenshot => self.pending_screenshot = true,
                _ => {}
            },
            Screen::Network => match a {
                Action::Up => {
                    self.net_sel =
                        (self.net_sel + net::INDEX_ROWS.len() - 1) % net::INDEX_ROWS.len()
                }
                Action::Down => self.net_sel = (self.net_sel + 1) % net::INDEX_ROWS.len(),
                Action::Confirm => match net::INDEX_ROWS[self.net_sel] {
                    "WIFI" => {
                        self.wifi_sel = 0;
                        self.wifi_status_refresh();
                        if self.wifi.is_empty() {
                            self.wifi_rescan();
                        }
                        self.screen = Screen::NetWifi;
                    }
                    "BLUETOOTH" => {
                        self.bt_sel = 0;
                        self.bt_refresh(false);
                        self.screen = Screen::NetBt;
                    }
                    "SSH" => self.screen = Screen::NetSsh,
                    "NTP" => self.screen = Screen::NetNtp,
                    _ => {
                        // HOSTNAME: edit, pre-filled with the current name.
                        let mut kb = KeyboardState::new("HOSTNAME", keyboard::Target::Hostname);
                        kb.set_text(util::hostname_get().unwrap_or_default());
                        self.kb = Some(kb);
                        self.kb_origin = self.screen;
                        self.screen = Screen::Keyboard;
                    }
                },
                Action::Back | Action::Start | Action::Menu => self.screen = Screen::Settings,
                Action::Screenshot => self.pending_screenshot = true,
                _ => {}
            },
            Screen::NetWifi => match a {
                Action::Up => {
                    self.wifi_sel = (self.wifi_sel + self.wifi.len()) % (self.wifi.len() + 1)
                }
                Action::Down => self.wifi_sel = (self.wifi_sel + 1) % (self.wifi.len() + 1),
                Action::Confirm => {
                    if self.wifi_sel == 0 {
                        let target = !self.library.wifi_on;
                        util::save_radio_state("wifi_on", target);
                        util::set_wifi(target);
                        std::thread::sleep(Duration::from_millis(600));
                        self.library.refresh_radio();
                        self.wifi_sel = 0;
                        self.wifi_rescan();
                    } else if let Some(netw) = self.wifi.get(self.wifi_sel - 1).cloned() {
                        if netw.secured && !netw.saved {
                            self.open_keyboard(
                                &format!("PASSWORD FOR {}", netw.ssid),
                                keyboard::Target::WifiPass(netw.ssid.clone()),
                            );
                        } else {
                            util::wifi_connect(&netw.ssid, None);
                            self.joining = Some((netw.ssid.clone(), Instant::now()));
                        }
                    }
                }
                Action::X => self.wifi_rescan(),
                Action::Y => {
                    if let Some(netw) = self.wifi.get(self.wifi_sel.saturating_sub(1)).cloned() {
                        if netw.saved {
                            util::wifi_forget(&netw.ssid);
                            self.wifi_rescan();
                        }
                    }
                }
                Action::Back | Action::Menu => self.screen = Screen::Network,
                Action::Start => self.wifi_rescan(),
                Action::Screenshot => self.pending_screenshot = true,
                _ => {}
            },
            Screen::NetBt => match a {
                Action::Up => self.bt_sel = (self.bt_sel + self.bt.len()) % (self.bt.len() + 1),
                Action::Down => self.bt_sel = (self.bt_sel + 1) % (self.bt.len() + 1),
                Action::Confirm => {
                    if self.bt_sel == 0 {
                        let target = !util::bt_on();
                        util::save_radio_state("bt_on", target);
                        self.bt_job(
                            if target {
                                "BLUETOOTH ENABLING.."
                            } else {
                                "BLUETOOTH OFF"
                            },
                            move || {
                                util::set_bt(target);
                            },
                        );
                    } else if let Some((mac, _)) = self.bt.get(self.bt_sel - 1).cloned() {
                        self.bt_job("CONNECTING..", move || {
                            util::bt_connect(&mac);
                        });
                    }
                }
                Action::X => self.bt_job("SCANNING BLUETOOTH..", || {
                    let _ = util::bt_devices(true);
                }),
                Action::Y => {
                    if let Some((mac, _)) = self.bt.get(self.bt_sel.saturating_sub(1)).cloned() {
                        // After forgetting, rediscover so the device
                        // shows up again (unpaired) without a BT toggle.
                        self.bt_job("FORGETTING..", move || {
                            util::bt_forget(&mac);
                            let _ = util::bt_devices(true);
                        });
                    }
                }
                Action::Back | Action::Menu => self.screen = Screen::Network,
                Action::Screenshot => self.pending_screenshot = true,
                _ => {}
            },
            Screen::NetSsh => match a {
                Action::Confirm | Action::Left | Action::Right => {
                    let target = !util::ssh_on();
                    util::set_ssh(target);
                    util::save_radio_state("ssh_on", target);
                    std::thread::sleep(Duration::from_millis(600));
                }
                Action::Back | Action::Start | Action::Menu => self.screen = Screen::Network,
                Action::Screenshot => self.pending_screenshot = true,
                _ => {}
            },
            Screen::NetNtp => match a {
                Action::Up | Action::Down => self.ntp_sel = (self.ntp_sel + 1) % 2,
                Action::Confirm | Action::Left | Action::Right => {
                    if self.ntp_sel == 0 {
                        let target = !util::ntp_on();
                        util::set_ntp(target);
                        util::save_radio_state("ntp_on", target);
                    } else {
                        let mut kb = KeyboardState::new("NTP SERVER", keyboard::Target::NtpServer);
                        kb.set_text(util::ntp_server_pref());
                        self.kb = Some(kb);
                        self.kb_origin = self.screen;
                        self.screen = Screen::Keyboard;
                    }
                }
                Action::Back | Action::Start | Action::Menu => self.screen = Screen::Network,
                Action::Screenshot => self.pending_screenshot = true,
                _ => {}
            },
            Screen::Keyboard => match a {
                Action::Up => self.kb_move(|k| k.up()),
                Action::Down => self.kb_move(|k| k.down()),
                Action::Left => self.kb_move(|k| k.left()),
                Action::Right => self.kb_move(|k| k.right()),
                Action::L1 | Action::L2 => self.kb_move(|k| k.cursor_left()),
                Action::R1 | Action::R2 => self.kb_move(|k| k.cursor_right()),
                Action::Confirm => self.kb_activate(),
                Action::Start => self.kb_commit(),
                Action::Back | Action::Menu => {
                    self.kb = None;
                    self.screen = self.kb_origin;
                }
                Action::Screenshot => self.pending_screenshot = true,
                _ => {}
            },
            Screen::Clock => match a {
                Action::Up => self.clock_edit(|c| c.adjust(1)),
                Action::Down => self.clock_edit(|c| c.adjust(-1)),
                Action::Left => self.clock_edit(|c| c.next_field(-1)),
                Action::Right => self.clock_edit(|c| c.next_field(1)),
                Action::X => {
                    self.settings.prefs.clock_12h = !self.settings.prefs.clock_12h;
                    self.library.set_prefs(self.settings.to_prefs());
                    self.library.flush();
                    crate::status::show(if self.settings.prefs.clock_12h {
                        "12H CLOCK"
                    } else {
                        "24H CLOCK"
                    });
                }
                Action::Confirm => {
                    if let Some(c) = self.settings.clock {
                        let ok = util::set_datetime(c.v);
                        crate::status::show(if ok { "CLOCK SET" } else { "CLOCK SET FAILED" });
                    }
                    self.screen = Screen::Settings;
                }
                Action::Back | Action::Start | Action::Menu => {
                    self.settings.clock = None;
                    self.screen = Screen::Settings;
                }
                Action::Screenshot => self.pending_screenshot = true,
                _ => {}
            },
        }
    }

    fn clock_edit(&mut self, f: impl FnOnce(&mut ClockEdit)) {
        if let Some(c) = &mut self.settings.clock {
            f(c);
        }
    }

    /// Hand the panel to the run host (BALTHASAR) and wait for it to exit.
    fn launch(&mut self, l: &crate::run::Launch) {
        // The child reads Config/magi.json for its runtime preferences; make
        // sure a just-changed shelf option is already on the card.
        self.library.flush();
        util::log(&format!("launch {} [{}]: {}", l.system, l.core, l.rom));
        let started = Instant::now();
        let status = util::spawn_self().and_then(|mut c| {
            if l.system == "n64" {
                c.args(["n64", "--rom", &l.rom]);
            } else {
                c.args([
                    "run", "--system", &l.system, "--core", &l.core, "--rom", &l.rom,
                ]);
            }
            c.status()
        });
        let session = started.elapsed().as_secs();
        // The game host disables the fb layer during scanout and may leave
        // its overlay layer behind; a crash must not keep either.
        #[cfg(target_os = "linux")]
        {
            crate::disp2::clear_stale_layer();
            crate::disp2::fb_restore();
        }
        match &status {
            Ok(s) if s.success() => util::log("game exited"),
            Ok(s) => {
                util::log(&format!("game exited: {s}"));
                crate::status::show("GAME FAILED — SEE LOGS/MAGI-RUN.LOG");
            }
            Err(e) => {
                util::log(&format!("launch failed: {e}"));
                crate::status::show("LAUNCH FAILED");
            }
        }
        if status.is_ok() {
            self.library.add_play_time(&l.rom, session);
        }
        // Games and ports may have changed volume/display values; pick them
        // up before flushing, or the stale shelf copy would overwrite them.
        self.library.reload_volume();
        self.reload_display();
        self.library.flush();
        // A game can hit the FAT error that remounts the card read-only.
        self.card_read_only = util::card_read_only();
        if self.card_read_only {
            self.card_repair_ready = util::card_repair_available();
            self.card_notice = true;
            self.card_dismissed = false;
            util::log("card: read-only after game; repair offered");
        }
    }

    fn update(&mut self) {
        let now = Instant::now();
        if let Some(update) = &mut self.update {
            update.poll();
        }
        if self.naoko.as_ref().is_some_and(|e| e.finished()) {
            self.naoko = None;
            #[cfg(target_os = "linux")]
            crate::audio::stop_boot_audio();
        }
        // The card is sampled once at startup; a game can also leave it
        // read-only, so launch() checks again when the child returns.
        if !self.card_alerted && self.screen != Screen::Boot {
            self.card_alerted = true;
            if self.card_read_only {
                util::log("card: read-only at boot; repair offered");
                self.card_notice = true;
            }
        }
        // POWER held: run the EVA countdown; released early, it aborts, and
        // at zero the console powers off (kept dark through the shutdown).
        if let (Some(t0), None, false) = (self.power_down, self.shutdown_t0, self.shutdown_sent) {
            if t0.elapsed() >= POWER_HOLD {
                self.shutdown_t0 = Some(Instant::now());
                util::log("power: shutdown countdown");
            }
        }
        if let Some(t0) = self.shutdown_t0 {
            if !self.shutdown_sent && t0.elapsed() >= SHUTDOWN_TOTAL {
                self.shutdown_sent = true;
                util::log("power: shutting down");
                self.library.flush();
                util::set_brightness(0);
                util::spawn_detached("poweroff");
            }
        }
        match self.screen {
            Screen::Boot => {
                // Drain the scan during the splash so the first shelf frame
                // already has its games.
                self.library.poll(now);
                if now.duration_since(self.boot_t0).as_secs_f32() >= boot::BOOT_SECS {
                    self.screen = Screen::Library;
                }
            }
            Screen::Library => self.library.poll(now),
            Screen::Settings | Screen::Clock => {}
            Screen::Info(_) => {
                if self.info_refresh.elapsed().as_millis() > 1000 {
                    self.refresh_info();
                }
            }
            Screen::NetWifi | Screen::NetSsh | Screen::NetNtp => {
                if let Ok(nets) = self.wifi_rx.try_recv() {
                    self.wifi = nets;
                    self.wifi_sel = self.wifi_sel.min(self.wifi.len());
                    self.wifi_busy_until = None;
                }
                if self.net_tick.elapsed().as_millis() > 1000 {
                    self.net_tick = now;
                    self.wifi_status_refresh();
                    // Rows light CONNECTED/SAVED the moment they are true,
                    // without leaving and re-entering the page.
                    let (current, saved) = util::wifi_states();
                    for net in self.wifi.iter_mut() {
                        net.current = !current.is_empty() && net.ssid == current;
                        net.saved = net.current || saved.contains(&net.ssid);
                    }
                }
            }
            Screen::NetBt => {
                if let Some(until) = self.bt_busy_until {
                    if Instant::now() >= until {
                        self.bt_busy_until = None;
                    } else if self.net_tick.elapsed().as_millis() > 1500 {
                        self.net_tick = now;
                        self.bt_refresh(false);
                    }
                }
            }
            _ => {}
        }
        if let Some((_, _, t0)) = &self.bar_hud {
            if t0.elapsed().as_millis() > 1400 {
                self.bar_hud = None;
            }
        }
        if let Some((ssid, t0)) = &self.joining {
            if t0.elapsed().as_secs() > 20 {
                util::log(&format!("network: joining {ssid} timed out"));
                crate::status::show("CONNECTION FAILED");
                self.joining = None;
            }
        }
        if self.screen != Screen::Boot {
            if let Some(msg) = self.card_result.take() {
                crate::status::show(msg);
            }
        }
    }

    fn render(&mut self) {
        match self.screen {
            Screen::Boot => {
                let t = self.boot_t0.elapsed().as_secs_f32();
                boot::draw(&mut self.cv, &mut self.fonts, t);
            }
            Screen::Library => {
                let t = self.started.elapsed().as_secs_f32();
                library::draw(&mut self.cv, &mut self.fonts, &mut self.library, t);
                if let Some(m) = &self.game_menu {
                    let sys = self.library.game_system(&m.path);
                    let stat = self.library.play_stat(&m.path);
                    let info = library::GameMenuInfo {
                        title: &m.title,
                        system: sys.map(|s| s.name.as_str()).unwrap_or("UNKNOWN"),
                        core: sys.and_then(|s| s.core.as_deref()).unwrap_or("NONE"),
                        path: &m.path,
                        play_secs: stat.secs,
                        tracking: stat.track,
                        favorite: self.library.favorites.contains(&m.path),
                        art_type: sys.map(|s| self.library.art_type_for(&s.id)).unwrap_or(0),
                        swap_ab: self.library.game_swap_ab(&m.path),
                        swap_l1r1: self.library.game_swap_l1r1(&m.path),
                        analog_dpad: self.library.game_analog_dpad(&m.path),
                    };
                    library::draw_game_menu(
                        &mut self.cv,
                        &mut self.fonts,
                        &info,
                        m.tab,
                        m.sel,
                        m.focus,
                        m.confirm,
                        t,
                    );
                }
            }
            Screen::Settings => settings::draw(
                &mut self.cv,
                &mut self.fonts,
                &self.settings,
                self.started.elapsed().as_secs_f32(),
            ),
            Screen::Clock => {
                if let Some(clock) = &self.settings.clock {
                    settings::draw_clock(
                        &mut self.cv,
                        &mut self.fonts,
                        clock,
                        self.settings.prefs.clock_12h,
                        self.started.elapsed().as_secs_f32(),
                    );
                }
            }
            Screen::Info(_) => info::draw(
                &mut self.cv,
                &mut self.fonts,
                "ABOUT",
                &self.info_lines,
                &[],
                0,
                self.started.elapsed().as_secs_f32(),
            ),
            Screen::Update => {
                if let Some(update) = &self.update {
                    update_screen::draw(
                        &mut self.cv,
                        &mut self.fonts,
                        update,
                        self.started.elapsed().as_secs_f32(),
                    );
                }
            }
            Screen::Shots => shots::draw(
                &mut self.cv,
                &mut self.fonts,
                &mut self.shots,
                self.started.elapsed().as_secs_f32(),
            ),
            Screen::Network => net::draw_index(
                &mut self.cv,
                &mut self.fonts,
                self.net_sel,
                &util::hostname_get().unwrap_or_default(),
                self.started.elapsed().as_secs_f32(),
            ),
            Screen::NetWifi => net::draw_wifi(
                &mut self.cv,
                &mut self.fonts,
                self.library.wifi_on,
                &self.wifi_line,
                &self.wifi,
                self.joining.as_ref().map(|(s, _)| s.as_str()),
                self.wifi_sel,
                self.started.elapsed().as_secs_f32(),
            ),
            Screen::NetBt => net::draw_bt(
                &mut self.cv,
                &mut self.fonts,
                self.library.bt_on,
                &self.bt,
                &self.bt_connected,
                self.bt_sel,
                self.started.elapsed().as_secs_f32(),
            ),
            Screen::NetSsh => net::draw_toggle_page(
                &mut self.cv,
                &mut self.fonts,
                "SSH",
                "SSH",
                util::ssh_on(),
                &net::ssh_status_lines(),
                0,
                self.started.elapsed().as_secs_f32(),
            ),
            Screen::NetNtp => {
                let server = util::ntp_server_pref();
                net::draw_ntp(
                    &mut self.cv,
                    &mut self.fonts,
                    util::ntp_on(),
                    &server,
                    &net::ntp_status_lines(&server),
                    self.ntp_sel,
                    self.started.elapsed().as_secs_f32(),
                );
            }
            Screen::Keyboard => {
                if let Some(kb) = &self.kb {
                    keyboard::draw(
                        &mut self.cv,
                        &mut self.fonts,
                        kb,
                        self.started.elapsed().as_secs_f32(),
                    );
                }
            }
        }
        if let Some(naoko) = &self.naoko {
            naoko.draw(&mut self.cv, &mut self.fonts);
        }
        if self.card_repairing {
            card::draw(
                &mut self.cv,
                &mut self.fonts,
                self.started.elapsed().as_secs_f32(),
                card::CardView::Repairing,
            );
        } else if self.card_confirm {
            card::draw(
                &mut self.cv,
                &mut self.fonts,
                self.started.elapsed().as_secs_f32(),
                card::CardView::Confirm,
            );
        } else if self.card_notice {
            card::draw(
                &mut self.cv,
                &mut self.fonts,
                self.started.elapsed().as_secs_f32(),
                card::CardView::Notice {
                    repair: self.card_repair_ready,
                },
            );
        }
        if let Some((icon, ratio, _)) = &self.bar_hud {
            crate::theme::bar_hud(&mut self.cv, &mut self.fonts, *icon, *ratio);
        }
        if let Some(t0) = self.shutdown_t0 {
            let remaining = SHUTDOWN_TOTAL.saturating_sub(t0.elapsed());
            crate::theme::draw_shutdown(
                &mut self.cv,
                &mut self.fonts,
                remaining,
                SHUTDOWN_TOTAL,
                self.started.elapsed().as_secs_f32(),
            );
        } else if self.shutdown_sent {
            crate::theme::draw_shutdown(
                &mut self.cv,
                &mut self.fonts,
                Duration::ZERO,
                SHUTDOWN_TOTAL,
                self.started.elapsed().as_secs_f32(),
            );
        }
    }
}

/// Route SDL's own logging into the shelf log (input category runs verbose).
extern "C" fn sdl_log(
    _userdata: *mut libc::c_void,
    _category: libc::c_int,
    _priority: sdl2::sys::SDL_LogPriority,
    message: *const libc::c_char,
) {
    if message.is_null() {
        return;
    }
    let msg = unsafe { std::ffi::CStr::from_ptr(message) }.to_string_lossy();
    util::log(&format!("sdl: {msg}"));
}

pub fn run() -> Result<()> {
    // Card themes are loaded before the saved theme index is applied.
    crate::theme::ensure_user_themes(std::path::Path::new(crate::systems::DIR_THEMES));
    crate::theme::load_user_themes(std::path::Path::new(crate::systems::DIR_THEMES));
    let dev = device::detect();
    let scale = device::window_scale();

    unsafe {
        sdl2::sys::SDL_LogSetOutputFunction(Some(sdl_log), std::ptr::null_mut());
        sdl2::sys::SDL_LogSetPriority(
            sdl2::sys::SDL_LogCategory::SDL_LOG_CATEGORY_INPUT as libc::c_int,
            sdl2::sys::SDL_LogPriority::SDL_LOG_PRIORITY_VERBOSE,
        );
    }

    let sdl = sdl2::init().map_err(anyhow::Error::msg)?;
    let mut presenter = Presenter::open(&sdl, dev, scale)?;
    let (cw, ch) = presenter.canvas_size();
    #[cfg(target_os = "linux")]
    util::set_cpu_governor("schedutil");

    let mut pads = Pads::new(&sdl);

    #[cfg(target_os = "linux")]
    let mut evdev = crate::evdev::Evdev::open();
    #[cfg(target_os = "linux")]
    let mut lid = util::LidWatch::new();

    let mut events = sdl.event_pump().map_err(anyhow::Error::msg)?;
    let mut ui = Ui::new(cw, ch)?;
    // A crashed game host can leave the fb layer covered or disabled; the
    // shelf needs it enabled and opaque.
    #[cfg(target_os = "linux")]
    crate::disp2::fb_restore();
    // A frontend that keeps dying before it settles would loop forever on the
    // same theme or overlay choice: the third fast restart starts on defaults
    // with no boot sequence.
    let safe_mode = util::startup_guard();
    if safe_mode {
        util::log("startup: safe mode after repeated fast restarts (defaults, no boot)");
    }
    let prefs = ui.library.prefs;
    crate::theme::set_theme_index(if safe_mode { 0 } else { prefs.theme });
    crate::theme::set_ui_scale(prefs.text_scale);
    crate::theme::set_bg_style(if safe_mode {
        Defaults::BACKGROUND
    } else {
        prefs.background
    });
    crate::theme::set_shelf_bg(prefs.shelf_bg && !safe_mode);
    crate::theme::set_anim_bg(prefs.anim_bg && !safe_mode);
    crate::theme::set_glow(prefs.glow && !safe_mode);
    if safe_mode {
        ui.screen = Screen::Library;
        crate::status::show("SAFE MODE");
    }
    input::set_shot_chord(prefs.shot_chord);
    // The power LED state is ours to keep (the attribute never reads back);
    // and it stays dark during sleep by default.
    util::set_led(prefs.led_on);
    util::set_led_sleep(false);
    // The libretro host uses software gain for volume; make sure a crashed
    // port cannot leave the ALSA mixer ducked.
    util::set_mixer_volume(100);
    // Radio choices survive reboots; BaseOS turns wlan0, dropbear and NTP on
    // at boot. Enforce the saved state on a worker: the off paths wait on
    // service commands and used to hold the first frame back.
    let wifi_on = util::saved_radio_state("wifi_on").unwrap_or(Defaults::WIFI);
    let ssh_on = util::saved_radio_state("ssh_on").unwrap_or(Defaults::SSH);
    let bt_on = util::saved_radio_state("bt_on").unwrap_or(Defaults::BT);
    let ntp_on = util::saved_radio_state("ntp_on").unwrap_or(Defaults::NTP);
    std::thread::spawn(move || {
        util::set_wifi(wifi_on);
        util::set_ssh(ssh_on);
        if bt_on {
            util::set_bt(true);
        }
        if ntp_on {
            util::ntp_reapply();
        } else {
            util::set_ntp(false);
        }
    });
    // The hostname default (or the saved choice) wins over BaseOS's model id.
    #[cfg(target_os = "linux")]
    {
        let host = util::saved_string_pref("hostname")
            .unwrap_or_else(|| crate::device::detect().id.to_ascii_uppercase());
        if util::hostname_get().as_deref() != Some(host.as_str()) {
            util::set_hostname(&host);
        }
    }
    // The panel's brightness resets with the power; reapply the saved one,
    // and the saved warm tint with it.
    if let Ok(text) = std::fs::read_to_string("Config/magi.json") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(pct) = v.get("brightness").and_then(|x| x.as_u64()) {
                ui.brightness = util::clamp_brightness(pct.min(100) as u32);
                util::set_brightness(ui.brightness);
            } else if let Some(pct) = util::brightness_now() {
                ui.brightness = util::clamp_brightness(pct);
                util::set_brightness(ui.brightness);
            }
            if let Some(w) = v.get("warmth").and_then(|x| x.as_u64()) {
                ui.warmth = w.min(100) as u32;
                crate::theme::set_warmth(ui.warmth as u8);
            }
        }
    }
    util::log(&format!(
        "display: brightness {}% warmth {}% volume {}%",
        ui.brightness,
        ui.warmth,
        ui.library.volume_percent()
    ));
    // The ports play through ALSA directly, so the mixer follows the saved
    // volume (the libretro host uses its own software gain instead).
    #[cfg(target_os = "linux")]
    util::set_mixer_volume(ui.library.volume_percent() as u8);
    let mut axis = AxisRepeat::new();
    let mut dpad = DpadRepeat::new();
    let mut first_frame = true;
    // After a game or port exits the still-held buttons must not leak into
    // the shelf (a held MENU would open SETTINGS).
    #[cfg(target_os = "linux")]
    let mut input_guard: Option<Instant> = None;
    // The idle-shutdown clock: any input event restarts it; games and ports
    // block this loop, and returning from them counts as activity.
    let mut last_input = Instant::now();

    #[cfg(target_os = "linux")]
    util::refresh_bootlogo();
    #[cfg(target_os = "linux")]
    util::hdmi_init();
    #[cfg(target_os = "linux")]
    crate::disp2::clear_stale_layer();
    util::log(&format!(
        "magi {} up: canvas {cw}x{ch} (build {})",
        env!("CARGO_PKG_VERSION"),
        option_env!("MAGI_BUILD").unwrap_or("dev")
    ));
    // Start the splash clock with the first visible frame, after the
    // presenter, bootlogo and display setup are done.
    ui.boot_t0 = Instant::now();
    // The ignition track rides the boot sequence (any button skips it,
    // the sound finishes on its own).
    #[cfg(target_os = "linux")]
    for p in [
        "System/magi/assets/boot/ignition.wav",
        "assets/boot/ignition.wav",
    ] {
        if std::path::Path::new(p).exists() {
            crate::audio::spawn_wav(std::path::Path::new(p), ui.library.volume_percent() as u8);
            break;
        }
    }

    #[cfg(target_os = "linux")]
    let mut hdmi = util::HdmiWatch::new();
    // The panel only needs a frame when something changed (or something is
    // animating); a still shelf repaints once a second.
    let mut last_present = Instant::now() - Duration::from_secs(1);
    loop {
        // Wake for input, not on a fixed tick. The stock pad arrives on evdev,
        // so blocking on its node reacts immediately and the shelf can idle at
        // 4 Hz. SDL only owns Bluetooth/USB pads: with one open it has to be
        // pumped at the animation rate.
        let animating = ui.animating();
        #[cfg(target_os = "linux")]
        let pad_here = !pads.idle();
        #[cfg(not(target_os = "linux"))]
        let pad_here = true;
        #[cfg(target_os = "linux")]
        let fast = animating || evdev.busy();
        #[cfg(not(target_os = "linux"))]
        let fast = animating;
        let wait_ms: u32 = if fast { 16 } else { 250 };
        let mut dirty = false;
        #[cfg(target_os = "linux")]
        if !pad_here {
            evdev.wait(Duration::from_millis(u64::from(wait_ms)));
        }
        let first = if pad_here {
            events.wait_event_timeout(wait_ms)
        } else {
            events.poll_event()
        };
        if let Some(ev) = first {
            last_input = Instant::now();
            dirty = true;
            let mut changed = false;
            handle_event(&ev, &mut ui, &mut axis, &mut pads, &mut dpad, &mut changed);
            while let Some(ev) = events.poll_event() {
                dirty = true;
                handle_event(&ev, &mut ui, &mut axis, &mut pads, &mut dpad, &mut changed);
            }
            #[cfg(target_os = "linux")]
            if changed {
                evdev.rescan();
            }
        }
        if let Some(a) = dpad.tick(Instant::now()) {
            last_input = Instant::now();
            dirty = true;
            ui.action(a);
        }
        #[cfg(target_os = "linux")]
        for a in evdev.poll() {
            last_input = Instant::now();
            dirty = true;
            let guarded = input_guard.is_some_and(|t| Instant::now() < t);
            if guarded && matches!(a, Action::Menu | Action::Select | Action::Confirm) {
                continue;
            }
            ui.action(a);
        }
        #[cfg(target_os = "linux")]
        if let Some(a) = evdev.tick(Instant::now()) {
            last_input = Instant::now();
            dirty = true;
            ui.action(a);
        }
        #[cfg(target_os = "linux")]
        if lid.poll() {
            last_input = Instant::now();
            util::log("lid closed (hallkey); sleeping");
            ui.action(Action::LidClosed);
        }
        #[cfg(target_os = "linux")]
        if ui.library.prefs.hdmi_auto {
            if let Some(on) = hdmi.poll() {
                util::log(&format!(
                    "hdmi cable {}",
                    if on { "connected" } else { "removed" }
                ));
                util::set_hdmi(on);
                last_input = Instant::now();
            }
        }
        if let Some(a) = axis.tick(Instant::now()) {
            last_input = Instant::now();
            dirty = true;
            ui.action(a);
        }
        ui.update();
        if let Some(l) = ui.pending_launch.take() {
            ui.launch(&l);
            // The child owned the input devices; drop anything queued while
            // we were paused, then redraw fresh.
            #[cfg(target_os = "linux")]
            evdev.flush();
            while events.poll_event().is_some() {}
            #[cfg(target_os = "linux")]
            {
                input_guard = Some(Instant::now() + Duration::from_millis(1000));
            }
            last_input = Instant::now();
            ui.library.reload_volume();
            ui.reload_display();
            dirty = true;
        }
        if dirty || animating || last_present.elapsed() >= Duration::from_secs(1) {
            ui.render();
            crate::theme::apply_warmth(&mut ui.cv);
            presenter.present(&ui.cv)?;
            last_present = Instant::now();
            if first_frame {
                util::log("first frame presented");
                first_frame = false;
            }
        }
        if ui.pending_screenshot {
            ui.pending_screenshot = false;
            let dir = std::path::Path::new(crate::systems::DIR_SHOTS);
            let _ = std::fs::create_dir_all(dir);
            let path = dir.join(format!("magi_{}.bmp", util::timestamp()));
            match ui.cv.save_bmp(&path) {
                Ok(()) => {
                    util::log(&format!("screenshot saved: {}", path.display()));
                    crate::status::show("SCREENSHOT SAVED");
                }
                Err(e) => {
                    util::log(&format!("screenshot failed: {e}"));
                    crate::status::show("SCREENSHOT FAILED");
                }
            }
        }
        if ui.library.prefs.idle_min > 0
            && last_input.elapsed() >= Duration::from_secs(ui.library.prefs.idle_min as u64 * 60)
        {
            util::log("idle shutdown");
            ui.library.flush();
            util::spawn_detached("poweroff");
            last_input = Instant::now();
        }
        if ui.should_quit {
            ui.library.flush();
            util::log("quit");
            return Ok(());
        }
    }
}

/// Holding POWER this long starts the shutdown countdown; a shorter press
/// sleeps. The countdown itself is five seconds.
const POWER_HOLD: Duration = Duration::from_millis(800);
const SHUTDOWN_TOTAL: Duration = Duration::from_millis(2500);

/// Holding a d-pad direction repeats it, as the evdev reader does for the
/// stock pad.
struct DpadRepeat {
    dir: Option<Action>,
    next: Instant,
}

impl DpadRepeat {
    fn new() -> Self {
        Self {
            dir: None,
            next: Instant::now(),
        }
    }

    fn pressed(&mut self, a: Action) {
        self.dir = Some(a);
        self.next = Instant::now() + DPAD_FIRST_DELAY;
    }

    fn released(&mut self, a: Action) {
        if self.dir == Some(a) {
            self.dir = None;
        }
    }

    fn tick(&mut self, now: Instant) -> Option<Action> {
        let dir = self.dir?;
        if now < self.next {
            return None;
        }
        self.next = now + DPAD_REPEAT;
        Some(dir)
    }
}

const DPAD_FIRST_DELAY: Duration = Duration::from_millis(380);
const DPAD_REPEAT: Duration = Duration::from_millis(100);

fn pad_button_action(b: PadButton) -> Option<Action> {
    match b {
        PadButton::A => Some(Action::Confirm),
        PadButton::B => Some(Action::Back),
        PadButton::X => Some(Action::X),
        PadButton::Y => Some(Action::Y),
        PadButton::LeftShoulder => Some(Action::L1),
        PadButton::RightShoulder => Some(Action::R1),
        PadButton::Back => Some(Action::Select),
        PadButton::Start => Some(Action::Start),
        PadButton::Guide => Some(Action::Menu),
        PadButton::DPadUp => Some(Action::Up),
        PadButton::DPadDown => Some(Action::Down),
        PadButton::DPadLeft => Some(Action::Left),
        PadButton::DPadRight => Some(Action::Right),
        _ => None,
    }
}

fn is_dpad(a: Action) -> bool {
    matches!(a, Action::Up | Action::Down | Action::Left | Action::Right)
}

fn handle_event(
    ev: &Event,
    ui: &mut Ui,
    axis: &mut AxisRepeat,
    pads: &mut Pads,
    dpad: &mut DpadRepeat,
    changed: &mut bool,
) {
    match ev {
        Event::Quit { .. } => ui.action(Action::Quit),
        Event::KeyDown {
            keycode: Some(k),
            repeat: false,
            ..
        } => {
            if let Some(a) = input::key_to_action(*k) {
                ui.action(a);
            }
        }
        Event::ControllerButtonDown { button, .. } => {
            if let Some(a) = pad_button_action(*button) {
                if is_dpad(a) {
                    dpad.pressed(a);
                }
                ui.action(a);
            }
        }
        Event::ControllerButtonUp { button, .. } => {
            if let Some(a) = pad_button_action(*button) {
                dpad.released(a);
            }
        }
        Event::ControllerAxisMotion {
            axis: pad_axis,
            value,
            ..
        } => match pad_axis {
            Axis::LeftX => {
                if let Some(a) = axis.on_axis(0, *value) {
                    ui.action(a);
                }
            }
            Axis::LeftY => {
                if let Some(a) = axis.on_axis(1, *value) {
                    ui.action(a);
                }
            }
            _ => {}
        },
        Event::ControllerDeviceAdded { which, .. } => {
            pads.added(*which);
            *changed = true;
        }
        Event::ControllerDeviceRemoved { which, .. } => {
            pads.removed(*which);
            *changed = true;
        }
        _ => {}
    }
}
