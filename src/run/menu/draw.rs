//! The pause menu: folder tabs, the state preview and the in-pane pages.

use std::ffi::{c_void, CString};
use std::os::raw::c_uint;
use std::path::Path;
use std::sync::atomic::Ordering;

use crate::canvas::Canvas;
use crate::font::Fonts;
use crate::image::Image;
use crate::netplay;
use crate::screens::views::marquee_offset;
use crate::theme::pal;

use super::super::prefs::ShaderMode;
use super::super::{
    ctx, disc_count, disc_index, visible_options, CoreOption, Ctx, DiskControl, GameGeometry,
    GameInfo, GameMenu, MenuPage, NetplayPage, OverlayLayers, StateCard, CTX, STATE_SLOTS,
};
use super::{menu_sections, netplay_status_line, option_rows_for_height, MenuSection};

pub(crate) fn draw_menu(cv: &mut Canvas, fonts: &mut Fonts, menu: &GameMenu, rom: &str, t: f32) {
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    cv.fill_rect_a(0, 0, w as u32, h as u32, pal().bg0, 240);

    let margin = p(8.0);
    let panel_w = (w - 2 * margin) as u32;
    let panel_h = (h - 2 * margin) as u32;
    crate::theme::glow_border(
        cv,
        margin,
        margin,
        panel_w,
        panel_h,
        pal().border,
        pal().glow,
        t,
        0.0,
    );

    let left = margin + p(16.0);
    let right = w - margin - p(16.0);
    let title_base = margin + p(22.0);
    fonts.draw_bold(cv, left, title_base, "MAGI // PAUSED", 15.0, pal().title);
    let linked = ctx().netplay_linked;
    let note = if linked {
        "NETPLAY LINKED"
    } else {
        "SYSTEM PAUSED"
    };
    let note_w = fonts.text_width(note, 9.0);
    let rom_title = Path::new(rom)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_uppercase();
    let rom_text = fonts.elide(&rom_title, 10.0, right - left - note_w - p(16.0));
    fonts.draw(
        cv,
        left,
        title_base + p(18.0),
        &rom_text,
        10.0,
        pal().text_faint,
    );

    let discs = disc_count();
    let status = if discs > 1 {
        format!(
            "SLOT {}/{}   DISC {}/{}",
            menu.slot + 1,
            STATE_SLOTS,
            disc_index().min(discs - 1) + 1,
            discs
        )
    } else {
        format!("SLOT {}/{}", menu.slot + 1, STATE_SLOTS)
    };
    right_text(cv, fonts, right, title_base, &status, 9.5, pal().text_dim);
    right_text(
        cv,
        fonts,
        right,
        title_base + p(18.0),
        note,
        9.0,
        if linked {
            pal().signal
        } else {
            pal().text_faint
        },
    );

    let div_y = margin + p(50.0);
    cv.hline(left, div_y, (right - left) as u32, pal().border);
    cv.fill_rect(
        left,
        div_y,
        p(42.0).max(1) as u32,
        p(3.0).max(1) as u32,
        pal().accent,
    );

    let body_top = div_y + p(8.0);
    let foot_y = margin + panel_h as i32 - p(12.0);
    let body_bottom = foot_y - p(16.0);
    let body_h = (body_bottom - body_top).max(p(80.0)) as u32;

    let tab_w = p(136.0);
    let pane_x = left + tab_w;
    let pane_w = (right - pane_x).max(p(140.0)) as u32;

    let sections = menu_sections();
    let sel = menu.sel.min(sections.len().saturating_sub(1));
    let n = sections.len().max(1) as i32;
    let gap = p(4.0);
    let pitch = ((body_h as i32 - gap * (n - 1).max(0)) / n)
        .min(p(44.0))
        .max(p(22.0));
    let tab_h = (pitch - gap).max(p(18.0));
    let tab_y = body_top + sel as i32 * pitch;

    cv.fill_rect_a(pane_x, body_top, pane_w, body_h, pal().bg1, 244);
    cv.hline(pane_x, body_top, pane_w, pal().border);
    cv.hline(pane_x, body_top + body_h as i32 - 1, pane_w, pal().border);
    cv.vline(pane_x + pane_w as i32 - 1, body_top, body_h, pal().border);
    if tab_y > body_top {
        cv.vline(pane_x, body_top, (tab_y - body_top) as u32, pal().border);
    }
    let below = tab_y + tab_h;
    if below < body_top + body_h as i32 {
        cv.vline(
            pane_x,
            below,
            (body_top + body_h as i32 - below) as u32,
            pal().border,
        );
    }
    for (i, section) in sections.iter().enumerate() {
        let y = body_top + i as i32 * pitch;
        let selected = i == sel;
        if selected {
            cv.fill_rect_a(left, y, tab_w as u32, tab_h as u32, pal().bg1, 244);
            cv.fill_rect(left, y, p(4.0).max(1) as u32, tab_h as u32, pal().accent);
            cv.hline(left, y, tab_w as u32, pal().accent);
            cv.hline(left, y + tab_h - 1, tab_w as u32, pal().accent);
            cv.vline(left, y, tab_h as u32, pal().accent);
        }
        let base = y + tab_h / 2 + p(6.0);
        let num = format!("{:02}", i + 1);
        fonts.draw(
            cv,
            left + p(12.0),
            base,
            &num,
            8.0,
            if selected {
                pal().accent
            } else {
                pal().text_faint
            },
        );
        fonts.draw(
            cv,
            left + p(36.0),
            base,
            section.label(),
            12.0,
            if selected {
                pal().accent
            } else {
                pal().text_dim
            },
        );
    }

    let section = sections[sel];
    let content_x = pane_x + p(12.0);
    let content_top = body_top + p(10.0);
    let content_bottom = body_top + body_h as i32 - p(10.0);
    let content_w = (pane_w as i32 - p(24.0)).max(p(80.0));
    let content_h = content_bottom - content_top;
    match section {
        MenuSection::Save | MenuSection::Load => draw_state_pane(
            cv,
            fonts,
            menu,
            section,
            content_x,
            content_top,
            content_w,
            content_h,
            t,
        ),
        MenuSection::Core => draw_core_pane(
            cv,
            fonts,
            menu,
            content_x,
            content_top,
            content_w,
            content_h,
            t,
        ),
        MenuSection::Video => draw_video_pane(
            cv,
            fonts,
            menu,
            content_x,
            content_top,
            content_w,
            content_bottom,
            t,
        ),
        MenuSection::Disc => draw_disc_pane(
            cv,
            fonts,
            menu,
            content_x,
            content_top,
            content_w,
            content_h,
            t,
        ),
        MenuSection::Resume | MenuSection::Exit => draw_info_pane(
            cv,
            fonts,
            menu,
            section,
            content_x,
            content_top,
            content_w,
            content_bottom,
            t,
        ),
        MenuSection::Netplay => draw_netplay_pane(
            cv,
            fonts,
            menu,
            content_x,
            content_top,
            content_w,
            content_bottom,
            t,
        ),
    }

    draw_footer(cv, fonts, menu, section, left, right, foot_y);
}

pub(crate) fn draw_hold_panel(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    menu: &GameMenu,
    t: f32,
    w: i32,
    h: i32,
) {
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let f = menu.hold.unwrap_or(0.0).clamp(0.0, 1.0);
    cv.fill_rect_a(0, 0, w as u32, h as u32, pal().bg0, 140);

    let ww = p(420.0).min(w - p(24.0));
    let hh = p(126.0).min(h - p(24.0));
    let x = w / 2 - ww / 2;
    let y = h / 2 - hh / 2;
    cv.fill_rect_a(x, y, ww as u32, hh as u32, pal().bg0, 248);
    crate::theme::glow_border(
        cv,
        x,
        y,
        ww as u32,
        hh as u32,
        pal().glow,
        pal().accent,
        t,
        0.5,
    );
    fonts.draw_center(cv, w / 2, y + p(30.0), "SAVE + EXIT", 16.0, pal().title);
    fonts.draw_center(
        cv,
        w / 2,
        y + p(54.0),
        &format!("SLOT {}/{}", menu.slot + 1, STATE_SLOTS),
        11.0,
        pal().signal,
    );

    let bw = (ww - p(48.0)).max(p(120.0)) as u32;
    let bh = p(12.0).max(8) as u32;
    let bx = w / 2 - bw as i32 / 2;
    let by = y + p(68.0);
    cv.rect_outline(bx, by, bw, bh, pal().border);
    let fw = ((bw - 2) as f32 * f) as u32;
    cv.fill_rect(bx + 1, by + 1, fw, bh.saturating_sub(2), pal().signal);
    fonts.draw_center(
        cv,
        w / 2,
        y + hh - p(16.0),
        "KEEP HOLDING   RELEASE TO CANCEL",
        9.0,
        pal().text_faint,
    );
}

fn draw_state_pane(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    menu: &GameMenu,
    section: MenuSection,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    t: f32,
) {
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let strip_h = p(28.0).min(h / 3).max(p(22.0));
    let image_h = (h - strip_h).max(p(40.0));
    cv.fill_rect(x, y, w as u32, image_h as u32, pal().bg0);
    let card = menu.states.get(menu.slot.min(STATE_SLOTS - 1));
    let exists = card.is_some_and(|c| c.exists);
    if let Some(img) = card.and_then(|c| c.thumb.as_ref()) {
        img.draw_fit(cv, x, y, w as u32, image_h as u32, 255);
        cv.shade_rect(x, y, w as u32, image_h as u32, 72);
    } else {
        let (title, sub) = match (section, exists) {
            (_, true) => ("NO PREVIEW", "STATE EXISTS, NO SCREENSHOT"),
            (MenuSection::Load, false) => ("EMPTY SLOT", "NO SAVED STATE"),
            _ => ("EMPTY SLOT", "PRESS A TO SAVE"),
        };
        fonts.draw_center(cv, x + w / 2, y + image_h / 2, title, 14.0, pal().text_dim);
        fonts.draw_center(
            cv,
            x + w / 2,
            y + image_h / 2 + p(20.0),
            sub,
            10.0,
            pal().text_faint,
        );
    }
    crate::theme::glow_border(
        cv,
        x,
        y,
        w as u32,
        image_h as u32,
        pal().accent,
        pal().glow,
        t,
        0.6,
    );

    let sy = y + image_h;
    cv.fill_rect(x, sy, w as u32, strip_h as u32, pal().bg2);
    cv.rect_outline(x, sy, w as u32, strip_h as u32, pal().border);
    let label = format!("SLOT {}/{}", menu.slot + 1, STATE_SLOTS);
    fonts.draw(
        cv,
        x + p(12.0),
        sy + strip_h / 2 + p(6.0),
        &label,
        12.0,
        pal().text,
    );
    if let Some(card) = card {
        if !card.age.is_empty() {
            right_text(
                cv,
                fonts,
                x + w - p(12.0),
                sy + strip_h / 2 + p(5.0),
                &card.age,
                10.0,
                pal().text_faint,
            );
        }
    }
}

fn draw_core_pane(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    menu: &GameMenu,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    t: f32,
) {
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let ids = visible_options(ctx());
    if ids.is_empty() {
        fonts.draw_center(
            cv,
            x + w / 2,
            y + p(30.0),
            "NO CORE OPTIONS",
            14.0,
            pal().text_dim,
        );
        return;
    }
    let rows = option_rows_for_height(h);
    super::OPTION_PANE_H.store(h, Ordering::Relaxed);
    let list: Vec<(String, String)> = ids
        .iter()
        .skip(menu.opt_top)
        .take(rows)
        .map(|&vi| {
            let o = &ctx().options[vi];
            let value = o.values.get(o.chosen).cloned().unwrap_or_default();
            (o.desc.clone(), value)
        })
        .collect();
    let sel = menu
        .opt_sel
        .saturating_sub(menu.opt_top)
        .min(list.len().saturating_sub(1));
    let row_h = p(26.0);
    let total = ids.len();
    let visible = rows.min(total);
    let has_scroll = total > visible;
    let row_w = if has_scroll { w - p(10.0) } else { w };
    draw_rows(
        cv,
        fonts,
        &list,
        sel,
        menu.focus_pane,
        x,
        y,
        row_w,
        row_h,
        menu.opt_since.elapsed().as_secs_f32(),
        t,
    );
    if has_scroll {
        let track_x = x + w - p(3.0);
        let track_h = (row_h * visible as i32 - p(4.0)).max(1) as u32;
        let thumb_h = ((track_h as usize * visible / total).max(p(14.0) as usize))
            .min(track_h as usize) as u32;
        let span = total - visible;
        let travel = track_h as usize - thumb_h as usize;
        let thumb_y = y + (travel * menu.opt_top.min(span) / span) as i32;
        cv.fill_rect(track_x, y, 2, track_h, pal().scrollbar_track);
        cv.fill_rect(track_x, thumb_y, 2, thumb_h, pal().scrollbar);
    }
}

fn draw_video_pane(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    menu: &GameMenu,
    x: i32,
    y: i32,
    w: i32,
    bottom: i32,
    t: f32,
) {
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let onoff = |b: bool| if b { "ON" } else { "OFF" };
    let rows = [
        ("FPS COUNTER", onoff(menu.fps).to_string()),
        ("SYSINFO", onoff(menu.sysinfo).to_string()),
        ("SHADER", menu.shader.label().to_string()),
        (
            "CRT",
            overlay_pick(&ctx().overlay_layers, menu.overlay_idx[0], 0),
        ),
        (
            "BEZEL",
            overlay_pick(&ctx().overlay_layers, menu.overlay_idx[1], 1),
        ),
    ];
    draw_kv_rows(cv, fonts, x, y + p(20.0), w, &rows);
    draw_pane_cta(
        cv,
        fonts,
        x,
        bottom - p(34.0),
        w as u32,
        "A   OPEN VIDEO OPTIONS",
        t,
        false,
    );
}

fn draw_disc_pane(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    menu: &GameMenu,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    t: f32,
) {
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let count = disc_count();
    if count == 0 {
        fonts.draw_center(cv, x + w / 2, y + p(30.0), "NO DISCS", 14.0, pal().text_dim);
        return;
    }
    let current = disc_index();
    let rows: Vec<(String, String)> = (0..count)
        .map(|i| {
            (
                format!("DISC {}", i + 1),
                if i == current { "INSERTED" } else { "" }.to_string(),
            )
        })
        .collect();
    let row_h = (h / count as i32).min(p(26.0)).max(p(18.0));
    draw_rows(
        cv,
        fonts,
        &rows,
        menu.disc_sel.min(count - 1),
        true,
        x,
        y,
        w,
        row_h,
        0.0,
        t,
    );
}

fn draw_rows(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    rows: &[(String, String)],
    selected: usize,
    focus: bool,
    x: i32,
    y: i32,
    w: i32,
    row_h: i32,
    hold: f32,
    t: f32,
) {
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    for (i, (label, value)) in rows.iter().enumerate() {
        let ry = y + i as i32 * row_h;
        let picked = focus && i == selected;
        if picked {
            let bh = (row_h - p(4.0)).max(p(18.0));
            cv.fill_rect(x, ry, w as u32, bh as u32, pal().bg3);
            crate::theme::glow_border(
                cv,
                x,
                ry,
                w as u32,
                bh as u32,
                pal().accent,
                pal().glow,
                t,
                0.4,
            );
        }
        let base = ry + row_h / 2 + p(5.0);
        let value_avail = (w / 2).max(p(40.0));
        let value_full = fonts.text_width(value, 11.5);
        let vw = value_full.min(value_avail);
        let label_avail = (w - vw - p(20.0)).max(p(40.0));
        let label_color = if picked { pal().accent } else { pal().text_dim };
        let label_w = fonts.text_width(label, 12.0);
        let label_shift = if picked && label_w > label_avail {
            marquee_offset(label_w, label_avail, hold)
        } else {
            None
        };
        if let Some(off) = label_shift {
            fonts.draw_clipped(
                cv,
                x + p(10.0) - off,
                base,
                label,
                12.0,
                label_color,
                x + p(10.0),
                x + p(10.0) + label_avail,
            );
        } else {
            let label = fonts.elide(label, 12.0, label_avail);
            fonts.draw(cv, x + p(10.0), base, &label, 12.0, label_color);
        }

        let value_color = if picked { pal().signal } else { pal().text };
        let value_shift = if picked && value_full > value_avail {
            marquee_offset(value_full, value_avail, hold)
        } else {
            None
        };
        let value_x = x + w - p(10.0) - value_avail;
        if let Some(off) = value_shift {
            fonts.draw_clipped(
                cv,
                value_x - off,
                base,
                value,
                11.5,
                value_color,
                value_x,
                value_x + value_avail,
            );
        } else {
            let value = fonts.elide(value, 11.5, value_avail);
            let vw = fonts.text_width(&value, 11.5);
            fonts.draw(cv, x + w - p(10.0) - vw, base, &value, 11.5, value_color);
        }
    }
}

fn draw_info_pane(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    menu: &GameMenu,
    section: MenuSection,
    x: i32,
    y: i32,
    w: i32,
    bottom: i32,
    t: f32,
) {
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let rows: Vec<(&str, String)> = match section {
        MenuSection::Resume => vec![
            ("GAME", game_title().to_uppercase()),
            ("SYSTEM", ctx().system.to_uppercase()),
            ("CORE", ctx().opt_key.to_uppercase()),
        ],
        _ => vec![
            ("SRAM", "FLUSHED ON EXIT".into()),
            (
                "RESUME POINT",
                if std::fs::metadata(&menu.auto_path).is_ok() {
                    "PRESENT"
                } else {
                    "NONE"
                }
                .into(),
            ),
            (
                "NETPLAY",
                if ctx().netplay.is_some() {
                    "WILL END"
                } else {
                    "IDLE"
                }
                .into(),
            ),
        ],
    };
    draw_kv_rows(cv, fonts, x, y + p(20.0), w, &rows);
    let (text, alert) = if section == MenuSection::Resume {
        ("A   RESUME", false)
    } else {
        ("A   EXIT GAME", true)
    };
    draw_pane_cta(cv, fonts, x, bottom - p(38.0), w as u32, text, t, alert);
}

fn draw_netplay_pane(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    menu: &GameMenu,
    x: i32,
    y: i32,
    w: i32,
    bottom: i32,
    t: f32,
) {
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let _ = menu;
    let rows = [
        ("STATUS", netplay_status_line()),
        (
            "MODE",
            ctx()
                .netplay
                .as_ref()
                .map(|s| s.transport().label().to_string())
                .unwrap_or_else(|| "NONE".into()),
        ),
        ("GAME", game_title().to_uppercase()),
    ];
    draw_kv_rows(cv, fonts, x, y + p(20.0), w, &rows);
    draw_pane_cta(
        cv,
        fonts,
        x,
        bottom - p(38.0),
        w as u32,
        "A   OPEN NETPLAY",
        t,
        false,
    );
}

fn draw_kv_rows(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    x: i32,
    y: i32,
    w: i32,
    rows: &[(&str, String)],
) {
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let row_h = p(24.0);
    for (i, (key, value)) in rows.iter().enumerate() {
        let by = y + i as i32 * row_h;
        fonts.draw(cv, x, by, key, 10.5, pal().text_faint);
        let value = fonts.elide(value, 11.5, w - p(90.0));
        let vw = fonts.text_width(&value, 11.5);
        fonts.draw(cv, x + w - vw, by, &value, 11.5, pal().text);
    }
}

fn draw_pane_cta(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    x: i32,
    y: i32,
    w: u32,
    text: &str,
    t: f32,
    alert: bool,
) {
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let h = p(30.0).max(22) as u32;
    cv.fill_rect(x, y, w, h, pal().bg3);
    let accent = if alert { pal().alert } else { pal().accent };
    crate::theme::glow_border(cv, x, y, w, h, accent, pal().glow, t, 0.7);
    fonts.draw_center(
        cv,
        x + w as i32 / 2,
        y + h as i32 / 2 + p(5.0),
        text,
        11.0,
        accent,
    );
}

fn draw_footer(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    menu: &GameMenu,
    section: MenuSection,
    left: i32,
    right: i32,
    foot_y: i32,
) {
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    if menu.hold.is_some() {
        return;
    }
    let hint = if menu.focus_pane {
        "U/D OPTION   L/R VALUE   A CHANGE   B BACK"
    } else {
        match section {
            MenuSection::Save => "L/R SLOT   A SAVE   Y DELETE   B RESUME",
            MenuSection::Load => "L/R SLOT   A LOAD   Y DELETE   B RESUME",
            MenuSection::Core => "U/D TAB   A ADJUST   B RESUME",
            MenuSection::Video => "A OPEN VIDEO OPTIONS   B RESUME",
            MenuSection::Netplay => "A OPEN NETPLAY   B RESUME",
            MenuSection::Disc => "L/R DISC   A SWAP   B RESUME",
            MenuSection::Resume => "A RESUME   B CLOSE",
            MenuSection::Exit => "A EXIT GAME   B RESUME",
        }
    };
    let msg_text = menu
        .msg
        .as_ref()
        .map(|(msg, _)| fonts.elide(msg, 9.0, (right - left) / 2));
    let mut avail = right - left;
    if let Some(text) = &msg_text {
        avail -= fonts.text_width(text, 9.0) + p(12.0);
    }
    let hint = fonts.elide(hint, 9.0, avail.max(p(80.0)));
    fonts.draw(cv, left, foot_y, &hint, 9.0, pal().text_faint);
    if let Some(text) = &msg_text {
        right_text(cv, fonts, right, foot_y, text, 9.0, pal().signal);
    }
}

fn right_text(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    right: i32,
    baseline: i32,
    text: &str,
    px: f32,
    color: crate::theme::Color,
) {
    let tw = fonts.text_width(text, px);
    fonts.draw(cv, right - tw, baseline, text, px, color);
}

fn overlay_pick(files: &OverlayLayers, idx: usize, layer: usize) -> String {
    if idx == 0 {
        return "NONE".into();
    }
    files[layer]
        .get(idx - 1)
        .and_then(|s| Path::new(s).file_name().and_then(|x| x.to_str()))
        .unwrap_or("NONE")
        .to_string()
}

fn game_title() -> String {
    Path::new(&ctx().rom)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("GAME")
        .to_string()
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

pub(crate) fn draw_overlay_menu(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    menu: &GameMenu,
    files: &OverlayLayers,
    t: f32,
) {
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let ww = p(320.0).min(w - p(24.0));
    let hh = p(92.0) + 5 * p(32.0) + p(24.0);
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 195);
    crate::theme::glow_border(
        cv,
        x0,
        y0,
        ww as u32,
        hh as u32,
        pal().border,
        pal().glow,
        t,
        0.2,
    );
    fonts.draw_center(cv, w / 2, y0 + p(24.0), "MAGI // VIDEO", 15.0, pal().title);
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(46.0),
        "OSD, SHADER AND OVERLAYS",
        10.0,
        pal().text_faint,
    );

    // Same row geometry as the pause menu: 32 px pitch, 24 px inset, the
    // text baseline 5 px under the row centre.
    let rows = ["FPS", "SYS INFO", "SHADER", "CRT", "BEZEL"];
    let pitch = p(32.0);
    for (row, label) in rows.iter().enumerate() {
        let c = y0 + p(76.0) + row as i32 * pitch + pitch / 2;
        let baseline = c + p(5.0);
        let bh = (pitch - p(8.0)).min(p(32.0));
        let sel = row == menu.overlay_sel;
        if sel {
            cv.fill_rect(
                x0 + p(24.0),
                c - bh / 2,
                (ww - p(48.0)) as u32,
                bh as u32,
                pal().bg3,
            );
            crate::theme::glow_border(
                cv,
                x0 + p(24.0),
                c - bh / 2,
                (ww - p(48.0)) as u32,
                bh as u32,
                pal().glow,
                crate::theme::mix(pal().glow, pal().accent, 0.35),
                t,
                0.4,
            );
        }
        let text = if sel { pal().accent } else { pal().text_dim };
        let label_w = fonts.text_width(label, 12.0);
        fonts.draw(cv, x0 + p(34.0), baseline, label, 12.0, text);
        let cell = |v: bool| if v { "ON" } else { "OFF" }.to_string();
        let max_w = (ww - p(34.0) * 2 - label_w - p(12.0)).max(p(40.0));
        let value = match row {
            0 => cell(menu.fps),
            1 => cell(menu.sysinfo),
            2 => menu.shader.label().to_string(),
            _ => match menu.overlay_idx[row - 3] {
                0 => "OFF".to_string(),
                k => {
                    let name = files
                        .get(row - 3)
                        .and_then(|l| l.get(k - 1))
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let name = Path::new(name)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(name);
                    fonts.elide(name, 12.0, max_w)
                }
            },
        };
        let vw = fonts.text_width(&value, 12.0);
        fonts.draw(
            cv,
            x0 + ww - p(34.0) - vw,
            baseline,
            &value,
            12.0,
            if sel { pal().signal } else { text },
        );
    }

    fonts.draw_center(
        cv,
        w / 2,
        y0 + hh - p(20.0),
        "UP/DOWN PICK   LEFT/RIGHT CHANGE   B BACK   MENU CLOSE",
        9.0,
        pal().text_faint,
    );
}

pub(crate) fn draw_netplay(cv: &mut Canvas, fonts: &mut Fonts, menu: &GameMenu, t: f32) {
    let session = ctx().netplay.as_ref();
    let status = session.map(|s| s.status());
    match menu.netplay_page {
        NetplayPage::Menu => netplay_panel(
            cv,
            fonts,
            "MAGI // NETPLAY",
            &clip(&game_title(), 38),
            &["HOST GAME", "JOIN GAME", "BACK"],
            menu.netplay_sel,
            "A SELECT   B BACK",
            t,
        ),
        NetplayPage::HostTransport => netplay_panel(
            cv,
            fonts,
            "HOST GAME",
            "PICK A CONNECTION",
            &["HOTSPOT", "WIFI", "BACK"],
            menu.netplay_sel,
            "A SELECT   B BACK",
            t,
        ),
        NetplayPage::JoinTransport => netplay_panel(
            cv,
            fonts,
            "JOIN GAME",
            "PICK A CONNECTION",
            &["HOTSPOT", "WIFI", "BACK"],
            menu.netplay_sel,
            "A SELECT   B BACK",
            t,
        ),
        NetplayPage::HostWait => draw_netplay_wait(cv, fonts, menu, t),
        NetplayPage::JoinScan => draw_netplay_scan(cv, fonts, menu, status, t),
        NetplayPage::Linked => netplay_panel(
            cv,
            fonts,
            "MAGI // NETPLAY",
            &format!("LINKED {}", session.map(|s| s.code()).unwrap_or("----")),
            &["END NETPLAY", "BACK"],
            menu.netplay_sel,
            "A SELECT   B BACK",
            t,
        ),
        NetplayPage::Error => draw_netplay_error(cv, fonts, menu, t),
    }
}

fn netplay_panel(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    title: &str,
    subtitle: &str,
    items: &[&str],
    sel: usize,
    footer: &str,
    t: f32,
) {
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let ww = p(400.0).min(w - p(24.0));
    let header = p(104.0);
    let pitch = ((h - p(12.0) - header - p(44.0)) / items.len().max(1) as i32)
        .min(p(40.0))
        .max(p(26.0));
    let hh = header + items.len().max(1) as i32 * pitch + p(44.0);
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 235);
    crate::theme::glow_border(
        cv,
        x0,
        y0,
        ww as u32,
        hh as u32,
        pal().border,
        pal().glow,
        t,
        0.0,
    );
    fonts.draw_center(cv, w / 2, y0 + p(26.0), title, 15.0, pal().title);
    fonts.draw_center(cv, w / 2, y0 + p(56.0), subtitle, 10.0, pal().text_faint);
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(80.0),
        "GAME BOY NETPLAY",
        9.0,
        pal().text_dim,
    );

    for (i, item) in items.iter().enumerate() {
        let c = y0 + header + i as i32 * pitch + pitch / 2;
        let y = c + p(5.5);
        let bh = (pitch - p(10.0)).min(p(36.0));
        let picked = i == sel.min(items.len().saturating_sub(1));
        if picked {
            cv.fill_rect(
                x0 + p(28.0),
                c - bh / 2,
                (ww - p(56.0)) as u32,
                bh as u32,
                pal().bg3,
            );
            crate::theme::glow_border(
                cv,
                x0 + p(28.0),
                c - bh / 2,
                (ww - p(56.0)) as u32,
                bh as u32,
                pal().glow,
                crate::theme::mix(pal().glow, pal().accent, 0.35),
                t,
                0.4,
            );
        }
        fonts.draw_center(
            cv,
            w / 2,
            y,
            item,
            12.0,
            if picked { pal().accent } else { pal().text_dim },
        );
    }
    fonts.draw_center(cv, w / 2, y0 + hh - p(26.0), footer, 9.0, pal().text_faint);
}

fn draw_netplay_wait(cv: &mut Canvas, fonts: &mut Fonts, menu: &GameMenu, t: f32) {
    let session = ctx().netplay.as_ref();
    let code = session.map(|s| s.code()).unwrap_or("----");
    let transport = session
        .map(|s| s.transport())
        .unwrap_or(menu.netplay_transport);
    let status = session
        .map(|s| s.status())
        .unwrap_or(netplay::Status::Stopped);
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let ww = p(400.0);
    let hh = p(300.0);
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 235);
    crate::theme::glow_border(
        cv,
        x0,
        y0,
        ww as u32,
        hh as u32,
        pal().border,
        pal().glow,
        t,
        0.0,
    );
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(26.0),
        "MAGI // NETPLAY",
        15.0,
        pal().title,
    );
    fonts.draw_center(cv, w / 2, y0 + p(56.0), "HOST GAME", 10.0, pal().text_faint);
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(80.0),
        &format!("{} CONNECTION", transport.label()),
        9.0,
        pal().text_dim,
    );

    let pill_w = p(190.0);
    let pill_h = p(62.0);
    let px = w / 2 - pill_w / 2;
    let py = y0 + p(104.0);
    cv.fill_rect(px, py, pill_w as u32, pill_h as u32, pal().bg3);
    crate::theme::glow_border(
        cv,
        px,
        py,
        pill_w as u32,
        pill_h as u32,
        pal().glow,
        crate::theme::mix(pal().glow, pal().accent, 0.35),
        t,
        0.4,
    );
    fonts.draw_center(cv, w / 2, py + p(42.0), code, 34.0, pal().accent);
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(192.0),
        "SELECT THIS CODE ON THE OTHER DEVICE",
        10.0,
        pal().text_dim,
    );

    let line = match &status {
        netplay::Status::Starting => format!("STARTING{}", netplay_dots(t)),
        netplay::Status::Waiting => format!("WAITING FOR CONNECTION{}", netplay_dots(t)),
        netplay::Status::Linked => "LINKED".into(),
        netplay::Status::Lost => "THE OTHER PLAYER LEFT".into(),
        netplay::Status::Error(e) => clip(e, 44),
        _ => "STOPPED".into(),
    };
    fonts.draw_center(cv, w / 2, y0 + p(226.0), &line, 11.0, pal().signal);
    let hint = match transport {
        netplay::Transport::Hotspot => format!("HOTSPOT MAGI-{code}"),
        netplay::Transport::Wifi => "SAME WI-FI NETWORK".into(),
    };
    fonts.draw_center(cv, w / 2, y0 + p(256.0), &hint, 9.0, pal().text_faint);
    fonts.draw_center(
        cv,
        w / 2,
        y0 + hh - p(22.0),
        "B CANCEL",
        9.0,
        pal().text_faint,
    );
}

fn draw_netplay_scan(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    menu: &GameMenu,
    status: Option<netplay::Status>,
    t: f32,
) {
    match &status {
        Some(netplay::Status::Error(e)) => {
            netplay_message(
                cv,
                fonts,
                "LINK FAILED",
                &clip(e, 44),
                "A RETRY   B BACK",
                t,
            );
        }
        Some(netplay::Status::Lost) => netplay_message(
            cv,
            fonts,
            "LINK ENDED",
            "THE OTHER PLAYER LEFT",
            "A RETRY   B BACK",
            t,
        ),
        Some(netplay::Status::Connecting) => {
            let code = ctx().netplay.as_ref().map(|s| s.code()).unwrap_or("----");
            netplay_message(
                cv,
                fonts,
                "JOIN GAME",
                &format!("CONNECTING TO {code}{}", netplay_dots(t)),
                "B CANCEL",
                t,
            );
        }
        Some(netplay::Status::Found) => {
            let hosts = ctx()
                .netplay
                .as_ref()
                .map(|s| s.hosts())
                .unwrap_or_default();
            if hosts.is_empty() {
                netplay_message(
                    cv,
                    fonts,
                    "JOIN GAME",
                    "NO HOSTS FOUND",
                    "A RESCAN   B BACK",
                    t,
                );
            } else {
                let n = hosts.len().min(5);
                let mut rows: Vec<String> = hosts
                    .iter()
                    .take(n)
                    .map(|h| {
                        if h.hotspot {
                            h.code.clone()
                        } else {
                            format!("{}  {}", h.code, clip(&h.title, 24))
                        }
                    })
                    .collect();
                while rows.len() < n {
                    rows.push(String::new());
                }
                let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
                netplay_panel(
                    cv,
                    fonts,
                    "JOIN GAME",
                    "PICK A HOST",
                    &refs,
                    menu.netplay_sel.min(n.saturating_sub(1)),
                    "A JOIN   B BACK",
                    t,
                );
            }
        }
        _ => {
            let transport = ctx()
                .netplay
                .as_ref()
                .map(|s| s.transport())
                .unwrap_or(menu.netplay_transport);
            let label = match transport {
                netplay::Transport::Hotspot => "SCANNING FOR HOTSPOTS",
                netplay::Transport::Wifi => "SCANNING FOR NETPLAY HOSTS",
            };
            netplay_message(
                cv,
                fonts,
                "JOIN GAME",
                &format!("{label}{}", netplay_dots(t)),
                "B CANCEL",
                t,
            );
        }
    }
}

fn draw_netplay_error(cv: &mut Canvas, fonts: &mut Fonts, menu: &GameMenu, t: f32) {
    let msg = menu.netplay_msg.clone().unwrap_or_else(|| {
        ctx()
            .netplay
            .as_ref()
            .map(|s| match s.status() {
                netplay::Status::Error(e) => e,
                netplay::Status::Lost => "THE OTHER PLAYER LEFT".into(),
                _ => "LINK FAILED".into(),
            })
            .unwrap_or_else(|| "LINK FAILED".into())
    });
    netplay_message(
        cv,
        fonts,
        "MAGI // NETPLAY",
        &clip(&msg, 44),
        "A RETRY   B BACK",
        t,
    );
}

fn netplay_message(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    title: &str,
    message: &str,
    footer: &str,
    t: f32,
) {
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let ww = p(400.0);
    let hh = p(190.0);
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 235);
    crate::theme::glow_border(
        cv,
        x0,
        y0,
        ww as u32,
        hh as u32,
        pal().border,
        pal().glow,
        t,
        0.0,
    );
    fonts.draw_center(cv, w / 2, y0 + p(30.0), title, 15.0, pal().title);
    fonts.draw_center(cv, w / 2, y0 + p(86.0), message, 12.0, pal().signal);
    fonts.draw_center(cv, w / 2, y0 + hh - p(26.0), footer, 9.0, pal().text_faint);
}

fn netplay_dots(t: f32) -> &'static str {
    ["", ".", "..", "..."][(t * 2.0) as usize % 4]
}

/// Render the overlays page with a given selection, for the headless check.
pub(crate) fn draw_overlay_menu_preview(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    sel: usize,
    idx: [usize; 2],
    files: &OverlayLayers,
    t: f32,
) {
    let menu = GameMenu {
        open: true,
        sel: 0,
        msg: None,
        exit_requested: false,
        page: MenuPage::Overlays,
        slot: 0,
        opt_sel: 0,
        opt_top: 0,
        opt_since: std::time::Instant::now(),
        overlay_sel: sel,
        overlay_idx: idx,
        shader: ShaderMode::Off,
        fps: false,
        sysinfo: false,
        hold: None,
        disc_sel: 0,
        netplay_page: NetplayPage::Menu,
        netplay_sel: 0,
        netplay_transport: netplay::Transport::Wifi,
        netplay_role: netplay::Role::Host,
        netplay_msg: None,
        focus_pane: false,
        states: Vec::new(),
        state_paths: Vec::new(),
        auto_path: std::path::PathBuf::new(),
        delete_armed: None,
    };
    draw_overlay_menu(cv, fonts, &menu, files, t);
}

/// A small synthetic frame for the state previews.
fn fixture_thumb(seed: u32) -> Image {
    let (w, h) = (240u32, 180u32);
    let mut px = vec![0u32; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let sky = y < h * 2 / 3;
            let p = if sky {
                let v = 40 + (x * 90 / w) + seed * 15;
                0xFF00_0000 | ((v.min(255)) << 16) | ((v / 2 + 40) << 8) | 90
            } else {
                let v = 30 + (y * 60 / h) + seed * 10;
                0xFF00_0000 | (v.min(255) << 16) | ((v / 2) << 8) | 30
            };
            px[(y * w + x) as usize] = p;
        }
    }
    let bx = 40 + seed * 30;
    for y in 60..120 {
        for x in bx..(bx + 60).min(w) {
            px[(y * w + x) as usize] = 0xFFE0_8020;
        }
    }
    Image { w, h, px }
}

unsafe extern "C" fn dc_eject(_: bool) -> bool {
    true
}
unsafe extern "C" fn dc_get_eject() -> bool {
    true
}
unsafe extern "C" fn dc_get_index() -> c_uint {
    0
}
unsafe extern "C" fn dc_set_index(_: c_uint) -> bool {
    true
}
unsafe extern "C" fn dc_num_images() -> c_uint {
    3
}
unsafe extern "C" fn dc_replace(_: c_uint, _: *const GameInfo) -> bool {
    true
}
unsafe extern "C" fn dc_add() -> bool {
    true
}

fn preview_option(
    key: &str,
    desc: &str,
    values: &[&str],
    chosen: usize,
    visible: bool,
) -> CoreOption {
    CoreOption {
        key: key.into(),
        desc: desc.into(),
        values: values.iter().map(|v| v.to_string()).collect(),
        c_values: values.iter().map(|v| CString::new(*v).unwrap()).collect(),
        chosen,
        visible,
    }
}

/// Render the dashboard with fixture data, for the headless check.
pub(crate) fn draw_menu_preview(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    sel: usize,
    focus: bool,
    hold: Option<f32>,
    t: f32,
) {
    unsafe extern "C" fn ser_size() -> usize {
        0
    }
    unsafe extern "C" fn ser(_: *mut c_void, _: usize) -> bool {
        false
    }
    unsafe extern "C" fn unser(_: *const c_void, _: usize) -> bool {
        false
    }

    let options = vec![
        preview_option(
            "gambatte_gb_link_mode",
            "GB Link Mode",
            &["Not Connected", "Network Client"],
            0,
            true,
        ),
        preview_option(
            "gambatte_gb_colorization",
            "Color Correction",
            &["Disabled", "GBC", "Auto"],
            2,
            true,
        ),
        preview_option(
            "gambatte_mix_frames",
            "Interframe Blending",
            &["Off", "On"],
            1,
            true,
        ),
        preview_option(
            "gambatte_rumble_level",
            "Rumble Strength",
            &["0", "1", "2", "3"],
            1,
            true,
        ),
    ];
    let mut fixture = Ctx {
        pixfmt: 0,
        geometry: GameGeometry {
            base_width: 160,
            base_height: 144,
            max_width: 160,
            max_height: 144,
            aspect_ratio: 10.0 / 9.0,
        },
        frame: std::ptr::null(),
        frame_seq: 0,
        pitch: 0,
        bits: 16,
        left: (0, 0),
        right: (0, 0),
        audio: Vec::new(),
        save_dir: CString::new("Saves").unwrap(),
        system_dir: CString::new("Saves").unwrap(),
        shutdown: false,
        sram_ptr: std::ptr::null_mut(),
        ser_size,
        ser,
        unser,
        options,
        opt_key: "gambatte".into(),
        opt_dirty: false,
        saved_opts: std::collections::HashMap::new(),
        overlay_layers: [
            vec!["CRT/Perfect_CRT.png".into()],
            vec!["GB/mugwomp93/Perfect_GBP-EX.png".into()],
        ],
        overlay_dirty: false,
        moto: None,
        rumble_on: false,
        rumble_ok: false,
        disk: Some(DiskControl {
            set_eject_state: dc_eject,
            get_eject_state: dc_get_eject,
            get_image_index: dc_get_index,
            set_image_index: dc_set_index,
            get_num_images: dc_num_images,
            replace_image_index: dc_replace,
            add_image_index: dc_add,
        }),
        system: "gb".into(),
        rom: "Metal Gear Solid (Europe) (En,Fr,De,Es).gbc".into(),
        netplay: None,
        netplay_linked: false,
        #[cfg(target_os = "linux")]
        hw_render: None,
    };
    let rom = fixture.rom.clone();
    let prev = CTX.swap(&mut fixture as *mut Ctx, Ordering::Relaxed);

    let slot = |n: usize| std::path::PathBuf::from(format!("Saves/preview.state{n}"));
    let states = vec![
        StateCard {
            path: slot(1),
            label: "SLOT 1".into(),
            age: "9M AGO".into(),
            thumb: Some(fixture_thumb(0)),
            exists: true,
        },
        StateCard {
            path: slot(2),
            label: "SLOT 2".into(),
            age: "3D AGO".into(),
            thumb: Some(fixture_thumb(2)),
            exists: true,
        },
        StateCard {
            path: slot(3),
            label: "SLOT 3".into(),
            age: String::new(),
            thumb: None,
            exists: false,
        },
    ];
    let menu = GameMenu {
        open: true,
        sel,
        msg: None,
        exit_requested: false,
        page: MenuPage::Main,
        slot: 0,
        opt_sel: 1,
        opt_top: 0,
        opt_since: std::time::Instant::now(),
        overlay_sel: 1,
        overlay_idx: [1, 1],
        shader: ShaderMode::Off,
        fps: true,
        sysinfo: false,
        hold,
        disc_sel: 1,
        netplay_page: NetplayPage::Menu,
        netplay_sel: 0,
        netplay_transport: netplay::Transport::Wifi,
        netplay_role: netplay::Role::Host,
        netplay_msg: None,
        focus_pane: focus,
        states,
        state_paths: vec![slot(1), slot(2), slot(3)],
        auto_path: std::path::PathBuf::from("Saves/preview.stateauto"),
        delete_armed: None,
    };
    draw_menu(cv, fonts, &menu, &rom, t);
    if menu.hold.is_some() {
        draw_hold_panel(cv, fonts, &menu, t, cv.w, cv.h);
    }
    CTX.store(prev, Ordering::Relaxed);
}
