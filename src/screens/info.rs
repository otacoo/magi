//! Read-only information screens (NETWORK, SYSTEM), refreshed while open.

use crate::canvas::Canvas;
use crate::font::Fonts;
use crate::theme::{self, pal};

pub fn draw(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    title: &str,
    lines: &[(String, String)],
    toggles: &[(&str, bool)],
    sel: usize,
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
    let title_text = format!("MAGI // {title}");
    fonts.draw_center(cv, w / 2, 42, &title_text, 15.0, pal().title);
    theme::hazard_bar(cv, w / 2 - 150, 54, 300, 5, t);

    let x0 = w / 2 - 230;
    let row_w = 460;
    let mut y = 100;
    for (i, (label, on)) in toggles.iter().enumerate() {
        let selected = i == sel;
        if selected {
            cv.fill_rect(x0 - 8, y - 16, row_w as u32 + 16, 24, pal().bg3);
            theme::glow_border(
                cv,
                x0 - 8,
                y - 16,
                row_w as u32 + 16,
                24,
                pal().glow,
                theme::mix(pal().glow, pal().accent, 0.35),
                t,
                0.4,
            );
        }
        fonts.draw(
            cv,
            x0,
            y,
            label,
            11.0,
            if selected {
                pal().accent
            } else {
                pal().text_dim
            },
        );
        let value = format!("< {} >", if *on { "ON" } else { "OFF" });
        let vw = fonts.text_width(&value, 11.0);
        fonts.draw(
            cv,
            x0 + row_w - vw,
            y,
            &value,
            11.0,
            if selected { pal().signal } else { pal().text },
        );
        y += 34;
    }
    if !toggles.is_empty() {
        y += 8;
    }
    for (label, value) in lines {
        fonts.draw(cv, x0, y, label, 11.0, pal().text_dim);
        let value = clip(value, 44);
        let tw = fonts.text_width(&value, 11.0);
        fonts.draw(cv, x0 + row_w - tw, y, &value, 11.0, pal().text);
        y += 34;
    }

    // Corner tags, as on the scanner station plates.
    fonts.draw(cv, 20, h - 44, ">> CENTRAL DOGMA", 8.0, pal().text_faint);
    let tag = "PRIBNOW BOX <<";
    let tag_w = fonts.text_width(tag, 8.0);
    fonts.draw(cv, w - 20 - tag_w, h - 44, tag, 8.0, pal().text_faint);
    let hint = if toggles.is_empty() {
        "B BACK"
    } else {
        "U/D PICK   A TOGGLE   B BACK"
    };
    fonts.draw_center(cv, w / 2, h - 26, hint, 9.0, pal().text_faint);
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(2)).collect();
        t.push_str("..");
        t
    }
}
