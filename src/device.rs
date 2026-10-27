//! Panel profiles and device detection for the H700 family.
//!
//! The session env and `/etc/baseos-release` come from BaseOS; the profiles
//! below mirror BaseOS's devices.json (panel geometry) plus MAGI's own needs.

#[derive(Clone, Copy, Debug)]
pub struct Device {
    pub id: &'static str,
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
}

const PROFILES: &[Device] = &[
    Device {
        id: "rg35xxsp",
        name: "ANBERNIC RG35XXSP",
        width: 640,
        height: 480,
    },
    Device {
        id: "rg35xxplus",
        name: "ANBERNIC RG35XX PLUS",
        width: 640,
        height: 480,
    },
    Device {
        id: "rg35xx2024",
        name: "ANBERNIC RG35XX 2024",
        width: 640,
        height: 480,
    },
    Device {
        id: "rg35xxh",
        name: "ANBERNIC RG35XX H",
        width: 640,
        height: 480,
    },
    Device {
        id: "rg35xxpro",
        name: "ANBERNIC RG35XX PRO",
        width: 640,
        height: 480,
    },
    Device {
        id: "rg40xxh",
        name: "ANBERNIC RG40XX H",
        width: 640,
        height: 480,
    },
    Device {
        id: "rg40xxv",
        name: "ANBERNIC RG40XX V",
        width: 640,
        height: 480,
    },
    Device {
        id: "rg34xx",
        name: "ANBERNIC RG34XX",
        width: 720,
        height: 480,
    },
    Device {
        id: "rg34xxsp",
        name: "ANBERNIC RG34XXSP",
        width: 720,
        height: 480,
    },
    Device {
        id: "rgsp",
        name: "ANBERNIC RG SP",
        width: 720,
        height: 480,
    },
    Device {
        id: "rgcubexx",
        name: "ANBERNIC RG CubeXX",
        width: 720,
        height: 720,
    },
    // Rotated panel; MAGI will render in landscape and rotate at presentation.
    Device {
        id: "rg28xx",
        name: "ANBERNIC RG28XX",
        width: 640,
        height: 480,
    },
];

impl Device {
    /// True when the panel is wider than 4:3 (the 3:2 720x480 models).
    pub fn wide(&self) -> bool {
        self.width * 3 > self.height * 4
    }

    /// Model name without the manufacturer, for compact on-screen labels.
    pub fn short_name(&self) -> &'static str {
        self.name.strip_prefix("ANBERNIC ").unwrap_or(self.name)
    }
}

pub fn default_device() -> &'static Device {
    &PROFILES[0]
}

pub fn detect() -> &'static Device {
    if let Ok(id) = std::env::var("MAGI_DEVICE") {
        if let Some(d) = find(&id) {
            crate::util::log(&format!("device from MAGI_DEVICE: {}", d.id));
            return d;
        }
        crate::util::log(&format!(
            "MAGI_DEVICE={id} is not a known profile; ignoring"
        ));
    }
    if let Some(d) = read_baseos_release() {
        crate::util::log(&format!("device from /etc/baseos-release: {}", d.id));
        return d;
    }
    let d = default_device();
    crate::util::log(&format!("no device found; defaulting to {}", d.id));
    d
}

fn find(id: &str) -> Option<&'static Device> {
    let key: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    PROFILES.iter().find(|d| d.id == key).or_else(|| {
        PROFILES.iter().find(|d| {
            let id: String = d.id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            key.contains(&id)
        })
    })
}

#[cfg(target_os = "linux")]
fn read_baseos_release() -> Option<&'static Device> {
    let text = std::fs::read_to_string("/etc/baseos-release").ok()?;
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.to_ascii_uppercase();
        if key.contains("DEVICE") || key.contains("TARGET") || key.contains("MODEL_STRING") {
            let value = value.trim().trim_matches('"');
            if let Some(d) = find(value) {
                return Some(d);
            }
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn read_baseos_release() -> Option<&'static Device> {
    None
}

/// Display scale for the SDL window. 1x everywhere, which is the device
/// pixel size; `MAGI_SCALE` enlarges the window when that helps.
pub fn window_scale() -> u32 {
    if let Ok(s) = std::env::var("MAGI_SCALE") {
        if let Ok(n) = s.parse::<u32>() {
            if (1..=8).contains(&n) {
                return n;
            }
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_matches_aliases() {
        assert_eq!(find("rg35xxsp").unwrap().id, "rg35xxsp");
        assert_eq!(find("RG35xxSP").unwrap().id, "rg35xxsp");
        assert_eq!(find("\"rg40xxv\"").unwrap().id, "rg40xxv");
        assert_eq!(find("rg34xxsp").unwrap().id, "rg34xxsp");
        assert_eq!(find("rg34xx-sp").unwrap().id, "rg34xxsp");
        assert_eq!(find("rg35xx-plus").unwrap().id, "rg35xxplus");
        assert_eq!(find("rg35xx-2024").unwrap().id, "rg35xx2024");
        assert!(find("gameboy").is_none());
    }
}
