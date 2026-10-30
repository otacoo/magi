//! Full-screen SD card alert and repair confirmation.

use crate::canvas::Canvas;
use crate::font::Fonts;
use crate::theme::{self, pal};

pub enum CardView {
    Notice { repair: bool },
    Confirm,
    Repairing,
}

pub fn draw(cv: &mut Canvas, fonts: &mut Fonts, t: f32, view: CardView) {
    let (w, h) = (cv.w, cv.h);
    cv.clear(pal().bg0);
    theme::background(cv, 12, 62, (w - 32) as u32, (h - 124) as u32, t);
    theme::glow_border(
        cv,
        8,
        8,
        (w - 16) as u32,
        (h - 16) as u32,
        pal().alert,
        pal().glow,
        t,
        0.0,
    );
    fonts.draw_center(cv, w / 2, 42, "MAGI // STORAGE ALERT", 15.0, pal().title);
    theme::hazard_bar(cv, w / 2 - 150, 54, 300, 5, t);

    match view {
        CardView::Notice { repair } => {
            fonts.draw_center(cv, w / 2, 124, "SD CARD IS READ-ONLY", 20.0, pal().alert);
            fonts.draw_center(
                cv,
                w / 2,
                158,
                "SAVES, STATES AND SETTINGS CANNOT BE WRITTEN.",
                11.0,
                pal().text,
            );
            fonts.draw_center(
                cv,
                w / 2,
                182,
                "THE CARD WAS REMOUNTED READ-ONLY AFTER A FILESYSTEM ERROR.",
                10.0,
                pal().text_dim,
            );
            if repair {
                fonts.draw_center(
                    cv,
                    w / 2,
                    258,
                    "SELECT+START   REPAIR CARD",
                    12.0,
                    pal().accent,
                );
                fonts.draw_center(
                    cv,
                    w / 2,
                    286,
                    "THE CARD IS UNMOUNTED, CHECKED AND THE DEVICE REBOOTS.",
                    9.0,
                    pal().text_faint,
                );
            } else {
                fonts.draw_center(cv, w / 2, 258, "REPAIR NEEDS A COMPUTER", 12.0, pal().alert);
                fonts.draw_center(
                    cv,
                    w / 2,
                    286,
                    "RUN CHKDSK OR FSCK ON THE CARD, THEN RESTART.",
                    9.0,
                    pal().text_faint,
                );
            }
            fonts.draw_center(
                cv,
                w / 2,
                h - 64,
                "B   CONTINUE WITHOUT SAVING",
                10.0,
                pal().text_dim,
            );
        }
        CardView::Confirm => {
            fonts.draw_center(cv, w / 2, 124, "REPAIR SD CARD?", 20.0, pal().alert);
            fonts.draw_center(
                cv,
                w / 2,
                158,
                "THE CARD IS UNMOUNTED AND CHECKED, THEN THE DEVICE REBOOTS.",
                11.0,
                pal().text,
            );
            fonts.draw_center(
                cv,
                w / 2,
                188,
                "BACK UP IMPORTANT FILES FIRST.",
                10.0,
                pal().text_dim,
            );
            fonts.draw_center(
                cv,
                w / 2,
                210,
                "THE REPAIR MAY MOVE OR DROP DAMAGED DATA.",
                10.0,
                pal().alert,
            );
            fonts.draw_center(cv, w / 2, 280, "A   REPAIR", 12.0, pal().accent);
            fonts.draw_center(cv, w / 2, 310, "B   CANCEL", 12.0, pal().text_dim);
        }
        CardView::Repairing => {
            fonts.draw_center(cv, w / 2, 140, "REPAIRING SD CARD", 20.0, pal().accent);
            theme::hazard_bar(cv, w / 2 - 180, h / 2 + 6, 360, 6, t);
            fonts.draw_center(cv, w / 2, h / 2 + 44, "DO NOT POWER OFF", 12.0, pal().alert);
            fonts.draw_center(
                cv,
                w / 2,
                h - 64,
                "THE DEVICE REBOOTS WHEN THE CHECK FINISHES.",
                10.0,
                pal().text_faint,
            );
        }
    }
}
