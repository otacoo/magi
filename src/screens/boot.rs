//! The MELCHIOR boot sequence: GEHIRN ignites in the centre, pulses hard,
//! fades into the backplate, then the three MAGI units light up and their
//! links to the core draw in. `ignition.wav` plays underneath.

use std::sync::OnceLock;

use crate::canvas::Canvas;
use crate::font::Fonts;
use crate::theme::pal;

mod gehirn;

const ROLE_NAMES: [&str; 3] = ["MELCHIOR-1", "BALTHASAR-2", "CASPER-3"];

/// Phase boundaries in seconds; links sit on ignition beats.
const IGNITE_END: f32 = 1.5;
const LINK_TIMES: [f32; 3] = [2.15, 2.40, 2.65];
const SYNC_TIMES: [f32; 3] = [2.95, 3.20, 3.50];
const NOMINAL: f32 = 4.0;

/// The splash lasts this long before the shelf appears.
pub const BOOT_SECS: f32 = 5.0;

/// The globe occupies a fixed block between the status line and the units.
const GLOBE_TOP: i32 = 116;
const GLOBE_CENTER_Y: i32 = 238;
const GLOBE_R: f32 = 98.0;

/// Boot art is fixed and must not follow the interface text setting.
fn art_px(px: f32) -> f32 {
    px / crate::theme::ui_scale()
}

/// Boot body text is pinned to the M step; the wordmark and status stay at S.
const MID_TEXT_SCALE: f32 = crate::screens::settings::TEXT_SCALES[1];

fn mid_px(px: f32) -> f32 {
    px * MID_TEXT_SCALE / crate::theme::ui_scale()
}

const BOX_W: i32 = 168;
const BOX_H: i32 = 62;
const BOX_GAP: i32 = 22;

/// Rings are clipped below the top HUD so they never wash over the wordmark.
const RING_TOP: i32 = 112;

/// `t` is seconds since the boot splash start; drives the whole animation.
pub fn draw(cv: &mut Canvas, fonts: &mut Fonts, t: f32) {
    let w = cv.w;
    let h = cv.h;
    cv.clear(pal().bg0);

    hex_backdrop(cv, t);

    // Frame.
    cv.rect_outline(8, 8, (w - 16) as u32, (h - 16) as u32, pal().border);

    let fade = (t / 0.4).min(1.0);
    let title_px = art_px(42.0);
    let sub_px = art_px(8.4);
    let tw = fonts.text_width("M A G I", title_px);
    let tx = (w - tw) / 2;
    fonts.draw_alpha(
        cv,
        tx + 2,
        62,
        "M A G I",
        title_px,
        pal().bg0,
        (fade * 200.0) as u8,
    );
    fonts.draw_alpha(
        cv,
        tx,
        60,
        "M A G I",
        title_px,
        pal().accent,
        (fade * 235.0) as u8,
    );
    fonts.draw_center(
        cv,
        w / 2,
        76,
        "MULTI ARCHITECTURE GAMING INTERFACE",
        sub_px,
        pal().text_dim,
    );
    let line_w = (fonts.text_width("MULTI ARCHITECTURE GAMING INTERFACE", sub_px) as f32
        * (t / 0.6).min(1.0)) as u32;
    if line_w > 2 {
        cv.hline(
            w / 2 - line_w as i32 / 2,
            88,
            line_w,
            crate::theme::mix(pal().bg0, pal().border, 0.8),
        );
    }

    // GEHIRN: a top-down scan reveal on the heartbeat, then a slow spin.
    let center = (w / 2, GLOBE_CENTER_Y);
    let beat = heartbeat(t);
    let breathe = 0.5 + 0.5 * crate::theme::pulse01(t, 5.5, 0.2);
    let globe_bottom = GLOBE_CENTER_Y + GLOBE_R as i32;
    let scan = (t / IGNITE_END).clamp(0.0, 1.0);
    let scan_y = GLOBE_TOP + ((h - GLOBE_TOP) as f32 * scan) as i32;
    let scan_fade = if scan_y <= globe_bottom {
        1.0
    } else {
        (1.0 - (scan_y - globe_bottom) as f32 / (h - globe_bottom) as f32 * 0.85).clamp(0.0, 1.0)
    };
    if t < IGNITE_END {
        // Ghost the whole globe, then reveal it under a scanline.
        let ghost = 0.18 * (0.3 + 0.7 * beat);
        let bright = 0.55 + 0.45 * beat;
        gehirn::draw(
            cv,
            center.0 as f32,
            center.1 as f32,
            GLOBE_R,
            t,
            ghost,
            RING_TOP,
            i32::MAX,
        );
        gehirn::draw(
            cv,
            center.0 as f32,
            center.1 as f32,
            GLOBE_R,
            t,
            bright,
            RING_TOP,
            scan_y,
        );
        let half = (GLOBE_R * 1.45) as i32;
        let scan_col = if scan_y <= globe_bottom {
            pal().glow
        } else {
            crate::theme::mix(pal().bg0, pal().glow, 0.85 * scan_fade)
        };
        cv.hline(center.0 - half, scan_y, (half * 2) as u32, scan_col);
        let below = crate::theme::mix(pal().bg0, pal().glow, 0.45 * scan_fade);
        let above = crate::theme::mix(pal().bg0, pal().glow, 0.30 * scan_fade);
        cv.hline(center.0 - half, scan_y + 1, (half * 2) as u32, below);
        cv.hline(
            center.0 - half,
            scan_y.saturating_sub(1),
            (half * 2) as u32,
            above,
        );
        cv.hline(
            10,
            scan_y,
            (w - 20) as u32,
            crate::theme::mix(pal().bg0, pal().glow, 0.10 * scan_fade),
        );
    } else {
        gehirn::draw(
            cv,
            center.0 as f32,
            center.1 as f32,
            GLOBE_R,
            t,
            0.50 + 0.10 * breathe,
            RING_TOP,
            i32::MAX,
        );
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

    for (i, name) in ROLE_NAMES.iter().enumerate() {
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
            mid_px(8.0),
            pal().text_faint,
        );
        let text = if on { pal().text } else { pal().text_faint };
        fonts.draw(cv, x + 10, y_top + 40, name, mid_px(13.0), text);

        let (state, state_col) = if t < t_on {
            ("OFFLINE", pal().text_faint)
        } else if t < t_sync {
            ("LINK", pal().accent)
        } else if t < t_sync + 0.20 {
            ("SYNC", pal().glow)
        } else {
            ("ONLINE", pal().signal)
        };
        fonts.draw(cv, x + 10, y_top + 55, state, mid_px(8.0), state_col);

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

    // Status line.
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
    fonts.draw_center(cv, w / 2, 108, &status, art_px(12.0), status_col);

    fonts.draw(
        cv,
        14,
        h - 20,
        &format!("MAGI v{}", env!("CARGO_PKG_VERSION")),
        mid_px(9.0),
        pal().text_faint,
    );
    if t > NOMINAL {
        let a = (128.0 + 100.0 * (t * 2.4).sin()) as u8;
        let hint = approval();
        let hx = w - 14 - fonts.text_width(hint, mid_px(10.0));
        fonts.draw_alpha(cv, hx, h - 20, hint, mid_px(10.0), pal().text_dim, a);
    }

    // Fade back to the shelf.
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

fn hash(x: i32, y: i32) -> u32 {
    let mut v = (x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663);
    v ^= v >> 13;
    v.wrapping_mul(1_274_126_177) ^ (v >> 16)
}

/// A sparse honeycomb backplate that gains presence over the boot.
fn hex_backdrop(cv: &mut Canvas, t: f32) {
    let cell = 34;
    let dx = cell * 2;
    let dy = (cell as f32 * 1.732) as i32;
    let rows = (cv.h + cell) / dy.max(1) + 1;
    let ramp = (t / BOOT_SECS).clamp(0.0, 1.0);
    let clip = (0, 0, cv.w, cv.h);
    let (cx0, cy0) = (cv.w as f32 / 2.0, GLOBE_CENTER_Y as f32);
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

/// Splash art without the wordmark.
pub fn render_bootlogo_art(cv: &mut Canvas) {
    let w = cv.w;
    let h = cv.h;
    cv.clear(pal().bg0);
    hex_backdrop(cv, 0.0);
    gehirn::draw_splash(cv, w as f32 / 2.0, GLOBE_CENTER_Y as f32, GLOBE_R, 0.28);
    cv.rect_outline(8, 8, (w - 16) as u32, (h - 16) as u32, pal().border);
}

/// The bootloader splash bitmap.
pub fn render_bootlogo(cv: &mut Canvas, fonts: &mut Fonts) {
    render_bootlogo_art(cv);
    let w = cv.w;
    let title_baseline = GLOBE_CENTER_Y + 16;
    let title_px = art_px(62.0);
    let sub_px = art_px(12.0);
    let tw = fonts.text_width("M A G I", title_px);
    let tx = (w - tw) / 2;
    fonts.draw_alpha(
        cv,
        tx + 2,
        title_baseline + 2,
        "M A G I",
        title_px,
        pal().bg0,
        200,
    );
    fonts.draw_alpha(
        cv,
        tx,
        title_baseline,
        "M A G I",
        title_px,
        pal().accent,
        235,
    );
    let sub_baseline = title_baseline + 18;
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (1, 1)] {
        fonts.draw_center(
            cv,
            w / 2 + dx,
            sub_baseline + dy,
            "MULTI ARCHITECTURE GAMING INTERFACE",
            sub_px,
            pal().bg0,
        );
    }
    fonts.draw_center(
        cv,
        w / 2,
        sub_baseline,
        "MULTI ARCHITECTURE GAMING INTERFACE",
        sub_px,
        pal().text_dim,
    );
}
