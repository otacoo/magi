//! Full-payload OTA updates from the GitHub release.
//!
//! The release carries unversioned copies of the panel payloads so the device
//! can always ask for `/releases/latest/download/magi-payload-640.zip`.

#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Checking,
    Downloading,
    Verifying,
    Extracting,
    Installing,
    Ready,
    UpToDate,
    Failed,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Checking => "CHECKING FOR UPDATES",
            Phase::Downloading => "DOWNLOADING PAYLOAD",
            Phase::Verifying => "VERIFYING CHECKSUM",
            Phase::Extracting => "UNPACKING PAYLOAD",
            Phase::Installing => "INSTALLING TO CARD",
            Phase::Ready => "UPDATE INSTALLED",
            Phase::UpToDate => "MAGI IS UP TO DATE",
            Phase::Failed => "UPDATE FAILED",
        }
    }

    pub fn active(self) -> bool {
        matches!(
            self,
            Phase::Checking
                | Phase::Downloading
                | Phase::Verifying
                | Phase::Extracting
                | Phase::Installing
        )
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
enum Msg {
    Progress(Phase, String, f32),
    Ready(String),
    UpToDate(String),
    Failed(String),
}

pub struct Update {
    pub phase: Phase,
    pub latest: String,
    pub detail: String,
    pub progress: f32,
    rx: Receiver<Msg>,
}

impl Update {
    pub fn start(panel: &str, current: &str) -> Self {
        let (tx, rx) = channel();
        let panel = panel.to_string();
        let current = current.to_string();
        std::thread::spawn(move || run(&tx, &panel, &current));
        Self {
            phase: Phase::Checking,
            latest: String::new(),
            detail: "CONTACTING GITHUB".into(),
            progress: 0.02,
            rx,
        }
    }

    pub fn poll(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Progress(phase, detail, progress) => {
                    self.phase = phase;
                    self.detail = detail;
                    self.progress = progress;
                }
                Msg::Ready(latest) => {
                    self.phase = Phase::Ready;
                    self.latest = latest;
                    self.detail = "RESTART TO FINISH".into();
                    self.progress = 1.0;
                }
                Msg::UpToDate(latest) => {
                    self.phase = Phase::UpToDate;
                    self.latest = latest;
                    self.detail = "NO NEW VERSION".into();
                    self.progress = 1.0;
                }
                Msg::Failed(e) => {
                    self.phase = Phase::Failed;
                    self.detail = e;
                    self.progress = 1.0;
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
const RELEASE: &str = "https://github.com/otacoo/magi/releases/latest/download";

#[cfg(target_os = "linux")]
fn send(tx: &Sender<Msg>, phase: Phase, detail: &str, progress: f32) {
    let _ = tx.send(Msg::Progress(phase, detail.to_string(), progress));
}

fn run(tx: &Sender<Msg>, panel: &str, current: &str) {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (panel, current);
        let _ = tx.send(Msg::Failed("UPDATES NEED BASEOS".into()));
    }

    #[cfg(target_os = "linux")]
    {
        let root = Path::new("/tmp/magi-update");
        let _ = std::fs::remove_dir_all(root);
        if let Err(e) = std::fs::create_dir_all(root) {
            let _ = tx.send(Msg::Failed(format!("NO TEMP SPACE: {e}")));
            return;
        }
        let version_url = format!("{RELEASE}/version.txt");
        let latest = match curl_text(&version_url) {
            Ok(v) => v.trim().to_string(),
            Err(e) => {
                let _ = tx.send(Msg::Failed(e));
                return;
            }
        };
        if !is_newer(&latest, current) {
            let _ = tx.send(Msg::UpToDate(latest));
            return;
        }

        send(tx, Phase::Downloading, "FETCHING PAYLOAD", 0.08);
        let zip_name = format!("magi-payload-{panel}.zip");
        let url = format!("{RELEASE}/{zip_name}");
        let zip = root.join(&zip_name);
        let total = content_length(&url).unwrap_or(0);
        if let Err(e) = download(&url, &zip, total, tx) {
            let _ = tx.send(Msg::Failed(e));
            return;
        }

        send(tx, Phase::Verifying, "CHECKING SHA-256", 0.62);
        let sums_url = format!("{RELEASE}/SHA256SUMS");
        let sums = match curl_text(&sums_url) {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(Msg::Failed(e));
                return;
            }
        };
        match verify_checksum(&zip, &zip_name, &sums) {
            Ok(()) => {}
            Err(e) => {
                let _ = tx.send(Msg::Failed(e));
                return;
            }
        }

        send(tx, Phase::Extracting, "UNPACKING PAYLOAD", 0.70);
        let stage = root.join("stage");
        let unzip = std::process::Command::new("unzip")
            .arg("-q")
            .arg("-o")
            .arg(&zip)
            .arg("-d")
            .arg(&stage)
            .status();
        if !unzip.map(|s| s.success()).unwrap_or(false) {
            let _ = tx.send(Msg::Failed("CANNOT UNPACK PAYLOAD".into()));
            return;
        }
        if !stage.join("System/frontend").is_file() {
            let _ = tx.send(Msg::Failed("PAYLOAD HAS NO FRONTEND".into()));
            return;
        }
        if let Err(e) = staged_version(&stage) {
            let _ = tx.send(Msg::Failed(e));
            return;
        }

        send(tx, Phase::Installing, "WRITING SYSTEM FILES", 0.78);
        if let Err(e) = install(&stage, tx) {
            let _ = tx.send(Msg::Failed(e));
            return;
        }
        let _ = std::process::Command::new("sync").status();
        let _ = std::fs::remove_dir_all(root);
        let _ = tx.send(Msg::Ready(latest));
    }
}

#[cfg(target_os = "linux")]
fn curl_text(url: &str) -> Result<String, String> {
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--connect-timeout", "10", "--max-time", "30", url])
        .output()
        .map_err(|_| "CURL IS MISSING".to_string())?;
    if !out.status.success() {
        return Err("GITHUB UNREACHABLE".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(target_os = "linux")]
fn content_length(url: &str) -> Option<u64> {
    let out = std::process::Command::new("curl")
        .args(["-fsSLI", "--connect-timeout", "10", "--max-time", "30", url])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().rev().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        if k.eq_ignore_ascii_case("content-length") {
            v.trim().parse().ok()
        } else {
            None
        }
    })
}

#[cfg(target_os = "linux")]
fn download(url: &str, path: &Path, total: u64, tx: &Sender<Msg>) -> Result<(), String> {
    let mut child = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--connect-timeout",
            "15",
            "--max-time",
            "600",
            "-o",
        ])
        .arg(path)
        .arg(url)
        .spawn()
        .map_err(|_| "CURL IS MISSING".to_string())?;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            if !status.success() {
                return Err("DOWNLOAD FAILED".into());
            }
            break;
        }
        if total > 0 {
            let got = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let p = (got as f32 / total as f32).clamp(0.0, 1.0);
            send(
                tx,
                Phase::Downloading,
                &format!("{} / {}", mib(got), mib(total)),
                0.08 + p * 0.52,
            );
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn mib(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f32 / (1024.0 * 1024.0))
}

#[cfg(target_os = "linux")]
fn verify_checksum(path: &Path, name: &str, sums: &str) -> Result<(), String> {
    let expected = sums
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let file = parts.next()?.trim_start_matches('*');
            (file == name).then_some(hash)
        })
        .ok_or_else(|| "CHECKSUM MISSING".to_string())?;
    let out = std::process::Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|_| "SHA256SUM IS MISSING".to_string())?;
    let actual = String::from_utf8_lossy(&out.stdout);
    let actual = actual
        .split_whitespace()
        .next()
        .ok_or_else(|| "CHECKSUM FAILED".to_string())?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err("CHECKSUM MISMATCH".into())
    }
}

#[cfg(target_os = "linux")]
fn staged_version(stage: &Path) -> Result<(), String> {
    let frontend = stage.join("System/frontend");
    let out = std::process::Command::new("/lib/ld-linux-aarch64.so.1")
        .arg(&frontend)
        .arg("--version")
        .output()
        .map_err(|_| "STAGED FRONTEND WILL NOT START".to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err("STAGED FRONTEND WILL NOT START".into())
    }
}

#[cfg(target_os = "linux")]
fn install(stage: &Path, tx: &Sender<Msg>) -> Result<(), String> {
    let mut files = Vec::new();
    for root in ["System", "Overlays"] {
        walk(&stage.join(root), &mut files);
    }
    let template = stage.join("Themes/_template.ini");
    let frontend = stage.join("System/frontend");
    files.retain(|p| p != &frontend);
    files.sort();
    if template.is_file() {
        files.push(template);
    }
    files.push(frontend);

    let total = files.len().max(1);
    for (i, file) in files.iter().enumerate() {
        let rel = file.strip_prefix(stage).map_err(|_| "BAD PAYLOAD PATH")?;
        let dest = Path::new(".").join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("MKDIR: {e}"))?;
        }
        let bytes = std::fs::read(file).map_err(|e| format!("READ: {e}"))?;
        crate::util::write_file_atomic(&dest, &bytes).map_err(|e| format!("WRITE: {e}"))?;
        let p = 0.78 + (i + 1) as f32 / total as f32 * 0.20;
        send(tx, Phase::Installing, &format!("{} / {}", i + 1, total), p);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn walk(path: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn is_newer(latest: &str, current: &str) -> bool {
    match (version(latest), version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => latest.trim_start_matches('v') != current.trim_start_matches('v'),
    }
}

#[cfg(any(target_os = "linux", test))]
fn version(s: &str) -> Option<(u32, u32, u32)> {
    let mut p = s.trim().trim_start_matches('v').split('.');
    Some((
        p.next()?.parse().ok()?,
        p.next()?.parse().ok()?,
        p.next()?.split('-').next()?.parse().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_order_is_numeric() {
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.1.0", "1.1.0"));
        assert!(!is_newer("1.0.9", "1.1.0"));
    }
}
