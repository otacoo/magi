//! The one transient notice line: any screen calls `show`, and the shelf's
//! bottom bar or the sub-page hint line displays it. Messages never own a
//! screen's state, so errors from anywhere land in the same place.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::canvas::Canvas;
use crate::font::Fonts;
use crate::theme::pal;

const HOLD: Duration = Duration::from_millis(2200);
const FADE: Duration = Duration::from_millis(350);

static NOTICE: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// Show `msg` for a moment, and record it in the process log.
pub fn show(msg: &str) {
    crate::util::log(&format!("status: {msg}"));
    if let Ok(mut notice) = NOTICE.lock() {
        *notice = Some((msg.to_string(), Instant::now()));
    }
}

/// The live notice and its opacity; an expired one is dropped here.
fn live() -> Option<(String, u8)> {
    let mut notice = NOTICE.lock().ok()?;
    let (msg, t0) = notice.as_ref()?;
    let age = t0.elapsed();
    if age >= HOLD {
        *notice = None;
        return None;
    }
    let alpha = if age > HOLD - FADE {
        ((HOLD - age).as_millis() * 255 / FADE.as_millis()) as u8
    } else {
        255
    };
    Some((msg.clone(), alpha))
}

/// Whether something is on the line, so the shelf keeps repainting until it
/// fades.
pub fn is_active() -> bool {
    live().is_some()
}

/// The shelf's bottom bar: draws between `left` and `right` and returns the
/// pen position after the text.
pub fn draw_bar(cv: &mut Canvas, fonts: &mut Fonts, left: i32, right: i32, y: i32) -> i32 {
    let Some((msg, alpha)) = live() else {
        return left;
    };
    let max_w = right - left - 10;
    if max_w < 40 {
        return left;
    }
    let msg = fonts.elide(&msg, 9.5, max_w);
    fonts.draw_alpha(cv, left + 10, y, &msg, 9.5, pal().text, alpha);
    left + 10 + fonts.text_width(&msg, 9.5)
}

/// Centered at `y` on a sub-page.
pub fn draw_center(cv: &mut Canvas, fonts: &mut Fonts, y: i32) {
    let Some((msg, alpha)) = live() else {
        return;
    };
    let msg = fonts.elide(&msg, 11.0, cv.w - 40);
    let tw = fonts.text_width(&msg, 11.0);
    fonts.draw_alpha(cv, (cv.w - tw) / 2, y, &msg, 11.0, pal().text, alpha);
}

/// The hint line of a sub-page, which a notice takes over while it is up.
pub fn hint_or_notice(cv: &mut Canvas, fonts: &mut Fonts, y: i32, hint: &str) {
    if live().is_some() {
        draw_center(cv, fonts, y);
    } else {
        let cx = cv.w / 2;
        fonts.draw_center(cv, cx, y, hint, 9.0, pal().text_faint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_expires() {
        NOTICE
            .lock()
            .unwrap()
            .replace(("HELLO".into(), Instant::now()));
        assert!(is_active());
        live();
        // Rewind past the hold, then the next read drops it.
        NOTICE
            .lock()
            .unwrap()
            .replace(("OLD".into(), Instant::now() - HOLD - FADE));
        assert!(!is_active());
        assert!(NOTICE.lock().unwrap().is_none());
    }
}
