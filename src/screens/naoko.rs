//! All systems are nominal.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::canvas::Canvas;
use crate::font::Fonts;
use crate::theme::{self, pal, Color};

const SLOT: f32 = 2.4;
const FLASH: f32 = 0.16;
const TOTAL: f32 = SLOT * 3.0;

const GLYPHS: &[&str] = &[
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "A", "B", "C", "D", "E", "F", "G", "H", "I",
    "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z", "@", "#",
    "$", "%", "&", "*", "+", "-", "/", "\\", "<", ">", "[", "]", "{", "}",
];

const BOOT_HEAD: &[&str] = &[
    "CPU MODE High",
    "MEMORY CHECK   640TB + 2048TB   OK",
    "",
    "MAGI HI-SYS RE-BOOT!",
    "OTACOO OS Ver.2.1.1b",
    "COPYRIGHT (C) 2026, 2027 MAGI WORKS",
    "",
    "CO-PROCESSOR              CHECK   256seg   OK",
    "I/O VECTORS               CHECK   OK",
    "CONSOLE DRIVERS           CHECK   OK",
    "ROOTING TABLES            CHECK   OK",
    "STATUS ANALYZER           CHECK   SLAVE GREEN",
    "VIRUS PROTECTION          CHECK   OK",
    "",
    "ADDR  PSP  BLKS   SIZE   OWNER/PARAMETERS",
    "----------------------------------------",
];

const BOOT_NAMES: &[&str] = &[
    "kozaic", "ersdrv", "smalldrv", "<free>", "shimem", "hemm386", "adam8b", "<config>", "share",
    "smalldrv", "<free>", "sys", "net", "vfs", "magi", "gehirn",
];

const TICKER: &[&str] = &[
    "LEVELK039XX9",
    "LeftGauge=170404233463",
    "RightGauge=409404468463",
    "LeftToyBox=48395127454",
    "RightToyBox=513395616445",
    "friction=5",
    "pupprob=6",
    "gkc\\ftshi.mid",
    "gkc\\ktn_low.mid",
    "gkc\\ktn_med.mid",
    "gkc\\ktn_hi.mid",
    "gkc\\bss_mus1.mid",
    "gkc\\gdn_low.mid",
    "gkc\\gdn_med.mid",
    "gkc\\fzp_low.mid",
    "bg=bg1w01a.zlt",
    "WindStart=200",
    "WindStart=300",
    "Name=K.1",
    "Level=45",
    "tsong=3",
    "tsong=4",
    "color=1",
    "Leveltoys=0??????1??????",
    "gauge=art\\wuk.ar",
    "K75n1=0449164",
    "K75n2=1297289",
    "K56n1=1223124",
    "K56n2=048226",
];

pub struct Naoko {
    started: Instant,
    order: [u8; 3],
    seed: u32,
}

impl Naoko {
    /// Rolls the one-in-three chance and returns the animation when it lands.
    pub fn roll() -> Option<Self> {
        Self::from_seed(seed())
    }

    fn from_seed(seed: u32) -> Option<Self> {
        if !seed.is_multiple_of(3) {
            return None;
        }
        let mut order = [1, 2, 3];
        let mut r = seed | 1;
        for i in (1..order.len()).rev() {
            r = r.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let j = (r as usize) % (i + 1);
            order.swap(i, j);
        }
        Some(Self {
            started: Instant::now(),
            order,
            seed,
        })
    }

    pub fn finished(&self) -> bool {
        self.started.elapsed().as_secs_f32() >= TOTAL
    }

    pub fn draw(&self, cv: &mut Canvas, fonts: &mut Fonts) {
        let t = self.started.elapsed().as_secs_f32();
        if t >= TOTAL {
            return;
        }
        let slot = (t / SLOT) as usize;
        let local = t - slot as f32 * SLOT;
        let flash_t = if local < FLASH {
            Some(local)
        } else if t >= TOTAL - FLASH {
            Some(t - (TOTAL - FLASH))
        } else {
            None
        };
        if let Some(flash_t) = flash_t {
            let on = ((flash_t / (FLASH / 4.0)) as u32).is_multiple_of(2);
            cv.clear(if on { pal().text } else { pal().bg0 });
            return;
        }
        cv.clear(pal().bg0);
        match self.order[slot.min(2)] {
            1 => scan(cv, fonts, local, self.seed),
            2 => boot(cv, fonts, local, self.seed),
            _ => ticker(cv, fonts, local, self.seed),
        }
        warnings(cv, fonts, local, self.seed);
    }
}

fn warnings(cv: &mut Canvas, fonts: &mut Fonts, t: f32, seed: u32) {
    let (w, h) = (cv.w, cv.h);
    let frame = (t * 9.0) as u32;
    let count = 2 + (hash(seed ^ 0x0BAD_F00D, frame / 3) % 2) as usize;
    for i in 0..count {
        if i > 0 && hash(seed ^ 0x1234_5678, frame ^ i as u32).is_multiple_of(5) {
            continue;
        }
        let label = match hash(seed ^ 0x9ABC_DEF0, frame ^ i as u32) % 4 {
            0 => "WARNING",
            1 => "WARNING // SIGNAL LOST",
            2 => "WARNING // PATTERN ORANGE",
            _ => "WARNING // SYNC RATIO",
        };
        let tw = fonts.text_width(label, 9.0);
        let avail = (w - tw - 16).max(8) as u32;
        let x = 8 + (hash(seed ^ 0xAAAA_BBBB, frame ^ i as u32) % avail) as i32;
        let y = 16 + (hash(seed ^ 0xCCCC_DDDD, frame ^ i as u32) % (h as u32 - 32).max(8)) as i32;
        let bw = (tw + 12) as u32;
        cv.fill_rect_a(x - 4, y - 13, bw, 18, pal().bg0, 210);
        cv.rect_outline(x - 4, y - 13, bw, 18, pal().alert);
        fonts.draw(cv, x + 2, y, label, 9.0, pal().alert);
    }
}

fn seed() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = nanos ^ (std::process::id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    (x ^ (x >> 31)) as u32
}

fn hash(a: u32, b: u32) -> u32 {
    let mut x = a ^ b.wrapping_mul(0x9E37_79B1) ^ 0x85EB_CA6B;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^ (x >> 16)
}

fn scan(cv: &mut Canvas, fonts: &mut Fonts, t: f32, seed: u32) {
    let (w, h) = (cv.w, cv.h);
    let (cx, cy) = (w / 2, h / 2);
    let frame = (t * 18.0) as u32;
    let max_r = (w as f32).hypot(h as f32) * 0.5 + 30.0;
    cv.clear(pal().bg0);
    for i in 0..12 {
        let r = 30.0 + i as f32 * (max_r - 30.0) / 12.0;
        let a = 0.08 + 0.12 * (0.5 + 0.5 * (t * 0.7 + i as f32 * 0.5).sin());
        hexagon(
            cv,
            cx,
            cy,
            r,
            t * 0.12 + i as f32 * 0.1,
            theme::mix(pal().bg0, pal().accent, a),
        );
    }
    for i in 0..4 {
        let phase = (t * 0.8 + i as f32 / 4.0) % 1.0;
        let r = 18.0 + phase * max_r;
        let a = (1.0 - phase) * 0.85;
        hexagon(
            cv,
            cx,
            cy,
            r,
            t * 0.45,
            theme::mix(pal().bg0, pal().text, a),
        );
    }
    for i in 0..12 {
        let a = t * 0.3 + i as f32 * std::f32::consts::PI / 6.0;
        let x = cx + (a.cos() * max_r) as i32;
        let y = cy + (a.sin() * max_r) as i32;
        line(cv, cx, cy, x, y, theme::mix(pal().bg0, pal().accent, 0.18));
    }
    let a = t * 2.4;
    let x = cx + (a.cos() * max_r) as i32;
    let y = cy + (a.sin() * max_r) as i32;
    line(cv, cx, cy, x, y, pal().text);
    for i in 0..3 {
        let r = 6.0 + i as f32 * 7.0 + (t * 18.0).sin().abs() * 3.0;
        hexagon(
            cv,
            cx,
            cy,
            r,
            -t * 1.1 + i as f32 * 0.2,
            if i == 0 { pal().text } else { pal().accent },
        );
    }
    for i in 0..10u32 {
        let a = t * 0.35 + i as f32 * 0.63;
        let r = 50.0 + (hash(seed ^ i, frame) % (max_r * 0.55) as u32) as f32;
        let x = cx + (a.cos() * r) as i32 - 10;
        let y = cy + (a.sin() * r) as i32;
        let label = format!("{:02X}", hash(seed ^ i.wrapping_mul(7), frame) & 0xFF);
        if x > 2 && x < w - 28 && y > 10 && y < h - 4 {
            fonts.draw(
                cv,
                x,
                y,
                &label,
                8.0,
                if i.is_multiple_of(3) {
                    pal().text
                } else {
                    pal().text_dim
                },
            );
        }
    }
    for i in 0..4u32 {
        let gy = (hash(seed ^ 0xBEEF, frame ^ i) % h as u32) as i32;
        let gw = 40 + hash(seed ^ 0xF00D, frame ^ i) % (w as u32 / 2).max(1);
        cv.fill_rect_a(
            0,
            gy,
            gw,
            1,
            if i.is_multiple_of(2) {
                pal().text
            } else {
                pal().accent
            },
            80,
        );
    }
}

fn hexagon(cv: &mut Canvas, cx: i32, cy: i32, r: f32, rot: f32, color: Color) {
    let mut prev = None;
    for i in 0..=6 {
        let a = rot + i as f32 * std::f32::consts::PI / 3.0;
        let p = (cx + (a.cos() * r) as i32, cy + (a.sin() * r) as i32);
        if let Some((x, y)) = prev {
            line(cv, x, y, p.0, p.1, color);
        }
        prev = Some(p);
    }
}

fn line(cv: &mut Canvas, mut x0: i32, mut y0: i32, x1: i32, y1: i32, color: Color) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        cv.put(x0, y0, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn boot(cv: &mut Canvas, fonts: &mut Fonts, t: f32, seed: u32) {
    let (w, h) = (cv.w, cv.h);
    let line_h = 14;
    let rows = (h / line_h) + 1;
    let frame = (t * 26.0) as u32;
    let scroll = (t * 30.0) as usize;
    let invert = hash(seed ^ 0xB007, frame / 2).is_multiple_of(17);
    let shake = if hash(seed ^ 0x51A4, frame).is_multiple_of(3) {
        (hash(seed, frame) % 11) as i32 - 5
    } else {
        0
    };
    if invert {
        cv.clear(pal().signal);
    } else {
        cv.clear(pal().bg0);
    }
    let base = if invert { pal().bg0 } else { pal().signal };
    let total = BOOT_HEAD.len() + BOOT_NAMES.len();
    for r in 0..rows {
        let y = 12 + r * line_h;
        if y > h {
            break;
        }
        let idx = (scroll + r as usize) % total;
        let (line, color) = if idx < BOOT_HEAD.len() {
            let line = BOOT_HEAD[idx];
            let color = if line.starts_with("MAGI")
                || line.starts_with("CPU")
                || line.starts_with("CO-")
                || line.starts_with("SYSTEM")
            {
                if invert {
                    pal().bg0
                } else {
                    pal().glow
                }
            } else {
                base
            };
            (line.to_string(), color)
        } else {
            let j = idx - BOOT_HEAD.len();
            let a = hash(seed ^ (j as u32).wrapping_mul(17), frame);
            let b = hash(seed ^ (j as u32).wrapping_mul(31), frame ^ 9);
            let name = BOOT_NAMES[(hash(seed, j as u32 ^ frame) as usize) % BOOT_NAMES.len()];
            let line = format!(
                "{:04X}  sys  {:>4}  {:>7}  {}",
                (a & 0xFFFF) as u16,
                1 + (b % 4),
                1024 + (a % 60_000),
                name
            );
            (line, base)
        };
        let jitter = if hash(seed ^ frame, r as u32).is_multiple_of(7) {
            (hash(seed ^ 0x33, r as u32 ^ frame) % 19) as i32 - 9
        } else {
            0
        };
        let x = 4 + shake + jitter;
        if hash(seed ^ 0xABC, r as u32 ^ frame).is_multiple_of(13) {
            cv.fill_rect(
                0,
                y - 10,
                w as u32,
                12,
                if invert { pal().bg0 } else { pal().signal },
            );
            fonts.draw(
                cv,
                x,
                y,
                &line,
                10.0,
                if invert { pal().signal } else { pal().bg0 },
            );
        } else {
            fonts.draw(cv, x, y, &line, 10.0, color);
        }
    }
    for i in 0..8u32 {
        let gy = (hash(seed ^ 0x7777, frame ^ i) % h as u32) as i32;
        let gw = 20 + hash(seed ^ 0x8888, frame ^ i) % (w as u32 / 2).max(1);
        let gx = (hash(seed ^ 0x9999, frame ^ i) % (w as u32).saturating_sub(gw).max(1)) as i32;
        let gh = 1 + (hash(seed ^ 0xAAAA, frame ^ i) % 3);
        cv.fill_rect_a(
            gx,
            gy,
            gw,
            gh,
            if invert { pal().bg0 } else { pal().glow },
            120,
        );
    }
    for i in 0..18u32 {
        let gx = (hash(seed ^ 0xBBBB, frame ^ i) % w as u32) as i32;
        let gy = 12 + (hash(seed ^ 0xCCCC, frame ^ i) % h as u32) as i32;
        let g = GLYPHS[(hash(seed ^ 0xDDDD, frame ^ i) as usize) % GLYPHS.len()];
        fonts.draw(
            cv,
            gx,
            gy.min(h),
            g,
            10.0,
            if invert { pal().bg0 } else { pal().text_faint },
        );
    }
    let blink = ((t * 8.0) as u32).is_multiple_of(2);
    if blink {
        let y = (12 + rows * line_h).min(h - 2);
        fonts.draw(cv, 4 + shake, y, "_", 10.0, pal().glow);
    }
}

fn ticker(cv: &mut Canvas, fonts: &mut Fonts, t: f32, seed: u32) {
    let (w, h) = (cv.w, cv.h);
    let row_h = 18;
    let rows = (h / row_h) + 1;
    let frame = (t * 14.0) as u32;
    let white = Color::rgb(235, 235, 235);
    for r in 0..rows {
        let y = 14 + r * row_h;
        if y > h {
            break;
        }
        let speed = 70.0 + (hash(seed, r as u32) % 160) as f32;
        let mut line = String::with_capacity(300);
        for i in 0..20 {
            let idx = (hash(seed ^ (r as u32).wrapping_mul(131), i as u32 ^ frame) as usize)
                % TICKER.len();
            line.push_str(TICKER[idx]);
            if i % 3 == 2 {
                line.push(' ');
            }
        }
        let width = fonts.text_width(&line, 11.0).max(1);
        let phase = (hash(seed, r as u32) % width as u32) as i32;
        let off = ((t * speed) as i32 + phase).rem_euclid(width);
        let color = if hash(seed ^ 0x7777, frame ^ r as u32).is_multiple_of(10) {
            pal().text_dim
        } else {
            white
        };
        fonts.draw_clipped(cv, -off, y, &line, 11.0, color, 0, w);
        if width < w + off {
            fonts.draw_clipped(cv, -off + width, y, &line, 11.0, color, 0, w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn only_one_seed_in_three_opens_naoko() {
        assert!(Naoko::from_seed(3).is_some());
        assert!(Naoko::from_seed(4).is_none());
        let naoko = Naoko::from_seed(6).unwrap();
        let mut order = naoko.order;
        order.sort_unstable();
        assert_eq!(order, [1, 2, 3]);
    }

    #[test]
    fn every_scene_draws() {
        let mut cv = Canvas::new(64, 48);
        let mut fonts = Fonts::new(crate::font::FONT_DATA).unwrap();
        for anim in 1u8..=3 {
            let mut naoko = Naoko::from_seed(anim as u32 * 3).unwrap();
            naoko.order[0] = anim;
            naoko.started = Instant::now() - Duration::from_secs_f32(FLASH + 0.5);
            naoko.draw(&mut cv, &mut fonts);
        }
    }
}
