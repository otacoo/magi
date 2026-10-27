//! SDL game controllers: the authoritative mapping for Bluetooth and USB
//! pads. SDL ships the community controller database, so Xbox, PlayStation
//! and generic pads map themselves. The stock ANBERNIC pad is invisible to
//! SDL and stays on the evdev path; controllers SDL opens are excluded
//! there so one press is one action.

use sdl2::controller::{Button as PadButton, GameController};
use sdl2::{GameControllerSubsystem, Sdl};

use crate::input;
use crate::util;

pub struct Pads {
    ctx: Option<GameControllerSubsystem>,
    open: Vec<GameController>,
}

impl Pads {
    pub fn new(sdl: &Sdl) -> Self {
        let ctx = match sdl.game_controller() {
            Ok(c) => Some(c),
            Err(e) => {
                util::log(&format!("controllers: {e}"));
                None
            }
        };
        let mut pads = Self {
            ctx,
            open: Vec::new(),
        };
        if let Some(ctx) = &pads.ctx {
            let mut loaded = false;
            for path in [
                "System/magi/controllers/gamecontrollerdb.txt",
                ".config/SDL-GameControllerDB/gamecontrollerdb.txt",
            ] {
                if !std::path::Path::new(path).exists() {
                    continue;
                }
                match ctx.load_mappings(path) {
                    Ok(n) => {
                        util::log(&format!("controllers: {n} mappings from {path}"));
                        loaded = true;
                        break;
                    }
                    Err(e) => util::log(&format!("controllers: {path}: {e:?}")),
                }
            }
            if !loaded {
                util::log("controllers: using SDL's built-in mappings");
            }
        }
        pads.open_connected();
        pads
    }

    fn open_connected(&mut self) {
        let n = match &self.ctx {
            Some(ctx) => ctx.num_joysticks().unwrap_or(0),
            None => return,
        };
        util::log(&format!("SDL game controllers: {n}"));
        for i in 0..n {
            let is = self
                .ctx
                .as_ref()
                .is_some_and(|ctx| ctx.is_game_controller(i));
            if is {
                self.open_index(i);
            }
        }
    }

    fn open_index(&mut self, index: u32) {
        let opened = match &self.ctx {
            Some(ctx) => ctx.open(index),
            None => return,
        };
        match opened {
            Ok(c) => {
                if self.open.iter().any(|o| o.instance_id() == c.instance_id()) {
                    return;
                }
                util::log(&format!(
                    "controller: {} ({:04x}:{:04x})",
                    c.name(),
                    c.vendor_id().unwrap_or(0),
                    c.product_id().unwrap_or(0)
                ));
                self.open.push(c);
                self.sync_owned();
            }
            Err(e) => util::log(&format!("controller {index}: {e}")),
        }
    }

    pub fn added(&mut self, joystick_index: u32) {
        self.open_index(joystick_index);
    }

    pub fn removed(&mut self, instance_id: u32) {
        let before = self.open.len();
        self.open.retain(|c| c.instance_id() != instance_id);
        if self.open.len() != before {
            util::log("controller removed");
            self.sync_owned();
        }
    }

    pub fn button_down(&self, button: PadButton) -> bool {
        self.open.iter().any(|c| c.button(button))
    }

    /// Any open SDL controller exposes a real analog stick.
    pub fn has_stick(&self) -> bool {
        self.open.iter().any(|c| c.mapping().contains("leftx:"))
    }

    /// No SDL controller is open: the panel's own pad arrives on evdev only,
    /// so the loop can block on it instead of polling SDL.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn idle(&self) -> bool {
        self.open.is_empty()
    }

    fn sync_owned(&self) {
        let ids: Vec<(u16, u16)> = self
            .open
            .iter()
            .filter_map(|c| Some((c.vendor_id()?, c.product_id()?)))
            .collect();
        input::set_sdl_owned(ids);
    }
}

impl Drop for Pads {
    fn drop(&mut self) {
        input::set_sdl_owned(Vec::new());
    }
}
