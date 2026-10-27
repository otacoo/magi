//! MAGI — a minimal, beautiful frontend for BaseOS on Anbernic H700 handhelds.
//!
//! One multicall binary:
//!   magi                 the UI (MELCHIOR)
//!   magi run …           the libretro host (BALTHASAR)
//!   magi --selftest      headless render check
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
mod util;

use anyhow::Result;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let log_name = match args.get(1).map(String::as_str) {
        Some("run") => "magi-run.log",
        Some("n64") => "magi-n64-supervisor.log",
        _ => "magi.log",
    };
    // Desktop: run the shelf (and the probe) against the fetched card payload
    // when there is one, so nothing is created in the repository root.
    let card_command = matches!(args.get(1).map(String::as_str), None | Some("--probe"));
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
    cv.save_bmp(path)?;
    println!("bootlogo written: {}", path.display());
    Ok(())
}
