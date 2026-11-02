//! The OTA update screen: GEHIRN's download and install telemetry.

use crate::canvas::Canvas;
use crate::font::Fonts;
use crate::theme::{self, pal};
use crate::update::{Phase, Update};

pub fn draw(cv: &mut Canvas, fonts: &mut Fonts, u: &Update, t: f32) {
    draw_state(cv, fonts, u.phase, &u.latest, &u.detail, u.progress, t);
}

pub fn draw_state(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    phase: Phase,
    latest: &str,
    detail: &str,
    progress: f32,
    t: f32,
) {
    let (w, h) = (cv.w, cv.h);
    cv.clear(pal().bg0);
    theme::background(cv, 12, 62, (w - 32) as u32, (h - 122) as u32, t);
    theme::glow_border(
        cv,
        8,
        8,
        (w - 16) as u32,
        (h - 16) as u32,
        pal().border,
        pal().glow,
        t,
        0.6,
    );
    fonts.draw_center(cv, w / 2, 42, "MAGI // UPDATE", 15.0, pal().title);
    theme::hazard_bar(cv, w / 2 - 150, 54, 300, 5, t);

    let color = match phase {
        Phase::Ready | Phase::UpToDate => pal().signal,
        Phase::Failed => pal().alert,
        _ => pal().accent,
    };

    // A rotating GEHIRN telemetry ring.
    let cx = w / 2;
    let cy = 158;
    let spin = t * 1.4;
    for i in 0..18 {
        let a = spin + i as f32 * std::f32::consts::TAU / 18.0;
        let r = 48.0 + (i % 3) as f32 * 8.0;
        let x = cx + (a.cos() * r) as i32;
        let y = cy + (a.sin() * r * 0.62) as i32;
        let alpha = 45 + (i as u16 * 190 / 17) as u8;
        cv.fill_rect_a(x - 1, y - 1, 3, 3, color, alpha);
    }
    let pulse = 0.5 + 0.5 * (t * 3.0).sin();
    cv.fill_rect(cx - 18, cy - 12, 36, 24, pal().bg1);
    cv.rect_outline(cx - 18, cy - 12, 36, 24, color);
    cv.fill_rect_a(cx - 8, cy - 3, 16, 6, color, (120.0 + 120.0 * pulse) as u8);

    let phase_label = phase.label();
    let tw = fonts.text_width(phase_label, 10.0);
    fonts.draw(cv, cx - tw / 2, 222, phase_label, 10.0, color);

    let bw = (w - 140).max(260);
    let bx = w / 2 - bw / 2;
    let by = 252;
    cv.fill_rect(bx, by, bw as u32, 18, pal().bg2);
    cv.rect_outline(bx, by, bw as u32, 18, pal().border);
    let fill = (bw as f32 * progress.clamp(0.0, 1.0)) as u32;
    if fill > 0 {
        cv.fill_rect(bx + 2, by + 2, fill.saturating_sub(4), 14, color);
    }
    // Segment ticks and a scanning highlight keep the bar moving.
    for x in (bx + 12..bx + bw - 4).step_by(16) {
        cv.vline(x, by + 3, 12, pal().bg0);
    }
    if phase.active() {
        let sweep = bx + 4 + ((t * 95.0) as i32 % (bw - 8).max(1));
        cv.fill_rect_a(sweep, by + 1, 10, 16, pal().signal, 90);
    }

    let pct = format!("{}%", (progress.clamp(0.0, 1.0) * 100.0).round() as u32);
    let pw = fonts.text_width(&pct, 9.0);
    fonts.draw(cv, bx + bw - pw, by + 32, &pct, 9.0, pal().text_dim);

    let detail = fonts.elide(detail, 10.0, bw - 12);
    fonts.draw_center(cv, w / 2, by + 40, &detail, 10.0, pal().text);

    let current = format!("CURRENT  {}", env!("CARGO_PKG_VERSION"));
    let latest = if latest.is_empty() {
        "LATEST   --".to_string()
    } else {
        format!("LATEST   {latest}")
    };
    let row_y = by + 76;
    fonts.draw(cv, bx, row_y, &current, 9.0, pal().text_faint);
    let lw = fonts.text_width(&latest, 9.0);
    fonts.draw(cv, bx + bw - lw, row_y, &latest, 9.0, pal().text_faint);

    let hint = if phase.active() {
        "PLEASE WAIT   DO NOT POWER OFF"
    } else if phase == Phase::Ready {
        "A RESTART"
    } else {
        "B BACK"
    };
    fonts.draw_center(cv, w / 2, h - 30, hint, 9.0, pal().text_faint);
    fonts.draw(
        cv,
        20,
        h - 44,
        ">> GEHIRN UPDATE SERVICE",
        8.0,
        pal().text_faint,
    );
    let tag = "GITHUB // LATEST <<";
    let tag_w = fonts.text_width(tag, 8.0);
    fonts.draw(cv, w - 20 - tag_w, h - 44, tag, 8.0, pal().text_faint);
}
