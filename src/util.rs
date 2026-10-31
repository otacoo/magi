//! Small platform probes and the persistent card log.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::sync::Mutex;
use std::time::Instant;

pub(crate) const LOG_CAP: u64 = 256 * 1024;

static LOG: Mutex<Option<(File, Instant)>> = Mutex::new(None);

/// Open the persistent log in the card's `Logs/` folder. Truncated every
/// launch, so the newest boot is always the whole story. The UI writes
/// `Logs/magi.log`; the run host writes `Logs/magi-run.log` so the two
/// processes never fight over one file.
pub fn log_init_as(name: &str) {
    let _ = std::fs::create_dir_all("Logs");
    let file = File::create(std::path::Path::new("Logs").join(name)).ok();
    *LOG.lock().unwrap() = file.map(|f| (f, Instant::now()));
}

fn append_capped(file: &mut File, bytes: &[u8]) -> std::io::Result<()> {
    let bytes = if bytes.len() as u64 > LOG_CAP {
        &bytes[bytes.len() - LOG_CAP as usize..]
    } else {
        bytes
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len + bytes.len() as u64 > LOG_CAP {
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
    }
    file.write_all(bytes)
}

/// Append to a child process log (N64 port/aplay) with the same 256 KB cap.
/// The file is reopened per line because the child owns the other pipe end.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn append_capped_path(path: &str, bytes: &[u8]) {
    let open = || OpenOptions::new().create(true).append(true).open(path);
    let mut file = match open() {
        Ok(file) => file,
        Err(_) => {
            if let Some(parent) = std::path::Path::new(path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match open() {
                Ok(file) => file,
                Err(_) => return,
            }
        }
    };
    let _ = append_capped(&mut file, bytes);
}

/// Progress diagnostics: stderr (→ /tmp/generic.log under BaseOS) and Logs/magi.log.
pub fn log(msg: &str) {
    eprintln!("magi: {msg}");
    if let Ok(mut guard) = LOG.lock() {
        if let Some((file, t0)) = guard.as_mut() {
            let line = format!("[{:>8.3}] {msg}\n", t0.elapsed().as_secs_f32());
            let _ = append_capped(file, line.as_bytes());
            let _ = file.flush();
        }
    }
}

/// Battery percentage from sysfs. `None` on non-Linux or when absent.
#[cfg(target_os = "linux")]
pub fn battery_percent() -> Option<u32> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in dir.flatten() {
        let cap = entry.path().join("capacity");
        if let Ok(text) = std::fs::read_to_string(&cap) {
            if let Ok(v) = text.trim().parse::<u32>() {
                return Some(v.min(100));
            }
        }
    }
    None
}

/// The charging state string ("Charging", "Full", "Discharging", ...).
#[cfg(target_os = "linux")]
pub fn battery_status() -> Option<String> {
    std::fs::read_to_string("/sys/class/power_supply/axp2202-battery/status")
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(not(target_os = "linux"))]
pub fn battery_status() -> Option<String> {
    None
}

/// Whether USB power is feeding the device.
#[cfg(target_os = "linux")]
pub fn usb_online() -> Option<bool> {
    let v = std::fs::read_to_string("/sys/class/power_supply/axp2202-usb/online").ok()?;
    Some(v.trim() == "1")
}

#[cfg(not(target_os = "linux"))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn usb_online() -> Option<bool> {
    None
}

/// The AXP717/2202 "charger enable" bit: register 0x18, bit 1 (R/W per
/// the datasheet; the vendor driver already clears it at 100%). sysfs
/// and the regmap debugfs are read-only, so this speaks i2c directly.
#[cfg(target_os = "linux")]
fn axp_charger_bits() -> Option<u8> {
    #[repr(C)]
    struct SmbusIoctlData {
        read_write: u8,
        command: u8,
        size: u32,
        data: *mut u8,
    }
    const I2C_SLAVE: libc::c_ulong = 0x0703;
    const I2C_SMBUS: libc::c_ulong = 0x0720;
    const I2C_SMBUS_BYTE_DATA: u32 = 2;
    const AXP_ADDR: libc::c_ulong = 0x34;
    const CHG_ENABLE_REG: u8 = 0x18;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/i2c-5")
        .ok()?;
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    if unsafe { libc::ioctl(fd, I2C_SLAVE, AXP_ADDR) } < 0 {
        return None;
    }
    let mut val: u8 = 0;
    let mut msg = SmbusIoctlData {
        read_write: 1,
        command: CHG_ENABLE_REG,
        size: I2C_SMBUS_BYTE_DATA,
        data: &mut val,
    };
    if unsafe { libc::ioctl(fd, I2C_SMBUS, &mut msg as *mut SmbusIoctlData) } < 0 {
        return None;
    }
    Some(val)
}

/// Read-modify-write the charger enable bit; leaves every other bit of
/// register 0x18 alone. Returns true when the write went through.
#[cfg(target_os = "linux")]
pub fn axp_set_charger(on: bool) -> bool {
    #[repr(C)]
    struct SmbusIoctlData {
        read_write: u8,
        command: u8,
        size: u32,
        data: *mut u8,
    }
    const I2C_SLAVE: libc::c_ulong = 0x0703;
    const I2C_SMBUS: libc::c_ulong = 0x0720;
    const I2C_SMBUS_BYTE_DATA: u32 = 2;
    const AXP_ADDR: libc::c_ulong = 0x34;
    const CHG_ENABLE_REG: u8 = 0x18;

    let Some(cur) = axp_charger_bits() else {
        return false;
    };
    let want = if on { cur | 0x02 } else { cur & !0x02 };
    if want == cur {
        return true;
    }
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/i2c-5")
    {
        Ok(f) => f,
        Err(e) => {
            log(&format!("battery: cannot open /dev/i2c-5: {e}"));
            return false;
        }
    };
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    if unsafe { libc::ioctl(fd, I2C_SLAVE, AXP_ADDR) } < 0 {
        log("battery: i2c slave select failed");
        return false;
    }
    let mut value = want;
    let mut msg = SmbusIoctlData {
        read_write: 0,
        command: CHG_ENABLE_REG,
        size: I2C_SMBUS_BYTE_DATA,
        data: &mut value,
    };
    if unsafe { libc::ioctl(fd, I2C_SMBUS, &mut msg as *mut SmbusIoctlData) } < 0 {
        log("battery: i2c write failed");
        return false;
    }
    let readback = axp_charger_bits();
    log(&format!(
        "battery: charger {} (reg18 readback {readback:02x?})",
        if on { "enabled" } else { "disabled" }
    ));
    true
}

#[cfg(not(target_os = "linux"))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn axp_set_charger(_on: bool) -> bool {
    false
}

/// One battery-policy tick, shared by the shelf and the game host.
/// `shutdown_pct` powers off below that charge (not while charging),
/// `charge_limit` stops charging at that level (100 = once full, stays
/// off the charger until unplugged). Returns true when the caller must
/// save and power off.
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
pub fn battery_policy(shutdown_pct: u32, charge_limit: u32, full_latch: &mut bool) -> bool {
    let pct = battery_percent();
    let status = battery_status();
    if shutdown_pct > 0 {
        if let (Some(pct), Some(status)) = (pct, status.as_deref()) {
            if pct <= shutdown_pct && status != "Charging" && status != "Full" {
                log(&format!(
                    "battery: {pct}% (<= {shutdown_pct}%); powering off"
                ));
                return true;
            }
        }
    }
    #[cfg(target_os = "linux")]
    if charge_limit > 0 {
        let online = usb_online().unwrap_or(false);
        if !online {
            if *full_latch {
                *full_latch = false;
                log("battery: unplugged; charge policy reset");
            }
        } else if let Some(pct) = pct {
            if charge_limit >= 100 {
                // Once full on USB: stay off the charger until unplug.
                if pct >= 100 && !*full_latch {
                    *full_latch = true;
                    log("battery: full on USB; charging disabled until unplug");
                    axp_set_charger(false);
                } else if *full_latch {
                    // The vendor driver re-enables after its recharge
                    // threshold; hold our decision.
                    axp_set_charger(false);
                }
            } else if pct >= charge_limit {
                if axp_charger_bits().is_some_and(|v| v & 0x02 != 0) {
                    log(&format!("battery: charge limit {charge_limit}% reached"));
                    axp_set_charger(false);
                }
            } else if pct + 3 <= charge_limit && axp_charger_bits().is_some_and(|v| v & 0x02 == 0) {
                log("battery: charge limit lifted; charging");
                axp_set_charger(true);
            }
        }
    }
    false
}

#[cfg(not(target_os = "linux"))]
pub fn battery_percent() -> Option<u32> {
    None
}

/// `HH:MM` in the device's local time. BaseOS sets the runtime timezone;
/// BusyBox `date` respects it. `None` when unavailable.
#[cfg(target_os = "linux")]
pub fn clock_hhmm() -> Option<String> {
    let out = std::process::Command::new("date")
        .arg("+%H:%M")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.len() == 5 {
        Some(s)
    } else {
        None
    }
}

#[cfg(not(target_os = "linux"))]
pub fn clock_hhmm() -> Option<String> {
    None
}

/// 24-hour hour/minute as 12-hour digits and a PM flag.
pub fn clock_12h(hour: u32, minute: u32) -> (String, bool) {
    let pm = hour >= 12;
    let h = match hour % 12 {
        0 => 12,
        h => h,
    };
    (format!("{h:02}:{minute:02}"), pm)
}

/// `HH:MM` to 12-hour digits and PM; `None` when the text is not a clock.
pub fn clock_12h_from_hhmm(text: &str) -> Option<(String, bool)> {
    let (h, m) = text.split_once(':')?;
    let hour = h.parse::<u32>().ok()?;
    let minute = m.parse::<u32>().ok()?;
    (hour < 24 && minute < 60).then(|| clock_12h(hour, minute))
}

#[cfg(not(target_os = "linux"))]
pub fn set_datetime(_v: [i32; 5]) -> bool {
    false
}

/// Local date and time as [year, month, day, hour, minute].
#[cfg(target_os = "linux")]
pub fn full_date() -> Option<[i32; 5]> {
    let out = std::process::Command::new("date")
        .arg("+%Y %m %d %H %M")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let parts: Vec<i32> = text
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    parts.try_into().ok()
}

#[cfg(not(target_os = "linux"))]
pub fn full_date() -> Option<[i32; 5]> {
    None
}

/// Set the system clock and write it back to the RTC (BaseOS reads the RTC
/// as UTC at boot).
#[cfg(target_os = "linux")]
pub fn set_datetime(v: [i32; 5]) -> bool {
    let stamp = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:00",
        v[0], v[1], v[2], v[3], v[4]
    );
    let ok = std::process::Command::new("date")
        .args(["-s", &stamp])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        log(&format!("clock: date -s '{stamp}' failed"));
        return false;
    }
    let hw = std::process::Command::new("hwclock")
        .arg("-w")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    log(&format!("clock: set to {stamp} (hwclock: {hw})"));
    true
}

#[cfg(not(target_os = "linux"))]
pub fn set_brightness(_pct: u32) -> bool {
    false
}

/// Power cuts must never corrupt a file on FAT: write a temp, then rename.
/// The temporary file is synced before the rename makes it visible, and the
/// directory entry is synced after.
pub fn write_file_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    write_file(path, bytes, true)
}

/// The same swap without the sync: for the writes that happen while the user
/// is holding a direction (cursor position, brightness/volume steps), where a
/// 20 ms card flush would drop a frame. A power cut may lose the last one.
pub fn write_file_fast(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    write_file(path, bytes, false)
}

fn write_file(path: &std::path::Path, bytes: &[u8], durable: bool) -> std::io::Result<()> {
    use std::io::Write;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        if durable {
            f.sync_all()?;
        }
    }
    std::fs::rename(&tmp, path)?;
    if durable {
        // The rename itself has to reach the card as well.
        if let Some(dir) = path.parent() {
            let dir = if dir.as_os_str().is_empty() {
                std::path::Path::new(".")
            } else {
                dir
            };
            if let Ok(d) = std::fs::File::open(dir) {
                let _ = d.sync_all();
            }
        }
    }
    Ok(())
}

/// Counts fast restarts so a frontend that dies before it settles is not
/// respawned into the same crash forever. True on the third start in a row
/// that came within 20 s of the previous one: the caller starts with the
/// default look and no boot sequence instead.
pub fn startup_guard() -> bool {
    let path = std::path::Path::new("Config/magi-boot.json");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut count = 1u64;
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            let last = v.get("last").and_then(|x| x.as_u64()).unwrap_or(0);
            let prev = v.get("count").and_then(|x| x.as_u64()).unwrap_or(0);
            if now.saturating_sub(last) < 20 {
                count = prev.saturating_add(1);
            }
        }
    }
    let body = serde_json::json!({ "last": now, "count": count });
    let _ = write_file_fast(path, body.to_string().as_bytes());
    count >= 3
}

// Is a process with this comm name running? (BaseOS has no pgrep in PATH.)
#[cfg(target_os = "linux")]
pub fn process_running(name: &str) -> bool {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return false;
    };
    for e in dir.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(p.join("comm")) {
            if comm.trim() == name {
                return true;
            }
        }
    }
    false
}

#[cfg(target_os = "linux")]
pub(crate) fn kill_by_name(name: &str) {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return;
    };
    for e in dir.flatten() {
        let p = e.path();
        let Some(pid) = p
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        if let Ok(comm) = std::fs::read_to_string(p.join("comm")) {
            if comm.trim() == name {
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn kill_cmdline(needle: &str) {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return;
    };
    for e in dir.flatten() {
        let p = e.path();
        let Some(pid) = p
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        if pid == std::process::id() as libc::pid_t {
            continue;
        }
        if let Ok(cmd) = std::fs::read_to_string(p.join("cmdline")) {
            if cmd.contains(needle) {
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
            }
        }
    }
}

/// The BaseOS `systemctl` shim, by absolute path (PATH independent).
#[cfg(target_os = "linux")]
fn systemctl(args: &[&str]) -> bool {
    for shim in ["/sbin/systemctl", "/usr/sbin/systemctl", "/bin/systemctl"] {
        if let Ok(o) = std::process::Command::new(shim).args(args).output() {
            if o.status.success() {
                return true;
            }
        }
    }
    false
}
/// Where the Wi-Fi credentials live (owned by MAGI on the card).
#[cfg(target_os = "linux")]
const WPA_CONF: &str = "Config/wpa_supplicant.conf";

/// Whether wlan0 is administratively up.
#[cfg(target_os = "linux")]
pub fn wifi_up() -> bool {
    std::fs::read_to_string("/sys/class/net/wlan0/flags")
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
        .map(|f| f & 1 != 0)
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn write_wpa_conf() {
    if std::path::Path::new(WPA_CONF).exists() {
        return;
    }
    let _ = std::fs::create_dir_all("Config");
    let _ = std::fs::write(
        WPA_CONF,
        "ctrl_interface=/run/wpa_supplicant\n\
         disable_scan_offload=1\n\
         update_config=1\n\
         wowlan_triggers=any\n",
    );
}

/// DHCP: BaseOS ships udhcpc; without it an association has no address.
#[cfg(target_os = "linux")]
fn start_dhcp() {
    kill_by_name("udhcpc");
    let _ = std::process::Command::new("udhcpc")
        .args(["-i", "wlan0", "-b"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Bring Wi-Fi up (radio, interface, supplicant, DHCP) or down.
#[cfg(target_os = "linux")]
pub fn set_wifi(on: bool) -> bool {
    if on {
        // rcS loads the radio in the background and hands off early, so wlan0
        // can appear seconds into the session. Wait for it rather than race
        // it with a supplicant that has no interface.
        for _ in 0..80 {
            if std::path::Path::new("/sys/class/net/wlan0").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        let _ = std::process::Command::new("rfkill")
            .args(["unblock", "wifi"])
            .output();
        for (cmd, args) in [
            ("ifconfig", vec!["wlan0", "up"]),
            ("ip", vec!["link", "set", "wlan0", "up"]),
        ] {
            if let Ok(o) = std::process::Command::new(cmd).args(&args).output() {
                if o.status.success() {
                    break;
                }
            }
        }
        write_wpa_conf();
        if !process_running("wpa_supplicant") {
            let _ = std::process::Command::new("wpa_supplicant")
                .args(["-B", "-i", "wlan0", "-c", WPA_CONF])
                .spawn();
        }
        start_dhcp();
    } else {
        let _ = std::process::Command::new("wpa_cli")
            .args(["-i", "wlan0", "terminate"])
            .output();
        kill_by_name("wpa_supplicant");
        kill_by_name("udhcpc");
        let _ = std::process::Command::new("ifconfig")
            .args(["wlan0", "down"])
            .output();
        let _ = std::process::Command::new("rfkill")
            .args(["block", "wifi"])
            .output();
    }
    std::thread::sleep(std::time::Duration::from_millis(800));
    let ok = wifi_up() == on;
    log(&format!("network: wifi {on} ({ok})"));
    ok
}

/// Whether wlan0 holds a routable IPv4 address. A link-local 169.254/16 does
/// not count: it means udhcpc never configured a lease.
#[cfg(target_os = "linux")]
pub fn wifi_has_ip() -> bool {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills a valid list and freeifaddrs releases it; every
    // pointer is checked before it is read.
    unsafe {
        if libc::getifaddrs(&mut list) != 0 {
            return false;
        }
        let mut found = false;
        let mut cur = list;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null() {
                let name = std::ffi::CStr::from_ptr(ifa.ifa_name);
                let family = (*ifa.ifa_addr).sa_family as i32;
                if name.to_bytes() == b"wlan0" && family == libc::AF_INET {
                    let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    let ip = u32::from_be(sin.sin_addr.s_addr);
                    if ip != 0 && ip >> 16 != 0xA9FE {
                        found = true;
                        break;
                    }
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(list);
        found
    }
}

/// Whether the supplicant is associated with an access point.
#[cfg(target_os = "linux")]
pub fn wifi_associated() -> bool {
    wpa(&["status"])
        .map(|s| s.lines().any(|line| line.trim() == "wpa_state=COMPLETED"))
        .unwrap_or(false)
}

/// Retry DHCP after an association that got no lease. BaseOS's boot udhcpc
/// can start before the association and give up.
#[cfg(target_os = "linux")]
pub fn retry_dhcp() {
    log("network: wifi has no address; retrying DHCP");
    start_dhcp();
}

#[cfg(not(target_os = "linux"))]
pub fn wifi_has_ip() -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn wifi_associated() -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn retry_dhcp() {}

#[cfg(target_os = "linux")]
fn wpa(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("wpa_cli")
        .args(["-i", "wlan0"])
        .args(args)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        None
    }
}

/// One visible network from a scan, merged with what the supplicant knows.
#[derive(Clone)]
pub struct WifiNet {
    pub ssid: String,
    pub signal: u8,
    pub secured: bool,
    pub saved: bool,
    pub current: bool,
}

/// `scan_results` lines: bssid / freq / signal / flags / ssid.
#[cfg(any(target_os = "linux", test))]
pub fn parse_wifi_scan(text: &str) -> Vec<(String, u8, bool)> {
    let mut nets: Vec<(String, u8, bool)> = Vec::new();
    for line in text.lines().skip(1) {
        let mut parts = line.split('\t');
        let (_bssid, _freq, signal, flags, ssid) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        );
        let (Some(signal), Some(flags), Some(ssid)) = (signal, flags, ssid) else {
            continue;
        };
        let ssid = ssid.trim();
        if ssid.is_empty() {
            continue;
        }
        let Ok(rssi) = signal.trim().parse::<i32>() else {
            continue;
        };
        let signal = (2 * (rssi + 100)).clamp(0, 100) as u8;
        let secured = flags.contains("PSK") || flags.contains("SAE") || flags.contains("802.1X");
        match nets.iter_mut().find(|(s, _, _)| s == ssid) {
            Some(existing) if existing.1 < signal => existing.1 = signal,
            Some(_) => {}
            None => nets.push((ssid.to_string(), signal, secured)),
        }
    }
    nets
}

/// `list_networks` lines: id / ssid / bssid / flags.
#[cfg(target_os = "linux")]
pub fn parse_wifi_networks(text: &str) -> Vec<(String, bool)> {
    let mut nets = Vec::new();
    for line in text.lines().skip(1) {
        let mut parts = line.split('\t');
        let (_id, ssid, _bssid, flags) = (parts.next(), parts.next(), parts.next(), parts.next());
        if let (Some(ssid), Some(flags)) = (ssid, flags) {
            nets.push((ssid.to_string(), flags.contains("CURRENT")));
        }
    }
    nets
}

/// `status` output: key=value lines.
#[cfg(target_os = "linux")]
pub fn parse_wpa_status(text: &str) -> (String, String, String) {
    let mut state = String::new();
    let mut ssid = String::new();
    let mut ip = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("wpa_state=") {
            state = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("ssid=") {
            ssid = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("ip_address=") {
            ip = v.trim().to_string();
        }
    }
    (state, ssid, ip)
}

/// Scan and merge: the visible networks with signal, security and state.
#[cfg(target_os = "linux")]
pub fn wifi_scan() -> Vec<WifiNet> {
    let _ = wpa(&["scan"]);
    std::thread::sleep(std::time::Duration::from_millis(2500));
    let results = wpa(&["scan_results"]).unwrap_or_default();
    let known = wpa(&["list_networks"]).unwrap_or_default();
    let status = wpa(&["status"]).unwrap_or_default();
    let (state, current_ssid, _ip) = parse_wpa_status(&status);
    let networks = parse_wifi_networks(&known);
    let mut nets: Vec<WifiNet> = parse_wifi_scan(&results)
        .into_iter()
        .map(|(ssid, signal, secured)| {
            let saved = networks.iter().any(|(s, _)| *s == ssid);
            WifiNet {
                current: state == "COMPLETED" && current_ssid == ssid,
                ssid,
                signal,
                secured,
                saved,
            }
        })
        .collect();
    // Saved networks out of range (or while down) still deserve a row.
    for (ssid, current) in &networks {
        if !nets.iter().any(|n| &n.ssid == ssid) {
            nets.push(WifiNet {
                ssid: ssid.clone(),
                signal: 0,
                secured: true,
                saved: true,
                current: *current,
            });
        }
    }
    nets.sort_by(|a, b| b.signal.cmp(&a.signal));
    nets
}

/// Connect: stored credentials are reused, otherwise `psk` is stored.
#[cfg(target_os = "linux")]
pub fn wifi_connect(ssid: &str, psk: Option<&str>) -> bool {
    let known = wpa(&["list_networks"]).unwrap_or_default();
    for line in known.lines().skip(1) {
        let mut parts = line.split('\t');
        if let (Some(id), Some(s)) = (parts.next(), parts.next()) {
            if s == ssid {
                let _ = wpa(&["select_network", id]);
                start_dhcp();
                return true;
            }
        }
    }
    let Some(id) = wpa(&["add_network"]).and_then(|t| t.trim().parse::<u32>().ok()) else {
        return false;
    };
    let id = id.to_string();
    let quoted = format!("\"{ssid}\"");
    let ok = wpa(&["set_network", &id, "ssid", &quoted]).is_some()
        && match psk {
            Some(pass) => {
                let quoted = format!("\"{pass}\"");
                wpa(&["set_network", &id, "psk", &quoted]).is_some()
            }
            None => wpa(&["set_network", &id, "key_mgmt", "NONE"]).is_some(),
        };
    if !ok {
        let _ = wpa(&["remove_network", &id]);
        return false;
    }
    let _ = wpa(&["enable_network", &id]);
    let _ = wpa(&["select_network", &id]);
    let _ = wpa(&["save_config"]);
    start_dhcp();
    log(&format!("network: connecting to {ssid}"));
    true
}

#[cfg(target_os = "linux")]
pub fn wifi_forget(ssid: &str) -> bool {
    let known = wpa(&["list_networks"]).unwrap_or_default();
    for line in known.lines().skip(1) {
        let mut parts = line.split('\t');
        if let (Some(id), Some(s)) = (parts.next(), parts.next()) {
            if s == ssid {
                let _ = wpa(&["remove_network", id]);
                let _ = wpa(&["save_config"]);
                log(&format!("network: forgot {ssid}"));
                return true;
            }
        }
    }
    false
}

/// Wi-Fi radio state: (up, connected, ssid, ip).
#[cfg(target_os = "linux")]
pub fn wifi_state() -> (bool, bool, String, String) {
    let up = wifi_up();
    let status = wpa(&["status"]).unwrap_or_default();
    let (state, ssid, ip) = parse_wpa_status(&status);
    (up, state == "COMPLETED", ssid, ip)
}

/// The Bluetooth controller is attached (hci0 present).
#[cfg(target_os = "linux")]
pub fn bt_on() -> bool {
    std::path::Path::new("/sys/class/bluetooth/hci0").exists()
}

/// Wired headphones. The h700_speaker_amp module exposes the jack as a
/// GPIO input named "Headphone detection".
#[cfg(target_os = "linux")]
pub fn headphones_in() -> bool {
    std::fs::read_to_string("/sys/kernel/debug/gpio")
        .map(|text| gpio_headphones(&text))
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
pub fn headphones_in() -> bool {
    false
}

#[cfg(any(target_os = "linux", test))]
fn gpio_headphones(text: &str) -> bool {
    text.lines()
        .find(|l| l.contains("Headphone detection"))
        .and_then(|l| l.rsplit_once(')'))
        .is_some_and(|(_, state)| state.split_whitespace().nth(1) == Some("hi"))
}

/// A2DP needs an audio endpoint; BlueZ alone has no profile on BaseOS.
/// BlueALSA is shipped but never started, so start it with the adapter.
#[cfg(target_os = "linux")]
fn ensure_bt_audio() {
    if process_running("bluealsa") {
        return;
    }
    match std::process::Command::new("bluealsa")
        .args(["-p", "a2dp-source", "-p", "a2dp-sink"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => {
            std::thread::sleep(std::time::Duration::from_millis(400));
            log(&format!(
                "network: bluealsa {}",
                if process_running("bluealsa") {
                    "started"
                } else {
                    "failed to start"
                }
            ));
        }
        Err(e) => log(&format!("network: bluealsa unavailable: {e}")),
    }
}

/// The BlueALSA PCM for the connected A2DP sink, if any. The full
/// `bluealsa:DEV=...` string pins the device (otherwise ALSA uses the most
/// recent one).
#[cfg(any(target_os = "linux", test))]
pub fn parse_bluealsa_pcm(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        (line.starts_with("bluealsa:") && line.contains("DEV=") && line.contains("PROFILE=a2dp"))
            .then(|| line.to_string())
    })
}

#[cfg(target_os = "linux")]
pub fn bt_audio_device() -> Option<String> {
    if !process_running("bluealsa") {
        return None;
    }
    let out = std::process::Command::new("timeout")
        .args(["3", "bluealsa-aplay", "-L"])
        .output()
        .ok()?;
    parse_bluealsa_pcm(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub fn bt_audio_device() -> Option<String> {
    None
}

/// The MAGI pairing agent: a NoInputNoOutput org.bluez.Agent1 served over
/// D-Bus in this process (bluetoothctl's own agent fails to register here).
#[cfg(target_os = "linux")]
struct BtAgent;

#[cfg(target_os = "linux")]
#[zbus::interface(name = "org.bluez.Agent1")]
impl BtAgent {
    fn release(&self) {}

    fn cancel(&self) {}

    fn request_pin_code(&self, _device: zbus::zvariant::OwnedObjectPath) -> String {
        "0000".into()
    }

    fn request_passkey(&self, _device: zbus::zvariant::OwnedObjectPath) -> u32 {
        0
    }

    fn request_confirmation(&self, _device: zbus::zvariant::OwnedObjectPath, _passkey: u32) {}

    fn request_authorization(&self, _device: zbus::zvariant::OwnedObjectPath) {}
}
#[cfg(target_os = "linux")]
struct BtAgents {
    zbus: Option<zbus::blocking::Connection>,
    ctl: Option<std::process::Child>,
}

#[cfg(target_os = "linux")]
static BT_AGENT: std::sync::Mutex<Option<BtAgents>> = std::sync::Mutex::new(None);

#[cfg(target_os = "linux")]
fn ensure_bt_agent() {
    let mut guard = BT_AGENT.lock().unwrap();
    if guard.is_some() {
        return;
    }
    // Stale agent instances hold the agent object path.
    kill_by_name("bluetoothctl");
    std::thread::sleep(std::time::Duration::from_millis(300));
    let mut agents = BtAgents {
        zbus: None,
        ctl: None,
    };
    // bluetoothctl registers its NoInputNoOutput agent at startup (--agent),
    // unlike the REPL `agent` command, which fails to register here.
    let mut cmd = std::process::Command::new("bluetoothctl");
    cmd.args(["--agent", "NoInputNoOutput"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match cmd.spawn() {
        Ok(child) => {
            log_agent("bluetoothctl --agent running");
            agents.ctl = Some(child);
        }
        Err(e) => log_agent(&format!("bluetoothctl --agent failed: {e}")),
    }
    // And our own D-Bus agent as belt and braces.
    let attempt = || -> Result<zbus::blocking::Connection, zbus::Error> {
        let conn = zbus::blocking::Connection::system()?;
        conn.object_server()
            .at("/magi/agent", BtAgent)
            .map_err(zbus::Error::from)?;
        conn.call_method(
            Some("org.bluez"),
            "/org/bluez",
            Some("org.bluez.AgentManager1"),
            "RegisterAgent",
            &(
                zbus::zvariant::ObjectPath::try_from("/magi/agent").map_err(zbus::Error::from)?,
                "NoInputNoOutput",
            ),
        )?;
        conn.call_method(
            Some("org.bluez"),
            "/org/bluez",
            Some("org.bluez.AgentManager1"),
            "RequestDefaultAgent",
            &zbus::zvariant::ObjectPath::try_from("/magi/agent").map_err(zbus::Error::from)?,
        )?;
        Ok(conn)
    };
    match attempt() {
        Ok(conn) => {
            log_agent("pairing agent registered");
            agents.zbus = Some(conn);
        }
        Err(e) => log_agent(&format!("pairing agent failed: {e}")),
    }
    *guard = Some(agents);
}

#[cfg(target_os = "linux")]
fn log_agent(msg: &str) {
    log(&format!("network: {msg}"));
}

#[cfg(target_os = "linux")]
fn stop_bt_agent() {
    let mut guard = BT_AGENT.lock().unwrap();
    if let Some(mut agents) = guard.take() {
        if let Some(mut c) = agents.ctl.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
/// Bounded bluetoothctl: never call it while bluetoothd is down (it blocks
/// forever), never without a timeout.
#[cfg(target_os = "linux")]
fn btctl(args: &[&str]) -> Option<String> {
    btctl_t(args, 6)
}

/// Bounded bluetoothctl with a per-operation timeout: pairing and connecting
/// take tens of seconds, while status calls must stay short.
#[cfg(target_os = "linux")]
fn btctl_t(args: &[&str], secs: u32) -> Option<String> {
    if !process_running("bluetoothd") {
        return None;
    }
    let out = std::process::Command::new("timeout")
        .arg(secs.to_string())
        .arg("bluetoothctl")
        .args(args)
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Attach the RTL8821CS: rfkill unblock, clear a stale attach and the vendor
/// lockfile, reload the wake module, run the vendor hciattach, hciconfig up,
/// restart BlueZ for a deterministic adapter, then power it on (bounded).
#[cfg(target_os = "linux")]
pub fn set_bt(on: bool) -> bool {
    if on {
        let _ = std::process::Command::new("rfkill")
            .args(["unblock", "bluetooth"])
            .output();
        let mut attached_now = false;
        if !bt_on() {
            // Give the chip two chances: a fresh attach right after a busy
            // session can miss the H5 sync window.
            for attempt in 0..2 {
                if process_running("rtk_hciattach") {
                    // An interrupted attach holds ttyS1; restart it cleanly.
                    log("network: clearing stale rtk_hciattach");
                    kill_by_name("rtk_hciattach");
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                let _ = std::fs::remove_file("/tmp/.init_bt");
                let _ = std::process::Command::new("rmmod")
                    .arg("rtl_btlpm")
                    .output();
                let _ = std::process::Command::new("insmod")
                    .args(["/lib/modules/rtl_btlpm.ko"])
                    .output();
                let _ = std::fs::create_dir_all("Logs");
                let logf = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("Logs/magi.log")
                    .ok();
                let mut cmd = std::process::Command::new("rtk_hciattach");
                cmd.args(["-n", "-s", "115200", "ttyS1", "rtk_h5"]);
                if let Some(f) = logf {
                    if let Ok(f2) = f.try_clone() {
                        cmd.stdout(f).stderr(f2);
                    }
                }
                let _ = cmd.spawn();
                for _ in 0..16 {
                    if bt_on() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                if bt_on() {
                    break;
                }
                log(&format!("network: attach attempt {} failed", attempt + 1));
            }
            attached_now = bt_on();
        }
        if bt_on() {
            let _ = std::process::Command::new("hciconfig")
                .args(["hci0", "up"])
                .output();
            let _ = std::process::Command::new("hciconfig")
                .args(["-a", "hci0", "lm", "master"])
                .output();
            let _ = std::process::Command::new("hciconfig")
                .args(["hci0", "auth"])
                .output();
            let _ = systemctl(&[if attached_now { "restart" } else { "start" }, "bluetooth"]);
            for _ in 0..20 {
                if process_running("bluetoothd") {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            for _ in 0..5 {
                let powered = btctl(&["show"]).map_or(false, |s| s.contains("Powered: yes"));
                if powered {
                    break;
                }
                let _ = btctl(&["power", "on"]);
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            ensure_bt_smp();
            ensure_bt_audio();
            // Discoverable forever: BaseOS leaves BlueZ's 180 s default.
            let _ = std::process::Command::new("dbus-send")
                .args([
                    "--system",
                    "--print-reply",
                    "--dest=org.bluez",
                    "/org/bluez/hci0",
                    "org.freedesktop.DBus.Properties.Set",
                    "string:org.bluez.Adapter1",
                    "string:DiscoverableTimeout",
                    "variant:uint32:0",
                ])
                .output();
            let _ = btctl(&["discoverable", "on"]);
            let _ = btctl(&["pairable", "on"]);
            // The adapter announces itself with our hostname, not "BlueZ
            // 5.66"; set it and verify (bluetoothd may still be settling).
            if let Some(host) = hostname_get() {
                for _ in 0..5 {
                    let _ = btctl_t(&["system-alias", &host], 6);
                    let ok = btctl_t(&["show"], 6)
                        .map_or(false, |s| s.contains(&format!("Alias: {host}")));
                    if ok {
                        log(&format!("network: bluetooth alias {host}"));
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
            ensure_bt_agent();
        }
    } else {
        stop_bt_agent();
        kill_by_name("bluealsa");
        kill_by_name("bluetoothctl");
        if process_running("bluetoothd") {
            let _ = btctl(&["scan", "off"]);
            let _ = btctl(&["power", "off"]);
        }
        kill_by_name("bluetoothctl");
        if !systemctl(&["stop", "bluetooth"]) {
            kill_by_name("bluetoothd");
        }
        let _ = std::process::Command::new("hciconfig")
            .args(["hci0", "down"])
            .output();
        kill_by_name("rtk_hciattach");
        let _ = std::fs::remove_file("/tmp/.init_bt");
        let _ = std::process::Command::new("rfkill")
            .args(["block", "bluetooth"])
            .output();
    }
    std::thread::sleep(std::time::Duration::from_millis(800));
    let ok = bt_on() == on;
    log(&format!("network: bluetooth {on} ({ok})"));
    ok
}
/// `bluetoothctl devices` lines: Device XX:..:XX Name.
#[cfg(target_os = "linux")]
pub fn parse_bt_devices(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("Device ") else {
            continue;
        };
        let (mac, name) = match rest.split_once(' ') {
            Some((mac, name)) => (mac, name.trim()),
            None => (rest.trim(), ""),
        };
        if !mac.is_empty() {
            out.push((
                mac.to_string(),
                if name.is_empty() {
                    mac.to_string()
                } else {
                    name.to_string()
                },
            ));
        }
    }
    out
}

/// Known Bluetooth devices; a short scan when `scan` is set.
#[cfg(target_os = "linux")]
pub fn bt_devices(scan: bool) -> Vec<(String, String)> {
    if !bt_on() {
        return Vec::new();
    }
    if scan {
        let _ = std::process::Command::new("timeout")
            .args(["9", "bluetoothctl", "--timeout", "8", "scan", "on"])
            .output();
    }
    btctl(&["devices"])
        .map(|t| parse_bt_devices(&t))
        .unwrap_or_default()
}

/// The kernel only registers the LE SMP channel when the adapter is powered
/// on through BlueZ; BaseOS' own script brings hci0 up first, so BLE pairing
/// fails until hci0 is power-cycled once.
#[cfg(target_os = "linux")]
fn smp_ready() -> bool {
    std::path::Path::new("/sys/kernel/debug/bluetooth/hci0/le_min_key_size").exists()
}

#[cfg(target_os = "linux")]
fn ensure_bt_smp() {
    if smp_ready() {
        return;
    }
    log("network: LE SMP missing; power-cycling hci0");
    let _ = btctl_t(&["power", "off"], 8);
    std::thread::sleep(std::time::Duration::from_millis(800));
    let _ = btctl_t(&["power", "on"], 8);
    std::thread::sleep(std::time::Duration::from_millis(800));
    log(&format!(
        "network: LE SMP {}",
        if smp_ready() {
            "registered"
        } else {
            "still missing"
        }
    ));
}

#[cfg(target_os = "linux")]
pub fn bt_connect(mac: &str) -> bool {
    ensure_bt_audio();
    ensure_bt_agent();
    ensure_bt_smp();
    // Unpaired LE devices are temporary in BlueZ and vanish when discovery
    // stops, so a scan must stay alive across pair/trust/connect.
    let mut scan = std::process::Command::new("timeout")
        .args(["40", "bluetoothctl", "--timeout", "38", "scan", "on"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok();
    std::thread::sleep(std::time::Duration::from_secs(3));
    let mut pair = btctl_t(&["pair", mac], 20).unwrap_or_default();
    if !pair.contains("successful") && !pair.contains("already") {
        // Controllers often miss the first request while waking up.
        std::thread::sleep(std::time::Duration::from_secs(1));
        pair = btctl_t(&["pair", mac], 20).unwrap_or_default();
    }
    let _ = btctl_t(&["trust", mac], 6);
    let conn = btctl_t(&["connect", mac], 15).unwrap_or_default();
    let _ = btctl_t(&["scan", "off"], 3);
    if let Some(mut s) = scan.take() {
        let _ = s.kill();
        let _ = s.wait();
    }
    let info = btctl(&["info", mac]).unwrap_or_default();
    let paired = info.contains("Paired: yes");
    let connected = info.contains("Connected: yes");
    let tail = |s: &str| {
        s.lines()
            .filter(|l| !l.trim().is_empty())
            .next_back()
            .unwrap_or("")
            .to_string()
    };
    log(&format!(
        "network: bt connect {mac} (paired {paired}, connected {connected}) pair='{}' connect='{}'",
        tail(&pair),
        tail(&conn)
    ));
    paired && connected
}

#[cfg(target_os = "linux")]
const RADIO_CONFIG: &str = "Config/radio.json";

/// Radio choices live in their own file: the shelf's frequent state saves
/// must never race a radio toggle and drop the saved value.
#[cfg(target_os = "linux")]
static RADIO_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Persist a radio choice; BaseOS powers wlan0 up at every boot, so the
/// saved state is re-applied by the frontend at startup.
#[cfg(target_os = "linux")]
pub fn save_radio_state(key: &str, on: bool) {
    let _guard = RADIO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = std::path::Path::new(RADIO_CONFIG);
    let mut v: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    v[key] = serde_json::json!(on);
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::create_dir_all("Config");
        if let Err(e) = write_file_atomic(path, text.as_bytes()) {
            log(&format!("network: cannot save radio state: {e}"));
        }
    }
}

#[cfg(target_os = "linux")]
pub fn saved_radio_state(key: &str) -> Option<bool> {
    std::fs::read_to_string(RADIO_CONFIG)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get(key).and_then(|x| x.as_bool()))
        .or_else(|| {
            std::fs::read_to_string("Config/magi.json")
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| v.get(key).and_then(|x| x.as_bool()))
        })
}

/// Persist a string choice (e.g. the hostname); other keys survive.
#[cfg(target_os = "linux")]
pub fn save_string_pref(key: &str, value: &str) {
    save_pref(key, serde_json::json!(value));
}

#[cfg(target_os = "linux")]
pub fn saved_string_pref(key: &str) -> Option<String> {
    std::fs::read_to_string("Config/magi.json")
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string()))
}

#[cfg(target_os = "linux")]
fn save_pref(key: &str, value: serde_json::Value) {
    let path = std::path::Path::new("Config/magi.json");
    let mut v: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    v[key] = value;
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::create_dir_all("Config");
        let _ = write_file_atomic(path, text.as_bytes());
    }
}

/// The power LED: a write-only AXP2202 attribute (no readback, no colour
/// channels on these boards — on/off only).
#[cfg(target_os = "linux")]
pub fn set_led(on: bool) -> bool {
    std::fs::write(
        "/sys/class/power_supply/axp2202-battery/work_led",
        if on { "1" } else { "0" },
    )
    .is_ok()
}

#[cfg(not(target_os = "linux"))]
pub fn set_led(_on: bool) -> bool {
    false
}

/// Set the CPU governor on every core; `schedutil` by default, `performance`
/// for the heaviest systems (PS1, Neo CD and native N64). Returns true if any
/// write landed.
#[cfg(target_os = "linux")]
pub fn set_cpu_governor(governor: &str) -> bool {
    let mut written = 0;
    for cpu in 0..8 {
        let path = format!("/sys/devices/system/cpu/cpu{cpu}/cpufreq/scaling_governor");
        if !std::path::Path::new(&path).exists() {
            continue;
        }
        match std::fs::write(&path, governor) {
            Ok(()) => written += 1,
            Err(e) => log(&format!("cpu: cannot set {governor} on cpu{cpu}: {e}")),
        }
    }
    if written > 0 {
        log(&format!("cpu: governor {governor} ({written} cores)"));
        true
    } else {
        false
    }
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub fn set_cpu_governor(_governor: &str) -> bool {
    false
}

/// The LED-during-sleep switch: 0 keeps the power LED dark while suspended
/// (the default MAGI sets), 1 leaves it on.
#[cfg(target_os = "linux")]
pub fn set_led_sleep(on: bool) -> bool {
    std::fs::write(
        "/sys/class/power_supply/axp2202-battery/workled_sleep",
        if on { "1" } else { "0" },
    )
    .is_ok()
}

/// A short vibration-motor pulse (the RUMBLE toggle's test buzz); off the
/// UI thread so the settings screen never stalls.
#[cfg(target_os = "linux")]
pub fn rumble_pulse() {
    std::thread::spawn(|| {
        use std::io::Write;
        let path = "/sys/class/power_supply/axp2202-battery/moto";
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(path) {
            let _ = f.write_all(b"1");
            std::thread::sleep(std::time::Duration::from_millis(160));
            let _ = f.write_all(b"0");
        }
    });
}

#[cfg(not(target_os = "linux"))]
pub fn rumble_pulse() {}

/// Keep the speaker amplifier off while the headphone route is selected.
#[cfg(target_os = "linux")]
pub fn speaker_amp_off() {
    let _ = std::process::Command::new("amixer")
        .args(["-c", "0", "sset", &amixer_control("SPK"), "off"])
        .output();
}

/// Set the ALSA mixer playback volume (0..100). Ports play through ALSA
/// directly, so this is how MAGI's volume setting governs them; the
/// libretro host keeps its own software gain (and resets this to 100).
#[cfg(target_os = "linux")]
pub fn set_mixer_volume(pct: u8) {
    type OpenFn = unsafe extern "C" fn(*mut *mut libc::c_void, libc::c_int) -> libc::c_int;
    type AttachFn = unsafe extern "C" fn(*mut libc::c_void, *const libc::c_char) -> libc::c_int;
    type RegisterFn = unsafe extern "C" fn(
        *mut libc::c_void,
        *mut libc::c_void,
        *mut libc::c_void,
    ) -> libc::c_int;
    type LoadFn = unsafe extern "C" fn(*mut libc::c_void) -> libc::c_int;
    type FirstFn = unsafe extern "C" fn(*mut libc::c_void) -> *mut libc::c_void;
    type NextFn = unsafe extern "C" fn(*mut libc::c_void) -> *mut libc::c_void;
    type GetTypeFn = unsafe extern "C" fn(*const libc::c_void) -> libc::c_int;
    type IsActiveFn = unsafe extern "C" fn(*const libc::c_void) -> libc::c_int;
    type HasVolFn = unsafe extern "C" fn(*const libc::c_void) -> libc::c_int;
    type RangeFn = unsafe extern "C" fn(
        *const libc::c_void,
        *mut libc::c_long,
        *mut libc::c_long,
    ) -> libc::c_int;
    type SetAllFn = unsafe extern "C" fn(*mut libc::c_void, libc::c_long) -> libc::c_int;
    type NameFn = unsafe extern "C" fn(*const libc::c_void) -> *const libc::c_char;
    type CloseFn = unsafe extern "C" fn(*mut libc::c_void) -> libc::c_int;

    unsafe fn load<F>(lib: *mut libc::c_void, name: &[u8]) -> Option<F> {
        let p = libc::dlsym(lib, name.as_ptr() as *const libc::c_char);
        if p.is_null() {
            None
        } else {
            Some(std::mem::transmute_copy(&p))
        }
    }

    unsafe {
        let lib = libc::dlopen(
            b"libasound.so.2\0".as_ptr() as *const libc::c_char,
            libc::RTLD_NOW,
        );
        if lib.is_null() {
            log("mixer: libasound.so.2 not found");
            return;
        }
        let (Some(open), Some(attach), Some(register), Some(load_m), Some(first), Some(next)) = (
            load::<OpenFn>(lib, b"snd_mixer_open\0"),
            load::<AttachFn>(lib, b"snd_mixer_attach\0"),
            load::<RegisterFn>(lib, b"snd_mixer_selem_register\0"),
            load::<LoadFn>(lib, b"snd_mixer_load\0"),
            load::<FirstFn>(lib, b"snd_mixer_first_elem\0"),
            load::<NextFn>(lib, b"snd_mixer_elem_next\0"),
        ) else {
            log("mixer: libasound lacks the mixer API");
            return;
        };
        let (Some(get_type), Some(is_active), Some(has_vol), Some(range), Some(set_all)) = (
            load::<GetTypeFn>(lib, b"snd_mixer_elem_get_type\0"),
            load::<IsActiveFn>(lib, b"snd_mixer_selem_is_active\0"),
            load::<HasVolFn>(lib, b"snd_mixer_selem_has_playback_volume\0"),
            load::<RangeFn>(lib, b"snd_mixer_selem_get_playback_volume_range\0"),
            load::<SetAllFn>(lib, b"snd_mixer_selem_set_playback_volume_all\0"),
        ) else {
            log("mixer: libasound lacks the simple-element API");
            return;
        };
        let name_of = load::<NameFn>(lib, b"snd_mixer_selem_get_name\0");
        let close = load::<CloseFn>(lib, b"snd_mixer_close\0");

        let mut mixer: *mut libc::c_void = std::ptr::null_mut();
        if open(&mut mixer, 0) < 0 || mixer.is_null() {
            log("mixer: cannot open");
            return;
        }
        let ok = attach(mixer, b"default\0".as_ptr() as *const libc::c_char) >= 0
            && register(mixer, std::ptr::null_mut(), std::ptr::null_mut()) >= 0
            && load_m(mixer) >= 0;
        if !ok {
            log("mixer: cannot attach 'default'");
            if let Some(close) = close {
                close(mixer);
            }
            return;
        }

        // Prefer a main playback control by name, else the first usable one.
        const PREFERRED: [&str; 5] = ["playback", "pcm", "digital", "dac", "master"];
        let mut chosen: *mut libc::c_void = std::ptr::null_mut();
        let mut chosen_name = String::new();
        let mut elem = first(mixer);
        while !elem.is_null() {
            if get_type(elem) == 1 && is_active(elem) != 0 && has_vol(elem) != 0 {
                let mut min: libc::c_long = 0;
                let mut max: libc::c_long = 0;
                if range(elem, &mut min, &mut max) >= 0 && max > min {
                    let elem_name = name_of
                        .map(|f| {
                            let p = f(elem);
                            if p.is_null() {
                                String::new()
                            } else {
                                std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
                            }
                        })
                        .unwrap_or_default();
                    let lower = elem_name.to_ascii_lowercase();
                    let preferred = PREFERRED.iter().any(|p| lower.contains(p));
                    if chosen.is_null() || preferred {
                        chosen = elem;
                        chosen_name = elem_name.clone();
                        if preferred && lower.contains("playback") {
                            break;
                        }
                    }
                }
            }
            elem = next(elem);
        }

        if chosen.is_null() {
            log("mixer: no playback volume control found");
            // The mixer API reports the H700's "digital volume" as inactive
            // (its route is off), but amixer still reaches it.
            if let Some(ctrl) = volume_discover() {
                if volume_set(&ctrl, u32::from(pct)) {
                    log(&format!("mixer: '{ctrl}' -> {pct}%"));
                } else {
                    log(&format!("mixer: cannot set '{ctrl}'"));
                }
            }
        } else {
            let mut min: libc::c_long = 0;
            let mut max: libc::c_long = 0;
            range(chosen, &mut min, &mut max);
            let value = min + (max - min) * (pct.min(100) as libc::c_long) / 100;
            if set_all(chosen, value) < 0 {
                log(&format!("mixer: cannot set '{}'", chosen_name));
            } else {
                log(&format!("mixer: '{}' -> {pct}%", chosen_name));
            }
        }
        if let Some(close) = close {
            close(mixer);
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn set_mixer_volume(_pct: u8) {}

#[cfg(not(target_os = "linux"))]
pub fn set_led_sleep(_on: bool) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn save_radio_state(_key: &str, _on: bool) {}

#[cfg(not(target_os = "linux"))]
pub fn saved_radio_state(_key: &str) -> Option<bool> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn save_string_pref(_key: &str, _value: &str) {}

/// MACs among `macs` that are currently connected (per-device info; the
/// `devices Connected` filter silently lists everything on some builds).
#[cfg(target_os = "linux")]
pub fn bt_connected(macs: &[String]) -> Vec<String> {
    if !process_running("bluetoothd") {
        return Vec::new();
    }
    macs.iter()
        .filter(|mac| btctl_t(&["info", mac], 3).map_or(false, |t| t.contains("Connected: yes")))
        .cloned()
        .collect()
}

#[cfg(target_os = "linux")]
pub fn bt_forget(mac: &str) -> bool {
    let _ = btctl(&["remove", mac]);
    log(&format!("network: bt forgot {mac}"));
    true
}

/// SSH via the BaseOS dropbear service.
#[cfg(target_os = "linux")]
pub fn ssh_on() -> bool {
    process_running("dropbear")
}

#[cfg(target_os = "linux")]
pub fn set_ssh(on: bool) -> bool {
    if on {
        if !ssh_on() {
            // The BaseOS systemctl shim has no unit for dropbear (the dev hook
            // starts it); ask the shim first, then start the daemon directly.
            let _ = systemctl(&["start", "dropbear"]);
            if !ssh_on() {
                start_dropbear();
            }
        }
    } else {
        let _ = systemctl(&["stop", "dropbear"]);
        // The shim does not always know dropbear; make sure it is gone.
        kill_by_name("dropbear");
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    let now = ssh_on();
    log(&format!(
        "network: ssh {} ({now})",
        if on { "start" } else { "stop" }
    ));
    now == on
}

/// Start dropbear with the host keys BaseOS keeps in /data, generating any
/// that are missing. The root password comes from /etc/shadow.
#[cfg(target_os = "linux")]
fn start_dropbear() {
    use std::process::Stdio;
    let _ = std::fs::create_dir_all("/data/dropbear");
    let mut args: Vec<String> = Vec::new();
    for kind in ["rsa", "ed25519"] {
        let key = format!("/data/dropbear/dropbear_{kind}_host_key");
        if !std::path::Path::new(&key).exists() {
            let _ = std::process::Command::new("dropbearkey")
                .args(["-t", kind, "-f", &key])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        if std::path::Path::new(&key).exists() {
            args.push("-r".into());
            args.push(key);
        }
    }
    match std::process::Command::new("dropbear")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => std::thread::sleep(std::time::Duration::from_millis(400)),
        Err(e) => log(&format!("ssh: cannot start dropbear: {e}")),
    }
}

/// NTP through the BaseOS supervisor: /data/ntp-enabled holds "yes"/"no",
/// and the supervisor only runs while enabled — enabling starts it (with a
/// custom server when set), disabling stops it.
#[cfg(target_os = "linux")]
pub fn ntp_on() -> bool {
    std::fs::read_to_string("/data/ntp-enabled")
        .map(|s| s.trim() == "yes")
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub fn set_ntp(on: bool) -> bool {
    let ok = std::fs::write("/data/ntp-enabled", if on { "yes\n" } else { "no\n" }).is_ok();
    ntp_supervisor(&ntp_server_pref());
    log(&format!(
        "network: ntp {} ({ok})",
        if on { "on" } else { "off" }
    ));
    ok
}

/// Persist and apply a custom NTP server (empty restores the default).
#[cfg(target_os = "linux")]
pub fn set_ntp_server(server: &str) -> bool {
    let server = server.trim();
    let valid = server.is_empty()
        || server
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    if !valid {
        return false;
    }
    let path = std::path::Path::new("Config/magi.json");
    let mut v: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    v["ntp_server"] = serde_json::json!(server);
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = write_file_atomic(path, text.as_bytes());
    }
    log(&format!("network: ntp server set to '{server}'"));
    ntp_supervisor(server);
    true
}

/// The saved NTP server override (empty = BaseOS pool).
#[cfg(target_os = "linux")]
pub fn ntp_server_pref() -> String {
    std::fs::read_to_string("Config/magi.json")
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| {
            v.get("ntp_server")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn ntp_supervisor(server: &str) {
    kill_cmdline("baseos-ntp");
    std::thread::sleep(std::time::Duration::from_millis(300));
    if !ntp_on() {
        return;
    }
    let mut cmd = std::process::Command::new("/usr/sbin/baseos-ntp");
    if !server.is_empty() {
        cmd.env("BASEOS_NTP_SERVERS", server);
    }
    let _ = cmd.spawn();
}

/// Re-apply the custom NTP server at startup (the BaseOS supervisor started
/// by rcS only knows the defaults).
#[cfg(target_os = "linux")]
pub fn ntp_reapply() {
    let server = ntp_server_pref();
    if !server.is_empty() && ntp_on() {
        ntp_supervisor(&server);
    }
}

#[cfg(not(target_os = "linux"))]
pub fn ntp_server_pref() -> String {
    String::new()
}

/// Cheap state for the Wi-Fi page rows: the connected SSID and the saved
/// set, without a scan (so CONNECTED lights up the moment it is true).
#[cfg(target_os = "linux")]
pub fn wifi_states() -> (String, Vec<String>) {
    let status = wpa(&["status"]).unwrap_or_default();
    let (state, ssid, _ip) = parse_wpa_status(&status);
    let current = if state == "COMPLETED" {
        ssid
    } else {
        String::new()
    };
    let known = wpa(&["list_networks"]).unwrap_or_default();
    let saved = parse_wifi_networks(&known)
        .into_iter()
        .map(|(s, _)| s)
        .collect();
    (current, saved)
}

/// Signal strength of the current connection, 0-100.
#[cfg(target_os = "linux")]
pub fn wifi_signal_pct() -> Option<u8> {
    let out = wpa(&["signal_poll"])?;
    for line in out.lines() {
        if let Some(v) = line.strip_prefix("RSSI=") {
            let rssi: i32 = v.trim().parse().ok()?;
            return Some((2 * (rssi + 100)).clamp(0, 100) as u8);
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
pub fn wifi_states() -> (String, Vec<String>) {
    (String::new(), Vec::new())
}

#[cfg(not(target_os = "linux"))]
pub fn wifi_signal_pct() -> Option<u8> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn bt_connected(_macs: &[String]) -> Vec<String> {
    Vec::new()
}

#[cfg(not(target_os = "linux"))]
pub fn set_ntp_server(_server: &str) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn ntp_reapply() {}

#[cfg(target_os = "linux")]
pub fn ntp_synced() -> bool {
    std::path::Path::new("/run/baseos-ntp-synchronized").exists()
}

/// The hostname in use, from the kernel.
#[cfg(target_os = "linux")]
pub fn hostname_get() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
}

/// baseos-config's rule: 1-63 ASCII letters, digits or hyphens.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn hostname_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Set the hostname now and persist it in TF1's baseos.conf for boots.
#[cfg(target_os = "linux")]
pub fn set_hostname(name: &str) -> bool {
    if !hostname_valid(name) {
        return false;
    }
    let live = std::fs::write("/proc/sys/kernel/hostname", format!("{name}\n")).is_ok();
    let path = std::path::Path::new("/mnt/sdcard/baseos.conf");
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in text.lines() {
        let is_host = line.trim_start().starts_with("hostname=");
        if is_host && !line.trim_start().starts_with('#') {
            lines.push(format!("hostname={name}"));
            replaced = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !replaced {
        lines.push(format!("hostname={name}"));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    let saved = write_file_atomic(path, out.as_bytes()).is_ok();
    log(&format!(
        "network: hostname {name} (live {live}, saved {saved})"
    ));
    live
}

#[cfg(not(target_os = "linux"))]
pub fn wifi_up() -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn set_wifi(_on: bool) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn wifi_scan() -> Vec<WifiNet> {
    Vec::new()
}

#[cfg(not(target_os = "linux"))]
pub fn wifi_connect(_ssid: &str, _psk: Option<&str>) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn wifi_forget(_ssid: &str) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn wifi_state() -> (bool, bool, String, String) {
    (false, false, String::new(), String::new())
}

#[cfg(not(target_os = "linux"))]
pub fn bt_on() -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn set_bt(_on: bool) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn bt_devices(_scan: bool) -> Vec<(String, String)> {
    Vec::new()
}

#[cfg(not(target_os = "linux"))]
pub fn bt_connect(_mac: &str) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn bt_forget(_mac: &str) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn ssh_on() -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn set_ssh(_on: bool) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn ntp_on() -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn set_ntp(_on: bool) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn ntp_synced() -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn hostname_get() -> Option<String> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn set_hostname(_name: &str) -> bool {
    false
}

/// Run a shell command detached (sleep, poweroff); logs the outcome.
pub fn spawn_detached(cmdline: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        log(&format!("spawn: sh -c '{cmdline}'"));
        return std::process::Command::new("/bin/sh")
            .args(["-c", cmdline])
            .spawn()
            .map(|_| true)
            .unwrap_or_else(|e| {
                log(&format!("spawn failed: {e}"));
                false
            });
    }
    #[cfg(not(target_os = "linux"))]
    {
        log(&format!("spawn (ignored on this platform): {cmdline}"));
        false
    }
}

/// `YYYYMMDD-HHMMSS` where a clock is available, epoch seconds otherwise.
pub fn timestamp() -> String {
    #[cfg(target_os = "linux")]
    if let Ok(out) = std::process::Command::new("date")
        .arg("+%Y%m%d-%H%M%S")
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// A command that re-runs this binary. BaseOS may start the frontend
/// through the dynamic loader (cards are FAT32, no executable bit), which
/// makes `current_exe()` the loader itself — so the real program path is
/// taken from `argv[0]`, and the spawn mirrors how BaseOS invokes it.
#[cfg(unix)]
pub fn spawn_self() -> std::io::Result<std::process::Command> {
    let program = program_path();
    match elf_interpreter(&program) {
        Some(interp) => {
            log(&format!(
                "spawn: {} via {}",
                program.display(),
                interp.display()
            ));
            let mut c = std::process::Command::new(interp);
            c.arg(program);
            Ok(c)
        }
        None => {
            log(&format!("spawn: {}", program.display()));
            Ok(std::process::Command::new(program))
        }
    }
}

#[cfg(not(unix))]
pub fn spawn_self() -> std::io::Result<std::process::Command> {
    Ok(std::process::Command::new(program_path()))
}

/// The binary that should be re-executed: `argv[0]` when it names a real
/// file (run through a loader, this is the program), else `current_exe()`.
fn program_path() -> std::path::PathBuf {
    if let Some(a0) = std::env::args_os().next() {
        let p = std::path::PathBuf::from(a0);
        if p.is_file() {
            return p;
        }
    }
    std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("magi"))
}

#[cfg(unix)]
fn elf_interpreter(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let data = std::fs::read(path).ok()?;
    parse_pinterp(&data)
}

/// PT_INTERP of a 64-bit little-endian ELF, if any.
#[cfg(any(unix, test))]
fn parse_pinterp(data: &[u8]) -> Option<std::path::PathBuf> {
    if data.len() < 64 || &data[0..4] != b"\x7fELF" || data[4] != 2 || data[5] != 1 {
        return None;
    }
    let phoff = u64::from_le_bytes(data[0x20..0x28].try_into().ok()?) as usize;
    let phentsize = u16::from_le_bytes(data[0x36..0x38].try_into().ok()?) as usize;
    let phnum = u16::from_le_bytes(data[0x38..0x3a].try_into().ok()?) as usize;
    for i in 0..phnum {
        let off = phoff + i * phentsize;
        if off + 40 > data.len() || phentsize < 40 {
            return None;
        }
        let p_type = u32::from_le_bytes(data[off..off + 4].try_into().ok()?);
        if p_type == 3 {
            let p_offset = u64::from_le_bytes(data[off + 8..off + 16].try_into().ok()?) as usize;
            let p_filesz = u64::from_le_bytes(data[off + 32..off + 40].try_into().ok()?) as usize;
            if p_filesz > 0 && p_offset + p_filesz <= data.len() {
                let s = std::str::from_utf8(&data[p_offset..p_offset + p_filesz - 1]).ok()?;
                return Some(std::path::PathBuf::from(s));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("magi-write-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("file.json")
    }

    #[test]
    fn finds_the_card_mount() {
        let text = "/dev/root / ext4 rw 0 0\n/dev/mmcblk0p7 /mnt/sdcard vfat ro,noatime 0 0\n";
        let mount = card_mount_in(text, "/mnt/sdcard").unwrap();
        assert_eq!(mount.device, "/dev/mmcblk0p7");
        assert_eq!(mount.point, "/mnt/sdcard");
        assert_eq!(mount.fs, "vfat");
        assert!(card_mount_in(text, "/data").is_none());
    }

    #[test]
    fn atomic_write_swaps_the_file_and_leaves_no_temp() {
        let path = scratch("atomic");
        write_file_atomic(&path, b"first").unwrap();
        write_file_atomic(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn release_values_skip_comments_and_quotes() {
        let text = "# Generated by build-rootfs.sh; do not edit.\n\
                    BASEOS=1\nBASEOS_VERSION=1.4.0\nBASEOS_BUILD=\"v1.4.0\"\n\
                    BASEOS_MODEL=\"RG35XX SP\"\n";
        assert_eq!(
            release_value(text, &["BASEOS_VERSION"]),
            Some("1.4.0".into())
        );
        assert_eq!(
            release_value(text, &["BASEOS_BUILD", "BUILD"]),
            Some("v1.4.0".into())
        );
        assert_eq!(
            release_value(text, &["BASEOS_MODEL"]),
            Some("RG35XX SP".into())
        );
        assert_eq!(release_value(text, &["MISSING"]), None);
    }

    #[test]
    fn wifi_scan_lines_parse_and_dedup() {
        let text = "bssid / frequency / signal level / flags / ssid\n\
                    aa:bb:cc:dd:ee:01\t2412\t-45\t[WPA2-PSK-CCMP][ESS]\tHomeNet\n\
                    aa:bb:cc:dd:ee:02\t2437\t-70\t[WPA2-PSK-CCMP][ESS]\tHomeNet\n\
                    aa:bb:cc:dd:ee:03\t2462\t-60\t[ESS]\tCafe\n\
                    aa:bb:cc:dd:ee:04\t5180\t-80\t[WPA2-PSK-CCMP][ESS]\t\n";
        let nets = parse_wifi_scan(text);
        assert_eq!(nets.len(), 2, "dedup by ssid, hidden skipped");
        assert_eq!(
            nets[0],
            ("HomeNet".into(), 100, true),
            "50+50 clamped to 100"
        );
        assert_eq!(nets[1], ("Cafe".into(), 80, false), "open network");
    }

    #[test]
    fn capped_log_rolls_over_at_the_cap() {
        let path = std::env::temp_dir().join(format!("magi_log_cap_{}", std::process::id()));
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        let lines = (LOG_CAP / 50) as usize;
        for i in 0..lines {
            let line = format!("line {i:04} ................................................\n");
            append_capped(&mut file, line.as_bytes()).unwrap();
        }
        drop(file);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.len() as u64 <= LOG_CAP);
        assert!(
            text.contains(&format!("line {:04}", lines - 1)),
            "keeps the newest lines"
        );
        assert!(!text.contains("line 0000"), "starts over at the cap");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn headphone_jack_reads_the_gpio_line() {
        let hi = " gpio-259 (                    |Headphone detection ) in  hi\n";
        let lo = " gpio-259 (                    |Headphone detection ) in  lo\n";
        assert!(gpio_headphones(hi));
        assert!(!gpio_headphones(lo));
        assert!(!gpio_headphones(""));
    }

    #[test]
    fn bluealsa_pcm_parser_pins_the_device() {
        let text = "bluealsa:DEV=90:7A:58:17:1F:6F,PROFILE=a2dp,SRV=org.bluealsa\n\
                    \x20   WI-C200, trusted audio-headset, playback\n";
        assert_eq!(
            parse_bluealsa_pcm(text).as_deref(),
            Some("bluealsa:DEV=90:7A:58:17:1F:6F,PROFILE=a2dp,SRV=org.bluealsa")
        );
        assert!(parse_bluealsa_pcm("bluealsa:DEV=AA,PROFILE=sco,SRV=x\n").is_none());
        assert!(parse_bluealsa_pcm("").is_none());
    }

    #[test]
    fn finds_pinterp_in_a_crafted_elf() {
        let mut buf = vec![0u8; 256];
        buf[0..4].copy_from_slice(b"\x7fELF");
        buf[4] = 2; // 64-bit
        buf[5] = 1; // little-endian
        buf[0x20..0x28].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
        buf[0x36..0x38].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
        buf[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
                                                              // program header 0: PT_INTERP at 64
        buf[64..68].copy_from_slice(&3u32.to_le_bytes());
        buf[72..80].copy_from_slice(&192u64.to_le_bytes()); // p_offset
        buf[96..104].copy_from_slice(&16u64.to_le_bytes()); // p_filesz
        buf[192..208].copy_from_slice(b"/lib/ld-test.so\0");
        assert_eq!(
            parse_pinterp(&buf),
            Some(std::path::PathBuf::from("/lib/ld-test.so"))
        );
        assert_eq!(parse_pinterp(b"not an elf"), None);
    }

    #[test]
    fn twelve_hour_clock_handles_midnight_and_noon() {
        assert_eq!(
            clock_12h_from_hhmm("00:05"),
            Some(("12:05".to_string(), false))
        );
        assert_eq!(
            clock_12h_from_hhmm("12:05"),
            Some(("12:05".to_string(), true))
        );
        assert_eq!(
            clock_12h_from_hhmm("13:05"),
            Some(("01:05".to_string(), true))
        );
        assert_eq!(clock_12h_from_hhmm("--:--"), None);
    }
}

/// Discover the mixer control to use for volume on H700 ALSA setups
/// (`digital volume` on vendor images; Master/PCM as fallbacks). The answer
/// is cached: the control does not change while the console runs.
#[cfg(target_os = "linux")]
pub fn volume_discover() -> Option<String> {
    static CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHE.get_or_init(discover_mixer_control).clone()
}

#[cfg(target_os = "linux")]
fn discover_mixer_control() -> Option<String> {
    let out = std::process::Command::new("amixer")
        .args(["-c", "0", "scontrols"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut names: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.split('\'').nth(1) {
            names.push(rest.to_string());
        }
    }
    for candidate in ["digital volume", "Soft Volume", "Master", "PCM", "DAC"] {
        if names.iter().any(|n| n == candidate) && volume_get(candidate).is_some() {
            return Some(candidate.to_string());
        }
    }
    for name in &names {
        if volume_get(name).is_some() {
            return Some(name.clone());
        }
    }
    None
}

/// The control name as amixer wants it: the names carry spaces ("digital
/// volume"), so they have to reach amixer quoted.
#[cfg(target_os = "linux")]
fn amixer_control(ctrl: &str) -> String {
    format!("'{ctrl}'")
}

#[cfg(target_os = "linux")]
pub fn volume_get(ctrl: &str) -> Option<u32> {
    let out = std::process::Command::new("amixer")
        .args(["-c", "0", "sget", &amixer_control(ctrl)])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let pct_pos = text.find('%')?;
    let digits: String = text[..pct_pos]
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    digits.parse().ok()
}

#[cfg(target_os = "linux")]
pub fn volume_set(ctrl: &str, pct: u32) -> bool {
    std::process::Command::new("amixer")
        .args([
            "-c",
            "0",
            "sset",
            &amixer_control(ctrl),
            &format!("{}%", pct.min(100)),
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Suspend to RAM, with the Hall lid left as a wake source and an RTC alarm
/// that powers off if the sleep outlives `shutdown_secs` (0 disables the
/// timer). BaseOS boots the stock Super Standby bit (16) on the AXP2202,
/// which excludes the hall from the wake set (POWER-only wake); clearing
/// the bit for the sleep makes closing the lid sleep the device and opening
/// it wake it, then the stock value goes back. Blocks until resume.
/// Arm a wake alarm on an RTC that can actually wake the H700.
///
/// `rtc1` is the SoC's sunxi RTC and the wakeupgen IRQ source. It has its own
/// counter, so the alarm is programmed relatively (`+seconds`) and does not
/// depend on the wall clock in `rtc0` (the pcf8563, which has no alarm IRQ).
/// Keep `rtc0` as a fallback for other boards, using an absolute epoch.
#[cfg(target_os = "linux")]
fn arm_wake_alarm(seconds: u32, deadline: u64) -> Option<String> {
    const RTCS: [(&str, bool); 2] = [
        ("/sys/class/rtc/rtc1", true),
        ("/sys/class/rtc/rtc0", false),
    ];
    for (rtc, relative) in RTCS {
        let alarm = format!("{rtc}/wakealarm");
        if std::fs::metadata(&alarm).is_err() {
            continue;
        }
        let _ = std::fs::write(format!("{rtc}/device/power/wakeup"), "enabled");
        let _ = std::fs::write(&alarm, "0");
        let value = if relative {
            format!("+{seconds}")
        } else {
            deadline.to_string()
        };
        if std::fs::write(&alarm, value).is_err() {
            continue;
        }
        let readback = std::fs::read_to_string(&alarm)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if readback.is_empty() {
            continue;
        }
        log(&format!("sleep: alarm on {rtc} (readback '{readback}')"));
        return Some(alarm);
    }
    None
}

#[cfg(target_os = "linux")]
pub fn sleep_now(shutdown_secs: u32) {
    const OS_SLEEP: &str = "/sys/class/power_supply/axp2202-battery/os_sleep";

    // The attribute reads back empty; only restore a real value.
    let prev = std::fs::read_to_string(OS_SLEEP)
        .ok()
        .filter(|v| !v.trim().is_empty());
    let _ = std::fs::write(OS_SLEEP, "0");

    let deadline = if shutdown_secs > 0 {
        epoch_secs().map(|now| now + shutdown_secs as u64)
    } else {
        None
    };
    // A wake with the lid still shut (a key pressed in a bag) has to go back
    // to sleep: the hall only reports an edge, so nothing else would.
    let mut attempt = 0;
    loop {
        let remaining = deadline.map(|d| {
            epoch_secs().map_or(shutdown_secs, |now| {
                d.saturating_sub(now).min(u64::from(shutdown_secs)) as u32
            })
        });
        // The power LED goes dark in sleep by default.
        set_led_sleep(false);
        let alarm = match (deadline, remaining) {
            (Some(d), Some(secs)) => arm_wake_alarm(secs, d),
            _ => None,
        };
        if deadline.is_some() && alarm.is_none() {
            log("sleep: cannot arm the RTC alarm; no auto power off");
        }

        log("sleep: entering suspend");
        match std::fs::write("/sys/power/state", "mem") {
            Ok(()) => log("sleep: resumed"),
            Err(e) => log(&format!("sleep: suspend failed: {e}")),
        }
        // The resume path forgets the LED register; the saved choice is ours
        // to keep, so re-assert it on every wake (and keep sleep dark).
        set_led(saved_radio_state("led_on").unwrap_or(crate::defaults::Defaults::LED_ON));
        set_led_sleep(false);

        // The timer is wall-clock based: if the deadline passed while
        // suspended, power off even when a later lid opening is what woke us.
        let timed_out = deadline.is_some_and(|d| epoch_secs().is_some_and(|now| now + 1 >= d));

        if let Some(path) = &alarm {
            let _ = std::fs::write(path, "0");
        }
        if timed_out {
            log("sleep: shutdown timer expired; powering off");
            // Darken the panel before the (multi-second) poweroff so the timer
            // never looks like the device woke up in the meantime.
            set_brightness(0);
            if !spawn_detached("poweroff") {
                log("sleep: cannot run poweroff");
            }
            std::thread::sleep(std::time::Duration::from_secs(3));
            break;
        }
        attempt += 1;
        if lid_closed() != Some(true) || attempt > 7 {
            break;
        }
        log(&format!(
            "sleep: lid still closed after wake; sleeping again ({attempt})"
        ));
    }
    if let Some(p) = prev {
        let _ = std::fs::write(OS_SLEEP, p.trim());
    }
}

#[cfg(not(target_os = "linux"))]
pub fn sleep_now(_shutdown_secs: u32) {
    log("sleep: not supported on this platform");
}
/// The H700's hall switch reports no input events at all on this kernel;
/// its state is the `hallkey` attribute (1 = lid open, 0 = closed, verified
/// on hardware). Poll it and report close edges.
#[cfg(target_os = "linux")]
pub struct LidWatch {
    last: Option<bool>,
    next: std::time::Instant,
}

#[cfg(target_os = "linux")]
impl LidWatch {
    pub fn new() -> Self {
        Self {
            last: lid_closed(),
            next: std::time::Instant::now(),
        }
    }

    /// True once, on the poll that sees the lid just closed.
    pub fn poll(&mut self) -> bool {
        let now = std::time::Instant::now();
        if now < self.next {
            return false;
        }
        self.next = now + std::time::Duration::from_millis(120);
        let cur = lid_closed();
        let closed = lid_transition(self.last, cur);
        if cur.is_some() {
            self.last = cur;
        }
        closed
    }
}

#[cfg(target_os = "linux")]
// ---- HDMI out (sunxi disp2) -------------------------------------------
// Flip disp0 between the LCD and the HDMI TX through the dispdbg debugfs
// files, then read-modify-write the fb0 scanout layer (the BSP fbdev glue
// has no set_par) so the DE scales our 640x480 page into the 720p mode we
// ask for. The framebuffer keeps the panel geometry.

/// HDMI cable state (extcon first, switch fallback).
#[cfg(target_os = "linux")]
pub fn hdmi_cable() -> bool {
    for p in [
        "/sys/class/extcon/hdmi/state",
        "/sys/class/extcon/hdmi/cable.0/state",
        "/sys/class/extcon/hdmi/cable.1/state",
        "/sys/class/switch/hdmi/cable.0/state",
    ] {
        if let Ok(s) = std::fs::read_to_string(p) {
            let s = s.trim();
            if s == "HDMI=1" || s == "1" {
                return true;
            }
            if s == "HDMI=0" || s == "0" {
                return false;
            }
        }
    }
    false
}

/// The live output type, read back from the disp driver.
#[cfg(target_os = "linux")]
pub fn hdmi_active() -> bool {
    std::fs::read_to_string("/sys/class/disp/disp/attr/sys")
        .map(|s| s.contains("hdmi output"))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Clone, Copy)]
struct DispRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Clone, Copy)]
struct DispRectSz {
    width: u32,
    height: u32,
}

#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Clone, Copy)]
struct DispRect64 {
    x: i64,
    y: i64,
    width: i64,
    height: i64,
}

#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Clone, Copy)]
struct DispFbInfo {
    addr: [u64; 3],
    size: [DispRectSz; 3],
    align: [u32; 3],
    format: i32,
    color_space: i32,
    trd_right_addr: [u32; 3],
    pre_multiply: bool,
    crop: DispRect64,
    flags: i32,
    scan: i32,
}

#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Clone, Copy)]
struct DispLayerInfo {
    mode: i32,
    zorder: u8,
    alpha_mode: u8,
    alpha_value: u8,
    screen_win: DispRect,
    b_trd_out: bool,
    out_trd_mode: i32,
    fb: DispFbInfo,
    id: u32,
}

#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Clone, Copy)]
struct DispLayerConfig {
    info: DispLayerInfo,
    enable: bool,
    channel: u32,
    layer_id: u32,
}

#[cfg(target_os = "linux")]
const DISP_LAYER_SET_CONFIG: libc::c_ulong = 0x47;
#[cfg(target_os = "linux")]
const DISP_LAYER_GET_CONFIG: libc::c_ulong = 0x48;

#[cfg(target_os = "linux")]
const _: () = assert!(std::mem::size_of::<DispLayerConfig>() == 184);

/// Commit the fb0 scanout layer geometry for the live output; retries until
/// a read-back shows the config stuck (late async restores are common).
#[cfg(target_os = "linux")]
fn disp_layer_commit(on: bool) -> bool {
    use std::os::unix::io::AsRawFd;
    let Some((fbw, fbh, vh)) = crate::present::fbdev::fb_geometry() else {
        return false;
    };
    let Ok(disp) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/disp")
    else {
        return false;
    };
    let (wx, wy, ww, wh) = if on {
        ((1920 - 1440) / 2, 0, 1440, 1080)
    } else {
        (0, 0, fbw, fbh)
    };
    let mut applied = false;
    for _ in 0..20 {
        let mut cfg: DispLayerConfig = unsafe { std::mem::zeroed() };
        cfg.channel = 1;
        cfg.layer_id = 0;
        let mut param: [libc::c_ulong; 4] =
            [0, (&mut cfg as *mut DispLayerConfig) as libc::c_ulong, 1, 0];
        if unsafe { libc::ioctl(disp.as_raw_fd(), DISP_LAYER_GET_CONFIG, param.as_mut_ptr()) } < 0 {
            break;
        }
        cfg.enable = true;
        for i in 0..3 {
            cfg.info.fb.size[i].width = fbw;
            cfg.info.fb.size[i].height = vh;
        }
        cfg.info.fb.crop.x = 0;
        cfg.info.fb.crop.y = 0;
        cfg.info.fb.crop.width = (fbw as i64) << 32;
        cfg.info.fb.crop.height = (fbh as i64) << 32;
        cfg.info.screen_win = DispRect {
            x: wx,
            y: wy,
            width: ww,
            height: wh,
        };
        if unsafe { libc::ioctl(disp.as_raw_fd(), DISP_LAYER_SET_CONFIG, param.as_mut_ptr()) } < 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut check: DispLayerConfig = unsafe { std::mem::zeroed() };
        check.channel = 1;
        check.layer_id = 0;
        let mut p2: [libc::c_ulong; 4] = [
            0,
            (&mut check as *mut DispLayerConfig) as libc::c_ulong,
            1,
            0,
        ];
        if unsafe { libc::ioctl(disp.as_raw_fd(), DISP_LAYER_GET_CONFIG, p2.as_mut_ptr()) } < 0 {
            break;
        }
        applied = check.info.fb.size[0].width == fbw
            && check.info.screen_win.width == ww
            && check.info.fb.crop.y == 0;
    }
    applied
}

/// Bytes in both framebuffer pages, from sysfs; falls back to the 640x480
/// double-buffer size.
#[cfg(target_os = "linux")]
fn fb_bytes() -> usize {
    let text = std::fs::read_to_string("/sys/class/graphics/fb0/virtual_size").unwrap_or_default();
    let (w, h) = text.trim().split_once(',').unwrap_or(("640", "960"));
    let bpp = std::fs::read_to_string("/sys/class/graphics/fb0/bits_per_pixel")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(32);
    let w = w.trim().parse::<usize>().unwrap_or(640);
    let h = h.trim().parse::<usize>().unwrap_or(960);
    w.saturating_mul(h).saturating_mul(bpp / 8)
}

/// Switch disp0 to the HDMI TX (true) or the LCD panel (false). Blocks for
/// ~1s while the output attaches; the panel shows black until it settles.
#[cfg(target_os = "linux")]
pub fn set_hdmi(on: bool) -> bool {
    if hdmi_active() == on {
        return true;
    }
    log(&format!(
        "hdmi: switching to {}",
        if on { "hdmi (1080p60 TV)" } else { "lcd" }
    ));
    let _ = std::fs::write("/sys/class/graphics/fb0/blank", "4");
    if let Ok(mut fb) = std::fs::OpenOptions::new().write(true).open("/dev/fb0") {
        use std::io::Write;
        let zeros = [0u8; 4096];
        let mut left = fb_bytes();
        while left > 0 {
            let n = left.min(zeros.len());
            if fb.write_all(&zeros[..n]).is_err() {
                break;
            }
            left -= n;
        }
    }
    let param = if on {
        "4 10 0 0 0x4 0x101 0 0 0 8"
    } else {
        "1 0"
    };
    let _ = std::fs::write("/sys/kernel/debug/dispdbg/name", "disp0");
    let _ = std::fs::write("/sys/kernel/debug/dispdbg/command", "switch");
    let _ = std::fs::write("/sys/kernel/debug/dispdbg/param", param);
    let _ = std::fs::write("/sys/kernel/debug/dispdbg/start", "1");
    let mut attached = false;
    for _ in 0..60 {
        if hdmi_active() == on {
            attached = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if attached {
        std::thread::sleep(std::time::Duration::from_millis(150));
    } else {
        log("hdmi: output did not attach in 3s");
    }
    let _ = disp_layer_commit(on);
    std::thread::sleep(std::time::Duration::from_millis(250));
    let _ = std::fs::write("/sys/class/graphics/fb0/blank", "0");
    let ok = disp_layer_commit(on);
    if !ok {
        log("hdmi: layer geometry did not stick");
    }
    ok
}

/// Sync the output to the cable at process start (before video opens).
#[cfg(target_os = "linux")]
pub fn hdmi_init() {
    let cable = hdmi_cable();
    if cable != hdmi_active() {
        set_hdmi(cable);
    } else {
        log(&format!(
            "hdmi: cable {}; output {}",
            if cable { "in" } else { "out" },
            if cable { "hdmi" } else { "lcd" }
        ));
    }
}

// ---- card repair -----------------------------------------------------
// The card is mounted with `errors=remount-ro`; a bad FAT chain turns the
// whole volume read-only. The shelf offers a repair, and this is the
// unmounted fsck path it hands over to.

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct CardMount {
    device: String,
    point: String,
    fs: String,
}

#[cfg(any(target_os = "linux", test))]
fn card_mount_in(text: &str, point: &str) -> Option<CardMount> {
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(device), Some(mounted), Some(fs)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if mounted == point && device.starts_with("/dev/") {
            return Some(CardMount {
                device: device.to_string(),
                point: mounted.to_string(),
                fs: fs.to_string(),
            });
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn card_mount() -> Option<CardMount> {
    let text = std::fs::read_to_string("/proc/mounts").ok()?;
    card_mount_in(&text, "/mnt/sdcard")
}

/// True when the card's mount is read-only (the kernel remounts it after a
/// FAT error; the card may still look writable from a PC).
#[cfg(target_os = "linux")]
pub fn card_read_only() -> bool {
    card_read_only_at("/mnt/sdcard")
}

#[cfg(target_os = "linux")]
fn card_read_only_at(path: &str) -> bool {
    let Ok(path) = std::ffi::CString::new(path) else {
        return false;
    };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
        return false;
    }
    (st.f_flag & (libc::ST_RDONLY as libc::c_ulong)) != 0
}

#[cfg(not(target_os = "linux"))]
pub fn card_read_only() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn repair_tool(fs: &str) -> Option<&'static str> {
    let tools: &'static [&'static str] = match fs {
        "vfat" => &["/sbin/fsck.fat", "/usr/sbin/fsck.fat", "/bin/fsck.fat"],
        "exfat" => &[
            "/sbin/fsck.exfat",
            "/usr/sbin/fsck.exfat",
            "/bin/fsck.exfat",
        ],
        _ => return None,
    };
    for &tool in tools {
        if std::path::Path::new(tool).is_file() {
            return Some(tool);
        }
    }
    None
}

/// A repair can run here only when the card's filesystem has an fsck tool.
#[cfg(target_os = "linux")]
pub fn card_repair_available() -> bool {
    card_mount().and_then(|m| repair_tool(&m.fs)).is_some()
}

#[cfg(not(target_os = "linux"))]
pub fn card_repair_available() -> bool {
    false
}

/// `exec` the staged repair script in place of the frontend. The card can
/// then be unmounted; BaseOS init respawns the frontend only after reboot.
#[cfg(target_os = "linux")]
pub fn start_card_repair() -> std::io::Error {
    use std::io::{Error, ErrorKind};
    let Some(m) = card_mount() else {
        return Error::new(ErrorKind::NotFound, "no card mount");
    };
    if repair_tool(&m.fs).is_none() {
        return Error::new(ErrorKind::NotFound, "no fsck for this card");
    }
    if let Err(e) = std::fs::write("/tmp/magi-repair.sh", include_str!("repair.sh")) {
        return e;
    }
    eprintln!("magi: repair: {} {} {}", m.device, m.point, m.fs);
    close_extra_fds();
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("/tmp/magi-repair.sh")
        .arg(&m.device)
        .arg(&m.point)
        .arg(&m.fs)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.exec()
}

#[cfg(not(target_os = "linux"))]
pub fn start_card_repair() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Unsupported, "device only")
}

/// Close every descriptor above stderr before the repair exec, so the card
/// has no inherited file handles when the script unmounts it.
#[cfg(target_os = "linux")]
fn close_extra_fds() {
    let Ok(dir) = std::fs::read_dir("/proc/self/fd") else {
        return;
    };
    for entry in dir.flatten() {
        let Ok(fd) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if fd > 2 {
            unsafe {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
    }
}

/// Written by repair.sh; read once on the next boot so the result is not
/// only visible in the repair log.
#[cfg(target_os = "linux")]
pub fn take_repair_result() -> Option<&'static str> {
    let path = "/data/magi-repair.result";
    let text = std::fs::read_to_string(path).ok()?;
    let _ = std::fs::remove_file(path);
    Some(match text.trim() {
        "repaired" => "SD CARD REPAIRED",
        "clean" => "SD CARD OK",
        _ => "REPAIR FAILED: USE A COMPUTER",
    })
}

#[cfg(not(target_os = "linux"))]
pub fn take_repair_result() -> Option<&'static str> {
    None
}

// ---- boot logo (U-Boot splash) ---------------------------------------
// The bootloader draws `bootlogo.bmp` from the `boot-resource` partition.
// System/bootlogo-<w>x<h>.bmp on the card is the source; the panel size
// comes from the logo already there (backed up once as
// bootlogo.baseos.bmp).

#[cfg(target_os = "linux")]
const BOOTLOGO_MOUNT: &str = "/tmp/magi-boot-resource";

/// Dims of an uncompressed, bottom-up, 24-bit BMP3 - the only kind the
/// bootloader draws; verifies the whole file matches those dims.
#[cfg(target_os = "linux")]
fn bmp_dims(bmp: &[u8]) -> Option<(u32, u32)> {
    if bmp.len() < 54 || &bmp[0..2] != b"BM" {
        return None;
    }
    let u16_at = |i: usize| u16::from_le_bytes([bmp[i], bmp[i + 1]]) as u32;
    let u32_at = |i: usize| u32::from_le_bytes([bmp[i], bmp[i + 1], bmp[i + 2], bmp[i + 3]]);
    let (w, h) = (u32_at(18), u32_at(22));
    let stride = ((w as usize * 3 + 3) / 4) * 4;
    let well_formed = u32_at(10) == 54
        && u32_at(14) == 40
        && u16_at(26) == 1
        && u16_at(28) == 24
        && u32_at(30) == 0
        && (1..=4096).contains(&w)
        && (1..=4096).contains(&h);
    (well_formed && bmp.len() == 54 + stride * h as usize).then_some((w, h))
}

#[cfg(target_os = "linux")]
fn boot_resource_partition() -> Option<String> {
    for entry in std::fs::read_dir("/sys/class/block").ok()?.flatten() {
        let uevent = std::fs::read_to_string(entry.path().join("uevent")).unwrap_or_default();
        if uevent.lines().any(|l| l == "PARTNAME=boot-resource") {
            return Some(format!(
                "/dev/{}",
                entry.file_name().to_string_lossy().into_owned()
            ));
        }
    }
    None
}

/// Install System/bootlogo-<w>x<h>.bmp as the bootloader splash, sized by
/// the logo already on the boot-resource partition. No-op when the card
/// ships no logo (desktop/dev) or the partition is missing.
#[cfg(target_os = "linux")]
pub fn refresh_bootlogo() {
    let system = std::path::Path::new("System");
    let ships = std::fs::read_dir(system)
        .map(|rd| {
            rd.flatten().any(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.starts_with("bootlogo") && n.ends_with(".bmp")
            })
        })
        .unwrap_or(false);
    if !ships {
        return;
    }
    let Some(dev) = boot_resource_partition() else {
        log("bootlogo: no boot-resource partition");
        return;
    };
    let mounted = std::fs::create_dir_all(BOOTLOGO_MOUNT).is_ok()
        && std::process::Command::new("mount")
            .args(["-t", "vfat", "-o", "rw,noatime", &dev, BOOTLOGO_MOUNT])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    if !mounted {
        log(&format!("bootlogo: could not mount {dev}"));
        return;
    }
    let dir = std::path::Path::new(BOOTLOGO_MOUNT);
    let size = ["bootlogo.baseos.bmp", "bootlogo.bmp"]
        .iter()
        .find_map(|n| std::fs::read(dir.join(n)).ok().and_then(|b| bmp_dims(&b)));
    let Some((w, h)) = size else {
        log("bootlogo: no logo on the partition to size ours by");
        let _ = std::process::Command::new("umount")
            .arg(BOOTLOGO_MOUNT)
            .status();
        return;
    };
    let source = system.join(format!("bootlogo-{w}x{h}.bmp"));
    let Some(logo) = std::fs::read(&source).ok() else {
        log(&format!(
            "bootlogo: {} missing for this panel",
            source.display()
        ));
        let _ = std::process::Command::new("umount")
            .arg(BOOTLOGO_MOUNT)
            .status();
        return;
    };
    if bmp_dims(&logo) != Some((w, h)) {
        log(&format!("bootlogo: {} is the wrong size", source.display()));
        let _ = std::process::Command::new("umount")
            .arg(BOOTLOGO_MOUNT)
            .status();
        return;
    }
    let current = std::fs::read(dir.join("bootlogo.bmp")).ok();
    if current.as_deref() == Some(logo.as_slice()) {
        log("bootlogo: up to date");
    } else {
        if let Some(cur) = &current {
            let backup = dir.join("bootlogo.baseos.bmp");
            if !backup.exists() {
                let _ = write_file_atomic(&backup, cur);
            }
        }
        match write_file_atomic(&dir.join("bootlogo.bmp"), &logo) {
            Ok(()) => log(&format!("bootlogo: installed {}", source.display())),
            Err(e) => log(&format!("bootlogo: install failed: {e}")),
        }
    }
    if !std::process::Command::new("umount")
        .arg(BOOTLOGO_MOUNT)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        log("bootlogo: could not unmount");
    }
}

/// Polls the cable every call; yields the new state when it changes.
#[cfg(target_os = "linux")]
pub struct HdmiWatch {
    cable: Option<bool>,
    last: Option<std::time::Instant>,
}

#[cfg(target_os = "linux")]
impl HdmiWatch {
    pub fn new() -> Self {
        Self {
            cable: None,
            last: None,
        }
    }

    /// Cheap enough for a per-frame call: the cable is only re-read twice a
    /// second.
    pub fn poll(&mut self) -> Option<bool> {
        let now = std::time::Instant::now();
        if let Some(last) = self.last {
            if now.duration_since(last) < std::time::Duration::from_secs(2) {
                return None;
            }
        }
        self.last = Some(now);
        let cable = hdmi_cable();
        let changed = self.cable != Some(cable);
        self.cable = Some(cable);
        if changed {
            Some(cable)
        } else {
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn lid_closed() -> Option<bool> {
    let v = std::fs::read_to_string("/sys/class/power_supply/axp2202-battery/hallkey").ok()?;
    match v.trim() {
        "0" => Some(true),
        "1" => Some(false),
        _ => None,
    }
}

/// A close is an open/unknown -> closed edge; the first read never fires.
#[cfg(target_os = "linux")]
fn lid_transition(prev: Option<bool>, cur: Option<bool>) -> bool {
    matches!((prev, cur), (Some(false), Some(true)))
}

#[cfg(target_os = "linux")]
fn epoch_secs() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// Panel brightness on the H700: vendor kernels expose no backlight class,
/// so it is the Allwinner disp2 ioctl on /dev/disp (0-255, stock 180).
/// Percent in, percent out at this API.
#[cfg(target_os = "linux")]
const DISP_LCD_SET_BRIGHTNESS: libc::c_ulong = 0x102;
#[cfg(target_os = "linux")]
const DISP_LCD_GET_BRIGHTNESS: libc::c_ulong = 0x103;

/// The brightness the panel boots with (the driver starts at 180 of 255).
/// Used when nothing was saved yet: the panel keeps what it shows.
pub const STOCK_BRIGHTNESS: u32 = 70;

#[cfg(target_os = "linux")]
fn disp_file() -> Option<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/disp")
        .ok()
}

#[cfg(target_os = "linux")]
pub fn brightness_now() -> Option<u32> {
    let f = disp_file()?;
    let mut args: [libc::c_ulong; 4] = [0; 4];
    let rc = unsafe {
        libc::ioctl(
            std::os::unix::io::AsRawFd::as_raw_fd(&f),
            DISP_LCD_GET_BRIGHTNESS,
            args.as_mut_ptr(),
        )
    };
    if rc < 0 {
        None
    } else {
        Some((rc as u32).min(255) * 100 / 255)
    }
}

/// Set panel brightness (0-100%); false when /dev/disp is unavailable.
#[cfg(target_os = "linux")]
pub fn set_brightness(pct: u32) -> bool {
    let Some(f) = disp_file() else { return false };
    let value = (pct.min(100) * 255 + 50) / 100;
    let mut args: [libc::c_ulong; 4] = [0, value as libc::c_ulong, 0, 0];
    let rc = unsafe {
        libc::ioctl(
            std::os::unix::io::AsRawFd::as_raw_fd(&f),
            DISP_LCD_SET_BRIGHTNESS,
            args.as_mut_ptr(),
        )
    };
    rc >= 0
}

#[cfg(not(target_os = "linux"))]
pub fn brightness_now() -> Option<u32> {
    None
}

/// A `KEY=VALUE` (optionally quoted) from a release-style file.
#[cfg(any(target_os = "linux", test))]
pub fn release_value(text: &str, keys: &[&str]) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if keys.iter().any(|k| k.eq_ignore_ascii_case(key.trim())) {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// System summary for the ABOUT screen.
#[cfg(target_os = "linux")]
pub fn system_lines() -> Vec<(String, String)> {
    fn read(p: &str) -> Option<String> {
        std::fs::read_to_string(p)
            .ok()
            .map(|s| s.trim().to_string())
    }
    let release = read("/etc/baseos-release").unwrap_or_default();
    let version = release_value(&release, &["BASEOS_VERSION", "VERSION"]);
    let build = release_value(&release, &["BASEOS_BUILD", "BUILD"]);
    let firmware = match (version, build) {
        (Some(v), Some(b)) if b != v => format!("{v} ({b})"),
        (Some(v), _) => v,
        _ => "--".into(),
    };
    let memory = read("/proc/meminfo")
        .and_then(|t| {
            let kb: u64 = t.lines().next()?.split_whitespace().nth(1)?.parse().ok()?;
            Some(format!("{} MB", kb / 1024))
        })
        .unwrap_or_else(|| "--".into());
    let uptime = read("/proc/uptime")
        .and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok())
        .map(|s| format!("{}h {:02}m", (s / 3600.0) as u64, ((s / 60.0) as u64) % 60))
        .unwrap_or_else(|| "--".into());
    let battery = read("/sys/class/power_supply/axp2202-battery/status").unwrap_or_default();
    vec![
        ("MAGI".into(), env!("CARGO_PKG_VERSION").to_string()),
        ("CREATOR".into(), "otacoo".into()),
        ("MODEL".into(), crate::device::detect().name.to_string()),
        ("FIRMWARE".into(), firmware),
        (
            "KERNEL".into(),
            read("/proc/sys/kernel/osrelease").unwrap_or_else(|| "--".into()),
        ),
        ("MEMORY".into(), memory),
        ("STORAGE".into(), storage_line()),
        ("UPTIME".into(), uptime),
        (
            "BATTERY".into(),
            match battery_percent() {
                Some(p) if battery.is_empty() => format!("{p}%"),
                Some(p) => format!("{p}% ({battery})"),
                None => "--".into(),
            },
        ),
    ]
}

#[cfg(not(target_os = "linux"))]
pub fn system_lines() -> Vec<(String, String)> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn storage_line() -> String {
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        let Ok(path) = std::ffi::CString::new(".") else {
            return "--".into();
        };
        if libc::statvfs(path.as_ptr(), &mut st) == 0 {
            let mb = |b: u64| b / (1024 * 1024);
            let total = st.f_blocks as u64 * st.f_frsize as u64;
            let free = st.f_bavail as u64 * st.f_frsize as u64;
            format!("{} MB free / {} MB", mb(free), mb(total))
        } else {
            "--".into()
        }
    }
}
