//! The MELCHIOR boot sequence: GEHIRN ignites in the centre, pulses hard,
//! fades into the backplate, then the three MAGI units light up and their
//! links to the brain draw in. `ignition.wav` plays underneath.

use std::sync::OnceLock;

use crate::canvas::Canvas;
use crate::font::Fonts;
use crate::image::{self, Image};
use crate::theme::pal;

const ROLE_NAMES: [(&str, &str); 3] = [
    ("MELCHIOR-1", "SHELF"),
    ("BALTHASAR-2", "HOST"),
    ("CASPER-3", "PORTS"),
];

/// Phase boundaries, seconds. The three link times sit on the ignition
/// track's strongest beats; the sync times fill the gap before nominal.
const IGNITE_END: f32 = 1.5;
const LINK_TIMES: [f32; 3] = [2.15, 2.40, 2.65];
const SYNC_TIMES: [f32; 3] = [2.95, 3.20, 3.50];
const NOMINAL: f32 = 4.0;

/// The splash lasts this long before the shelf appears.
pub const BOOT_SECS: f32 = 5.0;

/// The brain occupies a fixed block between the status line and the units.
const BRAIN_TOP: i32 = 116;
const BRAIN_H: u32 = 244;
const BRAIN_CENTER_Y: i32 = 238;

/// The static bootloader splash uses a larger version of the same brain.
const SPLASH_H: u32 = 300;

const BOX_W: i32 = 168;
const BOX_H: i32 = 62;
const BOX_GAP: i32 = 22;

/// Rings are clipped below the top HUD so they never wash over the wordmark.
const RING_TOP: i32 = 112;

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

/// The animated boot brain, cropped to its alpha bounds and graded once.
fn boot_brain() -> Option<&'static Image> {
    static IMG: OnceLock<Option<Image>> = OnceLock::new();
    IMG.get_or_init(|| build_graded_brain(BRAIN_H)).as_ref()
}

/// The static bootloader splash brain, a larger version of the same art.
fn splash_brain() -> Option<&'static Image> {
    static IMG: OnceLock<Option<Image>> = OnceLock::new();
    IMG.get_or_init(|| build_graded_brain(SPLASH_H)).as_ref()
}

fn build_graded_brain(target_h: u32) -> Option<Image> {
    let src = brain()?;
    let (x0, y0, x1, y1) = alpha_bounds(src)?;
    let (cw, ch) = (x1 - x0, y1 - y0);
    if cw == 0 || ch == 0 {
        return None;
    }
    let target_w = (cw as f32 * target_h as f32 / ch as f32).round().max(1.0) as u32;
    let mut px = vec![0u32; (target_w * target_h) as usize];
    for y in 0..target_h {
        let sy = y0 + y * ch / target_h;
        for x in 0..target_w {
            let sx = x0 + x * cw / target_w;
            let p = src.px[(sy * src.w + sx) as usize];
            px[(y * target_w + x) as usize] = grade_pixel(p);
        }
    }
    Some(Image {
        w: target_w,
        h: target_h,
        px,
    })
}

fn alpha_bounds(img: &Image) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (img.w, img.h, 0, 0);
    for y in 0..img.h {
        for x in 0..img.w {
            if img.px[(y * img.w + x) as usize] >> 24 > 8 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

/// Pull the anatomical colours toward the NERV palette without flattening
/// the tissue into a single orange.
fn grade_pixel(p: u32) -> u32 {
    let a = p >> 24;
    if a == 0 {
        return 0;
    }
    let r = ((p >> 16) & 0xff) as f32;
    let g = ((p >> 8) & 0xff) as f32;
    let b = (p & 0xff) as f32;
    let lum = 0.299 * r + 0.587 * g + 0.114 * b;
    let mut r = (0.82 * r + 0.18 * lum) * 1.04;
    let mut g = (0.82 * g + 0.18 * lum) * 0.95;
    let mut b = (0.82 * b + 0.18 * lum) * 0.72;
    let hi = (lum / 255.0).powi(2);
    r += (255.0 - r) * 0.08 * hi;
    g += (122.0 - g) * 0.08 * hi;
    b -= b * 0.12 * hi;
    a << 24 | (r as u32).min(255) << 16 | (g as u32).min(255) << 8 | (b as u32).min(255)
}

/// `t` is seconds since the boot splash start; drives the whole animation.
pub fn draw(cv: &mut Canvas, fonts: &mut Fonts, t: f32) {
    let w = cv.w;
    let h = cv.h;
    cv.clear(pal().bg0);

    hex_backdrop(cv, t);

    // Frame, corner accents and the boot progress along the bottom edge.
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
        60,
        "M A G I",
        26.0,
        pal().accent,
        (fade * 235.0) as u8,
    );
    fonts.draw_center(
        cv,
        w / 2,
        78,
        "MULTIPURPOSE ARTIFICIAL GENERAL INTELLIGENCE",
        9.0,
        pal().text_dim,
    );
    let line_w = (120.0 * (t / 0.6).min(1.0)) as u32;
    if line_w > 2 {
        cv.hline(
            w / 2 - line_w as i32 / 2,
            90,
            line_w,
            crate::theme::mix(pal().bg0, pal().border, 0.8),
        );
    }

    // GEHIRN: a top-down reveal on two heartbeat beats, then a slow ring.
    let center = (w / 2, BRAIN_CENTER_Y);
    let beat = heartbeat(t);
    let breathe = 0.5 + 0.5 * crate::theme::pulse01(t, 5.5, 0.2);
    if let Some(brain) = boot_brain() {
        let bx = (w - brain.w as i32) / 2;
        let brain_bottom = BRAIN_TOP + brain.h as i32;
        let scan = (t / IGNITE_END).clamp(0.0, 1.0);
        let scan_y = BRAIN_TOP + ((h - BRAIN_TOP) as f32 * scan) as i32;
        let reveal_y = scan_y.min(brain_bottom);
        let scan_fade = if scan_y <= brain_bottom {
            1.0
        } else {
            (1.0 - (scan_y - brain_bottom) as f32 / (h - brain_bottom) as f32 * 0.85)
                .clamp(0.0, 1.0)
        };
        let mul = if t < IGNITE_END {
            (70.0 + 185.0 * beat.max(0.15)) as u8
        } else {
            (80.0 + 32.0 * breathe) as u8
        };
        if t < IGNITE_END {
            // A faint ghost of the whole brain, then the bright revealed
            // part and a scanline: the panel is being scanned in, not cut.
            brain.draw(cv, bx, BRAIN_TOP, (mul as f32 * 0.18) as u8);
            brain.draw_clipped(cv, bx, BRAIN_TOP, mul, reveal_y);
            let scan_col = if scan_y <= brain_bottom {
                pal().glow
            } else {
                crate::theme::mix(pal().bg0, pal().glow, 0.85 * scan_fade)
            };
            cv.hline(bx, scan_y, brain.w, scan_col);
            let below = crate::theme::mix(pal().bg0, pal().glow, 0.45 * scan_fade);
            let above = crate::theme::mix(pal().bg0, pal().glow, 0.30 * scan_fade);
            cv.hline(bx, scan_y + 1, brain.w, below);
            cv.hline(bx, scan_y.saturating_sub(1), brain.w, above);
            cv.hline(
                10,
                scan_y,
                (w - 20) as u32,
                crate::theme::mix(pal().bg0, pal().glow, 0.10 * scan_fade),
            );
        } else {
            brain.draw(cv, bx, BRAIN_TOP, mul);
        }

        for start in [0.9, 1.4] {
            let dt = t - start;
            if (0.0..0.60).contains(&dt) {
                let a = ((1.0 - dt / 0.60) * 120.0) as u8;
                hex_ring(
                    cv,
                    center.0,
                    center.1,
                    (36.0 + dt * 420.0) as i32,
                    pal().glow,
                    a,
                );
                if dt > 0.10 {
                    let a2 = (a as f32 * 0.55) as u8;
                    hex_ring(
                        cv,
                        center.0,
                        center.1,
                        (30.0 + (dt - 0.10) * 420.0) as i32,
                        pal().glow,
                        a2,
                    );
                }
            }
        }
        let lock = t - 1.9;
        if (0.0..0.55).contains(&lock) {
            hex_ring(
                cv,
                center.0,
                center.1,
                (44.0 + lock * 380.0) as i32,
                pal().accent,
                ((1.0 - lock / 0.55) * 110.0) as u8,
            );
        }
        if t >= IGNITE_END {
            let ph = (t * 0.22) % 1.0;
            hex_ring(
                cv,
                center.0,
                center.1,
                (118.0 + ph * 90.0) as i32,
                pal().glow,
                ((1.0 - ph) * 44.0) as u8,
            );
        }
        let final_ring = t - NOMINAL;
        if (0.0..0.80).contains(&final_ring) {
            hex_ring(
                cv,
                center.0,
                center.1,
                (50.0 + final_ring * 380.0) as i32,
                pal().signal,
                ((1.0 - final_ring / 0.80) * 140.0) as u8,
            );
        }
    }

    // Links: draw in, then send the handshake reply back to the unit.
    let y_top = h - BOX_H - 44;
    for i in 0..ROLE_NAMES.len() {
        let box_x = box_left(w) + i as i32 * (BOX_W + BOX_GAP);
        let (x1, y1) = (box_x + BOX_W / 2, y_top);
        let link_t = LINK_TIMES[i];
        if t < link_t {
            continue;
        }
        let p = ((t - link_t) / 0.28).clamp(0.0, 1.0);
        let (x2, y2) = center;
        let ex = x1 + ((x2 - x1) as f32 * p) as i32;
        let ey = y1 + ((y2 - y1) as f32 * p) as i32;
        let color = if t < SYNC_TIMES[i] {
            pal().glow
        } else {
            crate::theme::mix(pal().border, pal().glow, 0.50)
        };
        dashed_line(cv, x1, y1, ex, ey, color, t);
        if p < 1.0 {
            cv.fill_rect(ex - 3, ey - 3, 7, 7, pal().accent);
        }
        let q = (t - SYNC_TIMES[i]) / 0.24;
        if (0.0..=1.0).contains(&q) {
            let qx = x2 + ((x1 - x2) as f32 * q) as i32;
            let qy = y2 + ((y1 - y2) as f32 * q) as i32;
            cv.fill_rect(qx - 2, qy - 2, 5, 5, pal().signal);
        }
    }

    // The MAGI units.
    for (i, (name, role)) in ROLE_NAMES.iter().enumerate() {
        let x = box_left(w) + i as i32 * (BOX_W + BOX_GAP);
        let t_on = LINK_TIMES[i];
        let t_sync = SYNC_TIMES[i];
        let on = t >= t_on;
        let synced = t >= t_sync;
        let pop = ((t - t_on) / 0.22).clamp(0.0, 1.0);
        let edge = if synced {
            pal().glow
        } else if on {
            crate::theme::mix(pal().border, pal().glow, pop)
        } else {
            pal().border
        };
        cv.fill_rect(x, y_top, BOX_W as u32, BOX_H as u32, pal().bg2);
        for (nx, ny) in [
            (x, y_top),
            (x + BOX_W - 6, y_top),
            (x, y_top + BOX_H - 6),
            (x + BOX_W - 6, y_top + BOX_H - 6),
        ] {
            cv.fill_rect(nx, ny, 6, 6, pal().bg0);
        }
        cv.rect_outline(x, y_top, BOX_W as u32, BOX_H as u32, edge);
        cv.fill_rect(x + 1, y_top, (BOX_W - 2) as u32, 2, edge);

        fonts.draw(
            cv,
            x + 10,
            y_top + 16,
            &format!("0{}", i + 1),
            8.0,
            pal().text_faint,
        );
        fonts.draw(cv, x + 34, y_top + 16, role, 8.0, pal().text_dim);
        let text = if on { pal().text } else { pal().text_faint };
        fonts.draw(cv, x + 10, y_top + 40, name, 13.0, text);

        let (state, state_col) = if t < t_on {
            ("OFFLINE", pal().text_faint)
        } else if t < t_sync {
            ("LINK", pal().accent)
        } else if t < t_sync + 0.20 {
            ("SYNC", pal().glow)
        } else {
            ("ONLINE", pal().signal)
        };
        fonts.draw(cv, x + 10, y_top + 55, state, 8.0, state_col);

        let lx = x + BOX_W - 20;
        let ly = y_top + 32;
        let lamp = if synced {
            pal().signal
        } else if on && (t * 24.0) as i32 % 3 != 0 {
            pal().accent
        } else {
            pal().bg3
        };
        cv.fill_rect(lx, ly, 8, 8, lamp);
        if !synced {
            cv.rect_outline(lx, ly, 8, 8, pal().border);
        }

        let fill = if t < t_on {
            0.0
        } else if t < t_sync {
            (t - t_on) / (t_sync - t_on)
        } else {
            1.0
        };
        let fw = ((BOX_W - 2) as f32 * fill) as u32;
        if fw > 0 {
            cv.fill_rect(
                x + 1,
                y_top + BOX_H - 3,
                fw,
                2,
                if synced { pal().signal } else { pal().accent },
            );
        }
    }

    // Status line, on the beats.
    let (status, status_col) = if t < IGNITE_END {
        (
            format!(
                "INITIALIZING{}",
                ".".repeat(((t * 3.0) as usize % 4).max(1))
            ),
            pal().text_dim,
        )
    } else if t < LINK_TIMES[0] {
        ("GEHIRN ONLINE".into(), pal().glow)
    } else if t < LINK_TIMES[1] {
        ("LINKING MELCHIOR-1".into(), pal().accent)
    } else if t < LINK_TIMES[2] {
        ("LINKING BALTHASAR-2".into(), pal().accent)
    } else if t < SYNC_TIMES[0] {
        ("LINKING CASPER-3".into(), pal().accent)
    } else if t < NOMINAL {
        ("SYNCING LINKS".into(), pal().glow)
    } else {
        ("ALL SYSTEMS NOMINAL".into(), pal().signal)
    };
    fonts.draw_center(cv, w / 2, 108, &status, 12.0, status_col);

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

    // Hand the panel back to the shelf on a clean fade.
    let progress = (t / BOOT_SECS).clamp(0.0, 1.0);
    let pw = ((w - 16) as f32 * progress) as u32;
    if pw > 0 {
        cv.hline(8, h - 9, pw, pal().accent);
    }
    let fade_out = ((t - (BOOT_SECS - 0.25)) / 0.25).clamp(0.0, 1.0);
    if fade_out > 0.0 {
        cv.fill_rect_a(
            0,
            0,
            w as u32,
            h as u32,
            pal().bg0,
            (fade_out * 255.0) as u8,
        );
    }
}

fn heartbeat(t: f32) -> f32 {
    let beat = |center: f32, width: f32| {
        let d = (t - center) / width;
        if d.abs() < 1.0 {
            (1.0 - d * d).max(0.0)
        } else {
            0.0
        }
    };
    (beat(0.9, 0.20) + 0.85 * beat(1.4, 0.24)).min(1.0)
}

fn box_left(w: i32) -> i32 {
    (w - (BOX_W * 3 + BOX_GAP * 2)) / 2
}

/// A hexagonal ring, clipped below the top HUD.
fn hex_ring(cv: &mut Canvas, cx: i32, cy: i32, r: i32, color: crate::theme::Color, alpha: u8) {
    if alpha == 0 || r < 6 {
        return;
    }
    let clip = (0, RING_TOP, cv.w, cv.h);
    let col = crate::theme::mix(pal().bg0, color, alpha as f32 / 255.0 * 0.85);
    crate::theme::hex_outline(cv, cx, cy, r, col, clip);
    if r > 70 {
        crate::theme::hex_outline(cv, cx, cy, r - 1, col, clip);
    }
}

fn approval() -> &'static str {
    static HINT: OnceLock<String> = OnceLock::new();
    HINT.get_or_init(|| format!("APPROVAL: {} ONLY", crate::device::detect().short_name()))
}

/// A sparse honeycomb backplate: faint lattice, a handful of cells glowing a
/// muted red, the whole field gaining presence over the boot.
fn hash(x: i32, y: i32) -> u32 {
    let mut v = (x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663);
    v ^= v >> 13;
    v.wrapping_mul(1_274_126_177) ^ (v >> 16)
}

fn hex_backdrop(cv: &mut Canvas, t: f32) {
    let cell = 34;
    let dx = cell * 2;
    let dy = (cell as f32 * 1.732) as i32;
    let rows = (cv.h + cell) / dy.max(1) + 1;
    let ramp = (t / BOOT_SECS).clamp(0.0, 1.0);
    let clip = (0, 0, cv.w, cv.h);
    let (cx0, cy0) = (cv.w as f32 / 2.0, BRAIN_CENTER_Y as f32);
    let beat = heartbeat(t);
    let breathe = 0.5 + 0.5 * crate::theme::pulse01(t, 5.5, 0.2);
    let pulse = 0.55 + 0.45 * beat.max(breathe);
    for row in 0..rows {
        let cy = row * dy;
        let mut cx = if row % 2 == 0 { 0 } else { dx / 2 };
        while cx < cv.w + cell {
            crate::theme::hex_outline(cv, cx, cy, cell, pal().bg2, clip);
            if ramp > 0.0 {
                let d = ((cx as f32 - cx0).powi(2) + (cy as f32 - cy0).powi(2))
                    .sqrt()
                    .max(1.0);
                let wave = (0.5 + 0.5 * ((d * 0.05) - t * 3.0).sin()).powi(10);
                let near = (1.0 - d / 340.0).clamp(0.0, 1.0);
                if wave > 0.55 && hash(cx, cy).is_multiple_of(4) {
                    let a = (0.04 + 0.12 * wave * near) * ramp * pulse;
                    if a > 0.012 {
                        crate::theme::hex_fill(
                            cv,
                            cx,
                            cy,
                            cell - 5,
                            crate::theme::mix(pal().bg0, pal().alert, a),
                            clip,
                        );
                    }
                }
            }
            cx += dx;
        }
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
    if let Some(brain) = splash_brain() {
        brain.draw(cv, (w - brain.w as i32) / 2, (h - brain.h as i32) / 2, 80);
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
