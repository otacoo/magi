//! Presentation: how a finished canvas reaches the panel.
//!
//! On the H700 the game host prefers EGL/GLES on the Mali GPU, where the
//! core frame is scaled and composited with overlays, then falls back to
//! direct `/dev/fb0`. SDL has no video driver for this panel, so it remains
//! in charge of joystick input and audio. Desktop dev keeps the SDL window.
//! `MAGI_PRESENTER=fbdev|sdl` overrides the choice.

use anyhow::Result;
use sdl2::Sdl;

use crate::canvas::Canvas;
use crate::device::Device;
use crate::util;

pub mod sdl_window;

#[cfg(target_os = "linux")]
pub mod fbdev;

#[cfg(target_os = "linux")]
pub mod gles;

pub enum Presenter {
    #[cfg(target_os = "linux")]
    Fbdev(fbdev::Fbdev),
    Window(sdl_window::SdlWindow),
}

impl Presenter {
    pub fn open(sdl: &Sdl, dev: &Device, scale: u32) -> Result<Self> {
        let requested = std::env::var("MAGI_PRESENTER").unwrap_or_else(|_| {
            if cfg!(target_os = "linux") {
                "fbdev".into()
            } else {
                "sdl".into()
            }
        });

        if requested != "sdl" {
            #[cfg(target_os = "linux")]
            match fbdev::Fbdev::open() {
                Ok(fb) => return Ok(Presenter::Fbdev(fb)),
                Err(e) => util::log(&format!("fbdev unavailable ({e:#}); falling back to SDL")),
            }
            #[cfg(not(target_os = "linux"))]
            util::log("fbdev is Linux-only; using SDL");
        }

        Ok(Presenter::Window(sdl_window::SdlWindow::open(
            sdl, dev, scale,
        )?))
    }

    pub fn canvas_size(&self) -> (u32, u32) {
        match self {
            #[cfg(target_os = "linux")]
            Presenter::Fbdev(fb) => (fb.w, fb.h),
            Presenter::Window(w) => w.size,
        }
    }

    /// Vsync tick without drawing: the game host's frame clock when the
    /// DE layers render the screen (no-op on the SDL window).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn tick(&mut self) {
        #[cfg(target_os = "linux")]
        if let Presenter::Fbdev(fb) = self {
            fb.pan_tick();
        }
    }

    /// Zero the framebuffer memory (both pages): with the layer on pixel
    /// alpha the fb becomes fully transparent.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn wipe(&mut self) {
        #[cfg(target_os = "linux")]
        if let Presenter::Fbdev(fb) = self {
            fb.wipe();
        }
    }

    pub fn present(&mut self, cv: &Canvas) -> Result<()> {
        match self {
            #[cfg(target_os = "linux")]
            Presenter::Fbdev(fb) => fb.present(cv),
            Presenter::Window(w) => w.present(cv),
        }
    }
}
