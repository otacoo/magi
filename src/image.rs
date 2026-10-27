//! PNG card assets (overlays, boot art): decode to the canvas' 0xAARRGGBB
//! layout, nearest-scale and alpha-aware drawing.

use std::path::Path;

use crate::canvas::Canvas;

#[derive(Clone)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u32>,
}

/// Decode a PNG into 0xAARRGGBB pixels (palette and grayscale expanded by
/// the decoder).
pub fn load_png(path: &Path) -> Option<Image> {
    let data = std::fs::read(path).ok()?;
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::ALPHA | png::Transformations::EXPAND);
    let mut reader = match decoder.read_info() {
        Ok(r) => r,
        Err(e) => {
            crate::util::log(&format!("image: cannot read {}: {e}", path.display()));
            return None;
        }
    };
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = match reader.next_frame(&mut buf) {
        Ok(i) => i,
        Err(e) => {
            crate::util::log(&format!("image: cannot decode {}: {e}", path.display()));
            return None;
        }
    };
    let (w, h) = (info.width, info.height);
    if w == 0 || h == 0 {
        return None;
    }
    let bytes = &buf[..info.buffer_size()];
    let sample = |i: usize| -> u32 {
        let (r, g, b, a) = match info.color_type {
            png::ColorType::Rgba => (
                bytes[i * 4] as u32,
                bytes[i * 4 + 1] as u32,
                bytes[i * 4 + 2] as u32,
                bytes[i * 4 + 3] as u32,
            ),
            png::ColorType::Rgb => (
                bytes[i * 3] as u32,
                bytes[i * 3 + 1] as u32,
                bytes[i * 3 + 2] as u32,
                255,
            ),
            png::ColorType::Grayscale => {
                let g = bytes[i] as u32;
                (g, g, g, 255)
            }
            png::ColorType::GrayscaleAlpha => {
                let g = bytes[i * 2] as u32;
                (g, g, g, bytes[i * 2 + 1] as u32)
            }
            png::ColorType::Indexed => (255, 0, 255, 255),
        };
        (a << 24) | (r << 16) | (g << 8) | b
    };
    let mut px = vec![0u32; (w * h) as usize];
    let (iw, ih) = (w as usize, h as usize);
    for y in 0..ih {
        for x in 0..iw {
            px[y * iw + x] = sample(y * iw + x);
        }
    }
    Some(Image { w, h, px })
}

impl Image {
    /// Nearest-neighbour scale to the given size.
    pub fn scaled(&self, w: u32, h: u32) -> Image {
        let (iw, ih) = (self.w as usize, self.h as usize);
        let mut px = vec![0u32; (w * h) as usize];
        for y in 0..h as usize {
            let sy = (y * ih / h as usize).min(ih - 1);
            for x in 0..w as usize {
                let sx = (x * iw / w as usize).min(iw - 1);
                px[y * w as usize + x] = self.px[sy * iw + sx];
            }
        }
        Image { w, h, px }
    }

    /// Source-over draw at `(x, y)` with every alpha scaled by `mul` (255
    /// keeps the image as authored, lower dims it).
    pub fn draw(&self, cv: &mut Canvas, x: i32, y: i32, mul: u8) {
        if mul == 0 {
            return;
        }
        for yy in 0..self.h as i32 {
            let cy = y + yy;
            if cy < 0 || cy >= cv.h {
                continue;
            }
            for xx in 0..self.w as i32 {
                let cx = x + xx;
                if cx < 0 || cx >= cv.w {
                    continue;
                }
                let src = self.px[yy as usize * self.w as usize + xx as usize];
                let sa = ((src >> 24) * mul as u32) / 255;
                if sa == 0 {
                    continue;
                }
                if sa == 255 {
                    cv.put(
                        cx,
                        cy,
                        crate::theme::Color::rgb(
                            ((src >> 16) & 0xff) as u8,
                            ((src >> 8) & 0xff) as u8,
                            (src & 0xff) as u8,
                        ),
                    );
                    continue;
                }
                let dst = cv.px[cy as usize * cv.w as usize + cx as usize];
                let da = 255 - sa;
                let r = ((src >> 16) & 0xff) * sa / 255 + ((dst >> 16) & 0xff) * da / 255;
                let g = ((src >> 8) & 0xff) * sa / 255 + ((dst >> 8) & 0xff) * da / 255;
                let b = (src & 0xff) * sa / 255 + (dst & 0xff) * da / 255;
                cv.px[cy as usize * cv.w as usize + cx as usize] =
                    0xFF00_0000 | (r << 16) | (g << 8) | b;
            }
        }
    }
}
