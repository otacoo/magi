//! Glyph atlas over the embedded JetBrains Mono (OFL) subset.

use std::collections::HashMap;

use fontdue::{Font, FontSettings};

use crate::canvas::Canvas;
use crate::theme::Color;

/// Embedded so the device build never depends on card assets for text.
pub static FONT_DATA: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");

struct Glyph {
    w: i32,
    h: i32,
    xmin: i32,
    ymin: i32,
    advance: f32,
    alpha: Vec<u8>,
}

pub struct Fonts {
    font: Font,
    cache: HashMap<(char, u32), Glyph>,
}

impl Fonts {
    pub fn new(data: &[u8]) -> anyhow::Result<Self> {
        let font = Font::from_bytes(data, FontSettings::default())
            .map_err(|e| anyhow::anyhow!("cannot load font: {e}"))?;
        Ok(Self {
            font,
            cache: HashMap::new(),
        })
    }

    fn glyph(&mut self, c: char, px: f32) -> &Glyph {
        // The UI text scale lives in the theme; applying it here means every
        // draw and measurement honours it without call-site changes.
        let px = px * crate::theme::ui_scale();
        let key = (c, (px * 10.0).round() as u32);
        self.cache.entry(key).or_insert_with(|| {
            let (m, alpha) = self.font.rasterize(c, px);
            Glyph {
                w: m.width as i32,
                h: m.height as i32,
                xmin: m.xmin,
                ymin: m.ymin,
                advance: m.advance_width,
                alpha,
            }
        })
    }

    pub fn text_width(&mut self, text: &str, px: f32) -> i32 {
        let mut w = 0.0f32;
        for c in text.chars() {
            w += self.glyph(c, px).advance;
        }
        w.round() as i32
    }

    /// `text`, shortened with an ellipsis when it is wider than `max_w`.
    pub fn elide(&mut self, text: &str, px: f32, max_w: i32) -> String {
        if self.text_width(text, px) <= max_w {
            return text.to_string();
        }
        let mut out = String::new();
        for c in text.chars() {
            let mut candidate = out.clone();
            candidate.push(c);
            candidate.push_str("...");
            if self.text_width(&candidate, px) > max_w {
                break;
            }
            out.push(c);
        }
        out.push_str("...");
        out
    }

    /// Draw at baseline `(x, baseline)`; returns the pen advance.
    pub fn draw(
        &mut self,
        cv: &mut Canvas,
        x: i32,
        baseline: i32,
        text: &str,
        px: f32,
        color: Color,
    ) -> i32 {
        self.draw_alpha(cv, x, baseline, text, px, color, 255)
    }

    /// One-pixel overstrike: a heavier weight without a second font face.
    /// The UI sizes are small, so this reads as bold rather than blurry.
    pub fn draw_bold(
        &mut self,
        cv: &mut Canvas,
        x: i32,
        baseline: i32,
        text: &str,
        px: f32,
        color: Color,
    ) -> i32 {
        let advance = self.draw_alpha(cv, x, baseline, text, px, color, 255);
        self.draw_alpha(cv, x + 1, baseline, text, px, color, 200);
        advance + 1
    }

    /// Like [`draw`](Self::draw), but pixels outside `x0..x1` are dropped.
    /// The shelf's title marquee uses it so a sliding name cannot spill into
    /// the scrollbar or the art pane.
    pub fn draw_clipped(
        &mut self,
        cv: &mut Canvas,
        x: i32,
        baseline: i32,
        text: &str,
        px: f32,
        color: Color,
        x0: i32,
        x1: i32,
    ) -> i32 {
        self.draw_alpha_clipped(cv, x, baseline, text, px, color, 255, x0, x1)
    }

    /// Like [`draw`](Self::draw), but scales every glyph's coverage by
    /// `alpha` — used for pulsing/fading text without extra passes.
    pub fn draw_alpha(
        &mut self,
        cv: &mut Canvas,
        x: i32,
        baseline: i32,
        text: &str,
        px: f32,
        color: Color,
        alpha: u8,
    ) -> i32 {
        self.draw_alpha_clipped(cv, x, baseline, text, px, color, alpha, i32::MIN, i32::MAX)
    }

    fn draw_alpha_clipped(
        &mut self,
        cv: &mut Canvas,
        x: i32,
        baseline: i32,
        text: &str,
        px: f32,
        color: Color,
        alpha: u8,
        x0: i32,
        x1: i32,
    ) -> i32 {
        let mut pen = x as f32;
        for c in text.chars() {
            let g = self.glyph(c, px);
            let gy = baseline - (g.ymin + g.h);
            let gx0 = pen.round() as i32 + g.xmin;
            for row in 0..g.h {
                for col in 0..g.w {
                    let gx = gx0 + col;
                    if gx < x0 || gx >= x1 {
                        continue;
                    }
                    let a = g.alpha[(row * g.w + col) as usize];
                    if a > 8 {
                        let a = ((a as u32 * alpha as u32) / 255) as u8;
                        cv.blend(gx, gy + row, color, a);
                    }
                }
            }
            pen += g.advance;
        }
        pen.round() as i32
    }

    /// Draw horizontally centred around `center_x`; returns the left edge used.
    pub fn draw_center(
        &mut self,
        cv: &mut Canvas,
        center_x: i32,
        baseline: i32,
        text: &str,
        px: f32,
        color: Color,
    ) -> i32 {
        let x = center_x - self.text_width(text, px) / 2;
        self.draw(cv, x, baseline, text, px, color);
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_loads_and_measures() {
        let mut f = Fonts::new(FONT_DATA).unwrap();
        assert!(f.text_width("MAGI", 12.0) > 0);
        assert!(f.text_width("MAGI", 24.0) > f.text_width("MAGI", 12.0));
    }

    #[test]
    fn clipped_text_stays_inside_the_band() {
        let mut f = Fonts::new(FONT_DATA).unwrap();
        let mut cv = Canvas::new(200, 40);
        cv.clear(Color::rgb(0, 0, 0));
        f.draw_clipped(
            &mut cv,
            20,
            28,
            "MELCHIOR",
            16.0,
            Color::rgb(255, 255, 255),
            40,
            120,
        );
        for y in 0..40 {
            for x in 0..200 {
                if cv.px[(y * 200 + x) as usize] != 0xFF00_0000 {
                    assert!((40..120).contains(&x), "lit pixel at {x}");
                }
            }
        }
    }
}
