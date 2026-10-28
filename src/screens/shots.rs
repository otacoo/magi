//! The screenshot gallery (SETTINGS -> SYSTEM -> SCREENSHOTS).

use std::path::{Path, PathBuf};

use crate::canvas::Canvas;
use crate::font::Fonts;
use crate::image::Image;
use crate::theme::{self, pal};

const LIST_ROWS: usize = 10;
const LIST_W: i32 = 250;
const TOP: i32 = 96;

pub struct ShotsState {
    paths: Vec<PathBuf>,
    sel: usize,
    top: usize,
    zoom: bool,
    cache: Option<(usize, Image)>,
}

impl ShotsState {
    pub fn load() -> Self {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(crate::systems::DIR_SHOTS)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_image(p))
            .collect();
        // Names start with a sortable timestamp; newest first.
        paths.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        let mut st = Self {
            paths,
            sel: 0,
            top: 0,
            zoom: false,
            cache: None,
        };
        st.clamp();
        st
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.paths.is_empty() {
            return;
        }
        let n = self.paths.len() as isize;
        self.sel = (((self.sel as isize + delta) % n + n) % n) as usize;
        self.clamp();
    }

    pub fn toggle_zoom(&mut self) {
        if !self.paths.is_empty() {
            self.zoom = !self.zoom;
        }
    }

    pub fn zoom(&self) -> bool {
        self.zoom
    }

    fn clamp(&mut self) {
        if self.paths.is_empty() {
            self.sel = 0;
            self.top = 0;
            self.cache = None;
            return;
        }
        self.sel = self.sel.min(self.paths.len() - 1);
        if self.sel < self.top {
            self.top = self.sel;
        }
        if self.sel >= self.top + LIST_ROWS {
            self.top = self.sel + 1 - LIST_ROWS;
        }
    }

    fn ensure_image(&mut self) {
        if self.cache.as_ref().map(|(i, _)| *i) == Some(self.sel) {
            return;
        }
        let idx = self.sel;
        let img = self
            .paths
            .get(idx)
            .and_then(|p| crate::image::load_image(p));
        self.cache = img.map(|img| (idx, img));
    }
}

fn is_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("bmp" | "png")
    )
}

fn display_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("SCREENSHOT");
    let raw = stem.strip_prefix("magi_").unwrap_or(stem);
    let b = raw.as_bytes();
    if b.len() >= 15
        && b.get(8) == Some(&b'-')
        && raw.is_char_boundary(8)
        && raw.is_char_boundary(9)
        && raw.is_char_boundary(15)
    {
        let (date, time) = (&raw[..8], &raw[9..15]);
        if date.bytes().all(|c| c.is_ascii_digit()) && time.bytes().all(|c| c.is_ascii_digit()) {
            return format!(
                "{}-{}-{}  {}:{}:{}",
                &date[..4],
                &date[4..6],
                &date[6..8],
                &time[..2],
                &time[2..4],
                &time[4..6]
            );
        }
    }
    stem.to_uppercase()
}

fn kind(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "PNG",
        _ => "BMP",
    }
}

pub fn draw(cv: &mut Canvas, fonts: &mut Fonts, st: &mut ShotsState, t: f32) {
    st.ensure_image();
    if st.zoom {
        draw_zoom(cv, fonts, st);
    } else {
        draw_list(cv, fonts, st, t);
    }
}

fn draw_zoom(cv: &mut Canvas, fonts: &mut Fonts, st: &ShotsState) {
    let (w, h) = (cv.w, cv.h);
    cv.clear(pal().bg0);
    if st.paths.is_empty() {
        return;
    }
    match &st.cache {
        Some((_, img)) => img.draw_fit(cv, 0, 0, w as u32, h as u32, 255),
        None => {
            fonts.draw_center(
                cv,
                w / 2,
                h / 2,
                "PREVIEW UNAVAILABLE",
                12.0,
                pal().text_faint,
            );
        }
    }
    cv.fill_rect_a(0, 0, w as u32, 26, pal().bg0, 190);
    cv.fill_rect_a(0, h - 26, w as u32, 26, pal().bg0, 190);
    let path = &st.paths[st.sel];
    let name = display_name(path);
    fonts.draw(cv, 12, 18, &name, 10.0, pal().text);
    let pos = format!("{}/{}", st.sel + 1, st.paths.len());
    let pw = fonts.text_width(&pos, 10.0);
    fonts.draw(cv, w - 12 - pw, 18, &pos, 10.0, pal().text_dim);
    fonts.draw_center(
        cv,
        w / 2,
        h - 8,
        "L/R PREV/NEXT   A OR B CLOSE",
        9.0,
        pal().text_dim,
    );
}

fn draw_list(cv: &mut Canvas, fonts: &mut Fonts, st: &ShotsState, t: f32) {
    let (w, h) = (cv.w, cv.h);
    cv.clear(pal().bg0);
    theme::background(cv, 12, 64, (w - 32) as u32, (h - 124) as u32, t);
    theme::glow_border(
        cv,
        8,
        8,
        (w - 16) as u32,
        (h - 16) as u32,
        pal().border,
        pal().glow,
        t,
        0.0,
    );
    fonts.draw_center(cv, w / 2, 42, "MAGI // SCREENSHOTS", 15.0, pal().title);
    theme::hazard_bar(cv, w / 2 - 150, 56, 300, 5, t);

    if st.paths.is_empty() {
        fonts.draw_center(cv, w / 2, h / 2 - 8, "NO SCREENSHOTS", 15.0, pal().text_dim);
        fonts.draw_center(
            cv,
            w / 2,
            h / 2 + 16,
            "TAKE ONE WITH THE SCREENSHOT CHORD",
            9.0,
            pal().text_faint,
        );
        fonts.draw_center(cv, w / 2, h - 26, "B BACK", 9.0, pal().text_faint);
        return;
    }

    let count = format!("{} SHOTS   NEWEST FIRST", st.paths.len());
    fonts.draw_center(cv, w / 2, 80, &count, 9.0, pal().text_faint);

    let bottom = h - 62;
    let list_x = 24;
    let preview_x = list_x + LIST_W + 16;
    let preview_w = (w - 24 - preview_x) as u32;
    cv.fill_rect(list_x, TOP, LIST_W as u32, (bottom - TOP) as u32, pal().bg1);
    cv.rect_outline(
        list_x,
        TOP,
        LIST_W as u32,
        (bottom - TOP) as u32,
        pal().border,
    );
    cv.fill_rect_a(
        preview_x,
        TOP,
        preview_w,
        (bottom - TOP) as u32,
        pal().bg1,
        232,
    );
    cv.rect_outline(
        preview_x,
        TOP,
        preview_w,
        (bottom - TOP) as u32,
        pal().border,
    );

    let end = (st.top + LIST_ROWS).min(st.paths.len());
    for (row, idx) in (st.top..end).enumerate() {
        let path = &st.paths[idx];
        let y = TOP + 6 + row as i32 * 30;
        let selected = idx == st.sel;
        if selected {
            cv.fill_rect(list_x + 4, y, LIST_W as u32 - 8, 26, pal().bg3);
            theme::glow_border(
                cv,
                list_x + 4,
                y,
                LIST_W as u32 - 8,
                26,
                pal().glow,
                theme::mix(pal().glow, pal().accent, 0.35),
                t,
                0.4,
            );
        }
        let name = display_name(path);
        let name = fonts.elide(&name, 11.0, LIST_W - 60);
        fonts.draw(
            cv,
            list_x + 12,
            y + 18,
            &name,
            11.0,
            if selected {
                pal().accent
            } else {
                pal().text_dim
            },
        );
        let k = kind(path);
        let kw = fonts.text_width(k, 8.0);
        fonts.draw(
            cv,
            list_x + LIST_W - 10 - kw,
            y + 17,
            k,
            8.0,
            pal().text_faint,
        );
    }

    if let Some((_, img)) = &st.cache {
        let (px, py) = (preview_x + 8, TOP + 8);
        let (pw, ph) = (preview_w - 16, (bottom - TOP - 52) as u32);
        cv.fill_rect(px, py, pw, ph, pal().bg0);
        img.draw_fit(cv, px, py, pw, ph, 255);
    } else {
        fonts.draw_center(
            cv,
            preview_x + preview_w as i32 / 2,
            TOP + (bottom - TOP) / 2,
            "PREVIEW UNAVAILABLE",
            10.0,
            pal().text_faint,
        );
    }

    let path = &st.paths[st.sel];
    let name = display_name(path);
    fonts.draw(cv, preview_x, bottom - 26, &name, 11.0, pal().text);
    let meta = match &st.cache {
        Some((_, img)) => format!("{}x{}  {}", img.w, img.h, kind(path)),
        None => kind(path).to_string(),
    };
    let mw = fonts.text_width(&meta, 9.0);
    fonts.draw(
        cv,
        preview_x + preview_w as i32 - mw,
        bottom - 26,
        &meta,
        9.0,
        pal().text_faint,
    );
    fonts.draw_center(
        cv,
        w / 2,
        h - 26,
        "U/D PICK   L/R PREV/NEXT   A ZOOM   B BACK",
        9.0,
        pal().text_faint,
    );
}

/// Render the gallery with fixture data, for the headless check.
pub(crate) fn draw_preview(cv: &mut Canvas, fonts: &mut Fonts, zoom: bool, t: f32) {
    let (w, h) = (320u32, 240u32);
    let mut px = vec![0u32; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let sky = y < h * 2 / 3;
            let p = if sky {
                let v = 50 + x * 120 / w;
                0xFF00_0000 | (v << 16) | ((v / 3 + 50) << 8) | 110
            } else {
                let v = 40 + y * 50 / h;
                0xFF00_0000 | (v << 16) | ((v / 2) << 8) | 40
            };
            px[(y * w + x) as usize] = p;
        }
    }
    for y in 90..170 {
        for x in 120..200 {
            px[(y * w + x) as usize] = 0xFFE0_8020;
        }
    }
    let mut st = ShotsState {
        paths: vec![
            PathBuf::from("Screenshots/magi_20261027-140312.bmp"),
            PathBuf::from("Screenshots/magi_20261026-183000.bmp"),
            PathBuf::from("Screenshots/magi_20261020-091200.png"),
        ],
        sel: 0,
        top: 0,
        zoom,
        cache: Some((0, Image { w, h, px })),
    };
    draw(cv, fonts, &mut st, t);
}

#[cfg(test)]
mod tests {
    use super::{display_name, kind};
    use std::path::Path;

    #[test]
    fn timestamp_names_are_readable() {
        assert_eq!(
            display_name(Path::new("Screenshots/magi_20261027-140312.bmp")),
            "2026-10-27  14:03:12"
        );
        assert_eq!(
            display_name(Path::new("Screenshots/other shot.png")),
            "OTHER SHOT"
        );
    }

    #[test]
    fn formats_are_reported() {
        assert_eq!(kind(Path::new("a.BMP")), "BMP");
        assert_eq!(kind(Path::new("a.png")), "PNG");
    }

    #[test]
    fn selection_wraps_and_scrolls() {
        let mut st = super::ShotsState {
            paths: (0..12)
                .map(|i| std::path::PathBuf::from(format!("{i}.bmp")))
                .collect(),
            sel: 0,
            top: 0,
            zoom: false,
            cache: None,
        };
        st.move_by(-1);
        assert_eq!(st.sel, 11);
        assert_eq!(st.top, 2);
        st.move_by(1);
        assert_eq!(st.sel, 0);
        assert_eq!(st.top, 0);
        st.move_by(3);
        assert_eq!(st.sel, 3);
        assert_eq!(st.top, 0);
    }
}
