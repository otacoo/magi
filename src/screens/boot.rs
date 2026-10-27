//! The MELCHIOR boot sequence: GEHIRN ignites in the centre, pulses hard,
//! fades into the backplate, then the three MAGI units light up and their
//! links to the brain draw in. `ignition.wav` plays underneath.

use std::sync::OnceLock;

use crate::canvas::Canvas;
use crate::font::Fonts;
use crate::image::{self, Image};
use crate::theme::pal;

const ROLES: [&str; 3] = ["MELCHIOR", "BALTHASAR", "CASPER"];

/// Phase boundaries, seconds.
const IGNITE_END: f32 = 1.5;
const LINK_START: f32 = 2.0;
const LINK_STEP: f32 = 0.5;
const NOMINAL: f32 = 4.0;

/// The splash lasts this long before the shelf appears.
pub const BOOT_SECS: f32 = 5.0;

/// The brain art, loaded once from the payload (or the repo during dev).
fn brain() -> Option<&'static Image> {
    static IMG: OnceLock<Option<Image>> = OnceLock::new();
    IMG.get_or_init(|| {
        for p in [
            "System/magi/assets/boot/gehirn.png",
            "assets/boot/gehirn.png",
        ] {
            if let Some(img) = image::load_png(std::path::Path::new(p)) {
                return Some(img);
            }
        }
        None
    })
    .as_ref()
}

/// `t` is seconds since the boot splash start; drives the whole animation.
pub fn draw(cv: &mut Canvas, fonts: &mut Fonts, t: f32) {
    let w = cv.w;
    let h = cv.h;
    cv.clear(pal().bg0);

    // The honeycomb field wakes up with the sequence: a few cells, muted.
    hex_backdrop(cv, t);

    // Frame with corner accents.
    cv.rect_outline(8, 8, (w - 16) as u32, (h - 16) as u32, pal().border);
    let corners = [
        (8, 8, 1, 1),
        (w - 9, 8, -1, 1),
        (8, h - 9, 1, -1),
        (w - 9, h - 9, -1, -1),
    ];
    for (cx, cy, dx, dy) in corners {
        for i in 0..14 {
            cv.put(cx + i * dx, cy, pal().accent);
            cv.put(cx, cy + i * dy, pal().accent);
        }
    }

    // Wordmark.
    let fade = (t / 0.4).min(1.0);
    let tw = fonts.text_width("M A G I", 26.0);
    fonts.draw_alpha(
        cv,
        (w - tw) / 2,
        66,
        "M A G I",
        26.0,
        pal().accent,
        (fade * 235.0) as u8,
    );
    fonts.draw_center(
        cv,
        w / 2,
        84,
        "MULTIPURPOSE ARTIFICIAL GENERAL INTELLIGENCE",
        9.0,
        pal().text_dim,
    );

    // GEHIRN: hard pulse while igniting, then a quiet backplate.
    let center = (w / 2, h / 2 - 26);
    let igniting = t < IGNITE_END;
    if let Some(brain) = brain() {
        let (mul, breathe) = if igniting {
            let fast = 0.5 + 0.5 * (t * 31.4).sin();
            ((120.0 + 130.0 * fast) as u8, fast)
        } else {
            let breathe = 0.5 + 0.5 * crate::theme::pulse01(t, 5.5, 0.2);
            ((34.0 + 14.0 * breathe) as u8, breathe)
        };
        brain.draw(cv, (w - brain.w as i32) / 2, (h - brain.h as i32) / 2, mul);
        // Expanding rings and a sweep line ride the pulse; once lit, the
        // rings settle into a slow ring around the brain.
        if igniting {
            for i in 0..3 {
                let ph = ((t * 1.7) + i as f32 * 0.33) % 1.0;
                let radius = 30.0 + ph * 230.0;
                let a = ((1.0 - ph) * 110.0 * (0.4 + 0.6 * breathe)) as u8;
                ring(cv, center.0, center.1, radius as i32, a);
            }
            let y = ((t * 130.0) as i32) % h;
            cv.hline(
                12,
                y,
                (w - 24) as u32,
                crate::theme::mix(pal().bg0, pal().glow, 0.10),
            );
        } else {
            let ph = (t * 0.22) % 1.0;
            let a = ((1.0 - ph) * 46.0) as u8;
            ring(cv, center.0, center.1, (90.0 + ph * 190.0) as i32, a);
        }
    }

    // The MAGI units; each lights up and links back to the brain.
    let box_w = 168;
    let box_h = 62;
    let gap = 22;
    let total = box_w * ROLES.len() as i32 + gap * (ROLES.len() as i32 - 1);
    let start = (w - total) / 2;
    let y_top = h - box_h - 44;
    for i in 0..ROLES.len() {
        let x = start + i as i32 * (box_w + gap);
        let t_on = LINK_START + i as f32 * LINK_STEP;
        let on = t >= t_on;

        // The link: drawn from the box up to the brain, then a pulse dot.
        if on {
            let p = ((t - t_on) / 0.35).min(1.0);
            let (x1, y1) = (x + box_w / 2, y_top);
            let (x2, y2) = center;
            let ex = x1 + ((x2 - x1) as f32 * p) as i32;
            let ey = y1 + ((y2 - y1) as f32 * p) as i32;
            dashed_line(cv, x1, y1, ex, ey, pal().glow, t);
            if p < 1.0 {
                cv.fill_rect(ex - 2, ey - 2, 5, 5, pal().glow);
            } else {
                let ph = (t * 0.9 + i as f32 * 0.3) % 1.0;
                let a = ((1.0 - ph) * 90.0) as u8;
                ring(cv, x2, y2, (12.0 + ph * 40.0) as i32, a);
            }
        }
    }

    for (i, label) in ROLES.iter().enumerate() {
        let x = start + i as i32 * (box_w + gap);
        let t_on = LINK_START + i as f32 * LINK_STEP;
        let on = t >= t_on;
        let pop = ((t - t_on) / 0.22).clamp(0.0, 1.0);

        let edge = if on {
            crate::theme::mix(pal().border, pal().glow, pop)
        } else {
            pal().border
        };
        cv.fill_rect(x, y_top, box_w as u32, box_h as u32, pal().bg2);
        // Corner notches give the plate a cut-technical feel.
        for (nx, ny) in [
            (x, y_top),
            (x + box_w - 6, y_top),
            (x, y_top + box_h - 6),
            (x + box_w - 6, y_top + box_h - 6),
        ] {
            cv.fill_rect(nx, ny, 6, 6, pal().bg0);
        }
        cv.rect_outline(x, y_top, box_w as u32, box_h as u32, edge);
        cv.fill_rect(x + 1, y_top, (box_w - 2) as u32, 2, edge);
        // Index, label and status lamp.
        fonts.draw(
            cv,
            x + 10,
            y_top + 18,
            &format!("0{}", i + 1),
            9.0,
            pal().text_faint,
        );
        let text = if on { pal().text } else { pal().text_faint };
        fonts.draw(cv, x + 10, y_top + 44, label, 13.0, text);
        let lx = x + box_w - 20;
        let ly = y_top + 40;
        if on {
            cv.fill_rect(
                lx - 2,
                ly - 2,
                12,
                12,
                crate::theme::mix(pal().bg2, pal().glow, 0.35),
            );
            cv.fill_rect(lx, ly, 8, 8, pal().signal);
        } else {
            cv.fill_rect(lx, ly, 8, 8, pal().bg3);
            cv.rect_outline(lx, ly, 8, 8, pal().border);
        }
    }

    // Status line.
    if t >= NOMINAL {
        let a = ((t - NOMINAL) / 0.4).min(1.0);
        let nw = fonts.text_width("ALL SYSTEMS NOMINAL", 12.0);
        fonts.draw_alpha(
            cv,
            (w - nw) / 2,
            108,
            "ALL SYSTEMS NOMINAL",
            12.0,
            pal().signal,
            (a * 255.0) as u8,
        );
    } else {
        let dots = ".".repeat(((t * 3.0) as usize % 4).max(1));
        fonts.draw_center(
            cv,
            w / 2,
            108,
            &format!("INITIALIZING{dots}"),
            12.0,
            pal().text_dim,
        );
    }

    // Footer.
    fonts.draw(
        cv,
        14,
        h - 20,
        &format!("MAGI v{}", env!("CARGO_PKG_VERSION")),
        9.0,
        pal().text_faint,
    );
    if t > NOMINAL {
        let a = (128.0 + 100.0 * (t * 2.4).sin()) as u8;
        let hint = approval();
        let hx = w - 14 - fonts.text_width(hint, 10.0);
        fonts.draw_alpha(cv, hx, h - 20, hint, 10.0, pal().text_dim, a);
    }
}

fn approval() -> &'static str {
    static HINT: OnceLock<String> = OnceLock::new();
    HINT.get_or_init(|| format!("APPROVAL: {} ONLY", crate::device::detect().short_name()))
}

/// A sparse honeycomb backplate: faint lattice, a handful of cells glowing a
/// muted red, the whole field gaining presence over the boot.
fn hex_backdrop(cv: &mut Canvas, t: f32) {
    let cell = 34;
    let dx = cell * 2;
    let dy = (cell as f32 * 1.732) as i32;
    let rows = (cv.h + cell) / dy.max(1) + 1;
    let ramp = (t / BOOT_SECS).clamp(0.0, 1.0);
    let clip = (0, 0, cv.w, cv.h);
    if ramp == 0.0 {
        for row in 0..rows {
            let cy = row * dy;
            let mut cx = if row % 2 == 0 { 0 } else { dx / 2 };
            while cx < cv.w + cell {
                crate::theme::hex_outline(cv, cx, cy, cell, pal().bg2, clip);
                cx += dx;
            }
        }
        return;
    }
    for row in 0..rows {
        let cy = row * dy;
        let mut cx = if row % 2 == 0 { 0 } else { dx / 2 };
        while cx < cv.w + cell {
            crate::theme::hex_outline(cv, cx, cy, cell, pal().bg2, clip);
            let hs = hash(cx, cy);
            // Roughly one cell in twenty takes a turn lighting up.
            if hs.is_multiple_of(21) {
                let phase = (hs % 89) as f32 / 89.0;
                let ph = (t * 0.30 + phase) % 1.0;
                if ph < 0.30 {
                    let s = 1.0 - (ph / 0.30 - 0.5).abs() * 2.0;
                    let a = (0.08 + 0.09 * s) * ramp;
                    crate::theme::hex_fill(
                        cv,
                        cx,
                        cy,
                        cell - 3,
                        crate::theme::mix(pal().bg0, pal().alert, a),
                        clip,
                    );
                }
            }
            cx += dx;
        }
    }
}

fn hash(x: i32, y: i32) -> u32 {
    let mut v = (x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663);
    v ^= v >> 13;
    v.wrapping_mul(1_274_126_177) ^ (v >> 16)
}

/// A thin circle outline (2-degree steps; the boot screen runs once).
fn ring(cv: &mut Canvas, cx: i32, cy: i32, r: i32, alpha: u8) {
    if alpha == 0 || r < 3 {
        return;
    }
    let a = ((alpha as f32 / 255.0) * 255.0) as u8;
    let _ = a;
    let col = crate::theme::mix(pal().bg0, pal().glow, alpha as f32 / 255.0 * 0.8);
    let steps = (r as f32 * 0.5).max(24.0);
    for i in 0..steps as i32 {
        let ang = (i as f32 / steps) * std::f32::consts::TAU;
        let x = cx + (r as f32 * ang.cos()) as i32;
        let y = cy + (r as f32 * ang.sin()) as i32;
        cv.put(x, y, col);
        cv.put(x + 1, y, col);
    }
}

/// A dashed tech line between two points, with a slow marching offset.
fn dashed_line(
    cv: &mut Canvas,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: crate::theme::Color,
    t: f32,
) {
    let dx = (x1 - x0) as f32;
    let dy = (y1 - y0) as f32;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let off = (t * 40.0) as i32 % 10;
    let mut i = -off;
    while i < len as i32 {
        if i >= 0 {
            cv.put(
                x0 + (ux * i as f32) as i32,
                y0 + (uy * i as f32) as i32,
                color,
            );
        }
        i += 10;
    }
}

/// Render the pre-boot logo BaseOS shows before the frontend (the card's
/// `System/bootlogo-640x480.bmp`, drawn by the bootloader).
pub fn render_bootlogo(cv: &mut Canvas, fonts: &mut Fonts) {
    let w = cv.w;
    let h = cv.h;
    cv.clear(pal().bg0);
    hex_backdrop(cv, 0.0);
    if let Some(brain) = brain() {
        brain.draw(cv, (w - brain.w as i32) / 2, (h - brain.h as i32) / 2, 70);
    }
    let tw = fonts.text_width("M A G I", 34.0);
    fonts.draw_alpha(
        cv,
        (w - tw) / 2,
        h / 2 + 4,
        "M A G I",
        34.0,
        pal().accent,
        235,
    );
    fonts.draw_center(
        cv,
        w / 2,
        h / 2 + 24,
        "MULTIPURPOSE ARTIFICIAL GENERAL INTELLIGENCE",
        9.0,
        pal().text_dim,
    );
    cv.rect_outline(8, 8, (w - 16) as u32, (h - 16) as u32, pal().border);
}
