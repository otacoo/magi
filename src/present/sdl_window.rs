//! SDL window presentation — the desktop dev path, and the fallback when
//! fbdev is unavailable. On a headless device this may bind SDL's dummy
//! driver; the log says so loudly.

use anyhow::Result;
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use sdl2::render::{Canvas as SdlCanvas, Texture, TextureCreator};
use sdl2::video::{Window, WindowContext};
use sdl2::Sdl;

use crate::canvas::Canvas;
use crate::device::Device;
use crate::util;

pub struct SdlWindow {
    pub size: (u32, u32),
    canvas: SdlCanvas<Window>,
    texture: Texture<'static>,
    scale: u32,
}

impl SdlWindow {
    pub fn open(sdl: &Sdl, dev: &Device, scale: u32) -> Result<Self> {
        let video = sdl.video().map_err(anyhow::Error::msg)?;
        let driver = video.current_video_driver().to_string();
        util::log(&format!("SDL video driver: '{driver}'"));
        if cfg!(target_os = "linux") && (driver == "dummy" || driver == "offscreen") {
            util::log("headless SDL driver — nothing will appear on the panel");
        }

        let window = video
            .window(
                &format!("MAGI [{}]", dev.name),
                dev.width * scale,
                dev.height * scale,
            )
            .position_centered()
            .build()?;
        let canvas = window.into_canvas().build()?;
        // The window lives for the whole process; leaking the creator keeps
        // the streaming texture alive with it.
        let creator: &'static TextureCreator<WindowContext> =
            Box::leak(Box::new(canvas.texture_creator()));
        let texture =
            creator.create_texture_streaming(PixelFormatEnum::ARGB8888, dev.width, dev.height)?;

        Ok(Self {
            size: (dev.width, dev.height),
            canvas,
            texture,
            scale,
        })
    }

    pub fn present(&mut self, cv: &Canvas) -> Result<()> {
        let bytes =
            unsafe { std::slice::from_raw_parts(cv.px.as_ptr() as *const u8, cv.px.len() * 4) };
        self.texture.update(None, bytes, (cv.w * 4) as usize)?;
        self.canvas.set_draw_color((0, 0, 0, 255));
        self.canvas.clear();
        let dest = Rect::new(0, 0, cv.w as u32 * self.scale, cv.h as u32 * self.scale);
        self.canvas
            .copy(&self.texture, None, Some(dest))
            .map_err(anyhow::Error::msg)?;
        self.canvas.present();
        Ok(())
    }
}
