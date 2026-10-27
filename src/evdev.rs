//! Direct evdev input. SDL's joystick layer found no pad on the H700 (vendor
//! pad, no udev daemon), so MAGI reads /dev/input/event* itself.
//! Non-blocking polls ride the existing frame loop.
//!
//! The H700 vendor pad reports its D-pad as ABS_HAT0X/Y, which has no kernel
//! autorepeat, so held directions are repeated in software here.

use std::collections::HashSet;
use std::fs::File;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};

use crate::input::{self, ev, Action, RawEvent, Shortcut, Shortcuts};
use crate::util;

const HOLD_DELAY: Duration = Duration::from_millis(380);
const REPEAT_DELAY: Duration = Duration::from_millis(100);

#[repr(C)]
#[derive(Clone, Copy)]
struct InputEvent {
    sec: libc::time_t,
    usec: libc::suseconds_t,
    type_: u16,
    code: u16,
    value: i32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// The stock ANBERNIC pad: scrambled BTN_* names, use as-is.
    Vendor,
    /// Conventional gamepad (DualShock/HID): NORTH is X, WEST is Y, ...
    Standard,
    /// Microsoft BT/HOGP pad: generic HID order, only X/Y are swapped.
    Xbox,
}

impl Layout {
    fn label(self) -> &'static str {
        match self {
            Layout::Vendor => "vendor",
            Layout::Standard => "standard",
            Layout::Xbox => "xbox",
        }
    }
}

struct Dev {
    file: File,
    path: String,
    name: String,
    layout: Layout,
    vendor: u16,
    product: u16,
    /// The keys we last delivered, in EVIOCGKEY layout, and the axes we
    /// follow: a dropped event batch is resynced against them.
    keys: [u8; 96],
    abs: [Option<i32>; TRACKED_ABS.len()],
    /// Raw axis min/max, used to normalize stick values to i16.
    ranges: [(i32, i32); TRACKED_ABS.len()],
    /// A real analog stick (an axis with more than a hat's range).
    stick: bool,
    /// Between SYN_DROPPED and the next SYN_REPORT the kernel's events are
    /// incomplete, so nothing may be forwarded.
    dropping: bool,
}

/// The absolute axes worth restoring after a dropped batch.
const TRACKED_ABS: [u16; 8] = [
    ev::ABS_X,
    ev::ABS_Y,
    ev::ABS_RX,
    ev::ABS_RY,
    ev::ABS_Z,
    ev::ABS_RZ,
    ev::ABS_HAT0X,
    ev::ABS_HAT0Y,
];

const EVIOCGKEY: libc::c_ulong =
    (2 << 30) | ((96 << 16) as libc::c_ulong) | ((b'E' as libc::c_ulong) << 8);

pub struct Evdev {
    devices: Vec<Dev>,
    /// inotify fd on /dev/input: hotplug wakes us instead of a poll; when
    /// the kernel refuses the watch the periodic rescan still covers it.
    watch: Option<File>,
    last_scan: Instant,
    unmapped: HashSet<(u16, u16)>,
    held: Option<Action>,
    held_axis: Option<u16>,
    /// The vendor hat does not always report the release (value 0) event;
    /// software repeat only engages once we have seen one, so a tap can
    /// never leave a direction repeating forever.
    hat_releases_seen: bool,
    next_repeat: Instant,
    shortcuts: Shortcuts,
    /// SELECT held: the d-pad becomes brightness/warmth, not directions.
    select_held: bool,
}

impl Evdev {
    pub fn open() -> Self {
        let mut ev = Self {
            devices: Vec::new(),
            watch: Self::watch_input_dir(),
            last_scan: Instant::now(),
            unmapped: HashSet::new(),
            held: None,
            held_axis: None,
            hat_releases_seen: false,
            next_repeat: Instant::now(),
            shortcuts: Shortcuts::default(),
            select_held: false,
        };
        ev.scan();
        if ev.devices.is_empty() {
            util::log("evdev: no readable /dev/input/event* devices");
        }
        ev
    }

    fn watch_input_dir() -> Option<File> {
        use std::os::unix::io::FromRawFd;
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            util::log("evdev: no inotify; falling back to periodic rescan");
            return None;
        }
        let path = std::ffi::CString::new("/dev/input").unwrap();
        let wd = unsafe {
            libc::inotify_add_watch(
                fd,
                path.as_ptr(),
                libc::IN_CREATE
                    | libc::IN_DELETE
                    | libc::IN_MOVED_TO
                    | libc::IN_MOVED_FROM
                    | libc::IN_ATTRIB,
            )
        };
        if wd < 0 {
            unsafe { libc::close(fd) };
            util::log("evdev: cannot watch /dev/input; periodic rescan only");
            return None;
        }
        Some(unsafe { File::from_raw_fd(fd) })
    }

    /// Drain pending inotify events; true when the directory changed.
    fn watch_fired(&self) -> bool {
        use std::os::unix::io::AsRawFd;
        let Some(f) = &self.watch else {
            return false;
        };
        let mut buf = [0u8; 1024];
        let mut fired = false;
        loop {
            let n = unsafe {
                libc::read(
                    f.as_raw_fd(),
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            if n > 0 {
                fired = true;
                continue;
            }
            break;
        }
        fired
    }

    /// Scan /dev/input for event nodes: drops vanished devices and opens new
    /// ones (BT pads appear here when they connect). Event-driven when
    /// inotify is available; a slow fallback rescan covers the rest.
    fn scan(&mut self) {
        self.last_scan = Instant::now();
        let mut paths: Vec<String> = Vec::new();
        if let Ok(dir) = std::fs::read_dir("/dev/input") {
            for e in dir.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("event") {
                    paths.push(format!("/dev/input/{name}"));
                }
            }
        }
        paths.sort();

        self.devices.retain(|d| {
            let owned = input::sdl_owns(d.vendor, d.product);
            let keep = !owned && paths.iter().any(|p| *p == d.path);
            if owned {
                util::log(&format!("evdev: {} handed to SDL", d.path));
            } else if !keep {
                util::log(&format!("evdev: {} gone ({})", d.path, d.name));
            }
            keep
        });

        for path in paths {
            if self.devices.iter().any(|d| d.path == path) {
                continue;
            }
            match std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)
            {
                Ok(f) => {
                    let name = device_name(f.as_raw_fd()).unwrap_or_default();
                    let (bus, vendor, product) = device_id(&path).unwrap_or((0, 0, 0));
                    if input::sdl_owns(vendor as u16, product as u16) {
                        util::log(&format!("evdev: {path} \"{name}\" is SDL's; skipping"));
                        continue;
                    }
                    let mut ranges = [(-32768, 32767); TRACKED_ABS.len()];
                    let mut stick = false;
                    for (i, code) in TRACKED_ABS.iter().enumerate() {
                        if let Some(range) = abs_range(f.as_raw_fd(), *code) {
                            ranges[i] = range;
                            if matches!(*code, ev::ABS_X | ev::ABS_Y | ev::ABS_RX | ev::ABS_RY)
                                && range.1 - range.0 > 2
                            {
                                stick = true;
                            }
                        }
                    }
                    // Only the stock pad uses the vendor's scrambled codes.
                    // Microsoft's Bluetooth pads share the generic HID order
                    // (bus 0x05, vendor 0x045e) and need their own layout;
                    // everything else is a conventional gamepad.
                    let layout = if name.contains("ANBERNIC") {
                        Layout::Vendor
                    } else if bus == 0x05 && vendor == 0x045e {
                        Layout::Xbox
                    } else if name.contains("Xbox") && name.contains("Wireless") {
                        Layout::Xbox
                    } else {
                        Layout::Standard
                    };
                    util::log(&format!("evdev: {path} \"{name}\" ({})", layout.label()));
                    self.devices.push(Dev {
                        file: f,
                        path,
                        name,
                        layout,
                        vendor: vendor as u16,
                        product: product as u16,
                        keys: [0; 96],
                        abs: [None; TRACKED_ABS.len()],
                        ranges,
                        stick,
                        dropping: false,
                    });
                }
                Err(e) => util::log(&format!("evdev: {path}: {e}")),
            }
        }
    }

    fn rescan_if_due(&mut self) {
        if self.last_scan.elapsed() >= Duration::from_millis(1500) {
            self.scan();
        }
    }

    /// Re-read /dev/input now (SDL took a pad over or released one).
    pub fn rescan(&mut self) {
        self.scan();
    }

    /// A real analog stick is attached (the stock d-pad hat does not count).
    pub fn has_stick(&self) -> bool {
        self.devices.iter().any(|d| d.stick)
    }

    /// True while the key is held, per the kernel's state (EVIOCGKEY). Used
    /// to catch buttons held across a process hand-off, where no press event
    /// is ever delivered to us.
    pub fn key_down(&self, code: u16) -> bool {
        const EVIOCGKEY: u32 = (2 << 30) | ((96 << 16) as u32) | ((b'E' as u32) << 8) | 0x18;
        let mut bits = [0u8; 96];
        for dev in self.devices.iter() {
            let fd = dev.file.as_raw_fd();
            let rc = unsafe {
                libc::ioctl(
                    fd,
                    EVIOCGKEY as libc::c_ulong,
                    bits.as_mut_ptr() as *mut libc::c_void,
                )
            };
            if rc < 0 {
                continue;
            }
            let byte = (code / 8) as usize;
            if bits[byte] & (1 << (code % 8)) != 0 {
                return true;
            }
        }
        false
    }

    /// Read everything pending from every device, raw.
    pub fn drain_raw(&mut self) -> Vec<RawEvent> {
        if self.watch.is_some() {
            // Event-driven, with a rare safety rescan in case the kernel
            // never reports (some kernels do not emit for devtmpfs).
            if self.watch_fired() || self.last_scan.elapsed() >= Duration::from_secs(10) {
                self.scan();
            }
        } else {
            self.rescan_if_due();
        }
        let mut out = Vec::new();
        let esz = std::mem::size_of::<InputEvent>();
        let mut buf = [0u8; 64 * 32];

        for (dev, d) in self.devices.iter_mut().enumerate() {
            let fd = d.file.as_raw_fd();
            loop {
                let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                if n <= 0 {
                    break;
                }
                let count = n as usize / esz;
                for i in 0..count {
                    let ev = unsafe {
                        std::ptr::read_unaligned(buf.as_ptr().add(i * esz) as *const InputEvent)
                    };
                    // The kernel threw events away (we fell behind): the state
                    // we show for this device is stale until it is resynced,
                    // and the release of a held key may be one of the losses.
                    if ev.type_ == ev::EV_SYN && ev.code == ev::SYN_DROPPED {
                        resync(d, dev, &mut out);
                        d.dropping = true;
                        continue;
                    }
                    if d.dropping {
                        if ev.type_ == ev::EV_SYN && ev.code == ev::SYN_REPORT {
                            d.dropping = false;
                        }
                        continue;
                    }
                    track(d, &ev);
                    let value = if is_stick(ev.code) {
                        let i = TRACKED_ABS.iter().position(|c| *c == ev.code).unwrap_or(0);
                        i32::from(axis_i16(ev.value, d.ranges[i].0, d.ranges[i].1))
                    } else {
                        ev.value
                    };
                    out.push(RawEvent {
                        dev,
                        type_: ev.type_,
                        code: mapped(d.layout, ev.code),
                        value,
                    });
                }
            }
        }
        out
    }

    /// Block until a pad or a device change is ready, or the timeout passes.
    /// Lets the UI idle without a fixed 16 ms tick.
    pub fn wait(&mut self, timeout: Duration) -> bool {
        use std::os::unix::io::AsRawFd;
        let mut fds: Vec<libc::pollfd> = self
            .devices
            .iter()
            .map(|d| libc::pollfd {
                fd: d.file.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        if let Some(w) = &self.watch {
            fds.push(libc::pollfd {
                fd: w.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }
        let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) > 0 }
    }

    /// A key or direction is down: held-repeat needs the fast tick.
    pub fn busy(&self) -> bool {
        self.held.is_some() || self.held_axis.is_some() || self.select_held
    }

    /// UI mode: raw events mapped to actions, with hat held-repeat.
    pub fn poll(&mut self) -> Vec<Action> {
        let events = self.drain_raw();
        let mut acts = Vec::new();

        for ev in events {
            let name = self.devices[ev.dev].name.clone();
            // The Hall lid switch: closing means sleep, opening means wake
            // (the wake itself is the kernel's, we only need to see close).
            if ev.type_ == ev::EV_SW && ev.code == ev::SW_LID {
                util::log(&format!(
                    "{name}: lid {}",
                    if ev.value == 1 { "closed" } else { "opened" }
                ));
                if ev.value == 1 {
                    acts.push(Action::LidClosed);
                }
                continue;
            }
            if ev.type_ == ev::EV_ABS {
                match ev.code {
                    ev::ABS_HAT0X | ev::ABS_HAT0Y => {
                        if ev.value == 0 {
                            if self.held_axis == Some(ev.code) {
                                util::log(&format!("{name}: {:#x} released", ev.code));
                            }
                            self.held = None;
                            self.held_axis = None;
                            self.hat_releases_seen = true;
                        } else if self.select_held {
                            // SELECT + d-pad: brightness and warmth, not
                            // directions. The gesture also silences the
                            // SELECT tap so CONTROLS does not pop up.
                            let combo = match (ev.code, ev.value) {
                                (ev::ABS_HAT0Y, v) if v < 0 => Some(Action::BrightnessUp),
                                (ev::ABS_HAT0Y, _) => Some(Action::BrightnessDown),
                                (ev::ABS_HAT0X, v) if v < 0 => Some(Action::WarmthDown),
                                (ev::ABS_HAT0X, _) => Some(Action::WarmthUp),
                                _ => None,
                            };
                            if let Some(a) = combo {
                                self.shortcuts.combo_used();
                                self.held = Some(a);
                                self.held_axis = Some(ev.code);
                                self.next_repeat = Instant::now() + HOLD_DELAY;
                                util::log(&format!(
                                    "{name}: SELECT+{:#x} value {} -> {a:?}",
                                    ev.code, ev.value
                                ));
                                acts.push(a);
                            }
                        } else if let Some(a) = input::evdev_action(ev.type_, ev.code, ev.value) {
                            self.held = Some(a);
                            self.held_axis = Some(ev.code);
                            self.next_repeat = Instant::now() + HOLD_DELAY;
                            util::log(&format!(
                                "{name}: {:#x} value {} -> {a:?}",
                                ev.code, ev.value
                            ));
                            acts.push(a);
                        }
                    }
                    _ => {}
                }
                continue;
            }

            // SELECT held state steers the d-pad combos above.
            if ev.type_ == ev::EV_KEY && ev.code == ev::BTN_TL {
                if ev.value == 1 {
                    self.select_held = true;
                } else if ev.value == 0 {
                    self.select_held = false;
                    if matches!(
                        self.held,
                        Some(
                            Action::BrightnessUp
                                | Action::BrightnessDown
                                | Action::WarmthUp
                                | Action::WarmthDown
                        )
                    ) {
                        self.held = None;
                        self.held_axis = None;
                    }
                }
            }

            // The selection cluster: taps on release, and the screenshot
            // chord; R1 alone stays a normal action (paging).
            if ev.type_ == ev::EV_KEY && input::tracked_code(ev.code) {
                if ev.code != input::CODE_R1 {
                    util::log(&format!("{name}: {:#x} value {}", ev.code, ev.value));
                }
                if let Some(sc) = self.shortcuts.feed(&ev) {
                    let (a, what) = match sc {
                        Shortcut::Screenshot => (Action::Screenshot, "screenshot chord"),
                        Shortcut::SelectTap => (Action::Select, "select tap"),
                        Shortcut::MenuTap => (Action::Menu, "menu tap"),
                    };
                    util::log(&format!("{name}: {what}"));
                    acts.push(a);
                    continue;
                }
                if ev.code != input::CODE_R1 {
                    continue;
                }
            }

            if let Some(a) = input::evdev_action(ev.type_, ev.code, ev.value) {
                util::log(&format!(
                    "{name}: {:#x} value {} -> {a:?}",
                    ev.code, ev.value
                ));
                acts.push(a);
            } else if ev.type_ == ev::EV_KEY
                && ev.value == 1
                && self.unmapped.insert((ev.type_, ev.code))
            {
                util::log(&format!("{name}: unmapped key {:#x}", ev.code));
            }
        }
        acts
    }

    /// Drop everything pending — used after a child process owned the input.
    pub fn flush(&mut self) {
        while !self.drain_raw().is_empty() {}
    }

    /// Software autorepeat for a held D-pad direction, plus grace-delayed
    /// SELECT/MENU taps from the shortcut watcher.
    pub fn tick(&mut self, now: Instant) -> Option<Action> {
        if let Some(sc) = self.shortcuts.tick(now) {
            return Some(match sc {
                Shortcut::SelectTap => Action::Select,
                Shortcut::MenuTap => Action::Menu,
                Shortcut::Screenshot => Action::Screenshot,
            });
        }
        if !self.hat_releases_seen {
            return None;
        }
        if let Some(dir) = self.held {
            if now >= self.next_repeat {
                self.next_repeat = now + REPEAT_DELAY;
                return Some(dir);
            }
        }
        None
    }
}

/// Input nodes and their analog ranges, for `--probe`.
pub fn probe_lines() -> Vec<String> {
    let mut out = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/dev/input") {
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("event") {
                paths.push(format!("/dev/input/{name}"));
            }
        }
    }
    paths.sort();
    for path in paths {
        let Ok(f) = std::fs::OpenOptions::new().read(true).open(&path) else {
            continue;
        };
        use std::os::unix::io::AsRawFd;
        let fd = f.as_raw_fd();
        let name = device_name(fd).unwrap_or_default();
        let (bus, vendor, product) = device_id(&path).unwrap_or((0, 0, 0));
        let mut line = format!(
            "input: {path} \"{name}\" bus={bus:04x} vendor={vendor:04x} product={product:04x}"
        );
        for code in TRACKED_ABS {
            let mut info = AbsInfo::default();
            let req = ioc_read(
                0x40 + libc::c_ulong::from(code),
                std::mem::size_of::<AbsInfo>() as libc::c_ulong,
            );
            if unsafe { libc::ioctl(fd, req, &mut info) } < 0 || info.maximum <= info.minimum {
                continue;
            }
            let label = match code {
                ev::ABS_X => "x",
                ev::ABS_Y => "y",
                ev::ABS_RX => "rx",
                ev::ABS_RY => "ry",
                ev::ABS_Z => "z",
                ev::ABS_RZ => "rz",
                ev::ABS_HAT0X => "hatx",
                ev::ABS_HAT0Y => "haty",
                _ => "?",
            };
            line.push_str(&format!(
                " {label}={}..{} cur={} fuzz={} flat={}",
                info.minimum, info.maximum, info.value, info.fuzz, info.flat
            ));
        }
        out.push(line);
    }
    out
}

fn device_name(fd: libc::c_int) -> Option<String> {
    let mut buf = [0u8; 128];
    let req = ioc_read(0x06, buf.len() as libc::c_ulong); // EVIOCGNAME(len)
    let rc = unsafe { libc::ioctl(fd, req, buf.as_mut_ptr()) };
    if rc < 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Some(String::from_utf8_lossy(&buf[..end]).into_owned())
}

/// Bus and vendor id from sysfs (e.g. bus 0x05 = Bluetooth, vendor 0x045e =
/// Microsoft). Missing files mean unknown, never a hard failure.
fn device_id(path: &str) -> Option<(u32, u32, u32)> {
    let node = path.rsplit('/').next()?;
    let read = |attr: &str| -> Option<u32> {
        let text =
            std::fs::read_to_string(format!("/sys/class/input/{node}/device/id/{attr}")).ok()?;
        u32::from_str_radix(text.trim(), 16).ok()
    };
    Some((read("bustype")?, read("vendor")?, read("product")?))
}

/// _IOC(_IOC_READ, 'E', nr, size)
fn ioc_read(nr: libc::c_ulong, size: libc::c_ulong) -> libc::c_ulong {
    (2 << 30) | (size << 16) | (0x45 << 8) | nr
}

#[cfg(test)]
mod tests {
    use super::axis_i16;

    #[test]
    fn stick_ranges_scale_to_i16() {
        assert_eq!(axis_i16(0, -4096, 4096), 0);
        assert_eq!(axis_i16(4096, -4096, 4096), 32767);
        assert_eq!(axis_i16(-4096, -4096, 4096), -32768);
    }
}

/// The kernel's `struct input_absinfo`.
#[repr(C)]
#[derive(Default)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

fn mapped(layout: Layout, code: u16) -> u16 {
    match layout {
        Layout::Vendor => code,
        Layout::Standard => input::standard_code(code),
        Layout::Xbox => input::xbox_code(code),
    }
}

/// Keep the per-device picture of what the kernel has been told we saw.
fn track(d: &mut Dev, ev: &InputEvent) {
    match ev.type_ {
        ev::EV_KEY => input::track_key(&mut d.keys, ev.code, ev.value),
        ev::EV_ABS => {
            if let Some(i) = TRACKED_ABS.iter().position(|c| *c == ev.code) {
                d.abs[i] = Some(ev.value);
            }
        }
        _ => {}
    }
}

fn is_stick(code: u16) -> bool {
    matches!(code, ev::ABS_X | ev::ABS_Y | ev::ABS_RX | ev::ABS_RY)
}

/// Map a raw axis value to the libretro analog range.
fn axis_i16(v: i32, min: i32, max: i32) -> i16 {
    if max <= min {
        return v.clamp(-32768, 32767) as i16;
    }
    let mid = (i64::from(min) + i64::from(max)) / 2;
    let half = (i64::from(max) - i64::from(min)) / 2;
    let scaled = if i64::from(v) < mid {
        (i64::from(v) - mid) * 32768 / half.max(1)
    } else {
        (i64::from(v) - mid) * 32767 / half.max(1)
    };
    scaled.clamp(-32768, 32767) as i16
}

/// The axis range, or None when the device does not expose that axis.
fn abs_range(fd: libc::c_int, code: u16) -> Option<(i32, i32)> {
    let mut info = AbsInfo::default();
    let req = ioc_read(
        0x40 + libc::c_ulong::from(code),
        std::mem::size_of::<AbsInfo>() as libc::c_ulong,
    );
    if unsafe { libc::ioctl(fd, req, &mut info) } < 0 || info.maximum <= info.minimum {
        None
    } else {
        Some((info.minimum, info.maximum))
    }
}

/// Replay the difference between what we last delivered and the kernel's live
/// state. A dropped batch loses releases as easily as presses, and a release
/// that never arrives leaves a key held forever.
fn resync(d: &mut Dev, dev: usize, out: &mut Vec<RawEvent>) {
    let fd = d.file.as_raw_fd();
    let mut replayed = 0;
    let mut bits = [0u8; 96];
    if unsafe { libc::ioctl(fd, EVIOCGKEY, bits.as_mut_ptr() as *mut libc::c_void) } >= 0 {
        for code in 0..768usize {
            let was = d.keys[code / 8] & (1 << (code % 8)) != 0;
            let now = bits[code / 8] & (1 << (code % 8)) != 0;
            if was != now {
                out.push(RawEvent {
                    dev,
                    type_: ev::EV_KEY,
                    code: mapped(d.layout, code as u16),
                    value: i32::from(now),
                });
                replayed += 1;
            }
        }
        d.keys = bits;
    }
    for (i, code) in TRACKED_ABS.iter().enumerate() {
        let Some(prev) = d.abs[i] else { continue };
        let mut info = AbsInfo::default();
        let req = ioc_read(
            0x40 + libc::c_ulong::from(*code),
            std::mem::size_of::<AbsInfo>() as libc::c_ulong,
        );
        if unsafe { libc::ioctl(fd, req, &mut info) } < 0 || info.value == prev {
            continue;
        }
        d.abs[i] = Some(info.value);
        let value = if is_stick(*code) {
            i32::from(axis_i16(info.value, d.ranges[i].0, d.ranges[i].1))
        } else {
            info.value
        };
        out.push(RawEvent {
            dev,
            type_: ev::EV_ABS,
            code: *code,
            value,
        });
        replayed += 1;
    }
    if replayed > 0 {
        util::log(&format!("{}: input resync ({replayed} events)", d.name));
    }
}
