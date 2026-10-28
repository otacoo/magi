//! Headless render check used by CI and by `cargo run -- --selftest`.
//! Renders key frames, verifies pixel content, writes PPMs for eyeballing.

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};

use crate::canvas::Canvas;
use crate::device;
use crate::font::{self, Fonts};
use crate::screens::{boot, library};
use crate::theme::pal;

pub fn run() -> Result<()> {
    // Render at the fresh-card text size, which is what the console runs.
    crate::theme::set_ui_scale(crate::defaults::Defaults::TEXT_SCALE);
    let dev = device::detect();
    let mut cv = Canvas::new(dev.width, dev.height);
    let mut fonts = Fonts::new(font::FONT_DATA)?;

    let out = Path::new("target/selftest");
    fs::create_dir_all(out)?;

    // Boot splash: the GEHIRN ignition pulse, then the activated roles.
    boot::draw(&mut cv, &mut fonts, 0.7);
    let s = stats(&cv);
    write_ppm(&out.join("boot.ppm"), &cv)?;
    println!(
        "boot:    accent={} warm={} signal={} nonbg={} ({:.1}%)",
        s.accent,
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.warm < 300 {
        bail!("boot ignition looks wrong (warm {})", s.warm);
    }
    boot::draw(&mut cv, &mut fonts, 4.5);
    let s = stats(&cv);
    write_ppm(&out.join("boot-on.ppm"), &cv)?;
    println!(
        "boot-on: accent={} warm={} signal={} nonbg={} ({:.1}%)",
        s.accent,
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.signal < 100 {
        bail!("boot role lights look wrong (signal {})", s.signal);
    }

    // Library, first frame, with a small fake game list.
    let mut lib = library::LibraryState::load(Path::new("."));
    // The fresh-card default is now Art; render the Simple shelf first.
    lib.prefs.interface = 0;
    lib.battery = Some(87);
    lib.clock = Some("19:07".into());
    lib.games.insert(
        "gb".into(),
        vec![
            crate::games::GameEntry {
                path: "Games/GB/Game 2.gb".into(),
                title: "GAME 2".into(),
            },
            crate::games::GameEntry {
                path: "Games/GB/SUPER MARIO LAND (USA).gb".into(),
                title: "SUPER MARIO LAND".into(),
            },
            crate::games::GameEntry {
                path: "Games/GB/Tetris.gb".into(),
                title: "TETRIS".into(),
            },
            crate::games::GameEntry {
                path: "Games/GB/THE LEGEND OF ZELDA - LINK'S AWAKENING.gb".into(),
                title: "THE LEGEND OF ZELDA - LINK'S AWAKENING".into(),
            },
        ],
    );
    lib.favorites.insert("Games/GB/Tetris.gb".into());
    lib.refresh_favorites();
    library::draw(&mut cv, &mut fonts, &mut lib, 0.0);
    let s = stats(&cv);
    write_ppm(&out.join("library.ppm"), &cv)?;
    println!(
        "library: warm={} signal={} nonbg={} ({:.1}%)",
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.warm < 300 || s.signal < 50 {
        bail!(
            "library frame looks wrong (warm {}, signal {})",
            s.warm,
            s.signal
        );
    }
    if lib.catalogue.systems.is_empty() {
        bail!("no systems in catalogue");
    }

    // The CONTROLS panel.
    lib.controls_open = true;
    library::draw(&mut cv, &mut fonts, &mut lib, 0.0);
    let s = stats(&cv);
    write_ppm(&out.join("controls.ppm"), &cv)?;
    println!(
        "controls: warm={} signal={} nonbg={} ({:.1}%)",
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.warm < 300 {
        bail!("controls panel looks wrong (warm {})", s.warm);
    }

    // The same list with the ART interface. No covers exist on a dev machine,
    // so this exercises the placeholder and the narrower list. A few more
    // systems populate the top strip.
    lib.controls_open = false;
    for (id, path, title) in [
        ("gba", "Games/GBA/Metroid Fusion.gba", "METROID FUSION"),
        ("nes", "Games/NES/Super Mario Bros.nes", "SUPER MARIO BROS."),
        ("snes", "Games/SNES/Super Metroid.sfc", "SUPER METROID"),
        (
            "psx",
            "Games/PS1/Final Fantasy VII.cue",
            "FINAL FANTASY VII",
        ),
        ("genesis", "Games/GENESIS/Sonic.md", "SONIC THE HEDGEHOG"),
        ("n64", "Games/N64/Mario/rom.z64", "SUPER MARIO 64"),
    ] {
        lib.games.insert(
            id.into(),
            vec![crate::games::GameEntry {
                path: path.into(),
                title: title.into(),
            }],
        );
    }
    lib.prefs.interface = 1;
    // The default config shows the backdrop on the shelf; check it against
    // the strip and the cover panel.
    crate::status::show("ART 12 SAVED");
    crate::theme::set_shelf_bg(true);
    library::draw(&mut cv, &mut fonts, &mut lib, 0.0);
    crate::theme::set_shelf_bg(false);
    let s = stats(&cv);
    write_ppm(&out.join("library-art.ppm"), &cv)?;
    println!(
        "library-art: warm={} signal={} nonbg={} ({:.1}%)",
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.warm < 300 || s.nonbg < 2000 {
        bail!(
            "art interface looks blank (warm {}, nonbg {})",
            s.warm,
            s.nonbg
        );
    }

    // The FAVORITES tab: the virtual system gathering starred games.
    lib.move_system(-1);
    library::draw(&mut cv, &mut fonts, &mut lib, 0.0);
    let s = stats(&cv);
    write_ppm(&out.join("library-favorites.ppm"), &cv)?;
    println!(
        "favorites: warm={} signal={} nonbg={} ({:.1}%)",
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.warm < 300 || s.nonbg < 2000 {
        bail!(
            "favorites tab looks blank (warm {}, nonbg {})",
            s.warm,
            s.nonbg
        );
    }
    lib.move_system(1);
    lib.prefs.interface = 0;

    // On-screen keyboard, cursor on the action row.
    let mut kb = crate::screens::keyboard::KeyboardState::new(
        "HOSTNAME",
        crate::screens::keyboard::Target::Hostname,
    );
    kb.text = "magi".into();
    kb.row = 4;
    kb.col = 0;
    crate::screens::keyboard::draw(&mut cv, &mut fonts, &kb, 0.6);
    let s = stats(&cv);
    write_ppm(&out.join("keyboard.ppm"), &cv)?;
    println!(
        "keyboard: warm={} signal={} nonbg={} ({:.1}%)",
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.warm < 200 {
        bail!("keyboard frame looks wrong (warm {})", s.warm);
    }
    // The action row must show the cursor: its top-left corner differs from
    // an unselected button's.
    let gx = (cv.w - 52 * 10) / 2;
    let ky = 120 + 4 * 44;
    let cursor = cv.px[(ky * cv.w + gx + 10) as usize];
    let plain = cv.px[(ky * cv.w + gx + 110) as usize];
    if cursor == plain {
        bail!("keyboard action row shows no cursor");
    }

    // The Y menu, CORE tab: per-system art type.
    library::draw(&mut cv, &mut fonts, &mut lib, 0.0);
    library::draw_game_menu(&mut cv, &mut fonts, "SUPER MARIO 64", 1, 0, false, 4, 0.4);
    let s = stats(&cv);
    write_ppm(&out.join("game-menu.ppm"), &cv)?;
    println!(
        "game menu: warm={} signal={} nonbg={} ({:.1}%)",
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.warm < 100 {
        bail!("game menu looks wrong (warm {})", s.warm);
    }

    // The pause dashboard: state preview, core options, video and disc panes.
    for (name, sel, focus) in [
        ("pause-resume", 0, false),
        ("pause-save", 1, false),
        ("pause-core", 3, true),
        ("pause-video", 4, true),
        ("pause-disc", 6, false),
        ("pause-exit", 7, false),
    ] {
        library::draw(&mut cv, &mut fonts, &mut lib, 0.0);
        crate::run::draw_menu_preview(&mut cv, &mut fonts, sel, focus, 0.4);
        let s = stats(&cv);
        write_ppm(&out.join(format!("{name}.ppm")), &cv)?;
        println!(
            "{name}: warm={} signal={} nonbg={} ({:.1}%)",
            s.warm,
            s.signal,
            s.nonbg,
            pct(&cv, &s)
        );
        if s.warm < 100 || s.nonbg < 5000 {
            bail!("{name} looks wrong (warm {}, nonbg {})", s.warm, s.nonbg);
        }
    }

    // Screenshot gallery: the list, then the full-screen zoom.
    crate::screens::shots::draw_preview(&mut cv, &mut fonts, false, 0.4);
    let s = stats(&cv);
    write_ppm(&out.join("shots.ppm"), &cv)?;
    println!(
        "shots:   warm={} signal={} nonbg={} ({:.1}%)",
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.warm < 100 || s.nonbg < 5000 {
        bail!(
            "screenshot gallery looks wrong (warm {}, nonbg {})",
            s.warm,
            s.nonbg
        );
    }

    crate::screens::shots::draw_preview(&mut cv, &mut fonts, true, 0.4);
    let s = stats(&cv);
    write_ppm(&out.join("shots-zoom.ppm"), &cv)?;
    println!(
        "shots2:  warm={} signal={} nonbg={} ({:.1}%)",
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.nonbg < 10_000 {
        bail!("screenshot zoom looks blank (nonbg {})", s.nonbg);
    }

    // Settings and the clock wizard.
    let mut settings = crate::screens::settings::SettingsState::default();
    crate::screens::settings::draw(&mut cv, &mut fonts, &settings, 0.4);
    let s = stats(&cv);
    write_ppm(&out.join("settings.ppm"), &cv)?;
    println!(
        "settings: warm={} signal={} nonbg={} ({:.1}%)",
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.warm < 200 {
        bail!("settings screen looks wrong (warm {})", s.warm);
    }

    // The THEME sub-page.
    settings.page = crate::screens::settings::Page::Theme;
    settings.sel = 0;
    crate::screens::settings::draw(&mut cv, &mut fonts, &settings, 0.4);
    let s = stats(&cv);
    write_ppm(&out.join("theme.ppm"), &cv)?;
    println!(
        "theme:   warm={} signal={} nonbg={} ({:.1}%)",
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.warm < 100 {
        bail!("theme page looks wrong (warm {})", s.warm);
    }

    // The SHELF sub-page: interface and art options.
    settings.page = crate::screens::settings::Page::Shelf;
    settings.sel = 0;
    crate::screens::settings::draw(&mut cv, &mut fonts, &settings, 0.4);
    let s = stats(&cv);
    write_ppm(&out.join("shelf.ppm"), &cv)?;
    println!(
        "shelf:   warm={} signal={} nonbg={} ({:.1}%)",
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.warm < 100 {
        bail!("shelf page looks wrong (warm {})", s.warm);
    }
    settings.page = crate::screens::settings::Page::Main;

    settings.clock = Some(crate::screens::settings::ClockEdit {
        v: [2026, 10, 17, 14, 3],
        field: 2,
    });
    let clock = settings.clock.unwrap();
    println!("clock stamp: {}", clock.stamp());
    crate::screens::settings::draw_clock(&mut cv, &mut fonts, &clock, true, 0.4);
    let s = stats(&cv);
    write_ppm(&out.join("clock.ppm"), &cv)?;
    println!(
        "clock: warm={} signal={} nonbg={} ({:.1}%)",
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.warm < 200 {
        bail!("clock wizard looks wrong (warm {})", s.warm);
    }

    // The shutdown countdown (POWER hold).
    crate::theme::draw_shutdown(
        &mut cv,
        &mut fonts,
        Duration::from_millis(3231),
        Duration::from_secs(5),
        0.7,
    );
    let s = stats(&cv);
    write_ppm(&out.join("shutdown.ppm"), &cv)?;
    println!(
        "shutdown: warm={} alert={} nonbg={} ({:.1}%)",
        s.warm,
        s.alert,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.warm < 200 {
        bail!("shutdown countdown looks wrong (warm {})", s.warm);
    }

    // Read-only info screens.
    let lines = vec![
        ("MODEL".to_string(), "ANBERNIC RG35XXSP".to_string()),
        ("FIRMWARE".to_string(), "BaseOS 1.4.0".to_string()),
        ("KERNEL".to_string(), "5.15.147".to_string()),
        ("MEMORY".to_string(), "1024 MB".to_string()),
        (
            "STORAGE".to_string(),
            "12000 MB free / 29000 MB".to_string(),
        ),
    ];
    crate::screens::info::draw(&mut cv, &mut fonts, "ABOUT", &lines, &[], 0, 0.4);
    let s = stats(&cv);
    write_ppm(&out.join("info.ppm"), &cv)?;
    println!(
        "info:    warm={} signal={} nonbg={} ({:.1}%)",
        s.warm,
        s.signal,
        s.nonbg,
        pct(&cv, &s)
    );
    if s.warm < 200 {
        bail!("info screen looks wrong (warm {})", s.warm);
    }

    println!("selftest ok ({} {}x{})", dev.id, dev.width, dev.height);
    Ok(())
}

struct Stats {
    accent: usize,
    warm: usize,
    signal: usize,
    alert: usize,
    nonbg: usize,
}

fn stats(cv: &Canvas) -> Stats {
    let a = pal().accent.to_u32();
    let g = pal().signal.to_u32();
    let r = pal().alert.to_u32();
    let bg = pal().bg0.to_u32();
    let mut s = Stats {
        accent: 0,
        warm: 0,
        signal: 0,
        alert: 0,
        nonbg: 0,
    };
    for &p in &cv.px {
        let (rr, gg, b) = ((p >> 16) & 0xff, (p >> 8) & 0xff, p & 0xff);
        if p == a {
            s.accent += 1;
        }
        // The warm family: accent, text, text_dim, clock — and their
        // antialiased blends against the dark surfaces.
        if rr >= 150 && (40..=210).contains(&gg) && b <= 90 {
            s.warm += 1;
        }
        if p == g {
            s.signal += 1;
        }
        if p == r {
            s.alert += 1;
        }
        if p != bg {
            s.nonbg += 1;
        }
    }
    s
}

fn pct(cv: &Canvas, s: &Stats) -> f64 {
    s.nonbg as f64 / (cv.px.len() as f64) * 100.0
}

fn write_ppm(path: &Path, cv: &Canvas) -> Result<()> {
    let mut bytes = Vec::with_capacity(cv.px.len() * 3 + 32);
    bytes.extend_from_slice(format!("P6\n{} {}\n255\n", cv.w, cv.h).as_bytes());
    for &p in &cv.px {
        bytes.push((p >> 16) as u8);
        bytes.push((p >> 8) as u8);
        bytes.push(p as u8);
    }
    fs::write(path, bytes)?;
    Ok(())
}
