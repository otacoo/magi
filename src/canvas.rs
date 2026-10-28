//! A plain 0xAARRGGBB frame buffer with the few drawing primitives MAGI needs.
//! Kept free of SDL so everything here is unit- and selftest-verifiable.
//!
//! Two modes share the same buffer: the default one is opaque (the shelf and
//! the software presenter draw straight into it) and the transparent one keeps
//! a real coverage in the alpha byte, so the GPU can composite the canvas
//! over the game frame instead of replacing it.

use crate::theme::Color;

#[derive(Clone)]
pub struct Canvas {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
    /// True when the alpha byte carries coverage: the canvas is a layer to
    /// blend over whatever is behind it.
    pub alpha: bool,
    /// The pixels touched since the last reset: (x0, y0, x1, y1), with x1 <=
    /// x0 meaning "nothing".
    pub dirty: (i32, i32, i32, i32),
}

/// The "nothing touched yet" bounds: an empty rectangle that any draw grows.
const DIRTY_NONE: (i32, i32, i32, i32) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);

impl Canvas {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w: w as i32,
            h: h as i32,
            px: vec![Color::rgb(3, 3, 3).to_u32(); (w * h) as usize],
            alpha: false,
            dirty: DIRTY_NONE,
        }
    }

    /// A fully transparent layer: nothing is covered until something is drawn
    /// on it.
    pub fn transparent(w: u32, h: u32) -> Self {
        Self {
            w: w as i32,
            h: h as i32,
            px: vec![0; (w * h) as usize],
            alpha: true,
            dirty: DIRTY_NONE,
        }
    }

    /// The touched area, empty when nothing was drawn.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn dirty_rect(&self) -> Option<(i32, i32, u32, u32)> {
        let (x0, y0, x1, y1) = self.dirty;
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some((x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
    }

    #[inline]
    fn mark(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let (dx0, dy0, dx1, dy1) = self.dirty;
        self.dirty = (dx0.min(x0), dy0.min(y0), dx1.max(x1), dy1.max(y1));
    }

    /// Drop the previous frame's content, keeping the layer transparent.
    pub fn clear_layer(&mut self) {
        let (x0, y0, x1, y1) = self.dirty;
        if x1 > x0 && y1 > y0 {
            for row in y0..y1.min(self.h) {
                let base = row as usize * self.w as usize;
                let a = base + x0.max(0) as usize;
                let b = base + x1.min(self.w) as usize;
                if b > a {
                    self.px[a..b].fill(0);
                }
            }
        }
        self.dirty = DIRTY_NONE;
    }

    #[inline]
    pub fn idx(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            None
        } else {
            Some(y as usize * self.w as usize + x as usize)
        }
    }

    pub fn clear(&mut self, c: Color) {
        self.px.fill(c.to_u32());
        self.mark(0, 0, self.w, self.h);
    }

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, c: Color) {
        if let Some(i) = self.idx(x, y) {
            self.px[i] = c.to_u32();
            self.mark(x, y, x + 1, y + 1);
        }
    }

    /// Source-over blend of `c` at opacity `a` (0..=255).
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, c: Color, a: u8) {
        if a == 0 {
            return;
        }
        if let Some(i) = self.idx(x, y) {
            if a == 255 {
                self.px[i] = c.to_u32();
            } else if self.alpha {
                // Straight-alpha source-over: the coverage grows, the colour
                // mixes with what was underneath.
                let dst = self.px[i];
                let da = dst >> 24;
                let sa = a as u32;
                let out_a = sa + da * (255 - sa) / 255;
                let keep = da * (255 - sa) / 255;
                let r = (c.r as u32 * sa + ((dst >> 16) & 0xff) * keep) / out_a;
                let g = (c.g as u32 * sa + ((dst >> 8) & 0xff) * keep) / out_a;
                let b = (c.b as u32 * sa + (dst & 0xff) * keep) / out_a;
                self.px[i] = (out_a << 24) | (r << 16) | (g << 8) | b;
            } else {
                let dst = self.px[i];
                let sa = a as u32;
                let da = 255 - sa;
                let r = (c.r as u32 * sa + ((dst >> 16) & 0xff) * da) / 255;
                let g = (c.g as u32 * sa + ((dst >> 8) & 0xff) * da) / 255;
                let b = (c.b as u32 * sa + (dst & 0xff) * da) / 255;
                self.px[i] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
            }
            self.mark(x, y, x + 1, y + 1);
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, c: Color) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w as i32).min(self.w);
        let y1 = (y + h as i32).min(self.h);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let v = c.to_u32();
        for yy in y0..y1 {
            let base = yy as usize * self.w as usize;
            self.px[base + x0 as usize..base + x1 as usize].fill(v);
        }
        self.mark(x0, y0, x1, y1);
    }

    /// A translucent rectangle: same as `fill_rect`, blended at `a`.
    pub fn fill_rect_a(&mut self, x: i32, y: i32, w: u32, h: u32, c: Color, a: u8) {
        if a == 0 {
            return;
        }
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w as i32).min(self.w);
        let y1 = (y + h as i32).min(self.h);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        if self.alpha {
            // A layer keeps the coverage: the colour is the source colour and
            // the alpha says how much of it shows through.
            let v = ((a as u32) << 24) | ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32;
            for yy in y0..y1 {
                let base = yy as usize * self.w as usize;
                for xx in x0..x1 {
                    self.px[base + xx as usize] = v;
                }
            }
            self.mark(x0, y0, x1, y1);
            return;
        }
        let sa = a as u32;
        let da = 255 - sa;
        let (cr, cg, cb) = (c.r as u32, c.g as u32, c.b as u32);
        for yy in y0..y1 {
            let base = yy as usize * self.w as usize;
            for xx in x0..x1 {
                let i = base + xx as usize;
                let dst = self.px[i];
                let r = (cr * sa + ((dst >> 16) & 0xff) * da) / 255;
                let g = (cg * sa + ((dst >> 8) & 0xff) * da) / 255;
                let b = (cb * sa + (dst & 0xff) * da) / 255;
                self.px[i] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
            }
        }
        self.mark(x0, y0, x1, y1);
    }

    pub fn rect_outline(&mut self, x: i32, y: i32, w: u32, h: u32, c: Color) {
        if w == 0 || h == 0 {
            return;
        }
        self.fill_rect(x, y, w, 1, c);
        self.fill_rect(x, y + h as i32 - 1, w, 1, c);
        self.fill_rect(x, y, 1, h, c);
        self.fill_rect(x + w as i32 - 1, y, 1, h, c);
    }

    /// Horizontal 1px line.
    pub fn hline(&mut self, x: i32, y: i32, w: u32, c: Color) {
        self.fill_rect(x, y, w, 1, c);
    }

    /// Vertical 1px line.
    pub fn vline(&mut self, x: i32, y: i32, h: u32, c: Color) {
        self.fill_rect(x, y, 1, h, c);
    }

    /// 24-bit BMP, bottom-up with 4-byte rows: opens anywhere, no encoder dep.
    pub fn save_bmp(&self, path: &std::path::Path) -> std::io::Result<()> {
        let row = (self.w as usize * 3 + 3) & !3;
        let img = row * self.h as usize;
        let mut out: Vec<u8> = Vec::with_capacity(54 + img);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&((54 + img) as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&self.w.to_le_bytes());
        out.extend_from_slice(&self.h.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(img as u32).to_le_bytes());
        out.extend_from_slice(&2835u32.to_le_bytes());
        out.extend_from_slice(&2835u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for y in (0..self.h as usize).rev() {
            for x in 0..self.w as usize {
                let p = self.px[y * self.w as usize + x];
                out.push(p as u8);
                out.push((p >> 8) as u8);
                out.push((p >> 16) as u8);
            }
            out.resize(out.len() + (row - self.w as usize * 3), 0);
        }
        crate::util::write_file_atomic(path, &out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layer_keeps_its_coverage() {
        let mut cv = Canvas::transparent(4, 4);
        cv.fill_rect_a(1, 1, 2, 2, Color::rgb(255, 0, 0), 204);
        let p = cv.px[5];
        assert_eq!(p >> 24, 204, "coverage kept");
        assert_eq!(p >> 16 & 0xff, 255, "source colour kept");
        assert_eq!(cv.px[0], 0, "untouched pixels stay clear");
        // The dirty area covers the drawn rectangle only.
        assert_eq!(cv.dirty_rect(), Some((1, 1, 2, 2)));
    }

    #[test]
    fn bmp_roundtrip() {
        let mut cv = Canvas::new(3, 2);
        cv.clear(Color::rgb(0, 0, 0));
        cv.put(1, 1, Color::rgb(255, 122, 0));
        let path = std::env::temp_dir().join(format!("magi_bmp_test_{}.bmp", std::process::id()));
        cv.save_bmp(&path).unwrap();
        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[0..2], b"BM");
        assert_eq!(data.len(), 54 + 12 * 2, "padded rows");
        // bottom-up: the image's bottom row (y=1) is stored first.
        let off = 57;
        assert_eq!(data[off], 0); // B
        assert_eq!(data[off + 1], 122); // G
        assert_eq!(data[off + 2], 255); // R
        std::fs::remove_file(&path).ok();
    }
}
