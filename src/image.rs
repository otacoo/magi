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

/// A sane ceiling for card images (64 MB at 4 bytes per pixel).
const MAX_IMAGE_PIXELS: usize = 16 << 20;

/// Load a card image: PNG, or the uncompressed 24-bit BMP the screenshot
/// writer produces.
pub fn load_image(path: &Path) -> Option<Image> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("bmp") => load_bmp(path),
        _ => load_png(path),
    }
}

/// Decode an uncompressed 24-bit BMP (bottom-up or top-down).
pub fn load_bmp(path: &Path) -> Option<Image> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 54 || &data[..2] != b"BM" {
        return None;
    }
    let le16 = |at: usize| -> Option<u16> {
        Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
    };
    let le32 = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
    };
    let offset = le32(10)? as usize;
    let width = le32(18)? as i32;
    let height = le32(22)? as i32;
    let bpp = le16(28)?;
    let compression = le32(30)?;
    if width <= 0 || height == 0 || bpp != 24 || compression != 0 {
        return None;
    }
    let (w, h, top_down) = (width as u32, height.unsigned_abs(), height < 0);
    let pixels = (w as usize).checked_mul(h as usize)?;
    if pixels > MAX_IMAGE_PIXELS {
        return None;
    }
    let stride = ((w as usize * 3 + 3) & !3).max(1);
    if offset.checked_add(stride.checked_mul(h as usize)?)? > data.len() {
        return None;
    }
    let mut px = vec![0u32; pixels];
    for y in 0..h {
        let row = if top_down { y } else { h - 1 - y } as usize;
        let base = offset + row * stride;
        for x in 0..w {
            let at = base + x as usize * 3;
            let b = data[at] as u32;
            let g = data[at + 1] as u32;
            let r = data[at + 2] as u32;
            px[(y * w + x) as usize] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
        }
    }
    Some(Image { w, h, px })
}

/// Encode 0xAARRGGBB pixels as RGBA PNG bytes.
pub fn encode_png(w: u32, h: u32, px: &[u32]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let mut data = Vec::with_capacity(px.len() * 4);
        for &p in px {
            data.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]);
        }
        writer
            .write_image_data(&data)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }
    Ok(out)
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
    let (iw, ih) = (reader.info().width as usize, reader.info().height as usize);
    if iw.checked_mul(ih)? > MAX_IMAGE_PIXELS {
        crate::util::log(&format!("image: too large: {}", path.display()));
        return None;
    }
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
        self.draw_clipped(cv, x, y, mul, i32::MAX);
    }

    /// `draw`, but rows at or below `clip_bottom` are skipped. The boot
    /// splash uses this for its top-to-bottom reveal.
    pub fn draw_clipped(&self, cv: &mut Canvas, x: i32, y: i32, mul: u8, clip_bottom: i32) {
        if mul == 0 {
            return;
        }
        for yy in 0..self.h as i32 {
            let cy = y + yy;
            if cy >= clip_bottom {
                break;
            }
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

    /// Nearest-neighbour scale to fit inside `(x, y, w, h)`, preserving the
    /// aspect ratio and centring the result.
    pub fn draw_fit(&self, cv: &mut Canvas, x: i32, y: i32, w: u32, h: u32, mul: u8) {
        if w == 0 || h == 0 || self.w == 0 || self.h == 0 || mul == 0 {
            return;
        }
        let scale = (w as f32 / self.w as f32).min(h as f32 / self.h as f32);
        let dw = ((self.w as f32 * scale).round() as u32).clamp(1, w);
        let dh = ((self.h as f32 * scale).round() as u32).clamp(1, h);
        let ox = x + ((w - dw) / 2) as i32;
        let oy = y + ((h - dh) / 2) as i32;
        for yy in 0..dh {
            let sy = (yy as u64 * self.h as u64 / dh as u64) as usize;
            let cy = oy + yy as i32;
            if cy < 0 || cy >= cv.h {
                continue;
            }
            for xx in 0..dw {
                let sx = (xx as u64 * self.w as u64 / dw as u64) as usize;
                let cx = ox + xx as i32;
                if cx < 0 || cx >= cv.w {
                    continue;
                }
                let src = self.px[sy * self.w as usize + sx];
                let sa = ((src >> 24) * mul as u32) / 255;
                if sa == 0 {
                    continue;
                }
                let dst = cv.px[cy as usize * cv.w as usize + cx as usize];
                if sa == 255 {
                    cv.px[cy as usize * cv.w as usize + cx as usize] = src;
                    continue;
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Color;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("magi-image-tests");
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    #[test]
    fn bmp_roundtrip_matches_pixels() {
        let mut cv = Canvas::new(4, 3);
        cv.put(0, 0, Color::rgb(255, 0, 0));
        cv.put(3, 2, Color::rgb(0, 255, 0));
        let path = temp("shot.bmp");
        cv.save_bmp(&path).unwrap();
        let img = load_image(&path).expect("bmp");
        assert_eq!((img.w, img.h), (4, 3));
        assert_eq!(img.px[0], 0xFFFF_0000);
        assert_eq!(img.px[2 * 4 + 3], 0xFF00_FF00);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn png_roundtrip_matches_pixels() {
        let px = vec![0xFFFF_0000, 0xFF00_FF00, 0xFF00_00FF, 0x80FF_FFFF];
        let bytes = encode_png(2, 2, &px).unwrap();
        let path = temp("shot.png");
        std::fs::write(&path, bytes).unwrap();
        let img = load_image(&path).expect("png");
        assert_eq!((img.w, img.h), (2, 2));
        assert_eq!(&img.px[..3], &px[..3]);
        assert_eq!(img.px[3] >> 24, 0x80);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn draw_fit_preserves_aspect() {
        let img = Image {
            w: 2,
            h: 1,
            px: vec![0xFFFF_0000, 0xFF00_FF00],
        };
        let mut cv = Canvas::new(8, 8);
        img.draw_fit(&mut cv, 0, 0, 8, 4, 255);
        assert_eq!(cv.px[0], 0xFFFF_0000);
        assert_eq!(cv.px[7], 0xFF00_FF00);
    }
}
