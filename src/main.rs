//! MAGI — a minimal, beautiful frontend for BaseOS on Anbernic H700 handhelds.
//!
//! One multicall binary:
//!   magi                 the UI (MELCHIOR)
//!   magi run …           the libretro host (BALTHASAR)
//!   magi --selftest      headless render check
//!   magi --boot-preview  loop the boot animation in a desktop window
//!   magi --bootlogo      write the bootloader splash
//!   magi --bootlogo-art  write the splash art without the wordmark
//!   magi --probe         environment report for bring-up debugging

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]
// Painter calls take a rect, colours and an animation clock; some test modules
// sit above the code they cover.
#![allow(clippy::too_many_arguments)]
#![allow(clippy::items_after_test_module)]

mod app;
mod art;
#[cfg(target_os = "linux")]
mod audio;
mod canvas;
mod defaults;
mod device;
#[cfg(target_os = "linux")]
mod disp2;
#[cfg(target_os = "linux")]
mod evdev;
mod font;
mod games;
mod image;
mod input;
mod n64;
mod netplay;
mod pads;
mod present;
mod probe;
mod resample;
mod run;
mod screens;
mod selftest;
mod status;
mod systems;
mod theme;
mod update;
mod util;

use anyhow::Result;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let log_name = match args.get(1).map(String::as_str) {
        Some("run") => "magi-run.log",
        Some("n64") => "magi-n64-supervisor.log",
        _ => "magi.log",
    };
    // Desktop: run the shelf (and the probe/preview) against the fetched card
    // payload when there is one, so nothing is created in the repository root.
    let card_command = matches!(
        args.get(1).map(String::as_str),
        None | Some("--probe") | Some("--boot-preview")
    );
    if card_command && cfg!(not(target_os = "linux")) {
        enter_dev_card();
    }
    util::log_init_as(log_name);
    std::panic::set_hook(Box::new(|info| {
        util::log(&format!("PANIC: {info}"));
    }));

    if let Err(e) = dispatch(&args) {
        // stderr is not visible on the device; every failure must land in
        // the process log or it is lost.
        util::log(&format!("error: {e:#}"));
        std::process::exit(1);
    }
}

/// Desktop development: enter `payload-latest` (or `MAGI_CARD`) so the shelf
/// reads and writes the card layout instead of making one beside the repo.
fn enter_dev_card() {
    let card = std::env::var_os("MAGI_CARD")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let local = std::path::Path::new("payload-latest");
            if local.is_dir() {
                return Some(local.to_path_buf());
            }
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("payload-latest");
            repo.is_dir().then_some(repo)
        })
        // Without a payload, keep the repo root clean with a scratch card.
        .unwrap_or_else(|| std::path::PathBuf::from("target/dev-card"));
    let _ = std::fs::create_dir_all(&card);
    match std::env::set_current_dir(&card) {
        Ok(()) => eprintln!(
            "magi: dev card {}",
            std::env::current_dir().unwrap_or(card).display()
        ),
        Err(e) => eprintln!("magi: cannot enter {}: {e}", card.display()),
    }
}

fn dispatch(args: &[String]) -> Result<()> {
    match args.get(1).map(String::as_str) {
        Some("--version") | Some("version") => {
            println!("magi {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--selftest") => selftest::run(),
        Some("--boot-preview") => boot_preview(),
        Some("--disp2probe") => {
            #[cfg(target_os = "linux")]
            {
                disp2::probe()
            }
            #[cfg(not(target_os = "linux"))]
            {
                anyhow::bail!("--disp2probe is device-only")
            }
        }
        Some("--bootlogo") => {
            let path = args
                .get(2)
                .map(String::as_str)
                .unwrap_or("assets/boot/bootlogo-640x480.bmp");
            render_bootlogo(path)
        }
        Some("--bootlogo-art") => {
            let path = args
                .get(2)
                .map(String::as_str)
                .unwrap_or("target/bootlogo-art.png");
            render_bootlogo_art(path)
        }
        Some("--probe") | Some("probe") => probe::run(),
        Some("run") => run::main(args),
        Some("n64") => n64::main(args),
        _ => app::run(),
    }
}

/// Render the pre-boot logo BaseOS shows before the frontend starts
/// (`System/bootlogo-<w>x<h>.bmp`; a 24-bit BMP3 the bootloader draws).
fn render_bootlogo(path: &str) -> Result<()> {
    let dev = device::detect();
    let mut cv = canvas::Canvas::new(dev.width, dev.height);
    let mut fonts = font::Fonts::new(font::FONT_DATA)?;
    screens::boot::render_bootlogo(&mut cv, &mut fonts);
    let path = std::path::Path::new(path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    save_canvas(&cv, path)?;
    println!("bootlogo written: {}", path.display());
    Ok(())
}

/// Render the bootloader splash without the wordmark, for mockups.
fn render_bootlogo_art(path: &str) -> Result<()> {
    let dev = device::detect();
    let mut cv = canvas::Canvas::new(dev.width, dev.height);
    screens::boot::render_bootlogo_art(&mut cv);
    let path = std::path::Path::new(path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    save_canvas(&cv, path)?;
    println!("bootlogo art written: {}", path.display());
    Ok(())
}

/// BMP for the bootloader, PNG for mockups.
fn save_canvas(cv: &canvas::Canvas, path: &std::path::Path) -> Result<()> {
    let png = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if png {
        let bytes = image::encode_png(cv.w as u32, cv.h as u32, &cv.px)?;
        util::write_file_atomic(path, &bytes)?;
    } else {
        cv.save_bmp(path)?;
    }
    Ok(())
}

/// Run the boot sequence in a desktop window on a loop, for looking at the
/// animation without the five-second shelf handoff. ESC closes, SPACE pauses.
fn boot_preview() -> Result<()> {
    use sdl2::event::Event;
    use sdl2::keyboard::Keycode;
    use std::time::{Duration, Instant};

    let sdl = sdl2::init().map_err(anyhow::Error::msg)?;
    let dev = device::detect();
    let mut presenter = present::Presenter::open(&sdl, dev, device::window_scale())?;
    let (w, h) = presenter.canvas_size();
    let mut cv = canvas::Canvas::new(w, h);
    let mut fonts = font::Fonts::new(font::FONT_DATA)?;
    let mut pump = sdl.event_pump().map_err(anyhow::Error::msg)?;
    let cycle = screens::boot::BOOT_SECS - 0.4;
    let started = Instant::now();
    let mut held = 0.0;
    let mut paused = false;
    util::log("boot preview: ESC closes, SPACE pauses");

    loop {
        for event in pump.poll_iter() {
            match event {
                Event::Quit { .. } => return Ok(()),
                Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => return Ok(()),
                Event::KeyDown {
                    keycode: Some(Keycode::Space),
                    ..
                } => paused = !paused,
                _ => {}
            }
        }
        let t = if paused {
            held
        } else {
            let t = started.elapsed().as_secs_f32() % cycle;
            held = t;
            t
        };
        screens::boot::draw(&mut cv, &mut fonts, t);
        presenter.present(&cv)?;
        std::thread::sleep(Duration::from_millis(16));
    }
}
