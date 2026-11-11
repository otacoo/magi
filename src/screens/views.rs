//! Shelf list layouts. The shelf hands over the rows and the geometry; the
//! look lives here, so another interface is one more function.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::canvas::Canvas;
use crate::font::Fonts;
use crate::games::GameEntry;
use crate::image::Image;
use crate::theme::{pal, Color};

/// The interfaces offered in SHELF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Simple,
    Art,
}

pub const LABELS: [&str; 2] = ["SIMPLE", "ART"];

impl View {
    pub fn from_pref(value: u8) -> Self {
        if value == 1 {
            View::Art
        } else {
            View::Simple
        }
    }
}

/// One screenful of games plus the geometry the shelf gives it.
pub struct List<'a> {
    pub games: &'a [GameEntry],
    pub sel: usize,
    pub first: usize,
    pub rows: usize,
    pub favorites: &'a HashSet<String>,
    /// Left edge of the row text.
    pub x0: i32,
    /// Right edge of the selection box.
    pub right: i32,
    /// Baseline of the first row.
    pub baseline: i32,
    pub row_h: i32,
    /// Seconds the selected row has been held, for the title marquee.
    pub sel_hold: f32,
}

/// `Games/<CONSOLE>/…` → the console folder.
fn console_folder(game_path: &str) -> Option<&str> {
    let mut parts = game_path.split('/');
    if parts.next()? != "Games" {
        return None;
    }
    parts.next().filter(|s| !s.is_empty())
}

/// The name a cover is filed under: the ROM's stem, or the game folder for a
/// native port (`Games/N64/<Title>/rom.z64`).
fn cover_stem(game_path: &str) -> Option<&str> {
    let mut parts = game_path.split('/').skip(2);
    let first = parts.next()?;
    if parts.next().is_some() {
        Some(first)
    } else {
        Path::new(first).file_stem()?.to_str()
    }
}

/// The file for one art type. Boxart is the base name; snaps, titles and
/// logos add a suffix.
pub fn cover_path_for(game_path: &str, art_type: u8) -> Option<PathBuf> {
    let suffix = ["", ".snap", ".title", ".logo"].get(art_type as usize)?;
    Some(
        Path::new("Covers")
            .join(console_folder(game_path)?)
            .join(format!("{}{suffix}.png", cover_stem(game_path)?)),
    )
}

/// Where each part of the shelf sits. SIMPLE keeps the system rail on the
/// left; ART moves the systems into a strip along the top, which frees the
/// full width for the list and the cover.
pub struct Layout {
    /// System list area: (x, y, w, h).
    pub systems: (i32, i32, i32, i32),
    /// System name and folder lines, and the first row of the game list.
    pub header_y: i32,
    pub subtitle_y: i32,
    pub list_top: i32,
    /// Game list columns.
    pub list_x: i32,
    pub list_right: i32,
    /// Cover frame (ART only): (x, y, w, h).
    pub art: Option<(i32, i32, i32, i32)>,
}

pub fn layout(view: View, w: i32, h: i32, top: i32, bottom: i32, scale: f32) -> Layout {
    let p = |v: f32| (v * scale).round() as i32;
    let rail_w = p(150.0);
    match view {
        View::Simple => Layout {
            systems: (0, top + 1, rail_w, h - bottom - top - 1),
            header_y: 70,
            subtitle_y: 94,
            list_top: 118,
            list_x: rail_w + 20,
            list_right: w - 12,
            art: None,
        },
        View::Art => {
            let strip_h = p(26.0);
            let strip_y = top + 1;
            // No system heading in ART: the path line sits right under the
            // strip so the game list gets the extra height.
            let subtitle_y = strip_y + strip_h + p(20.0);
            let list_top = subtitle_y + p(14.0);
            // The cover frame starts right under the strip, leaving the same
            // margin at the top as above the bottom bar.
            let art_y = strip_y + strip_h + p(8.0);
            let art_h = (h - bottom - 8) - art_y;
            // Box art is portrait; the frame keeps that ratio and grows with
            // the pane.
            let art_w = art_h * 3 / 4;
            let art_x = w - p(14.0) - art_w;
            Layout {
                systems: (0, strip_y, w, strip_h),
                header_y: subtitle_y,
                subtitle_y,
                list_top,
                list_x: p(18.0),
                list_right: art_x - p(16.0),
                art: Some((art_x, art_y, art_w, art_h)),
            }
        }
    }
}

/// The system list: a rail down the left, or a strip along the top. The
/// backing band and its divider belong to the interface, not the shelf.
pub fn draw_systems(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    view: View,
    names: &[&str],
    sel: usize,
    area: (i32, i32, i32, i32),
    scale: f32,
    t: f32,
) {
    let (_, y, w, h) = area;
    match view {
        View::Simple => {
            cv.fill_rect(0, y, w as u32, h as u32, pal().bg1);
            cv.vline(
                w,
                y,
                h as u32,
                crate::theme::glow_color(pal().border, pal().glow, t, 0.45),
            );
            draw_rail(cv, fonts, names, sel, area, scale, t);
        }
        View::Art => {
            cv.fill_rect(0, y, w as u32, h as u32, pal().bg1);
            cv.hline(
                0,
                y + h,
                w as u32,
                crate::theme::glow_color(pal().border, pal().glow, t, STRIP_GLOW_PHASE),
            );
            draw_strip(cv, fonts, names, sel, area, scale, t);
        }
    }
}

fn draw_rail(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    names: &[&str],
    sel: usize,
    area: (i32, i32, i32, i32),
    scale: f32,
    t: f32,
) {
    let p = |v: f32| (v * scale).round() as i32;
    let (_, top, rail_w, height) = area;
    let item_h = p(30.0);
    let y0 = top + p(20.0);
    let max_rows = (((height - p(20.0) - p(8.0)) / item_h).max(1)) as usize;
    let first = rail_first(names.len(), sel, max_rows);
    for (i, name) in names.iter().enumerate().skip(first).take(max_rows) {
        let y = y0 + (i - first) as i32 * item_h;
        let idx = format!("{:02}", i + 1);
        let iw = fonts.text_width(&idx, 9.0);
        let (label, size) = crate::screens::library::fit_title(
            fonts,
            name,
            11.0,
            9.0,
            rail_w - p(14.0) - iw - p(20.0),
        );
        if i == sel {
            let box_h = p(24.0);
            cv.fill_rect(4, y - p(16.0), (rail_w - 9) as u32, box_h as u32, pal().bg3);
            crate::theme::glow_border(
                cv,
                4,
                y - p(16.0),
                (rail_w - 9) as u32,
                box_h as u32,
                pal().glow,
                crate::theme::mix(pal().glow, pal().accent, 0.35),
                t,
                0.0,
            );
            fonts.draw(cv, p(14.0), y, &label, size, pal().accent);
        } else {
            fonts.draw(cv, p(14.0), y, &label, size, pal().text_dim);
        }
        fonts.draw(cv, rail_w - 10 - iw, y, &idx, 9.0, pal().text_faint);
    }
    if names.len() > max_rows {
        let track_top = y0 - p(12.0);
        let track_h = max_rows as i32 * item_h;
        cv.fill_rect(
            rail_w - 4,
            track_top,
            2,
            track_h as u32,
            pal().scrollbar_track,
        );
        let thumb_h = (track_h * max_rows as i32 / names.len() as i32).max(p(8.0));
        let span = (track_h - thumb_h).max(1);
        let pos = first as i32 * span / (names.len() - max_rows).max(1) as i32;
        cv.fill_rect(
            rail_w - 4,
            track_top + pos,
            2,
            thumb_h as u32,
            pal().scrollbar,
        );
    }
}

fn rail_first(len: usize, sel: usize, max_rows: usize) -> usize {
    if len <= max_rows {
        0
    } else {
        sel.saturating_sub(max_rows.saturating_sub(1))
            .min(len - max_rows)
    }
}

/// How many systems the strip shows at once.
const STRIP_CHIPS: usize = 5;

/// The strip divider and its selected tab share a phase so they breathe
/// together instead of glowing at their own pace.
const STRIP_GLOW_PHASE: f32 = 0.45;

fn draw_strip(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    names: &[&str],
    sel: usize,
    area: (i32, i32, i32, i32),
    scale: f32,
    t: f32,
) {
    if names.is_empty() {
        return;
    }
    let p = |v: f32| (v * scale).round() as i32;
    let (x, y, w, h) = area;
    let first = (sel / STRIP_CHIPS) * STRIP_CHIPS;
    let visible: Vec<&str> = names
        .iter()
        .skip(first)
        .take(STRIP_CHIPS)
        .copied()
        .collect();
    let has_prev = first > 0;
    let has_next = first + visible.len() < names.len();
    // The left gutter only exists while there is a page to go back to; the
    // right one is always kept so the tabs do not jump on the last page.
    // Both leave room for the selected tab's padding and the page triangle.
    let left_gutter = if has_prev { p(30.0) } else { 0 };
    let right_gutter = p(32.0);
    let avail = (w - left_gutter - right_gutter).max(p(80.0));
    // One size for every tab: the largest at which the longest name still
    // leaves a little air between the labels.
    let min_gap = p(6.0);
    let max_gap = p(26.0);
    let mut size = 10.0;
    while size > 8.0 {
        let total: i32 = visible.iter().map(|n| fonts.text_width(n, size)).sum();
        if total + min_gap * (visible.len() as i32 + 1) <= avail {
            break;
        }
        size -= 0.5;
    }
    let labels: Vec<String> = visible
        .iter()
        .map(|n| crate::screens::library::elide(fonts, n, size, avail / 2))
        .collect();
    let widths: Vec<i32> = labels.iter().map(|l| fonts.text_width(l, size)).collect();
    let total: i32 = widths.iter().sum();
    let n = labels.len() as i32;
    // Equal gaps, capped, with the whole row centred: tabs line up the same
    // way whether two systems or five are on the page.
    let gap = if n > 1 {
        ((avail - total) / (n + 1)).clamp(min_gap, max_gap)
    } else {
        0
    };
    let group = total + gap * (n - 1).max(0);
    let baseline = y + h / 2 + 4;
    let start = x + left_gutter + (avail - group) / 2;

    // The selected title is a folder tab: its top replaces the header rule
    // and its sides run through the strip divider into the content below.
    let title = crate::theme::glow_color(pal().accent, pal().glow, t, STRIP_GLOW_PHASE);
    if let Some(i) = (0..labels.len()).find(|&i| first + i == sel) {
        let pad = if n > 1 {
            p(10.0).min((gap / 2).max(p(4.0)))
        } else {
            p(10.0)
        };
        let before: i32 = widths[..i].iter().sum::<i32>() + gap * i as i32;
        let tab_w = widths[i] + pad * 2;
        let tab_x = (start + before - pad).min(x + w - tab_w).max(x);
        let tab_y = y - 1;
        let tab_h = h + 2;
        let edge = crate::theme::glow_color(pal().border, pal().glow, t, STRIP_GLOW_PHASE);
        cv.fill_rect(tab_x, tab_y, tab_w as u32, tab_h as u32, pal().bg1);
        cv.fill_rect(tab_x, tab_y, tab_w as u32, p(3.0).max(2) as u32, title);
        cv.vline(tab_x, tab_y, tab_h as u32, edge);
        cv.vline(tab_x + tab_w - 1, tab_y, tab_h as u32, edge);
    }

    let mut tx = start;
    for (i, label) in labels.iter().enumerate() {
        if first + i == sel {
            fonts.draw_bold(cv, tx, baseline, label, size, title);
        } else {
            fonts.draw(cv, tx, baseline, label, size, pal().text_dim);
        }
        tx += widths[i] + gap;
    }

    // A page triangle replaces the old 04/07 counter.
    let tri_h = p(10.0).max(7);
    let tri_w = (tri_h * 4 / 5).max(6);
    let tri_y = y + (h - tri_h) / 2;
    if has_prev {
        triangle(cv, x + p(6.0), tri_y, tri_h, tri_w, false, pal().text_dim);
    }
    if has_next {
        triangle(
            cv,
            x + w - p(10.0) - tri_w,
            tri_y,
            tri_h,
            tri_w,
            true,
            pal().text_dim,
        );
    }
}

/// A small filled page triangle, pointing right (or left when `right` is
/// false). Supersampled so the short diagonal does not look stepped.
fn triangle(cv: &mut Canvas, x: i32, y: i32, h: i32, w: i32, right: bool, c: Color) {
    if h <= 0 || w <= 0 {
        return;
    }
    let (x0, y0, x1, y1) = (x as f32, y as f32, (x + w) as f32, (y + h) as f32);
    let (ax, ay, bx, by, cx, cy) = if right {
        (x0, y0, x0, y1, x1, y0 + h as f32 / 2.0)
    } else {
        (x1, y0, x1, y1, x0, y0 + h as f32 / 2.0)
    };
    const S: i32 = 8;
    let samples = (S * S) as u32;
    for py in y..y + h {
        for px in x..x + w {
            let mut hits = 0u32;
            for sy in 0..S {
                for sx in 0..S {
                    let fx = px as f32 + (sx as f32 + 0.5) / S as f32;
                    let fy = py as f32 + (sy as f32 + 0.5) / S as f32;
                    if in_triangle(fx, fy, ax, ay, bx, by, cx, cy) {
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                cv.blend(px, py, c, (hits * 255 / samples) as u8);
            }
        }
    }
}

fn in_triangle(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32) -> bool {
    let d1 = (px - bx) * (ay - by) - (ax - bx) * (py - by);
    let d2 = (px - cx) * (by - cy) - (bx - cx) * (py - cy);
    let d3 = (px - ax) * (cy - ay) - (cx - ax) * (py - ay);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// Horizontal offset for a selected title that does not fit: none until the
/// two-second hold, then one slow scroll with a pause at the end. It stops
/// after that pass, so a still shelf can go back to its idle tick.
pub(crate) fn marquee_offset(full_w: i32, avail: i32, hold: f32) -> Option<i32> {
    let overflow = full_w - avail;
    if overflow <= 0 || hold < 2.0 {
        return None;
    }
    const SPEED: f32 = 30.0;
    const PAUSE: f32 = 1.5;
    let phase = hold - 2.0;
    if phase >= overflow as f32 / SPEED + PAUSE {
        return None;
    }
    Some((phase * SPEED).min(overflow as f32) as i32)
}

fn draw_rows(cv: &mut Canvas, fonts: &mut Fonts, list: &List, right: i32, t: f32) -> bool {
    let box_w = (right - list.x0 + 8).max(40) as u32;
    let last = (list.first + list.rows).min(list.games.len());
    let mut scrolling = false;
    for (row, gi) in (list.first..last).enumerate() {
        let g = &list.games[gi];
        let y = list.baseline + row as i32 * list.row_h;
        let selected = gi == list.sel;
        if selected {
            cv.fill_rect(list.x0 - 6, y - 18, box_w, 24, pal().bg3);
            crate::theme::glow_border(
                cv,
                list.x0 - 6,
                y - 18,
                box_w,
                24,
                pal().glow,
                crate::theme::mix(pal().glow, pal().accent, 0.35),
                t,
                0.5,
            );
        }
        let mut tx = list.x0 + 4;
        if list.favorites.contains(&g.path) {
            fonts.draw(cv, tx, y, "*", 12.0, pal().signal);
            tx += 14;
        }
        let color = if selected {
            pal().accent
        } else {
            pal().text_dim
        };
        let avail = (right - tx - 14).max(24);
        let full_w = if selected {
            fonts.text_width(&g.title, 12.0)
        } else {
            0
        };
        let mut marquee = false;
        let mut shifted = None;
        if selected && full_w > avail {
            if list.sel_hold < 2.0 {
                // Keep the loop at the animation rate so the scroll starts
                // exactly when the hold expires.
                marquee = true;
            } else if let Some(off) = marquee_offset(full_w, avail, list.sel_hold) {
                marquee = true;
                shifted = Some(off);
            }
        }
        if let Some(off) = shifted {
            fonts.draw_clipped(cv, tx - off, y, &g.title, 12.0, color, tx, tx + avail);
        } else {
            let title = crate::screens::library::elide(fonts, &g.title, 12.0, avail);
            fonts.draw(cv, tx, y, &title, 12.0, color);
        }
        scrolling |= marquee;
    }
    scrolling
}

/// Text rows only. Returns true while a title is scrolling.
pub fn draw_simple(cv: &mut Canvas, fonts: &mut Fonts, list: &List, t: f32) -> bool {
    draw_rows(cv, fonts, list, list.right, t)
}

/// Text rows plus the selected game's cover in `art`. `card` draws the plate
/// and border behind it; without one the cover floats on a drop shadow.
/// Returns true while a title is scrolling.
pub fn draw_art(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    list: &List,
    art: (i32, i32, i32, i32),
    cover: Option<&Image>,
    card: bool,
    t: f32,
) -> bool {
    let (x, y, w, h) = art;
    let scrolling = draw_rows(cv, fonts, list, list.right, t);

    if card {
        cv.fill_rect(x, y, w as u32, h as u32, pal().bg2);
        cv.rect_outline(x, y, w as u32, h as u32, pal().border);
    }
    match cover {
        Some(img) if img.w > 0 && img.h > 0 => {
            let inset = if card { 5 } else { 9 };
            let (bw, bh) = (w - inset * 2, h - inset * 2);
            let ratio = (bw as f32 / img.w as f32).min(bh as f32 / img.h as f32);
            let sw = ((img.w as f32 * ratio) as u32).max(1);
            let sh = ((img.h as f32 * ratio) as u32).max(1);
            let ix = x + (w - sw as i32) / 2;
            let iy = y + (h - sh as i32) / 2;
            if !card {
                drop_shadow(cv, ix, iy, sw as i32, sh as i32);
            }
            let scaled = img.scaled(sw, sh);
            scaled.draw(cv, ix, iy, 255);
        }
        _ => {
            let cx = x + w / 2;
            let cy = y + h / 2;
            fonts.draw_center(cv, cx, cy, "NO ART", 10.0, pal().text_faint);
            let name = list
                .games
                .get(list.sel)
                .map(|g| g.title.clone())
                .unwrap_or_default();
            let name = crate::screens::library::elide(fonts, &name, 9.0, w - 20);
            fonts.draw_center(cv, cx, cy + 20, &name, 9.0, pal().text_dim);
        }
    }
    scrolling
}

/// A soft shadow under a floating cover: translucent rings widen the falloff
/// without a blur pass or blending the area the cover covers anyway.
fn drop_shadow(cv: &mut Canvas, x: i32, y: i32, w: i32, h: i32) {
    let black = crate::theme::Color::rgb(0, 0, 0);
    for i in (0..8).rev() {
        let (sx, sy) = (x + 6 - i, y + 8 - i);
        let (sw, sh) = (w + i * 2, h + i * 2);
        let a = 10 + (7 - i) as u8 * 4;
        cv.fill_rect_a(sx, sy, sw as u32, 1, black, a);
        cv.fill_rect_a(sx, sy + sh - 1, sw as u32, 1, black, a);
        cv.fill_rect_a(sx, sy, 1, sh as u32, black, a);
        cv.fill_rect_a(sx + sw - 1, sy, 1, sh as u32, black, a);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_sit_beside_the_rom() {
        assert_eq!(
            cover_path_for("Games/GBA/Zelda (USA).gba", 0).as_deref(),
            Some(Path::new("Covers/GBA/Zelda (USA).png"))
        );
        assert_eq!(
            cover_path_for("Games/PS1/Game.Disc 1.bin", 0).as_deref(),
            Some(Path::new("Covers/PS1/Game.Disc 1.png"))
        );
        assert_eq!(
            cover_path_for("Games/PS1/Final Fantasy VII.m3u", 0).as_deref(),
            Some(Path::new("Covers/PS1/Final Fantasy VII.png"))
        );
        assert_eq!(cover_path_for("loose", 0), None);
    }

    #[test]
    fn the_system_rail_scrolls_to_keep_the_selection_visible() {
        assert_eq!(rail_first(5, 0, 10), 0);
        assert_eq!(rail_first(12, 0, 10), 0);
        assert_eq!(rail_first(12, 9, 10), 0);
        assert_eq!(rail_first(12, 10, 10), 1);
        assert_eq!(rail_first(12, 11, 10), 2);
        assert_eq!(rail_first(12, 11, 1), 11);
    }

    #[test]
    fn long_titles_hold_then_scroll() {
        assert_eq!(marquee_offset(100, 100, 9.0), None);
        assert_eq!(marquee_offset(160, 100, 1.9), None);
        assert_eq!(marquee_offset(160, 100, 2.0), Some(0));
        let off = marquee_offset(160, 100, 3.0).expect("scrolling");
        assert!(off > 0 && off < 60, "offset {off}");
        // The pass ends after the scroll and its pause, then the row reverts
        // to the elided title and the shelf can idle again.
        assert_eq!(marquee_offset(160, 100, 5.4), Some(60));
        assert_eq!(marquee_offset(160, 100, 5.5), None);
    }

    #[test]
    fn art_types_have_their_own_files() {
        assert_eq!(
            cover_path_for("Games/GB/Tetris.gb", 0).as_deref(),
            Some(Path::new("Covers/GB/Tetris.png"))
        );
        assert_eq!(
            cover_path_for("Games/GB/Tetris.gb", 1).as_deref(),
            Some(Path::new("Covers/GB/Tetris.snap.png"))
        );
        assert_eq!(
            cover_path_for("Games/GB/Tetris.gb", 2).as_deref(),
            Some(Path::new("Covers/GB/Tetris.title.png"))
        );
        assert_eq!(
            cover_path_for("Games/GB/Tetris.gb", 3).as_deref(),
            Some(Path::new("Covers/GB/Tetris.logo.png"))
        );
    }
}
