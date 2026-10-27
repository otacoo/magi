//! `magi --probe`: a one-shot environment report for bring-up debugging.
//! Runs anywhere; most useful on the device over adb/ssh, and its output also
//! lands in Logs/magi.log.

use std::ffi::CStr;
use std::path::Path;

use anyhow::Result;

use crate::{device, systems, util};

pub fn run() -> Result<()> {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("magi {} probe", env!("CARGO_PKG_VERSION")));

    let dev = device::detect();
    lines.push(format!(
        "device: {} {}x{} wide={}",
        dev.id,
        dev.width,
        dev.height,
        dev.wide()
    ));
    if let Ok(text) = std::fs::read_to_string("System/magi/panel") {
        lines.push(format!("payload panel: {}", text.trim()));
    }

    match std::env::current_dir() {
        Ok(cwd) => lines.push(format!("cwd: {}", cwd.display())),
        Err(e) => lines.push(format!("cwd: unreadable ({e})")),
    }

    let catalogue = systems::Catalogue::load(Path::new("."));
    lines.push(format!(
        "systems: {} (from {})",
        catalogue.systems.len(),
        catalogue.source
    ));

    crate::theme::load_user_themes(Path::new(crate::systems::DIR_THEMES));
    lines.push(format!(
        "themes: {} ({} user)",
        crate::theme::themes().len(),
        crate::theme::themes()
            .len()
            .saturating_sub(crate::theme::BUILTIN.len())
    ));

    lines.push(format!(
        "/dev/fb0: {}",
        if Path::new("/dev/fb0").exists() {
            "present"
        } else {
            "missing"
        }
    ));
    for (name, path) in [
        ("fb mode", "/sys/class/graphics/fb0/modes"),
        ("fb virtual", "/sys/class/graphics/fb0/virtual_size"),
        ("fb bpp", "/sys/class/graphics/fb0/bits_per_pixel"),
    ] {
        if let Ok(text) = std::fs::read_to_string(path) {
            lines.push(format!("{name}: {}", text.trim()));
        }
    }

    #[cfg(target_os = "linux")]
    lines.push(format!(
        "hdmi: cable={} output={}",
        util::hdmi_cable(),
        if util::hdmi_active() { "hdmi" } else { "lcd" }
    ));
    #[cfg(target_os = "linux")]
    {
        lines.push(format!(
            "battery: {}",
            util::battery_percent()
                .map(|v| format!("{v}%"))
                .unwrap_or_else(|| "--".into())
        ));
        if let Ok(v) = std::fs::read_to_string("/sys/class/power_supply/axp2202-battery/hallkey") {
            lines.push(format!("lid: hallkey={}", v.trim()));
        }
    }

    // Auto art waits for an address, so report what the shelf would see.
    #[cfg(target_os = "linux")]
    lines.push(format!(
        "wifi: wlan0 up={} ip={} associated={} signal={}",
        util::wifi_up(),
        util::wifi_has_ip(),
        util::wifi_associated(),
        util::wifi_signal_pct()
            .map(|v| format!("{v}%"))
            .unwrap_or_else(|| "--".into())
    ));

    #[cfg(target_os = "linux")]
    for line in crate::evdev::probe_lines() {
        lines.push(line);
    }

    #[cfg(target_os = "linux")]
    match crate::present::fbdev::Fbdev::open() {
        Ok(fb) => lines.push(format!(
            "fbdev: OK id=\"{}\" {}x{} {}bpp",
            fb.id, fb.w, fb.h, fb.bpp
        )),
        Err(e) => lines.push(format!("fbdev: {e:#}")),
    }
    #[cfg(not(target_os = "linux"))]
    lines.push("fbdev: not probed on this platform".into());

    #[cfg(target_os = "linux")]
    {
        match std::process::Command::new("amixer")
            .args(["-c", "0", "scontrols"])
            .output()
        {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                for l in text.lines().take(12) {
                    lines.push(format!("mixer: {l}"));
                }
            }
            Ok(_) => lines.push("mixer: amixer -c 0 scontrols failed".into()),
            Err(e) => lines.push(format!("mixer: amixer unavailable ({e})")),
        }
        match util::volume_discover() {
            Some(ctrl) => lines.push(format!("mixer volume control: {ctrl}")),
            None => lines.push("mixer volume control: none found".into()),
        }
    }

    unsafe {
        sdl2::sys::SDL_Init(0);
        let n = sdl2::sys::SDL_GetNumVideoDrivers();
        lines.push(format!("SDL video drivers: {n}"));
        for i in 0..n {
            let cname = sdl2::sys::SDL_GetVideoDriver(i);
            let name = cstr(cname).unwrap_or_else(|| "<null>".into());
            let rc = sdl2::sys::SDL_VideoInit(cname);
            if rc == 0 {
                let active = cstr(sdl2::sys::SDL_GetCurrentVideoDriver()).unwrap_or_default();
                lines.push(format!("  {name}: OK (active: {active})"));
                sdl2::sys::SDL_VideoQuit();
            } else {
                let err = cstr(sdl2::sys::SDL_GetError()).unwrap_or_default();
                lines.push(format!("  {name}: FAIL: {err}"));
                sdl2::sys::SDL_VideoQuit();
            }
        }

        sdl2::sys::SDL_InitSubSystem(sdl2::sys::SDL_INIT_JOYSTICK);
        let jn = sdl2::sys::SDL_NumJoysticks();
        lines.push(format!("joysticks: {jn}"));
        for j in 0..jn {
            let name =
                cstr(sdl2::sys::SDL_JoystickNameForIndex(j)).unwrap_or_else(|| "<null>".into());
            let joy = sdl2::sys::SDL_JoystickOpen(j);
            if joy.is_null() {
                let err = cstr(sdl2::sys::SDL_GetError()).unwrap_or_default();
                lines.push(format!("  {j}: {name} (open failed: {err})"));
            } else {
                let axes = sdl2::sys::SDL_JoystickNumAxes(joy);
                let buttons = sdl2::sys::SDL_JoystickNumButtons(joy);
                let hats = sdl2::sys::SDL_JoystickNumHats(joy);
                lines.push(format!(
                    "  {j}: {name} (axes {axes}, buttons {buttons}, hats {hats})"
                ));
                sdl2::sys::SDL_JoystickClose(joy);
            }
        }
        sdl2::sys::SDL_Quit();
    }

    for l in &lines {
        util::log(l);
    }
    Ok(())
}

unsafe fn cstr(p: *const std::os::raw::c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(CStr::from_ptr(p).to_string_lossy().into_owned())
    }
}
