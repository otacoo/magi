//! The on-screen keyboard: passphrases and the hostname.

use crate::canvas::Canvas;
use crate::font::Fonts;
use crate::theme::{self, pal};

pub const ROWS: [&str; 5] = [
    "1234567890",
    "qwertyuiop",
    "asdfghjkl.",
    "zxcvbnm-_@",
    "SHIFT SPACE DEL CANCEL DONE",
];

/// What the text is for when Done is pressed.
#[derive(Clone, PartialEq)]
pub enum Target {
    WifiPass(String),
    Hostname,
    NtpServer,
    /// Rename the game at this path (the keyboard is pre-filled with its
    /// current stem).
    Rename(String),
}

pub struct KeyboardState {
    pub title: String,
    text: String,
    /// Insertion point, in characters.
    cursor: usize,
    pub target: Target,
    pub row: usize,
    pub col: usize,
    pub caps: bool,
}

impl KeyboardState {
    pub fn new(title: &str, target: Target) -> Self {
        Self {
            title: title.to_string(),
            text: String::new(),
            cursor: 0,
            target,
            row: 0,
            col: 0,
            caps: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.chars().count();
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn cursor_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn cursor_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.chars().count());
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len())
    }

    fn insert(&mut self, c: char) {
        let at = self.byte_at(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        let at = self.byte_at(self.cursor);
        self.text.remove(at);
    }

    fn row_len(&self, row: usize) -> usize {
        match row {
            4 => 5,
            r => ROWS[r].chars().count(),
        }
    }

    pub fn up(&mut self) {
        self.row = (self.row + 4) % 5;
        self.col = self.col.min(self.row_len(self.row) - 1);
    }

    pub fn down(&mut self) {
        self.row = (self.row + 1) % 5;
        self.col = self.col.min(self.row_len(self.row) - 1);
    }

    pub fn left(&mut self) {
        let n = self.row_len(self.row);
        self.col = (self.col + n - 1) % n;
    }

    pub fn right(&mut self) {
        let n = self.row_len(self.row);
        self.col = (self.col + 1) % n;
    }

    /// The character or action under the cursor.
    fn selected(&self) -> String {
        if self.row == 4 {
            ["SHIFT", "SPACE", "DEL", "CANCEL", "DONE"][self.col].to_string()
        } else {
            let c = ROWS[self.row].chars().nth(self.col).unwrap_or(' ');
            self.label(c).to_string()
        }
    }

    /// The shifted layer: digits become the usual symbols, the three punctuation
    /// keys in the bottom letter row become `+`, `=` and `&`, and `.` becomes
    /// `:`. Letters become capitals.
    fn label(&self, c: char) -> char {
        if !self.caps {
            return c;
        }
        match c {
            '1' => '!',
            '2' => '@',
            '3' => '#',
            '4' => '$',
            '5' => '%',
            '6' => '^',
            '7' => '&',
            '8' => '*',
            '9' => '(',
            '0' => ')',
            '.' => ':',
            '-' => '+',
            '_' => '=',
            '@' => '&',
            c if c.is_ascii_lowercase() => c.to_ascii_uppercase(),
            c => c,
        }
    }

    pub fn toggle_shift(&mut self) {
        self.caps = !self.caps;
    }

    /// Act on the current key. `None` = still editing; `Some(false)` =
    /// cancelled; `Some(true)` = committed (the caller reads `text`).
    pub fn activate(&mut self) -> Option<bool> {
        match self.selected().as_str() {
            "SHIFT" => {
                self.toggle_shift();
                None
            }
            "SPACE" => {
                self.insert(' ');
                None
            }
            "DEL" => {
                self.backspace();
                None
            }
            "CANCEL" => Some(false),
            "DONE" => Some(true),
            c => {
                if let Some(ch) = c.chars().next() {
                    self.insert(ch);
                }
                None
            }
        }
    }
}

pub fn draw(cv: &mut Canvas, fonts: &mut Fonts, st: &KeyboardState, t: f32) {
    let (w, h) = (cv.w, cv.h);
    cv.clear(pal().bg0);
    theme::glow_border(
        cv,
        8,
        8,
        (w - 16) as u32,
        (h - 16) as u32,
        pal().border,
        pal().glow,
        t,
        0.2,
    );
    fonts.draw_center(cv, w / 2, 40, &st.title, 14.0, pal().title);

    // The buffer, with the insertion point kept in view.
    let bw = w - 120;
    cv.fill_rect(60, 60, bw as u32, 34, pal().bg2);
    cv.rect_outline(60, 60, bw as u32, 34, pal().border);
    let chars: Vec<char> = st.text().chars().collect();
    let total = chars.len();
    let cursor = st.cursor().min(total);
    let window = 44usize;
    let start = cursor
        .saturating_sub(window / 2)
        .min(total.saturating_sub(window));
    let end = (start + window).min(total);
    let shown: String = chars[start..end].iter().collect();
    let before: String = chars[start..cursor].iter().collect();
    fonts.draw(cv, 70, 84, &shown, 13.0, pal().text);
    let cx = 70 + fonts.text_width(&before, 13.0);
    cv.fill_rect(cx, 69, 2, 18, pal().signal);

    // The key grid.
    let cell_w = 52;
    let cell_h = 44;
    let gx = (w - cell_w * 10) / 2;
    let gy = 120;
    for (r, line) in ROWS.iter().enumerate() {
        if r == 4 {
            for (i, item) in ["SHIFT", "SPACE", "DEL", "CANCEL", "DONE"]
                .iter()
                .enumerate()
            {
                let x = gx + 10 + i as i32 * 100;
                let y = gy + r as i32 * cell_h;
                let selected = st.row == r && st.col == i;
                let base = if *item == "CANCEL" || *item == "DONE" {
                    pal().text_faint
                } else {
                    pal().bg3
                };
                cv.fill_rect(x, y, 92, 36, pal().bg2);
                if selected {
                    // Same ring as the letter keys: the action buttons had a
                    // dark base colour and barely showed the cursor.
                    theme::glow_border(
                        cv,
                        x,
                        y,
                        92,
                        36,
                        pal().glow,
                        pal().accent,
                        t,
                        0.1 * i as f32,
                    );
                } else {
                    cv.rect_outline(x, y, 92, 36, base);
                }
                let label = if *item == "SHIFT" && st.caps {
                    "SHIFT*"
                } else {
                    item
                };
                let lw = fonts.text_width(label, 10.0);
                fonts.draw(
                    cv,
                    x + (92 - lw) / 2,
                    y + 25,
                    label,
                    10.0,
                    if selected {
                        pal().accent
                    } else {
                        pal().text_dim
                    },
                );
            }
            continue;
        }
        for (c, ch) in line.chars().enumerate() {
            let x = gx + c as i32 * cell_w;
            let y = gy + r as i32 * cell_h;
            let selected = st.row == r && st.col == c;
            let shown = st.label(ch);
            cv.fill_rect(x, y, (cell_w - 4) as u32, 36, pal().bg2);
            if selected {
                theme::glow_border(
                    cv,
                    x,
                    y,
                    (cell_w - 4) as u32,
                    36,
                    pal().glow,
                    pal().accent,
                    t,
                    0.1 * c as f32,
                );
            }
            let cw = fonts.text_width(&shown.to_string(), 13.0);
            fonts.draw(
                cv,
                x + (cell_w - 4 - cw) / 2,
                y + 26,
                &shown.to_string(),
                13.0,
                if selected { pal().accent } else { pal().text },
            );
        }
    }

    fonts.draw_center(
        cv,
        w / 2,
        h - 16,
        "U/D/L/R PICK   L/R CURSOR   X SHIFT   A TYPE   START DONE   B CANCEL",
        9.0,
        pal().text_faint,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_layer_has_punctuation() {
        let mut kb = KeyboardState::new("TEST", Target::Hostname);
        kb.toggle_shift();
        assert_eq!(kb.label('1'), '!');
        assert_eq!(kb.label('7'), '&');
        assert_eq!(kb.label('.'), ':');
        assert_eq!(kb.label('-'), '+');
        assert_eq!(kb.label('a'), 'A');

        kb.row = 2;
        kb.col = 9;
        kb.activate();
        assert_eq!(kb.text(), ":");
        kb.row = 0;
        kb.col = 0;
        kb.activate();
        assert_eq!(kb.text(), ":!");
    }

    #[test]
    fn edits_at_the_cursor() {
        let mut kb = KeyboardState::new("TEST", Target::Hostname);
        kb.set_text("abcd");
        assert_eq!(kb.cursor(), 4);
        kb.cursor_left();
        kb.cursor_left();
        kb.insert('X');
        assert_eq!(kb.text(), "abXcd");
        kb.backspace();
        assert_eq!(kb.text(), "abcd");
        assert_eq!(kb.cursor(), 2);
        kb.cursor_right();
        kb.cursor_right();
        assert_eq!(kb.cursor(), 4);
        kb.cursor_right();
        assert_eq!(kb.cursor(), 4);
    }
}
