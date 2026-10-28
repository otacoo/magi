//! The pause-menu pages: dashboard, state grid, options, overlays, disc and
//! netplay.

use std::ffi::{c_void, CString};
use std::path::Path;
use std::sync::atomic::Ordering;

use crate::canvas::Canvas;
use crate::font::Fonts;
use crate::image::Image;
use crate::netplay;
use crate::theme::pal;

use super::super::{
    ctx, disc_count, disc_index, visible_options, CoreOption, Ctx, GameGeometry, GameMenu,
    MenuPage, NetplayPage, OverlayLayers, StateCard, CTX, STATE_CARDS, STATE_SLOTS,
};
use super::{menu_sections, netplay_status_line, section_summary, MenuSection, OPT_ROWS};

pub(crate) fn draw_menu(cv: &mut Canvas, fonts: &mut Fonts, menu: &GameMenu, rom: &str, t: f32) {
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    cv.fill_rect_a(0, 0, w as u32, h as u32, pal().bg0, 242);

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
    let title_base = margin + p(24.0);
    fonts.draw_bold(cv, left, title_base, "MAGI // PAUSED", 17.0, pal().title);
    let rom_title = Path::new(rom)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let rom_text = fonts.elide(&rom_title.to_uppercase(), 12.0, (right - left) / 2);
    fonts.draw(
        cv,
        left,
        title_base + p(20.0),
        &rom_text,
        12.0,
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
    let sw = fonts.text_width(&status, 10.5);
    fonts.draw(cv, right - sw, title_base, &status, 10.5, pal().text_dim);
    let linked = ctx().netplay_linked;
    let note = if linked {
        "NETPLAY LINKED"
    } else {
        "SYSTEM PAUSED"
    };
    let nw = fonts.text_width(note, 9.5);
    fonts.draw(
        cv,
        right - nw,
        title_base + p(20.0),
        note,
        9.5,
        if linked {
            pal().signal
        } else {
            pal().text_faint
        },
    );

    let div_y = margin + p(54.0);
    cv.hline(left, div_y, (right - left) as u32, pal().border);
    cv.fill_rect(
        left,
        div_y,
        p(34.0).max(1) as u32,
        p(2.0).max(1) as u32,
        pal().accent,
    );

    let body_top = div_y + p(10.0);
    let foot_y = margin + panel_h as i32 - p(12.0);
    let body_bottom = foot_y - p(18.0);
    let body_h = (body_bottom - body_top).max(p(80.0)) as u32;

    let rail_w = p(148.0);
    let rail_x = left;
    let pane_x = rail_x + rail_w + p(12.0);
    let pane_w = (right - pane_x).max(p(120.0)) as u32;

    let sections = menu_sections();
    let sel = menu.sel.min(sections.len().saturating_sub(1));
    let pitch = ((body_h as i32 - p(8.0)) / sections.len().max(1) as i32).min(p(44.0));
    let pitch = pitch.max(p(26.0));
    for (i, section) in sections.iter().enumerate() {
        let y = body_top + p(4.0) + i as i32 * pitch;
        let row_h = (pitch - p(8.0)).max(p(20.0)) as u32;
        let selected = i == sel;
        if selected {
            cv.fill_rect(rail_x, y, rail_w as u32, row_h, pal().bg3);
            let (base, peak) = if menu.focus_pane {
                (pal().border, pal().border)
            } else {
                (
                    pal().glow,
                    crate::theme::mix(pal().glow, pal().accent, 0.35),
                )
            };
            crate::theme::glow_border(cv, rail_x, y, rail_w as u32, row_h, base, peak, t, 0.35);
        }
        let num = format!("{:02}", i + 1);
        let base_y = y + row_h as i32 / 2 + p(4.0);
        fonts.draw(
            cv,
            rail_x + p(8.0),
            base_y,
            &num,
            8.5,
            if selected {
                pal().accent
            } else {
                pal().text_faint
            },
        );
        fonts.draw(
            cv,
            rail_x + p(28.0),
            base_y,
            section.label(),
            14.0,
            if selected {
                pal().accent
            } else {
                pal().text_dim
            },
        );
    }

    cv.fill_rect_a(pane_x, body_top, pane_w, body_h, pal().bg1, 232);
    if menu.focus_pane {
        crate::theme::glow_border(
            cv,
            pane_x,
            body_top,
            pane_w,
            body_h,
            pal().glow,
            crate::theme::mix(pal().glow, pal().accent, 0.35),
            t,
            0.5,
        );
    } else {
        cv.rect_outline(pane_x, body_top, pane_w, body_h, pal().border);
    }

    let section = sections[sel];
    draw_pane_header(cv, fonts, section, pane_x, body_top, pane_w);
    let content_x = pane_x + p(12.0);
    let content_y = body_top + p(52.0);
    let content_w = (pane_w as i32 - p(24.0)).max(p(80.0));
    let content_bottom = body_top + body_h as i32 - p(12.0);
    if section == MenuSection::States {
        draw_state_grid(
            cv,
            fonts,
            menu,
            content_x,
            content_y,
            content_w,
            content_bottom - content_y,
            t,
        );
    } else {
        draw_section_info(
            cv,
            fonts,
            menu,
            section,
            content_x,
            content_y,
            content_w,
            content_bottom,
            t,
        );
    }

    if let Some(f) = menu.hold {
        let bw = p(260.0);
        let x = w / 2 - bw / 2;
        let y = foot_y - p(4.0);
        cv.rect_outline(x, y, bw as u32, p(8.0) as u32, pal().border);
        let fw = ((bw - 2) as f32 * f.clamp(0.0, 1.0)) as u32;
        cv.fill_rect(
            x + 1,
            y + 1,
            fw,
            (p(8.0) as u32).saturating_sub(2),
            pal().signal,
        );
        fonts.draw_center(
            cv,
            w / 2,
            y - p(5.0),
            "HOLD MENU: SAVE + EXIT",
            9.0,
            pal().signal,
        );
    } else {
        let hint = if !menu.focus_pane {
            "U/D SECTION   A SELECT   RIGHT DETAILS   B RESUME"
        } else {
            match section {
                MenuSection::States => "A LOAD   X SAVE   Y DELETE   B BACK",
                MenuSection::Resume => "A RESUME   B BACK",
                MenuSection::Exit => "A EXIT GAME   B BACK",
                _ => "A OPEN   B BACK",
            }
        };
        fonts.draw(cv, left, foot_y, hint, 9.5, pal().text_faint);
        if let Some((msg, _)) = &menu.msg {
            let text = fonts.elide(msg, 9.5, (right - left) / 2);
            let mw = fonts.text_width(&text, 9.5);
            fonts.draw(cv, right - mw, foot_y, &text, 9.5, pal().signal);
        }
    }
}

fn draw_pane_header(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    section: MenuSection,
    x: i32,
    y: i32,
    w: u32,
) {
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let hx = x + p(12.0);
    let hy = y + p(22.0);
    cv.fill_rect(
        hx,
        hy - p(9.0),
        p(3.0).max(1) as u32,
        p(10.0).max(4) as u32,
        pal().accent,
    );
    fonts.draw(cv, hx + p(8.0), hy, section.title(), 15.0, pal().title);
    let summary = section_summary(section);
    let summary = fonts.elide(&summary, 10.0, w as i32 - p(28.0));
    fonts.draw(
        cv,
        hx + p(8.0),
        hy + p(15.0),
        &summary,
        10.0,
        pal().text_faint,
    );
}

fn draw_state_grid(
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
    let gap = p(8.0);
    let cw = (w - gap) / 2;
    let ch = (h - gap) / 2;
    if cw < p(40.0) || ch < p(40.0) {
        return;
    }
    for idx in 0..STATE_CARDS {
        let Some(card) = menu.states.get(idx) else {
            continue;
        };
        let cx = x + (idx as i32 % 2) * (cw + gap);
        let cy = y + (idx as i32 / 2) * (ch + gap);
        let selected = menu.focus_pane && menu.state_sel == idx;
        let armed = matches!(menu.delete_armed, Some((a, _)) if a == idx);
        cv.fill_rect(
            cx,
            cy,
            cw as u32,
            ch as u32,
            if card.exists { pal().bg2 } else { pal().bg1 },
        );
        if armed {
            cv.rect_outline(cx, cy, cw as u32, ch as u32, pal().alert);
        } else if selected {
            crate::theme::glow_border(
                cv,
                cx,
                cy,
                cw as u32,
                ch as u32,
                pal().accent,
                pal().glow,
                t,
                0.6,
            );
        } else {
            cv.rect_outline(cx, cy, cw as u32, ch as u32, pal().border);
        }

        let pad = p(4.0);
        let label_h = p(18.0);
        let tw = (cw - 2 * pad).max(p(16.0)) as u32;
        let th = (ch - label_h - 3 * pad).max(p(16.0)) as u32;
        cv.fill_rect(cx + pad, cy + pad, tw, th, pal().bg0);
        if let Some(img) = &card.thumb {
            img.draw_fit(cv, cx + pad, cy + pad, tw, th, 255);
        } else {
            let text = if card.exists { "NO PREVIEW" } else { "EMPTY" };
            fonts.draw_center(
                cv,
                cx + cw / 2,
                cy + pad + th as i32 / 2 + p(4.0),
                text,
                9.5,
                pal().text_faint,
            );
        }
        let base = cy + ch - p(6.0);
        let label_color = if card.auto {
            pal().signal
        } else if card.exists {
            pal().text
        } else {
            pal().text_faint
        };
        fonts.draw(cv, cx + pad, base, &card.label, 11.5, label_color);
        let age = if armed {
            "Y AGAIN".to_string()
        } else {
            card.age.clone()
        };
        if !age.is_empty() {
            let age = fonts.elide(&age, 10.0, cw / 2);
            let aw = fonts.text_width(&age, 10.0);
            fonts.draw(
                cv,
                cx + cw - pad - aw,
                base,
                &age,
                10.0,
                if armed { pal().alert } else { pal().text_faint },
            );
        }
    }
}

fn draw_section_info(
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
    let mut rows: Vec<(&str, String)> = Vec::new();
    match section {
        MenuSection::Resume => {
            rows.push(("GAME", game_title().to_uppercase()));
            rows.push(("SYSTEM", ctx().system.to_uppercase()));
            rows.push(("CORE", ctx().opt_key.to_uppercase()));
            rows.push(("SLOT", format!("{}/{}", menu.slot + 1, STATE_SLOTS)));
        }
        MenuSection::Core => {
            rows.push(("CORE", ctx().opt_key.to_uppercase()));
            rows.push(("VISIBLE OPTIONS", visible_options(ctx()).len().to_string()));
            rows.push(("STORAGE", "PER GAME, ON CARD".into()));
            rows.push(("RESTART", "SOME NEED A RELOAD".into()));
        }
        MenuSection::Video => {
            rows.push(("FPS COUNTER", if menu.fps { "ON" } else { "OFF" }.into()));
            rows.push(("SYSINFO", if menu.sysinfo { "ON" } else { "OFF" }.into()));
            rows.push((
                "CRT",
                overlay_pick(&ctx().overlay_layers, menu.overlay_idx[0], 0),
            ));
            rows.push((
                "BEZEL",
                overlay_pick(&ctx().overlay_layers, menu.overlay_idx[1], 1),
            ));
            rows.push(("SCALE", "SELECT + X".into()));
        }
        MenuSection::Link => {
            rows.push(("STATUS", netplay_status_line()));
            rows.push((
                "MODE",
                ctx()
                    .netplay
                    .as_ref()
                    .map(|s| s.transport().label().to_string())
                    .unwrap_or_else(|| "NONE".into()),
            ));
            rows.push(("GAME", game_title().to_uppercase()));
        }
        MenuSection::Disc => {
            rows.push((
                "DISC",
                format!(
                    "{}/{}",
                    disc_index().min(disc_count().saturating_sub(1)) + 1,
                    disc_count()
                ),
            ));
            rows.push(("GAME", game_title().to_uppercase()));
            rows.push(("SWAP", "NO RESET NEEDED".into()));
        }
        MenuSection::Exit => {
            rows.push(("SRAM", "FLUSHED ON EXIT".into()));
            rows.push((
                "RESUME POINT",
                if std::fs::metadata(&menu.auto_path).is_ok() {
                    "PRESENT"
                } else {
                    "NONE"
                }
                .into(),
            ));
            rows.push((
                "NETPLAY",
                if ctx().netplay.is_some() {
                    "WILL END"
                } else {
                    "IDLE"
                }
                .into(),
            ));
        }
        MenuSection::States => {}
    }
    draw_kv_rows(cv, fonts, x, y + p(2.0), w, &rows);

    let (cta, alert) = match section {
        MenuSection::Resume => ("A   RESUME", false),
        MenuSection::Core => ("A   OPEN CORE OPTIONS", false),
        MenuSection::Video => ("A   OPEN OVERLAYS", false),
        MenuSection::Link => ("A   OPEN NETPLAY", false),
        MenuSection::Disc => ("A   SWAP DISC", false),
        MenuSection::Exit => ("A   EXIT GAME", true),
        MenuSection::States => return,
    };
    draw_pane_cta(cv, fonts, x, bottom - p(34.0), w as u32, cta, t, alert);
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
    let row_h = p(22.0);
    for (i, (key, value)) in rows.iter().enumerate() {
        let by = y + i as i32 * row_h;
        fonts.draw(cv, x, by, key, 9.0, pal().text_faint);
        let value = fonts.elide(value, 11.0, w - p(80.0));
        let vw = fonts.text_width(&value, 11.0);
        fonts.draw(cv, x + w - vw, by, &value, 11.0, pal().text);
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
    let h = p(26.0).max(20) as u32;
    cv.fill_rect(x, y, w, h, pal().bg3);
    let accent = if alert { pal().alert } else { pal().accent };
    crate::theme::glow_border(cv, x, y, w, h, accent, pal().glow, t, 0.7);
    fonts.draw_center(
        cv,
        x + w as i32 / 2,
        y + h as i32 / 2 + p(4.0),
        text,
        11.0,
        accent,
    );
}

fn overlay_pick(files: &OverlayLayers, idx: usize, layer: usize) -> String {
    if idx == 0 {
        return "NONE".into();
    }
    files[layer]
        .get(idx - 1)
        .and_then(|s| Path::new(s).file_stem().and_then(|x| x.to_str()))
        .unwrap_or("NONE")
        .to_string()
}

/// The netplay page family. The session lives in `Ctx`; this only draws the
/// current step.
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

fn game_title() -> String {
    Path::new(&ctx().rom)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("GAME")
        .to_string()
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
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 216);
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
    fonts.draw_center(cv, w / 2, y0 + p(26.0), title, 16.0, pal().title);
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
            15.0,
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
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 216);
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
        16.0,
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
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 216);
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
    fonts.draw_center(cv, w / 2, y0 + p(30.0), title, 16.0, pal().title);
    fonts.draw_center(cv, w / 2, y0 + p(86.0), message, 13.0, pal().signal);
    fonts.draw_center(cv, w / 2, y0 + hh - p(26.0), footer, 9.0, pal().text_faint);
}

fn netplay_dots(t: f32) -> &'static str {
    ["", ".", "..", "..."][(t * 2.0) as usize % 4]
}

/// The DISC page: the images the core reported, current one marked.
pub(crate) fn draw_disc(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    sel: usize,
    msg: Option<&str>,
    count: usize,
    current: usize,
    t: f32,
) {
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let count = count.max(1);
    let current = current.min(count - 1);
    let ww = p(400.0).min(w - p(24.0));
    let header = p(104.0);
    let footer = if msg.is_some() { p(80.0) } else { p(44.0) };
    let pitch = ((h - p(12.0) - header - footer) / count as i32)
        .min(p(36.0))
        .max(p(24.0));
    let hh = header + count as i32 * pitch + footer;
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 216);
    crate::theme::glow_border(
        cv,
        x0,
        y0,
        ww as u32,
        hh as u32,
        pal().border,
        pal().glow,
        t,
        0.3,
    );
    fonts.draw_center(cv, w / 2, y0 + p(28.0), "MAGI // DISC", 16.0, pal().title);
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(56.0),
        &format!("DISC {} OF {}", current + 1, count),
        11.0,
        pal().text_dim,
    );
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(80.0),
        "PICK A DISC, THEN INSERT",
        9.0,
        pal().text_faint,
    );

    for i in 0..count {
        let c = y0 + header + i as i32 * pitch + pitch / 2;
        let y = c + p(5.5);
        let bh = (pitch - p(10.0)).min(p(34.0));
        let picked = i == sel.min(count - 1);
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
        let label = if i == current {
            format!("DISC {}   (IN)", i + 1)
        } else {
            format!("DISC {}", i + 1)
        };
        fonts.draw_center(
            cv,
            w / 2,
            y,
            &label,
            15.0,
            if picked { pal().accent } else { pal().text_dim },
        );
    }

    if let Some(msg) = msg {
        fonts.draw_center(cv, w / 2, y0 + hh - p(58.0), msg, 10.0, pal().signal);
    }
    fonts.draw_center(
        cv,
        w / 2,
        y0 + hh - p(24.0),
        "U/D PICK   A INSERT   B BACK",
        9.0,
        pal().text_faint,
    );
}

pub(crate) fn draw_options(cv: &mut Canvas, fonts: &mut Fonts, menu: &GameMenu, t: f32) {
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let options = &ctx().options;
    let ids = visible_options(ctx());
    let count = ids.len();
    let ww = p(500.0).min(w - p(24.0));
    let header = p(88.0);
    let pitch = ((h - p(12.0) - header - p(28.0)) / OPT_ROWS as i32)
        .min(p(36.0))
        .max(p(26.0));
    let hh = header + OPT_ROWS as i32 * pitch + p(28.0);
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 216);
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

    let title = "MAGI // CORE OPTIONS";
    fonts.draw_center(cv, w / 2, y0 + p(28.0), title, 16.0, pal().title);
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(52.0),
        &format!("{}/{}", (menu.opt_sel + 1).min(count), count),
        11.0,
        pal().text_faint,
    );

    for (row, vi) in ids.iter().skip(menu.opt_top).take(OPT_ROWS).enumerate() {
        let o = &options[*vi];
        let c = y0 + header + row as i32 * pitch + pitch / 2;
        let y = c + p(4.9);
        let bh = (pitch - p(10.0)).min(p(30.0));
        let sel = menu.opt_top + row == menu.opt_sel;
        if sel {
            cv.fill_rect(
                x0 + p(14.0),
                c - bh / 2,
                (ww - p(28.0)) as u32,
                bh as u32,
                pal().bg3,
            );
            crate::theme::glow_border(
                cv,
                x0 + p(14.0),
                c - bh / 2,
                (ww - p(28.0)) as u32,
                bh as u32,
                pal().glow,
                pal().accent,
                t,
                0.4,
            );
        }
        let text = if sel { pal().accent } else { pal().text_dim };
        fonts.draw(cv, x0 + p(24.0), y, clip(&o.desc, 30).as_str(), 13.5, text);
        let value = format!("< {} >", clip(&o.values[o.chosen], 18));
        let vw = fonts.text_width(&value, 13.5);
        fonts.draw(
            cv,
            x0 + ww - p(24.0) - vw,
            y,
            &value,
            13.5,
            if sel { pal().signal } else { text },
        );
    }

    fonts.draw_center(
        cv,
        w / 2,
        y0 + hh - p(22.0),
        "UP/DOWN PICK   LEFT/RIGHT CHANGE   B BACK   MENU CLOSE",
        9.0,
        pal().text_faint,
    );
}

pub(crate) fn draw_overlay_menu(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    menu: &GameMenu,
    files: &OverlayLayers,
    t: f32,
) {
    use crate::theme::pal;
    let (w, h) = (cv.w, cv.h);
    let s = crate::theme::ui_scale();
    let p = |v: f32| (v * s).round() as i32;
    let ww = p(500.0).min(w - p(24.0));
    let hh = p(106.0) + 4 * p(36.0) + p(28.0);
    let x0 = w / 2 - ww / 2;
    let y0 = h / 2 - hh / 2;
    cv.fill_rect_a(x0, y0, ww as u32, hh as u32, pal().bg0, 216);
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
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(28.0),
        "MAGI // OVERLAYS",
        16.0,
        pal().title,
    );
    fonts.draw_center(
        cv,
        w / 2,
        y0 + p(52.0),
        "OSD, THEN LOWEST LAYER FIRST",
        11.0,
        pal().text_faint,
    );

    // Same row geometry as the pause menu: 36 px pitch, 28 px inset, the
    // text baseline 5.5 px under the row centre.
    let rows = ["FPS", "SYS INFO", "CRT", "BEZEL"];
    let pitch = p(36.0);
    for (row, label) in rows.iter().enumerate() {
        let c = y0 + p(88.0) + row as i32 * pitch + pitch / 2;
        let baseline = c + p(5.5);
        let bh = (pitch - p(10.0)).min(p(36.0));
        let sel = row == menu.overlay_sel;
        if sel {
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
        let text = if sel { pal().accent } else { pal().text_dim };
        let label_w = fonts.text_width(label, 13.5);
        fonts.draw(cv, x0 + p(38.0), baseline, label, 13.5, text);
        let cell = |v: bool| if v { "ON" } else { "OFF" }.to_string();
        let max_w = (ww - p(38.0) * 2 - label_w - p(12.0)).max(p(40.0));
        let value = match row {
            0 => cell(menu.fps),
            1 => cell(menu.sysinfo),
            _ => match menu.overlay_idx[row - 2] {
                0 => "OFF".to_string(),
                k => {
                    let name = files
                        .get(row - 2)
                        .and_then(|l| l.get(k - 1))
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    fonts.elide(name, 13.5, max_w)
                }
            },
        };
        let vw = fonts.text_width(&value, 13.5);
        fonts.draw(
            cv,
            x0 + ww - p(38.0) - vw,
            baseline,
            &value,
            13.5,
            if sel { pal().signal } else { text },
        );
    }

    fonts.draw_center(
        cv,
        w / 2,
        y0 + hh - p(22.0),
        "UP/DOWN PICK   LEFT/RIGHT CHANGE   B BACK   MENU CLOSE",
        9.0,
        pal().text_faint,
    );
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
        overlay_sel: sel,
        overlay_idx: idx,
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
        state_sel: 0,
        state_paths: Vec::new(),
        auto_path: std::path::PathBuf::new(),
        delete_armed: None,
    };
    draw_overlay_menu(cv, fonts, &menu, files, t);
}

/// A small synthetic frame for the state-card previews.
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
    // A bright block so the preview has a clear subject.
    let bx = 40 + seed * 30;
    for y in 60..120 {
        for x in bx..(bx + 60).min(w) {
            px[(y * w + x) as usize] = 0xFFE0_8020;
        }
    }
    Image { w, h, px }
}

/// Render the pause dashboard with fixture data, for the headless check.
pub(crate) fn draw_menu_preview(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    sel: usize,
    focus_pane: bool,
    state_sel: usize,
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

    let option = CoreOption {
        key: "gambatte_gb_link_mode".into(),
        desc: "GB Link Mode".into(),
        values: vec!["Not Connected".into(), "Network Client".into()],
        c_values: vec![
            CString::new("Not Connected").unwrap(),
            CString::new("Network Client").unwrap(),
        ],
        chosen: 0,
        visible: true,
    };
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
        options: vec![option],
        opt_key: "gambatte".into(),
        opt_dirty: false,
        saved_opts: std::collections::HashMap::new(),
        overlay_layers: std::array::from_fn(|_| Vec::new()),
        overlay_dirty: false,
        moto: None,
        rumble_on: false,
        rumble_ok: false,
        disk: None,
        system: "gb".into(),
        rom: "Super Mario Land (World).gb".into(),
        netplay: None,
        netplay_linked: false,
    };
    let rom = fixture.rom.clone();
    let prev = CTX.swap(&mut fixture as *mut Ctx, Ordering::Relaxed);

    let slot = |n: usize| std::path::PathBuf::from(format!("Saves/preview.state{n}"));
    let states = vec![
        StateCard {
            path: slot(1),
            label: "SLOT 1".into(),
            age: "2H AGO".into(),
            thumb: Some(fixture_thumb(0)),
            exists: true,
            auto: false,
        },
        StateCard {
            path: slot(2),
            label: "SLOT 2".into(),
            age: String::new(),
            thumb: None,
            exists: false,
            auto: false,
        },
        StateCard {
            path: slot(3),
            label: "SLOT 3".into(),
            age: "3D AGO".into(),
            thumb: Some(fixture_thumb(2)),
            exists: true,
            auto: false,
        },
        StateCard {
            path: std::path::PathBuf::from("Saves/preview.stateauto"),
            label: "AUTO".into(),
            age: "5M AGO".into(),
            thumb: Some(fixture_thumb(4)),
            exists: true,
            auto: true,
        },
    ];
    let menu = GameMenu {
        open: true,
        sel,
        msg: None,
        exit_requested: false,
        page: MenuPage::Main,
        slot: 0,
        opt_sel: 0,
        opt_top: 0,
        overlay_sel: 0,
        overlay_idx: [1, 1],
        fps: true,
        sysinfo: false,
        hold: None,
        disc_sel: 0,
        netplay_page: NetplayPage::Menu,
        netplay_sel: 0,
        netplay_transport: netplay::Transport::Wifi,
        netplay_role: netplay::Role::Host,
        netplay_msg: None,
        focus_pane,
        states,
        state_sel,
        state_paths: vec![slot(1), slot(2), slot(3)],
        auto_path: std::path::PathBuf::from("Saves/preview.stateauto"),
        delete_armed: None,
    };
    draw_menu(cv, fonts, &menu, &rom, t);
    CTX.store(prev, Ordering::Relaxed);
}
