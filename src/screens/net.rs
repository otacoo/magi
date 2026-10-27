//! The network pages: index, Wi-Fi, Bluetooth, SSH, NTP and the hostname.

use crate::canvas::Canvas;
use crate::font::Fonts;
use crate::theme::{self, pal};
use crate::util::{self, WifiNet};

/// Shared chrome: frame, title plate, hazard divider.
fn chrome(cv: &mut Canvas, fonts: &mut Fonts, title: &str, t: f32) -> (i32, i32) {
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
    (w, h)
}

/// One settings-style row with an optional right value; returns the next y.
fn row(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    x0: i32,
    y: i32,
    row_w: i32,
    label: &str,
    value: Option<&str>,
    selected: bool,
    t: f32,
) -> i32 {
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
    if let Some(value) = value {
        let vw = fonts.text_width(value, 11.0);
        fonts.draw(
            cv,
            x0 + row_w - vw,
            y,
            value,
            11.0,
            if selected { pal().signal } else { pal().text },
        );
    }
    y + 30
}

fn onoff(on: bool) -> String {
    format!("< {} >", if on { "ON" } else { "OFF" })
}

/// NETWORK index: WIFI, BLUETOOTH, SSH, NTP pages and the hostname.
pub const INDEX_ROWS: [&str; 5] = ["WIFI", "BLUETOOTH", "SSH", "NTP", "HOSTNAME"];

pub fn draw_index(cv: &mut Canvas, fonts: &mut Fonts, sel: usize, hostname: &str, t: f32) {
    let (w, h) = chrome(cv, fonts, "NETWORK", t);
    let x0 = w / 2 - 230;
    let row_w = 460;
    let mut y = 108;
    for (i, label) in INDEX_ROWS.iter().enumerate() {
        let value = match *label {
            "HOSTNAME" => hostname.to_string(),
            _ => ">".to_string(),
        };
        y = row(cv, fonts, x0, y, row_w, label, Some(&value), i == sel, t);
    }
    crate::status::hint_or_notice(cv, fonts, h - 26, "A OPEN   B BACK");
}

/// WIFI page: state line, scan results with signal/lock/state.
pub const WIFI_TOGGLE: &str = "WIFI";

pub fn draw_wifi(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    on: bool,
    state: &str,
    nets: &[WifiNet],
    joining: Option<&str>,
    sel: usize,
    t: f32,
) {
    let (w, h) = chrome(cv, fonts, "WIFI", t);
    let x0 = w / 2 - 230;
    let row_w = 460;

    // The toggle is row 0, the network rows follow it.
    let mut y = row(
        cv,
        fonts,
        x0,
        y_start(),
        row_w,
        WIFI_TOGGLE,
        Some(&onoff(on)),
        sel == 0,
        t,
    );

    if !on {
        fonts.draw_center(
            cv,
            w / 2,
            y + 30,
            "WIFI IS OFF - PRESS A TO ENABLE",
            11.0,
            pal().text_faint,
        );
        crate::status::hint_or_notice(cv, fonts, h - 26, "A TOGGLE   B BACK");
        return;
    }
    fonts.draw(cv, x0, y, state, 9.0, pal().text_faint);
    y += 24;

    if nets.is_empty() {
        fonts.draw_center(
            cv,
            w / 2,
            y + 30,
            "NO NETWORKS - X TO SCAN",
            11.0,
            pal().text_faint,
        );
    }
    let mut draw_y = y;
    for (i, net) in nets.iter().enumerate() {
        if draw_y > h - 44 {
            break;
        }
        let selected = i + 1 == sel;
        if selected {
            cv.fill_rect(x0 - 8, draw_y - 14, row_w as u32 + 16, 22, pal().bg3);
            theme::glow_border(
                cv,
                x0 - 8,
                draw_y - 14,
                row_w as u32 + 16,
                22,
                pal().glow,
                pal().accent,
                t,
                0.4,
            );
        }
        let color = if selected {
            pal().accent
        } else {
            pal().text_dim
        };
        theme::signal_bars(
            cv,
            x0 + 2,
            draw_y - 10,
            net.signal,
            if selected {
                pal().text
            } else {
                pal().text_faint
            },
        );
        if net.secured {
            theme::lock_icon(
                cv,
                x0 + 26,
                draw_y - 10,
                if selected {
                    pal().text
                } else {
                    pal().text_faint
                },
            );
        }
        let name = clip(&net.ssid, 26);
        fonts.draw(cv, x0 + 42, draw_y, &name, 11.0, color);
        let state_text = if joining.is_some_and(|j| j == net.ssid) {
            "JOINING"
        } else if net.current {
            "CONNECTED"
        } else if net.saved {
            "SAVED"
        } else {
            ""
        };
        if !state_text.is_empty() {
            let col = match state_text {
                "CONNECTED" => pal().signal,
                "JOINING" => pal().accent,
                _ => pal().text_faint,
            };
            let tw = fonts.text_width(state_text, 10.0);
            fonts.draw(cv, x0 + row_w - tw, draw_y, state_text, 10.0, col);
        }
        draw_y += 30;
    }

    crate::status::hint_or_notice(
        cv,
        fonts,
        h - 26,
        "A TOGGLE/CONNECT   X RESCAN   Y FORGET   B BACK",
    );
}

fn y_start() -> i32 {
    108
}

/// BLUETOOTH page: controller toggle and the known device list.
pub const BT_TOGGLE: &str = "BLUETOOTH";

pub fn draw_bt(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    on: bool,
    devices: &[(String, String)],
    connected: &[String],
    sel: usize,
    t: f32,
) {
    let (w, h) = chrome(cv, fonts, "BLUETOOTH", t);
    let x0 = w / 2 - 230;
    let row_w = 460;

    let mut y = row(
        cv,
        fonts,
        x0,
        y_start(),
        row_w,
        BT_TOGGLE,
        Some(&onoff(on)),
        sel == 0,
        t,
    );
    if !on {
        fonts.draw_center(
            cv,
            w / 2,
            y + 30,
            "BLUETOOTH IS OFF - PRESS A TO ENABLE",
            11.0,
            pal().text_faint,
        );
        crate::status::hint_or_notice(cv, fonts, h - 26, "A TOGGLE   B BACK");
        return;
    }
    if devices.is_empty() {
        fonts.draw_center(
            cv,
            w / 2,
            y + 30,
            "NO DEVICES - X TO SCAN",
            11.0,
            pal().text_faint,
        );
    }
    for (i, (mac, name)) in devices.iter().enumerate() {
        if y > h - 44 {
            break;
        }
        let selected = i + 1 == sel;
        if selected {
            cv.fill_rect(x0 - 8, y - 14, row_w as u32 + 16, 22, pal().bg3);
            theme::glow_border(
                cv,
                x0 - 8,
                y - 14,
                row_w as u32 + 16,
                22,
                pal().glow,
                pal().accent,
                t,
                0.4,
            );
        }
        theme::bt_icon(
            cv,
            x0 + 2,
            y - 12,
            if selected {
                pal().text
            } else {
                pal().text_faint
            },
        );
        let shown = clip(name, 22);
        fonts.draw(
            cv,
            x0 + 18,
            y,
            &shown,
            11.0,
            if selected {
                pal().accent
            } else {
                pal().text_dim
            },
        );
        let nw = fonts.text_width(&shown, 11.0);
        fonts.draw(cv, x0 + 26 + nw, y, mac, 8.0, pal().text_faint);
        if connected.iter().any(|m| m == mac) {
            let label = "CONNECTED";
            let lw = fonts.text_width(label, 10.0);
            fonts.draw(cv, x0 + row_w - lw, y, label, 10.0, pal().signal);
        }
        y += 30;
    }

    crate::status::hint_or_notice(
        cv,
        fonts,
        h - 26,
        "A TOGGLE/CONNECT   X SCAN   Y FORGET   B BACK",
    );
}

/// SSH page: one toggle plus status rows.
pub fn draw_toggle_page(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    title: &str,
    toggle_label: &str,
    on: bool,
    status: &[(String, String)],
    sel: usize,
    t: f32,
) {
    let (w, h) = chrome(cv, fonts, title, t);
    let x0 = w / 2 - 230;
    let row_w = 460;
    let mut y = row(
        cv,
        fonts,
        x0,
        y_start(),
        row_w,
        toggle_label,
        Some(&onoff(on)),
        sel == 0,
        t,
    );
    y += 8;
    for (label, value) in status {
        fonts.draw(cv, x0, y, label, 11.0, pal().text_dim);
        let vw = fonts.text_width(value, 11.0);
        fonts.draw(cv, x0 + row_w - vw, y, value, 11.0, pal().text);
        y += 30;
    }
    crate::status::hint_or_notice(cv, fonts, h - 26, "A TOGGLE   B BACK");
}

/// NTP page: the toggle plus an editable server row and sync state.
pub fn draw_ntp(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    on: bool,
    server: &str,
    status: &[(String, String)],
    sel: usize,
    t: f32,
) {
    let (w, h) = chrome(cv, fonts, "NTP", t);
    let x0 = w / 2 - 230;
    let row_w = 460;
    let mut y = row(
        cv,
        fonts,
        x0,
        y_start(),
        row_w,
        "NTP",
        Some(&onoff(on)),
        sel == 0,
        t,
    );
    let server_label = if server.is_empty() {
        "< POOL.NTP.ORG >".to_string()
    } else {
        format!("< {} >", clip(server, 30))
    };
    y = row(
        cv,
        fonts,
        x0,
        y,
        row_w,
        "SERVER",
        Some(&server_label),
        sel == 1,
        t,
    );
    y += 8;
    for (label, value) in status {
        fonts.draw(cv, x0, y, label, 11.0, pal().text_dim);
        let vw = fonts.text_width(value, 11.0);
        fonts.draw(cv, x0 + row_w - vw, y, value, 11.0, pal().text);
        y += 30;
    }
    crate::status::hint_or_notice(cv, fonts, h - 26, "U/D PICK   A TOGGLE/EDIT   B BACK");
}

pub fn ssh_status_lines() -> Vec<(String, String)> {
    let host = util::hostname_get().unwrap_or_else(|| "--".into());
    vec![
        (
            "SERVICE".into(),
            if util::ssh_on() {
                "RUNNING".into()
            } else {
                "STOPPED".into()
            },
        ),
        ("USER".into(), "root".into()),
        ("HOSTNAME".into(), host),
        ("PORT".into(), "22".into()),
    ]
}

pub fn ntp_status_lines(server: &str) -> Vec<(String, String)> {
    let now = util::clock_hhmm().unwrap_or_else(|| "--:--".into());
    vec![
        (
            "SYNCHRONIZED".into(),
            if util::ntp_synced() {
                "YES".into()
            } else {
                "NO".into()
            },
        ),
        ("TIME".into(), now),
        (
            "USING".into(),
            if server.is_empty() {
                "BASEOS DEFAULT POOL".into()
            } else {
                server.to_string()
            },
        ),
    ]
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
