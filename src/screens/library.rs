//! The library: system rail, game list, favorites, volume and state.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::art;
use crate::canvas::Canvas;
use crate::defaults::Defaults;
use crate::font::Fonts;
use crate::games::{self, GameEntry};
use crate::screens::settings::{Prefs, FF_STEPS_MAX, FF_STEPS_MIN, TEXT_SCALES};
use crate::screens::views;
use crate::systems::{Catalogue, SystemDef};
use crate::theme::pal;
use crate::util;

const ROW_H: i32 = 26;
const SAVE_DEBOUNCE: Duration = Duration::from_secs(2);

/// The virtual system gathering favorites from every folder.
const FAV_ID: &str = "favorites";

#[derive(Default, Serialize, Deserialize)]
struct StateFile {
    #[serde(default)]
    system: Option<String>,
    #[serde(default)]
    positions: HashMap<String, usize>,
    #[serde(default)]
    favorites: Vec<String>,
    #[serde(default)]
    volume: Option<u32>,
    #[serde(default)]
    interface: Option<u8>,
    #[serde(default)]
    auto_art: Option<bool>,
    #[serde(default)]
    art_type: Option<u8>,
    #[serde(default)]
    art_bg: Option<u8>,
    #[serde(default)]
    art_types: HashMap<String, u8>,
    #[serde(default)]
    ff_steps: Option<u32>,
    #[serde(default)]
    ff_sound: Option<bool>,
    #[serde(default)]
    text_scale: Option<f32>,
    #[serde(default)]
    background: Option<u8>,
    #[serde(default)]
    anim_bg: Option<bool>,
    #[serde(default)]
    glow: Option<bool>,
    #[serde(default)]
    shelf_bg: Option<bool>,
    #[serde(default)]
    theme: Option<u8>,
    #[serde(default)]
    shot_chord: Option<bool>,
    #[serde(default)]
    clock_12h: Option<bool>,
    #[serde(default)]
    battery_pct: Option<bool>,
    #[serde(default)]
    lid_sleep: Option<bool>,
    #[serde(default)]
    shutdown_min: Option<u32>,
    #[serde(default)]
    idle_min: Option<u32>,
    #[serde(default)]
    bat_shutdown: Option<u32>,
    #[serde(default)]
    charge_limit: Option<u32>,
    #[serde(default)]
    led_on: Option<bool>,
    #[serde(default)]
    rumble: Option<bool>,
    #[serde(default)]
    hdmi_auto: Option<bool>,
}

impl StateFile {
    /// The stored values layered over `Prefs::default()`; anything the user
    /// has not changed keeps the fresh-card default.
    fn prefs(&self) -> Prefs {
        let mut p = Prefs::default();
        if let Some(v) = self.interface {
            p.interface = v.min(1);
        }
        if let Some(v) = self.auto_art {
            p.auto_art = v;
        }
        if let Some(v) = self.art_type {
            // ALL was removed; an old value falls back to boxart.
            p.art_type = if (v as usize) < crate::art::LABELS.len() {
                v
            } else {
                0
            };
        }
        if let Some(v) = self.art_bg {
            p.art_bg = v.min(1);
        }
        if let Some(v) = self.ff_steps {
            p.ff_steps = v.clamp(FF_STEPS_MIN, FF_STEPS_MAX);
        }
        if let Some(v) = self.ff_sound {
            p.ff_sound = v;
        }
        if let Some(v) = self.text_scale {
            p.text_scale = v.clamp(TEXT_SCALES[0], TEXT_SCALES[TEXT_SCALES.len() - 1]);
        }
        if let Some(v) = self.background {
            p.background = v.min((crate::theme::bg_styles().len() - 1) as u8);
        }
        if let Some(v) = self.shelf_bg {
            p.shelf_bg = v;
        }
        if let Some(v) = self.anim_bg {
            p.anim_bg = v;
        }
        if let Some(v) = self.glow {
            p.glow = v;
        }
        if let Some(v) = self.theme {
            p.theme = v.min((crate::theme::themes().len() - 1) as u8);
        }
        if let Some(v) = self.shot_chord {
            p.shot_chord = v;
        }
        if let Some(v) = self.clock_12h {
            p.clock_12h = v;
        }
        if let Some(v) = self.battery_pct {
            p.battery_pct = v;
        }
        if let Some(v) = self.lid_sleep {
            p.lid_sleep = v;
        }
        if let Some(v) = self.shutdown_min {
            p.shutdown_min = v;
        }
        if let Some(v) = self.idle_min {
            p.idle_min = v;
        }
        if let Some(v) = self.bat_shutdown {
            p.bat_shutdown = v;
        }
        if let Some(v) = self.charge_limit {
            p.charge_limit = v;
        }
        if let Some(v) = self.led_on {
            p.led_on = v;
        }
        if let Some(v) = self.rumble {
            p.rumble = v;
        }
        if let Some(v) = self.hdmi_auto {
            p.hdmi_auto = v;
        }
        p
    }
}

/// The system whose `Games/<folder>` holds `path`.
fn system_for_path<'a>(systems: &'a [SystemDef], path: &str) -> Option<&'a SystemDef> {
    systems
        .iter()
        .find(|s| path.starts_with(&format!("Games/{}/", s.folder())))
}

/// Favorites in catalogue order, ready for the virtual system.
fn gather_favorites(
    systems: &[SystemDef],
    games: &HashMap<String, Vec<GameEntry>>,
    favorites: &HashSet<String>,
) -> Vec<GameEntry> {
    let mut list = Vec::new();
    for sys in systems {
        if let Some(entries) = games.get(&sys.id) {
            list.extend(
                entries
                    .iter()
                    .filter(|g| favorites.contains(&g.path))
                    .cloned(),
            );
        }
    }
    list
}

pub struct LibraryState {
    pub catalogue: Catalogue,
    pub games: HashMap<String, Vec<GameEntry>>,
    pub scanning: HashSet<String>,
    pub favorites: HashSet<String>,
    /// The favorites gathered in catalogue order, shown as `fav_system`.
    fav_games: Vec<GameEntry>,
    fav_system: SystemDef,
    pub controls_open: bool,
    pub battery: Option<u32>,
    pub clock: Option<String>,
    pub wifi_on: bool,
    pub wifi_signal: u8,
    pub bt_on: bool,
    sel_system: usize,
    /// The system the state file asked for, resolved once its scan lands.
    want_system: Option<String>,
    positions: HashMap<String, usize>,
    first_visible: usize,
    /// True while the selected title is scrolling; the app loop then runs at
    /// the animation rate instead of its idle tick.
    marquee: bool,
    /// When the highlighted game last changed, for the title marquee.
    sel_since: Instant,
    sel_id: String,
    sel_path: String,
    scan_rx: Option<Receiver<(String, Vec<GameEntry>)>>,
    art_rx: Option<Receiver<art::Msg>>,
    art_stop: Option<Arc<AtomicBool>>,
    /// The system being fetched and its (done, total).
    art_progress: Option<(String, usize, usize)>,
    art_quiet: bool,
    /// Systems whose art was attempted this run, so auto never loops.
    art_done: HashSet<String>,
    /// One log line each for the reasons auto art stays idle without saying so.
    art_wait_logged: bool,
    art_idle_logged: bool,
    volume_pct: Option<u32>,
    /// The last decoded cover, keyed by ROM path and art type.
    cover: Option<(String, u8, Option<crate::image::Image>)>,
    /// Per-system art type overrides, by system id.
    art_types: HashMap<String, u8>,
    pub prefs: Prefs,
    state_dirty: bool,
    last_save: Instant,
    cwd: PathBuf,
    last_poll: Instant,
    /// When DHCP was last retried, and whether wlan0 last held an address.
    dhcp_retry: Instant,
    wifi_had_ip: bool,
    charge_latch: bool,
}

impl LibraryState {
    pub fn load(cwd: &Path) -> Self {
        let catalogue = Catalogue::load(cwd);
        let state: StateFile = std::fs::read_to_string(cwd.join("Config/magi.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();

        let want_system = state.system.clone();
        let prefs = state.prefs();

        let mut st = Self {
            catalogue,
            games: HashMap::new(),
            scanning: HashSet::new(),
            favorites: state
                .favorites
                .into_iter()
                .map(|p| games::card_path(&p))
                .collect(),
            art_types: state
                .art_types
                .into_iter()
                .filter(|(_, t)| (*t as usize) < art::LABELS.len())
                .collect(),
            fav_games: Vec::new(),
            fav_system: SystemDef {
                id: FAV_ID.into(),
                name: "FAVORITES".into(),
                exts: Vec::new(),
                core: None,
                folder: None,
                art: None,
            },
            controls_open: false,
            battery: None,
            clock: None,
            wifi_on: false,
            wifi_signal: 0,
            bt_on: false,
            sel_system: 0,
            want_system,
            positions: state.positions,
            first_visible: 0,
            marquee: false,
            sel_since: Instant::now(),
            sel_id: String::new(),
            sel_path: String::new(),
            scan_rx: None,
            art_rx: None,
            art_stop: None,
            art_progress: None,
            art_quiet: false,
            art_done: HashSet::new(),
            art_wait_logged: false,
            art_idle_logged: false,
            volume_pct: state.volume,
            cover: None,
            prefs,
            state_dirty: false,
            last_save: Instant::now(),
            cwd: cwd.to_path_buf(),
            last_poll: Instant::now() - Duration::from_secs(3600),
            dhcp_retry: Instant::now(),
            wifi_had_ip: false,
            charge_latch: false,
        };
        st.poll_status(Instant::now());
        st
    }

    /// Kick off the background scan of every system folder.
    pub fn start_scan(&mut self) {
        let mut jobs: Vec<(String, String, Vec<String>)> = self
            .catalogue
            .systems
            .iter()
            .map(|s| {
                let mut exts = s.exts.clone();
                // Cards packaged before playlist support may not list m3u.
                if s.id == "psx" && !exts.iter().any(|e| e.eq_ignore_ascii_case("m3u")) {
                    exts.push("m3u".into());
                }
                (s.id.clone(), s.folder(), exts)
            })
            .collect();
        // The saved system is the one on screen: scan it first so the shelf
        // has its list even if the card walk is slow on a cold boot.
        if let Some(want) = self.want_system.clone() {
            if let Some(pos) = jobs.iter().position(|(id, _, _)| *id == want) {
                let job = jobs.remove(pos);
                jobs.insert(0, job);
            }
        }
        self.scanning = jobs.iter().map(|(id, _, _)| id.clone()).collect();
        let cwd = self.cwd.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for (id, folder, exts) in jobs {
                let list = if id == "n64" {
                    games::scan_n64(&cwd, &format!("Games/{folder}"))
                } else {
                    games::scan_dir(&cwd, &format!("Games/{folder}"), &exts)
                };
                util::log(&format!("scanned {} entries in {folder}/", list.len()));
                if tx.send((id, list)).is_err() {
                    break;
                }
            }
        });
        self.scan_rx = Some(rx);
    }

    pub fn poll(&mut self, now: Instant) {
        if let Some(rx) = &self.scan_rx {
            let mut fresh = false;
            loop {
                match rx.try_recv() {
                    Ok((id, list)) => {
                        self.scanning.remove(&id);
                        self.games.insert(id, list);
                        fresh = true;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.scan_rx = None;
                        break;
                    }
                }
            }
            if fresh {
                self.refresh_favorites();
            }
        }
        // Art job news: progress for the shelf line, and a cache drop when a
        // cover lands for the selected game.
        if let Some(rx) = self.art_rx.take() {
            let mut keep = true;
            loop {
                match rx.try_recv() {
                    Ok(art::Msg::Progress { done, total }) => {
                        if let Some((_, d, t)) = &mut self.art_progress {
                            *d = done;
                            *t = total;
                        }
                    }
                    Ok(art::Msg::Saved { rom }) => {
                        if self.cover.as_ref().is_some_and(|(key, _, _)| *key == rom) {
                            self.cover = None;
                        }
                    }
                    Ok(art::Msg::Finished {
                        saved,
                        missed,
                        offline,
                    }) => {
                        keep = false;
                        self.art_stop = None;
                        self.art_progress = None;
                        self.art_quiet = false;
                        util::log(&format!(
                            "art: finished saved={saved} missed={missed} offline={offline}"
                        ));
                        if offline {
                            crate::status::show("ART: OFFLINE");
                        } else if saved > 0 {
                            crate::status::show(&format!("ART {saved} SAVED"));
                        } else {
                            // A job only starts when something is missing, so
                            // zero saved is news even when the shelf asked for
                            // it silently.
                            crate::status::show("NO ART FOUND");
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        keep = false;
                        self.art_stop = None;
                        self.art_progress = None;
                        break;
                    }
                }
            }
            if keep {
                self.art_rx = Some(rx);
            }
        }
        self.poll_status(now);
        // Resolve the state file's system once its scan has landed, and keep
        // the cursor inside the rail when systems come and go.
        if let Some(id) = self.want_system.clone() {
            let vis = self.visible_systems();
            if let Some(pos) = vis.iter().position(|s| s.id == id) {
                self.sel_system = pos;
                self.want_system = None;
            } else if self.scanning.is_empty() && !vis.is_empty() {
                self.sel_system = 0;
                self.want_system = None;
            }
        }
        let n = self.visible_systems().len();
        if n > 0 && self.sel_system >= n {
            self.sel_system = n - 1;
        }
        if self.state_dirty && now.duration_since(self.last_save) > SAVE_DEBOUNCE {
            self.save();
        }
        self.maybe_fetch_art();
    }

    /// Auto art: once per system per run, when the scan has landed and wifi
    /// is up. A failed or empty attempt is not retried automatically.
    fn maybe_fetch_art(&mut self) {
        if !self.prefs.auto_art || self.art_rx.is_some() {
            return;
        }
        // Downloads only pay off in the Art interface.
        if views::View::from_pref(self.prefs.interface) != views::View::Art {
            if !self.art_idle_logged {
                self.art_idle_logged = true;
                util::log("art: auto idle in the Simple interface");
            }
            return;
        }
        if !util::wifi_has_ip() {
            if !self.art_wait_logged {
                self.art_wait_logged = true;
                util::log("art: auto waits for a wifi address");
                crate::status::show("ART: NO WIFI");
            }
            return;
        }
        self.art_wait_logged = false;
        let Some(id) = self.current_id() else {
            return;
        };
        if self.art_done.contains(&id) || self.scanning.contains(&id) {
            return;
        }
        if self.current_games().is_empty() {
            return;
        }
        self.start_art(true);
    }

    pub fn flush(&mut self) {
        if self.state_dirty {
            self.save();
        }
    }

    fn save(&mut self) {
        let state = StateFile {
            system: self.current_id(),
            positions: self.positions.clone(),
            favorites: {
                let mut v: Vec<String> = self.favorites.iter().cloned().collect();
                v.sort();
                v
            },
            volume: self.volume_pct,
            interface: Some(self.prefs.interface),
            auto_art: Some(self.prefs.auto_art),
            art_type: Some(self.prefs.art_type),
            art_bg: Some(self.prefs.art_bg),
            art_types: self.art_types.clone(),
            ff_steps: Some(self.prefs.ff_steps),
            ff_sound: Some(self.prefs.ff_sound),
            text_scale: Some(self.prefs.text_scale),
            background: Some(self.prefs.background),
            anim_bg: Some(self.prefs.anim_bg),
            glow: Some(self.prefs.glow),
            shelf_bg: Some(self.prefs.shelf_bg),
            theme: Some(self.prefs.theme),
            shot_chord: Some(self.prefs.shot_chord),
            clock_12h: Some(self.prefs.clock_12h),
            battery_pct: Some(self.prefs.battery_pct),
            lid_sleep: Some(self.prefs.lid_sleep),
            shutdown_min: Some(self.prefs.shutdown_min),
            idle_min: Some(self.prefs.idle_min),
            bat_shutdown: Some(self.prefs.bat_shutdown),
            charge_limit: Some(self.prefs.charge_limit),
            led_on: Some(self.prefs.led_on),
            rumble: Some(self.prefs.rumble),
            hdmi_auto: Some(self.prefs.hdmi_auto),
        };
        let path = self.cwd.join("Config/magi.json");
        let _ = std::fs::create_dir_all(self.cwd.join("Config"));
        // Merge over the live file: keys owned by others (wifi_on, ssh_on,
        // bt_on, ntp_server, ...) must survive our saves.
        let mut v: serde_json::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .filter(|v: &serde_json::Value| v.is_object())
            .unwrap_or_else(|| serde_json::json!({}));
        if let Ok(serde_json::Value::Object(fields)) = serde_json::to_value(&state) {
            for (k, val) in fields {
                v[k] = val;
            }
        }
        match serde_json::to_string_pretty(&v) {
            Ok(text) => {
                if let Err(e) = util::write_file_fast(&path, text.as_bytes()) {
                    util::log(&format!("cannot save Config/magi.json: {e}"));
                } else {
                    self.state_dirty = false;
                    self.last_save = Instant::now();
                }
            }
            Err(e) => util::log(&format!("cannot serialize Config/magi.json: {e}")),
        }
    }

    pub fn poll_status(&mut self, now: Instant) {
        if now.duration_since(self.last_poll) < Duration::from_secs(10) {
            return;
        }
        self.last_poll = now;
        self.battery = util::battery_percent();
        self.clock = util::clock_hhmm();
        self.refresh_radio();
        // BaseOS's boot udhcpc can miss the lease when it starts before the
        // association. While the shelf is up, retry once there is an
        // association but still no address.
        let has_ip = util::wifi_has_ip();
        if has_ip != self.wifi_had_ip {
            util::log(&format!(
                "network: wifi address {}",
                if has_ip { "acquired" } else { "lost" }
            ));
            self.wifi_had_ip = has_ip;
        }
        if self.wifi_on
            && !has_ip
            && now.duration_since(self.dhcp_retry) > Duration::from_secs(20)
            && util::wifi_associated()
        {
            self.dhcp_retry = now;
            util::retry_dhcp();
        }
        // `work_led` is write-only and the AXP can reassert it late; keep
        // the user's choice authoritative while the shelf is up.
        #[cfg(target_os = "linux")]
        util::set_led(self.prefs.led_on);
        // Battery policy: the charge limit runs here too (a long shelf sit
        // while plugged would otherwise top off unmanaged).
        if util::battery_policy(
            self.prefs.bat_shutdown,
            self.prefs.charge_limit,
            &mut self.charge_latch,
        ) {
            self.flush();
            util::set_brightness(0);
            util::spawn_detached("poweroff");
        }
    }

    /// Wi-Fi and Bluetooth state, also refreshed right after a toggle.
    pub fn refresh_radio(&mut self) {
        self.wifi_on = util::wifi_up();
        self.bt_on = util::bt_on();
        // BaseOS powers wlan0 up (and starts dropbear) at boot, possibly
        // after we already applied a saved "off"; keep the choice enforced.
        if self.wifi_on && !util::saved_radio_state("wifi_on").unwrap_or(Defaults::WIFI) {
            util::log("wifi: off by default or saved off; turning it back off");
            // Service commands can take a second; keep them off the UI thread.
            self.wifi_on = false;
            self.wifi_signal = 0;
            std::thread::spawn(|| {
                util::set_wifi(false);
            });
        }
        if !util::saved_radio_state("ssh_on").unwrap_or(Defaults::SSH) && util::ssh_on() {
            util::log("ssh: off by default or saved off; stopping dropbear again");
            std::thread::spawn(|| {
                util::set_ssh(false);
            });
        }
        self.wifi_signal = if self.wifi_on {
            util::wifi_signal_pct().unwrap_or(0)
        } else {
            0
        };
    }

    // ----- accessors -----

    /// The strip: FAVORITES first when it has entries, then the systems that
    /// actually hold games. Folders with only a README never appear.
    pub fn visible_systems(&self) -> Vec<&SystemDef> {
        let mut list: Vec<&SystemDef> = Vec::new();
        if !self.fav_games.is_empty() {
            list.push(&self.fav_system);
        }
        list.extend(
            self.catalogue
                .systems
                .iter()
                .filter(|s| self.games.get(&s.id).is_some_and(|g| !g.is_empty())),
        );
        list
    }

    pub fn current(&self) -> Option<&SystemDef> {
        self.visible_systems().get(self.sel_system).copied()
    }

    pub fn current_id(&self) -> Option<String> {
        self.current().map(|s| s.id.clone())
    }

    pub fn current_games(&self) -> &[GameEntry] {
        let Some(id) = self.current_id() else {
            return &[];
        };
        if id == FAV_ID {
            return &self.fav_games;
        }
        self.games.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// The system a game path belongs to, by its `Games/<folder>`.
    fn game_system(&self, path: &str) -> Option<&SystemDef> {
        system_for_path(&self.catalogue.systems, path)
    }

    /// Rebuild the favorites list after a change, keeping the cursor valid.
    pub(crate) fn refresh_favorites(&mut self) {
        let keep = self.current_id();
        self.fav_games = gather_favorites(&self.catalogue.systems, &self.games, &self.favorites);
        if let Some(pos) = self.positions.get_mut(FAV_ID) {
            *pos = (*pos).min(self.fav_games.len().saturating_sub(1));
        }
        // The tabs may have moved under the cursor (FAVORITES comes and goes);
        // keep the same system selected.
        let pos = {
            let vis = self.visible_systems();
            keep.as_deref()
                .and_then(|id| vis.iter().position(|s| s.id == id))
                .unwrap_or_else(|| self.sel_system.min(vis.len().saturating_sub(1)))
        };
        self.sel_system = pos;
    }

    pub fn sel_game(&self) -> usize {
        self.current_id()
            .and_then(|id| self.positions.get(&id).copied())
            .unwrap_or(0)
    }

    pub fn selected_game(&self) -> Option<GameEntry> {
        self.current_games().get(self.sel_game()).cloned()
    }

    /// Rename the ROM file in place (extension kept); refreshes the list and
    /// keeps the renamed entry selected.
    pub fn rename_game(&mut self, path: &str, new_stem: &str) -> Result<(), String> {
        let Some(stem) = games::sanitize_game_name(new_stem) else {
            return Err("NAME NOT ALLOWED".into());
        };
        let Some(sys) = self.game_system(path).cloned() else {
            return Err("RENAME FAILED".into());
        };
        if sys.id == "n64" {
            return self.rename_n64(path, &stem, &sys.id);
        }
        let old = std::path::Path::new(path);
        let Some(dir) = old.parent() else {
            return Err("RENAME FAILED".into());
        };
        if old.file_stem().is_some_and(|s| s == stem.as_str()) {
            return Ok(());
        }
        let mut file_name = stem.clone();
        if let Some(ext) = old.extension() {
            file_name.push('.');
            file_name.push_str(&ext.to_string_lossy());
        }
        let new_path = dir.join(&file_name);
        if new_path.exists() {
            return Err("NAME ALREADY EXISTS".into());
        }
        std::fs::rename(old, &new_path).map_err(|e| {
            util::log(&format!("rename failed: {e}"));
            "RENAME FAILED".to_string()
        })?;
        util::log(&format!(
            "renamed {} -> {}",
            old.display(),
            new_path.display()
        ));
        let target = new_path.to_string_lossy().replace('\\', "/");
        if self.favorites.remove(path) {
            self.favorites.insert(target.clone());
        }
        self.rescan_system(&sys.id);
        if let Some(pos) = self.current_games().iter().position(|g| g.path == target) {
            self.set_sel_game(pos);
        }
        Ok(())
    }

    /// A native port is a folder around `rom.z64`; rename the folder and
    /// keep the port's label (its shelf title) in step.
    fn rename_n64(&mut self, path: &str, stem: &str, sys_id: &str) -> Result<(), String> {
        let rom = std::path::Path::new(path);
        let Some(folder) = rom.parent() else {
            return Err("RENAME FAILED".into());
        };
        if folder.file_name().is_some_and(|s| s == stem) {
            return Ok(());
        }
        let Some(parent) = folder.parent() else {
            return Err("RENAME FAILED".into());
        };
        let target = parent.join(stem);
        if target.exists() {
            return Err("NAME ALREADY EXISTS".into());
        }
        std::fs::rename(folder, &target).map_err(|e| {
            util::log(&format!("rename failed: {e}"));
            "RENAME FAILED".to_string()
        })?;
        let config = target.join("config.json");
        let mut json: serde_json::Value = std::fs::read_to_string(&config)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        json["label"] = serde_json::json!(stem);
        if let Ok(text) = serde_json::to_string_pretty(&json) {
            let _ = util::write_file_atomic(&config, text.as_bytes());
        }
        util::log(&format!(
            "renamed {} -> {}",
            folder.display(),
            target.display()
        ));
        let new_rom = target.join("rom.z64").to_string_lossy().replace('\\', "/");
        if self.favorites.remove(path) {
            self.favorites.insert(new_rom.clone());
        }
        self.rescan_system(sys_id);
        if let Some(pos) = self.current_games().iter().position(|g| g.path == new_rom) {
            self.set_sel_game(pos);
        }
        Ok(())
    }

    /// Delete the ROM file from the card (saves are left alone). Native
    /// port entries remove their whole game folder instead.
    pub fn delete_game(&mut self, path: &str) -> Result<(), String> {
        let Some(sys_id) = self.game_system(path).map(|s| s.id.clone()) else {
            return Err("DELETE FAILED".into());
        };
        let result = if sys_id == "n64" {
            let Some(folder) = std::path::Path::new(path).parent() else {
                return Err("DELETE FAILED".into());
            };
            std::fs::remove_dir_all(folder)
        } else {
            std::fs::remove_file(path)
        };
        result.map_err(|e| {
            util::log(&format!("delete failed: {e}"));
            "DELETE FAILED".to_string()
        })?;
        util::log(&format!("deleted {path}"));
        self.favorites.remove(path);
        self.rescan_system(&sys_id);
        let n = self.current_games().len();
        if n > 0 {
            self.set_sel_game(self.sel_game().min(n - 1));
        }
        Ok(())
    }

    /// Synchronously rescan one system's folder (rename/delete).
    fn rescan_system(&mut self, id: &str) {
        let Some(sys) = self.catalogue.systems.iter().find(|s| s.id == id).cloned() else {
            return;
        };
        let list = if sys.id == "n64" {
            games::scan_n64(&self.cwd, &format!("Games/{}", sys.folder()))
        } else {
            games::scan_dir(&self.cwd, &format!("Games/{}", sys.folder()), &sys.exts)
        };
        self.games.insert(sys.id, list);
        self.refresh_favorites();
    }

    pub fn is_scanning(&self) -> bool {
        self.current_id()
            .map(|id| self.scanning.contains(&id))
            .unwrap_or(false)
    }

    /// True while any system's scan is still in flight; the app loop uses it
    /// to keep polling even when the current list is not visible yet.
    pub fn scan_pending(&self) -> bool {
        self.scan_rx.is_some() || !self.scanning.is_empty()
    }

    /// True while the selected title is scrolling.
    pub fn title_scrolling(&self) -> bool {
        self.marquee
    }

    fn set_sel_game(&mut self, pos: usize) {
        if let Some(id) = self.current_id() {
            self.positions.insert(id, pos);
            self.state_dirty = true;
        }
    }

    /// Seconds the highlighted row has been held; resets when it changes.
    fn selection_hold(&mut self, system_id: &str, path: &str) -> f32 {
        if self.sel_id != system_id || self.sel_path != path {
            self.sel_id.clear();
            self.sel_id.push_str(system_id);
            self.sel_path.clear();
            self.sel_path.push_str(path);
            self.sel_since = Instant::now();
        }
        self.sel_since.elapsed().as_secs_f32()
    }

    // ----- actions -----

    pub fn move_system(&mut self, delta: i32) {
        let n = self.visible_systems().len() as i32;
        if n == 0 {
            return;
        }
        self.sel_system = (self.sel_system as i32 + delta).rem_euclid(n) as usize;
        self.first_visible = 0;
        self.state_dirty = true;
    }

    pub fn move_game(&mut self, delta: i32) {
        let n = self.current_games().len();
        if n == 0 {
            return;
        }
        let pos = (self.sel_game() as i32 + delta).clamp(0, n as i32 - 1) as usize;
        self.set_sel_game(pos);
    }

    /// Up/Down: move inside the game list; with an empty list, browse systems.
    pub fn nav_up(&mut self) {
        self.nav(-1);
    }

    pub fn nav_down(&mut self) {
        self.nav(1);
    }

    fn nav(&mut self, delta: i32) {
        if self.current_games().is_empty() {
            self.move_system(delta);
        } else {
            self.move_game(delta);
        }
    }

    /// The game the A button would launch right now.
    pub fn selected_launch(&self) -> Option<(SystemDef, GameEntry)> {
        let game = self.selected_game()?;
        let sys = match self.current() {
            Some(s) if s.id != FAV_ID => s.clone(),
            _ => self.game_system(&game.path)?.clone(),
        };
        Some((sys, game))
    }

    /// X on the shelf: add or drop the selected game in FAVORITES.
    pub fn toggle_favorite(&mut self) {
        let Some(game) = self.selected_game() else {
            return;
        };
        let added = self.favorites.insert(game.path.clone());
        if !added {
            self.favorites.remove(&game.path);
        }
        self.state_dirty = true;
        self.refresh_favorites();
        crate::status::show(if added {
            "FAVORITE ADDED"
        } else {
            "FAVORITE REMOVED"
        });
    }

    /// The selected game's cover: decoded once per path and art type, then
    /// reused. ALL shows the first file that exists (boxart first).
    pub fn cover_for(&mut self, game_path: &str, art_type: u8) -> Option<crate::image::Image> {
        if let Some((key, ty, img)) = &self.cover {
            if key == game_path && *ty == art_type {
                return img.clone();
            }
        }
        let img = views::cover_paths(game_path, art_type)
            .into_iter()
            .find_map(|path| crate::image::load_png(&path));
        self.cover = Some((game_path.to_string(), art_type, img.clone()));
        img
    }

    /// The art type for a system: its CORE ART override, else the global one.
    pub fn art_type_for(&self, system_id: &str) -> u8 {
        self.art_types
            .get(system_id)
            .copied()
            .unwrap_or(self.prefs.art_type)
    }

    /// Y -> CORE -> CORE ART: cycle the current system's type.
    pub fn cycle_system_art(&mut self, delta: i32) {
        let Some(id) = self.current_id() else {
            return;
        };
        let next =
            ((self.art_type_for(&id) as i32 + delta).rem_euclid(art::LABELS.len() as i32)) as u8;
        self.art_types.insert(id, next);
        self.state_dirty = true;
        self.cover = None;
    }

    /// Fetch the current system's missing art. A running job is left alone.
    pub fn start_art_job(&mut self) -> bool {
        self.start_art(false)
    }

    /// B on the shelf: stop after the request in flight.
    pub fn stop_art(&mut self) {
        if let Some(stop) = &self.art_stop {
            stop.store(true, Ordering::Relaxed);
        }
    }

    pub fn is_fetching_art(&self) -> bool {
        self.art_rx.is_some()
    }

    fn start_art(&mut self, quiet: bool) -> bool {
        if self.art_rx.is_some() {
            if !quiet {
                crate::status::show("ART ALREADY RUNNING");
            }
            return false;
        }
        let Some(sys) = self.current().cloned() else {
            return false;
        };
        let art_type = self.art_type_for(&sys.id);
        let types: Vec<u8> = if art_type >= 4 {
            (0..4).collect()
        } else {
            vec![art_type]
        };
        let games: Vec<art::Game> = self
            .current_games()
            .iter()
            .filter_map(|g| {
                let covers: Vec<(u8, std::path::PathBuf)> = types
                    .iter()
                    .filter_map(|t| {
                        let cover = self.cwd.join(views::cover_path_for(&g.path, *t)?);
                        (!cover.is_file()).then_some((*t, cover))
                    })
                    .collect();
                (!covers.is_empty()).then(|| art::Game {
                    rom: g.path.clone(),
                    system: system_for_path(&self.catalogue.systems, &g.path)
                        .unwrap_or(&sys)
                        .clone(),
                    covers,
                })
            })
            .collect();
        if games.is_empty() {
            self.art_done.insert(sys.id.clone());
            if !quiet {
                crate::status::show("ART IS COMPLETE");
            }
            return false;
        }
        let total = games.len();
        util::log(&format!(
            "art: {} game{} to fill for {} (types {types:?})",
            total,
            if total == 1 { "" } else { "s" },
            sys.id
        ));
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let job = art::Job {
            cwd: self.cwd.clone(),
            games,
        };
        let flag = stop.clone();
        std::thread::spawn(move || art::run(job, tx, &flag));
        self.art_rx = Some(rx);
        self.art_stop = Some(stop);
        self.art_progress = Some((sys.id.clone(), 0, total));
        self.art_quiet = quiet;
        self.art_done.insert(sys.id);
        if !quiet {
            crate::status::show("FETCHING ART");
        }
        true
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn volume_percent(&self) -> u32 {
        self.volume_pct.unwrap_or(100)
    }

    pub fn volume(&mut self, delta: i32) {
        let base = self.volume_pct.unwrap_or(100) as i32;
        let pct = (base + delta).clamp(0, 100) as u32;
        self.volume_pct = Some(pct);
        self.state_dirty = true;
    }

    /// The run host writes the volume back; pick it up after a game exits.
    pub fn reload_volume(&mut self) {
        let path = self.cwd.join("Config/magi.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(f) = serde_json::from_str::<StateFile>(&text) {
                if let Some(v) = f.volume {
                    self.volume_pct = Some(v.min(100));
                }
            }
        }
    }

    /// Settings-screen preferences, persisted in the same config file.
    pub fn set_prefs(&mut self, prefs: Prefs) {
        self.prefs = prefs;
        self.state_dirty = true;
    }

    pub fn toggle_controls(&mut self) {
        self.controls_open = !self.controls_open;
    }
}

pub fn draw(cv: &mut Canvas, fonts: &mut Fonts, st: &mut LibraryState, t: f32) {
    let w = cv.w;
    let h = cv.h;
    let sc = crate::theme::ui_scale();
    let p = |v: f32| (v * sc).round() as i32;
    let top_h = p(27.0);
    let bottom_bar = p(24.0);
    let view = views::View::from_pref(st.prefs.interface);
    cv.clear(pal().bg0);
    st.marquee = false;

    // Top bar; its divider breathes with the glow theme.
    cv.fill_rect(0, 0, w as u32, top_h as u32, pal().bg1);
    cv.hline(
        0,
        top_h,
        w as u32,
        crate::theme::glow_color(pal().border, pal().glow, t, 0.1),
    );
    fonts.draw(
        cv,
        p(10.0),
        top_h - p(8.0),
        "MAGI // SYSTEM-01",
        10.0,
        pal().title,
    );

    // The clock in NERV timer digits, with AM/PM in 12-hour mode.
    let mut clock = st.clock.clone().unwrap_or_else(|| "--:--".into());
    let mut pm = None;
    if st.prefs.clock_12h {
        if let Some((text, is_pm)) = util::clock_12h_from_hhmm(&clock) {
            clock = text;
            pm = Some(is_pm);
        }
    }
    let clock_h = p(14.0);
    let cw = crate::theme::seven_segment_width(&clock, clock_h);
    let suffix = pm.map(|pm| if pm { "PM" } else { "AM" });
    let suffix_w = suffix
        .map(|s| fonts.text_width(s, 9.0) + p(3.0))
        .unwrap_or(0);
    let clock_x = w - p(12.0) - suffix_w - cw;
    crate::theme::seven_segment(
        cv,
        clock_x,
        (top_h - clock_h) / 2,
        clock_h,
        &clock,
        pal().clock,
    );
    if let Some(suffix) = suffix {
        fonts.draw(
            cv,
            clock_x + cw + p(3.0),
            top_h / 2 + 4,
            suffix,
            9.0,
            pal().clock,
        );
    }

    // The radio icons, right next to the clock.
    let mut icons_x = clock_x - p(8.0);
    if st.bt_on {
        crate::theme::bt_icon(cv, icons_x - p(12.0), p(9.0), pal().link);
        icons_x -= p(16.0);
    }
    if st.wifi_on {
        let color = if st.wifi_signal > 0 {
            pal().signal
        } else {
            pal().text_faint
        };
        crate::theme::wifi_icon(cv, icons_x - p(16.0), p(8.0), color, st.wifi_signal);
    }

    // The interface decides where the systems, the header and the list sit.
    let lay = views::layout(view, w, h, top_h, bottom_bar, sc);

    // Optionally the backdrop reaches the content pane too.
    if crate::theme::shelf_bg() {
        let (sx, sy, sw, sh) = lay.systems;
        let (bx, by, bw, bh) = match view {
            views::View::Simple => (sx + sw + 1, sy, w - sx - sw - 1, sh),
            views::View::Art => (0, sy + sh, w, h - bottom_bar - sy - sh),
        };
        crate::theme::background(cv, bx, by, bw as u32, bh as u32, t);
    }

    {
        let systems = st.visible_systems();
        let names: Vec<&str> = systems.iter().map(|s| s.name.as_str()).collect();
        views::draw_systems(cv, fonts, view, &names, st.sel_system, lay.systems, sc, t);
    }

    // Game list pane.
    let px0 = lay.list_x;
    let pane_right = lay.list_right;
    let list_top = lay.list_top;
    let list_bottom = h - bottom_bar - 8;
    let row_h = (ROW_H as f32 * sc).round() as i32;
    let vis = ((list_bottom - list_top) / row_h).max(1) as usize;

    if let Some(sys) = st.current().cloned() {
        let total = st.current_games().len();
        let sel = st.sel_game();
        let (title, title_sz) = fit_title(fonts, &sys.name, 20.0, 12.0, pane_right - p(14.0) - px0);
        fonts.draw(cv, px0, lay.header_y, &title, title_sz, pal().title);

        let pos_txt = format!("{}/{}", sel + 1, total);
        let count_w = if total > 0 {
            fonts.text_width(&pos_txt, 10.0)
        } else {
            0
        };
        if total > 0 {
            fonts.draw(
                cv,
                pane_right - count_w,
                lay.subtitle_y,
                &pos_txt,
                10.0,
                pal().accent,
            );
        }

        let sub_max = pane_right - count_w - p(14.0) - px0;
        if st.is_scanning() {
            fonts.draw(cv, px0, lay.subtitle_y, "SCANNING...", 10.0, pal().text_dim);
        } else {
            let sub = if sys.id == FAV_ID {
                format!("{total} FAVORITE{}", if total == 1 { "" } else { "S" })
            } else {
                format!("Games/{}/   {total} GAMES", sys.folder())
            };
            let sub = elide(fonts, &sub, 10.0, sub_max);
            fonts.draw(cv, px0, lay.subtitle_y, &sub, 10.0, pal().text_faint);
        }

        if total == 0 && !st.is_scanning() {
            let (msg, hint) = if sys.id == FAV_ID {
                ("NO FAVORITES".to_string(), "PRESS X ON A GAME".to_string())
            } else {
                (
                    "NO GAMES FOUND".to_string(),
                    format!("COPY GAMES TO  Games/{}/", sys.folder()),
                )
            };
            fonts.draw(cv, px0, list_top + p(42.0), &msg, 14.0, pal().text_dim);
            fonts.draw(cv, px0, list_top + p(68.0), &hint, 11.0, pal().text_faint);
        } else {
            // Keep the selection visible with minimal scrolling.
            if sel < st.first_visible {
                st.first_visible = sel;
            } else if sel >= st.first_visible + vis {
                st.first_visible = sel + 1 - vis;
            }
            let first = st.first_visible.min(total.saturating_sub(1));

            let sel_path = st
                .current_games()
                .get(sel)
                .map(|g| g.path.clone())
                .unwrap_or_default();
            let sel_hold = st.selection_hold(&sys.id, &sel_path);
            let art_type = st.art_type_for(&sys.id);
            let cover = match view {
                views::View::Art => st.cover_for(&sel_path, art_type),
                views::View::Simple => None,
            };
            let list = views::List {
                games: st.current_games(),
                sel,
                first,
                rows: vis,
                favorites: &st.favorites,
                x0: px0,
                right: pane_right,
                baseline: list_top + (18.0 * sc) as i32,
                row_h,
                sel_hold,
            };
            let scrolling = match view {
                views::View::Simple => views::draw_simple(cv, fonts, &list, t),
                views::View::Art => views::draw_art(
                    cv,
                    fonts,
                    &list,
                    lay.art.unwrap_or_default(),
                    cover.as_ref(),
                    st.prefs.art_bg > 0,
                    t,
                ),
            };
            st.marquee = scrolling;

            // Scrollbar.
            if total > vis {
                let track_x = pane_right + 4;
                let track_h = (list_bottom - list_top) as u32;
                cv.fill_rect(track_x, list_top - 6, 2, track_h, pal().scrollbar_track);
                let thumb_h = ((track_h as usize * vis / total).max(14)) as u32;
                let span = (total - vis).max(1);
                let thumb_y =
                    list_top - 6 + ((track_h as usize - thumb_h as usize) * first / span) as i32;
                cv.fill_rect(track_x, thumb_y, 2, thumb_h, pal().scrollbar);
            }
        }
    }

    // Bottom bar: battery, status line, art in flight and version.
    cv.fill_rect(0, h - bottom_bar, w as u32, bottom_bar as u32, pal().bg1);
    cv.hline(
        0,
        h - bottom_bar,
        w as u32,
        crate::theme::glow_color(pal().border, pal().glow, t, 0.7),
    );
    let battery_right = draw_battery(cv, fonts, 10, h - 19, st.battery, st.prefs.battery_pct);

    let v = format!("MAGI v{}", env!("CARGO_PKG_VERSION"));
    let vw = fonts.text_width(&v, 9.0);
    let version_x = w - 10 - vw;
    fonts.draw(cv, version_x, h - 8, &v, 9.0, pal().text_faint);

    let mut right = version_x;
    if st.is_fetching_art() {
        if let Some((_, done, total)) = &st.art_progress {
            let label = format!("ART {done}/{total}");
            let lw = fonts.text_width(&label, 9.0);
            right -= p(12.0);
            fonts.draw(cv, right - lw, h - 8, &label, 9.0, pal().accent);
            right -= lw + p(8.0);
        }
        crate::theme::spinner(cv, right - p(4.0), h - bottom_bar / 2, t, pal().signal);
        right -= p(14.0);
    }

    // Info and errors live on this line instead of a floating box.
    crate::status::draw_bar(cv, fonts, battery_right, right, h - 8);

    if st.controls_open {
        draw_controls(cv, fonts, t);
    }
}

/// The game actions overlay (Y on the shelf): GAME and CORE tabs.
pub fn draw_game_menu(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    title: &str,
    tab: u8,
    sel: usize,
    confirm: bool,
    art_type: u8,
    t: f32,
) {
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let ww = p(400.0).min(w - p(24.0));
    let hh = p(216.0);
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 236);
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

    // Two tabs, the active one underlined.
    let tabs = ["GAME", "CORE"];
    for (i, label) in tabs.iter().enumerate() {
        let x = w / 2 + if i == 0 { -p(56.0) } else { p(56.0) };
        let active = i as u8 == tab;
        fonts.draw_center(
            cv,
            x,
            y0 + p(28.0),
            label,
            13.0,
            if active {
                pal().accent
            } else {
                pal().text_faint
            },
        );
        if active {
            let lw = fonts.text_width(label, 13.0);
            cv.fill_rect(x - lw / 2, y0 + p(35.0), lw as u32, 2, pal().accent);
        }
    }

    let name = elide(fonts, title, 9.0, ww - p(40.0));
    fonts.draw_center(cv, w / 2, y0 + p(58.0), &name, 9.0, pal().text_faint);

    if confirm {
        fonts.draw_center(
            cv,
            w / 2,
            y0 + p(110.0),
            "DELETE THIS FILE?",
            13.0,
            pal().alert,
        );
        fonts.draw_center(
            cv,
            w / 2,
            y0 + p(136.0),
            "A YES    B NO",
            11.0,
            pal().text_dim,
        );
        return;
    }

    let row_x = x0 + p(34.0);
    let row_w = ww - p(60.0);
    let mut row = |i: usize, label: &str, value: Option<&str>| {
        let y = y0 + p(96.0) + i as i32 * p(30.0);
        let selected = i == sel;
        if selected {
            cv.fill_rect(
                row_x - p(8.0),
                y - p(16.0),
                row_w as u32,
                p(22.0) as u32,
                pal().bg3,
            );
        }
        fonts.draw(
            cv,
            row_x,
            y,
            label,
            11.0,
            if selected {
                pal().accent
            } else {
                pal().text_dim
            },
        );
        if let Some(value) = value {
            let vw = fonts.text_width(value, 11.0);
            fonts.draw(cv, x0 + ww - p(30.0) - vw, y, value, 11.0, pal().text);
        }
    };
    if tab == 0 {
        row(0, "RENAME", None);
        row(1, "DELETE", None);
    } else {
        let value = format!(
            "< {} >",
            crate::art::LABELS[(art_type as usize).min(crate::art::LABELS.len() - 1)]
        );
        row(0, "CORE ART", Some(&value));
    }

    fonts.draw_center(
        cv,
        w / 2,
        y0 + hh - p(20.0),
        "L1/R1 TAB   U/D PICK   L/R CHANGE   A SELECT   B BACK",
        9.0,
        pal().text_faint,
    );
}

/// The CONTROLS panel: every button, both on the shelf and in a game.
fn draw_controls(cv: &mut Canvas, fonts: &mut Fonts, t: f32) {
    let (w, h) = (cv.w, cv.h);
    cv.fill_rect(0, 0, w as u32, h as u32, pal().bg0);
    crate::theme::background(cv, 12, 62, (w - 32) as u32, (h - 122) as u32, t);
    crate::theme::glow_border(
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
    let title = "CONTROLS // MAGI";
    fonts.draw_center(cv, w / 2, 42, title, 15.0, pal().title);
    crate::theme::hazard_bar(cv, w / 2 - 150, 56, 300, 5, t);
    cv.vline(w / 2, 76, (h - 130) as u32, pal().bg3);

    // Section headers stand apart: bigger, tinted, underlined with stripes.
    let section = |cv: &mut Canvas, fonts: &mut Fonts, x: i32, label: &str| {
        fonts.draw(cv, x, 96, label, 14.0, pal().title);
        crate::theme::hazard_bar(cv, x, 102, 150, 3, t);
    };
    let left_x = 44;
    let right_x = w / 2 + 36;
    section(cv, fonts, left_x, "SHELF");
    section(cv, fonts, right_x, "IN GAME");

    let mut shelf: Vec<(&str, &str)> = vec![
        ("D-PAD L/R", "SWITCH SYSTEM"),
        ("L2 / R2", "SWITCH SYSTEM"),
        ("D-PAD U/D", "BROWSE GAMES"),
        ("L1 / R1", "PAGE"),
        ("A", "PLAY"),
        ("X", "FAVORITE"),
        ("MENU", "SETTINGS"),
        ("SELECT", "THIS PANEL"),
        ("SELECT+U/D", "BRIGHTNESS"),
        ("SELECT+L/R", "TEMPERATURE"),
    ];
    if crate::input::shot_chord() {
        shelf.push(("SEL+MEN+R1", "SCREENSHOT"));
    }
    for (i, (key, what)) in shelf.iter().enumerate() {
        let y = 132 + i as i32 * 30;
        fonts.draw(cv, left_x, y, key, 11.0, pal().text);
        fonts.draw(cv, left_x + 150, y, what, 11.0, pal().text_dim);
    }

    let mut game: Vec<(&str, &str)> = vec![
        ("L2 HOLD", "REWIND"),
        ("R2 HOLD", "FAST FORWARD"),
        ("R2 DOUBLE-TAP", "FF LATCH"),
        ("SELECT+R1", "SAVE STATE"),
        ("SELECT+L1", "LOAD STATE"),
        ("SELECT+X", "SCALE MODE"),
        ("SELECT+U/D", "BRIGHTNESS"),
        ("SELECT+L/R", "TEMPERATURE"),
        ("HOLD MENU", "SAVE + EXIT"),
    ];
    if crate::input::shot_chord() {
        game.push(("SEL+MEN+R1", "SCREENSHOT"));
    }
    for (i, (key, what)) in game.iter().enumerate() {
        let y = 132 + i as i32 * 30;
        fonts.draw(cv, right_x, y, key, 11.0, pal().text);
        fonts.draw(cv, right_x + 150, y, what, 11.0, pal().text_dim);
    }

    fonts.draw_center(
        cv,
        w / 2,
        h - 26,
        "PAUSE MENU: STATES / CORE / VIDEO / EXIT",
        9.0,
        pal().text_faint,
    );
    fonts.draw_center(
        cv,
        w / 2,
        h - 12,
        "ANY BUTTON TO CLOSE",
        9.0,
        pal().text_dim,
    );
}

/// Largest size (down to `min_px`) that fits `max_w`, then elided.
pub(crate) fn fit_title(
    fonts: &mut Fonts,
    text: &str,
    max_px: f32,
    min_px: f32,
    max_w: i32,
) -> (String, f32) {
    let mut size = max_px;
    while size > min_px && fonts.text_width(text, size) > max_w {
        size -= 0.5;
    }
    (elide(fonts, text, size, max_w), size)
}

pub(crate) fn elide(fonts: &mut Fonts, text: &str, size: f32, max_w: i32) -> String {
    fonts.elide(text, size, max_w)
}

fn draw_battery(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    left: i32,
    y: i32,
    pct: Option<u32>,
    show_pct: bool,
) -> i32 {
    cv.rect_outline(left, y, 24, 12, pal().battery_shell);
    cv.fill_rect(left + 24, y + 3, 2, 6, pal().battery_shell);
    let (col, txt) = match pct {
        Some(p) if p <= 15 => (pal().alert, format!("{p}%")),
        Some(p) => (pal().battery, format!("{p}%")),
        None => (pal().text_dim, "--%".into()),
    };
    if let Some(p) = pct {
        let fill_w = 20 * p.min(100) / 100;
        cv.fill_rect(left + 2, y + 2, fill_w, 8, col);
    }
    let mut right = left + 26;
    if show_pct {
        let tw = fonts.text_width(&txt, 10.0);
        fonts.draw(cv, right + 6, y + 11, &txt, 10.0, col);
        right += 6 + tw;
    }
    right
}

#[cfg(test)]
mod state_tests {
    use super::*;

    #[test]
    fn saved_values_layer_over_defaults() {
        let empty: StateFile = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.prefs().text_scale, Defaults::TEXT_SCALE);
        assert_eq!(empty.prefs().ff_sound, Defaults::FF_SOUND);
        assert_eq!(empty.prefs().charge_limit, Defaults::CHARGE_LIMIT);

        let stored: StateFile = serde_json::from_str(
            r#"{"ff_sound":false,"text_scale":9.0,"theme":99,"background":9}"#,
        )
        .unwrap();
        let p = stored.prefs();
        assert!(!p.ff_sound, "explicit value wins");
        assert_eq!(p.text_scale, 1.5, "clamped to the largest step");
        assert_eq!(p.theme, (crate::theme::themes().len() - 1) as u8);
        assert_eq!(p.background, (crate::theme::bg_styles().len() - 1) as u8);
    }

    fn sys(id: &str, folder: Option<&str>) -> SystemDef {
        SystemDef {
            id: id.into(),
            name: id.to_uppercase(),
            exts: Vec::new(),
            core: None,
            folder: folder.map(str::to_string),
            art: None,
        }
    }

    #[test]
    fn a_game_path_resolves_to_its_system() {
        let systems = [sys("n64", None), sys("psx", Some("PS1"))];
        assert_eq!(
            system_for_path(&systems, "Games/N64/Mario/rom.z64").map(|s| s.id.as_str()),
            Some("n64")
        );
        assert_eq!(
            system_for_path(&systems, "Games/PS1/Final Fantasy.cue").map(|s| s.id.as_str()),
            Some("psx")
        );
        assert!(system_for_path(&systems, "Games/SNES/Game.sfc").is_none());
    }
}
