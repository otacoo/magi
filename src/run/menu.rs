//! The in-game pause menu: state, navigation and drawing.

use std::time::{Duration, Instant, SystemTime};

use super::*;

mod draw;

pub(crate) use draw::draw_menu_preview;
pub(super) use draw::{draw_menu, draw_netplay};

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuSection {
    Resume,
    Save,
    Load,
    Core,
    Video,
    Link,
    Disc,
    Exit,
}

impl MenuSection {
    fn label(self) -> &'static str {
        match self {
            Self::Resume => "RESUME",
            Self::Save => "SAVE STATE",
            Self::Load => "LOAD STATE",
            Self::Core => "CORE",
            Self::Video => "VIDEO",
            Self::Link => "LINK",
            Self::Disc => "DISC",
            Self::Exit => "EXIT",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Resume => "RESUME GAME",
            Self::Save => "SAVE STATE",
            Self::Load => "LOAD STATE",
            Self::Core => "CORE OPTIONS",
            Self::Video => "VIDEO",
            Self::Link => "GAME BOY LINK",
            Self::Disc => "DISC",
            Self::Exit => "EXIT GAME",
        }
    }
}

/// Rows that fit in the option pane at the current text scale.
fn option_rows() -> usize {
    match crate::theme::ui_scale() {
        s if s >= 1.4 => 4,
        s if s >= 1.2 => 6,
        _ => 8,
    }
}

fn menu_sections() -> Vec<MenuSection> {
    let linked = ctx().netplay_linked;
    let mut v = vec![MenuSection::Resume];
    if !linked {
        v.push(MenuSection::Save);
        v.push(MenuSection::Load);
        if ctx().options.iter().any(|o| o.visible) {
            v.push(MenuSection::Core);
        }
    }
    v.push(MenuSection::Video);
    if netplay_supported() {
        v.push(MenuSection::Link);
    }
    if disc_count() > 1 {
        v.push(MenuSection::Disc);
    }
    v.push(MenuSection::Exit);
    v
}

fn netplay_status_line() -> String {
    let Some(session) = ctx().netplay.as_ref() else {
        return if ctx().netplay_linked {
            "LINKED".into()
        } else {
            "NOT CONNECTED".into()
        };
    };
    match session.status() {
        netplay::Status::Starting => "STARTING".into(),
        netplay::Status::Waiting => "HOSTING: WAITING".into(),
        netplay::Status::Scanning => "JOINING: SCANNING".into(),
        netplay::Status::Found => "HOSTS FOUND".into(),
        netplay::Status::Connecting => "CONNECTING".into(),
        netplay::Status::Linked => format!("LINKED {}", session.code()),
        netplay::Status::Lost => "LINK LOST".into(),
        netplay::Status::Error(e) => e,
        netplay::Status::Stopped => "STOPPED".into(),
    }
}

fn section_summary(section: MenuSection) -> String {
    match section {
        MenuSection::Resume => "RETURN TO THE GAME".into(),
        MenuSection::Save => "WRITE THE CURRENT POSITION".into(),
        MenuSection::Load => "RETURN TO A SAVED POSITION".into(),
        MenuSection::Core => "PER-GAME SETTINGS".into(),
        MenuSection::Video => "DISPLAY AND LAYER OPTIONS".into(),
        MenuSection::Link => netplay_status_line(),
        MenuSection::Disc => "SWAP WITHOUT RESET".into(),
        MenuSection::Exit => "FLUSH SRAM AND QUIT".into(),
    }
}

fn format_state_age(age: Duration) -> String {
    let secs = age.as_secs();
    if secs < 60 {
        "JUST NOW".into()
    } else if secs < 3600 {
        format!("{}M AGO", secs / 60)
    } else if secs < 86_400 {
        format!("{}H AGO", secs / 3600)
    } else if secs < 7 * 86_400 {
        format!("{}D AGO", secs / 86_400)
    } else {
        format!("{}W AGO", secs / (7 * 86_400))
    }
}

fn read_state_card(path: &Path, label: String, now: SystemTime) -> StateCard {
    let meta = std::fs::metadata(path).ok();
    let exists = meta.is_some();
    let age = meta
        .and_then(|m| m.modified().ok())
        .and_then(|m| now.duration_since(m).ok())
        .map(format_state_age)
        .unwrap_or_default();
    let thumb = if exists {
        let thumb = crate::image::load_image(&state_thumb_path(path));
        if thumb.is_none() {
            util::log(&format!(
                "state thumb: missing {}",
                state_thumb_path(path).display()
            ));
        }
        thumb
    } else {
        None
    };
    StateCard {
        path: path.to_path_buf(),
        label,
        age,
        thumb,
        exists,
    }
}

fn netplay_supported() -> bool {
    matches!(ctx().system.as_str(), "gb" | "gbc")
        && ctx().opt_key == "gambatte"
        && ctx()
            .options
            .iter()
            .any(|o| o.key == "gambatte_gb_link_mode")
}

fn cycled(cur: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (((cur as isize + delta) % len as isize + len as isize) % len as isize) as usize
}

impl GameMenu {
    pub(super) fn open_menu(&mut self) {
        self.open = true;
        self.page = MenuPage::Main;
        self.focus_pane = false;
        self.sel = 0;
        self.slot = self.slot.min(STATE_SLOTS - 1);
        self.disc_sel = disc_index().min(disc_count().saturating_sub(1));
        self.msg = None;
        self.delete_armed = None;
        self.refresh_states();
    }

    pub(super) fn handle_raw(&mut self, e: &RawEvent) {
        if e.value == 0 {
            return; // act on presses only; hat directions arrive as +/- 1
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
        let y = e.type_ == ev::EV_KEY && e.code == ev::BTN_C;

        if self.page == MenuPage::Netplay {
            self.handle_netplay(up, down, a, b);
            return;
        }
        self.handle_main(up, down, left, right, a, b, y);
    }

    fn handle_main(
        &mut self,
        up: bool,
        down: bool,
        left: bool,
        right: bool,
        a: bool,
        b: bool,
        y: bool,
    ) {
        let sections = menu_sections();
        if self.sel >= sections.len() {
            self.sel = 0;
        }
        let section = sections[self.sel];

        if self.focus_pane {
            match section {
                MenuSection::Core => {
                    self.handle_core(up, down, left, right, a, b);
                    return;
                }
                MenuSection::Video => {
                    self.handle_video(up, down, left, right, a, b);
                    return;
                }
                _ => self.focus_pane = false,
            }
        }

        if up {
            self.sel = cycled(self.sel, sections.len(), -1);
            self.delete_armed = None;
        } else if down {
            self.sel = cycled(self.sel, sections.len(), 1);
            self.delete_armed = None;
        } else if left || right {
            let delta = if right { 1 } else { -1 };
            match section {
                MenuSection::Save | MenuSection::Load => {
                    self.slot = cycled(self.slot, STATE_SLOTS, delta);
                    self.delete_armed = None;
                }
                MenuSection::Disc => {
                    let count = disc_count();
                    self.disc_sel = cycled(self.disc_sel, count, delta);
                }
                MenuSection::Core | MenuSection::Video if right => {
                    self.focus_pane = true;
                }
                _ => {}
            }
        } else if a {
            match section {
                MenuSection::Resume => self.open = false,
                MenuSection::Save => self.save_selected_state(),
                MenuSection::Load => self.load_selected_state(),
                MenuSection::Core | MenuSection::Video => self.focus_pane = true,
                MenuSection::Link => self.open_netplay(),
                MenuSection::Disc => {
                    if disc_count() > 1 {
                        self.msg = Some(match disc_swap(self.disc_sel) {
                            true => (
                                format!("DISC {} INSERTED", self.disc_sel + 1),
                                Instant::now(),
                            ),
                            false => ("DISC SWAP FAILED".into(), Instant::now()),
                        });
                    }
                }
                MenuSection::Exit => {
                    netplay_stop();
                    self.exit_requested = true;
                }
            }
        } else if y {
            if matches!(section, MenuSection::Save | MenuSection::Load) {
                self.delete_selected_state();
            }
        } else if b {
            self.open = false;
        }
    }

    fn handle_core(&mut self, up: bool, down: bool, left: bool, right: bool, a: bool, b: bool) {
        let count = visible_options(ctx()).len();
        if count == 0 {
            self.focus_pane = false;
            return;
        }
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
            self.focus_pane = false;
        }
    }

    fn handle_video(&mut self, up: bool, down: bool, left: bool, right: bool, a: bool, b: bool) {
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
            self.focus_pane = false;
        }
    }

    pub(super) fn refresh_states(&mut self) {
        let now = SystemTime::now();
        self.states.clear();
        for (i, path) in self.state_paths.iter().enumerate() {
            self.states
                .push(read_state_card(path, format!("SLOT {}", i + 1), now));
        }
        if self.slot >= self.states.len() {
            self.slot = 0;
        }
    }

    fn save_selected_state(&mut self) {
        if ctx().netplay_linked {
            self.msg = Some(("LINKED: STATES LOCKED".into(), Instant::now()));
            return;
        }
        if self.states.len() < STATE_SLOTS {
            self.refresh_states();
        }
        let idx = self.slot.min(self.states.len().saturating_sub(1));
        let Some(card) = self.states.get(idx) else {
            return;
        };
        let (path, label) = (card.path.clone(), card.label.clone());
        let saved = save_state_to(&path);
        if saved == Some(true) {
            // An explicit save is also the resume point for the next launch.
            match save_state_to(&self.auto_path) {
                Some(true) => util::log("auto state saved (menu)"),
                Some(false) => util::log("auto state save failed (menu)"),
                None => {}
            }
        }
        self.msg = Some(match saved {
            Some(true) => (format!("{label} SAVED"), Instant::now()),
            Some(false) => ("SAVE FAILED".into(), Instant::now()),
            None => ("SAVES NOT SUPPORTED".into(), Instant::now()),
        });
        self.refresh_states();
    }

    fn load_selected_state(&mut self) {
        if ctx().netplay_linked {
            self.msg = Some(("LINKED: STATES LOCKED".into(), Instant::now()));
            return;
        }
        if self.states.len() < STATE_SLOTS {
            self.refresh_states();
        }
        let idx = self.slot.min(self.states.len().saturating_sub(1));
        let Some(card) = self.states.get(idx) else {
            return;
        };
        let (path, label, exists) = (card.path.clone(), card.label.clone(), card.exists);
        if !exists {
            self.msg = Some((format!("{label} EMPTY"), Instant::now()));
            return;
        }
        match load_state_from(&path) {
            Ok(true) => self.open = false,
            Ok(false) => self.msg = Some(("LOAD FAILED".into(), Instant::now())),
            Err(_) => self.msg = Some(("NO STATE FILE".into(), Instant::now())),
        }
    }

    fn delete_selected_state(&mut self) {
        if ctx().netplay_linked {
            self.msg = Some(("LINKED: STATES LOCKED".into(), Instant::now()));
            return;
        }
        if self.states.len() < STATE_SLOTS {
            self.refresh_states();
        }
        let idx = self.slot.min(self.states.len().saturating_sub(1));
        let Some(card) = self.states.get(idx) else {
            return;
        };
        let (path, label, exists) = (card.path.clone(), card.label.clone(), card.exists);
        if !exists {
            self.msg = Some((format!("{label} EMPTY"), Instant::now()));
            self.delete_armed = None;
            return;
        }
        let now = Instant::now();
        if let Some((armed, at)) = self.delete_armed {
            if armed == idx && now.duration_since(at) < Duration::from_secs(3) {
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::remove_file(state_thumb_path(&path));
                self.delete_armed = None;
                self.refresh_states();
                self.msg = Some((format!("{label} DELETED"), now));
                return;
            }
        }
        self.delete_armed = Some((idx, now));
        self.msg = Some((format!("PRESS Y AGAIN: DELETE {label}"), now));
    }

    fn scroll_options(&mut self, count: usize) {
        let rows = option_rows();
        if self.opt_sel < self.opt_top {
            self.opt_top = self.opt_sel;
        }
        if self.opt_sel >= self.opt_top + rows {
            self.opt_top = self.opt_sel + 1 - rows;
        }
        self.opt_top = self.opt_top.min(count.saturating_sub(rows));
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

    fn open_netplay(&mut self) {
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
}

#[cfg(test)]
mod state_tests {
    use super::{format_state_age, read_state_card};
    use std::time::{Duration, SystemTime};

    #[test]
    fn ages_read_short() {
        assert_eq!(format_state_age(Duration::from_secs(5)), "JUST NOW");
        assert_eq!(format_state_age(Duration::from_secs(120)), "2M AGO");
        assert_eq!(format_state_age(Duration::from_secs(3 * 3600)), "3H AGO");
        assert_eq!(format_state_age(Duration::from_secs(2 * 86_400)), "2D AGO");
        assert_eq!(
            format_state_age(Duration::from_secs(9 * 7 * 86_400)),
            "9W AGO"
        );
    }

    #[test]
    fn missing_state_is_empty() {
        let dir = std::env::temp_dir().join("magi-state-tests");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("missing.state");
        let _ = std::fs::remove_file(&path);
        let card = read_state_card(&path, "SLOT 1".into(), SystemTime::now());
        assert!(!card.exists);
        assert!(card.thumb.is_none());
        assert!(card.age.is_empty());
    }
}
