//! ALSA playback via dlopen. BaseOS ships `libasound.so.2` but no headers or
//! dev symlink, and MAGI's static SDL2 was built without an ALSA backend, so
//! the run host loads the handful of PCM functions it needs at runtime.

use std::ffi::c_void;
use std::os::raw::{c_char, c_int, c_long, c_uint, c_ulong};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::util;

type OpenFn = unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int;
type SetParamsFn =
    unsafe extern "C" fn(*mut c_void, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int;
type WriteiFn = unsafe extern "C" fn(*mut c_void, *const c_void, c_ulong) -> c_long;
type RecoverFn = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int;
type DrainFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type CloseFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type StrErrorFn = unsafe extern "C" fn(c_int) -> *const c_char;

const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_FORMAT_S16_LE: c_int = 2;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;

/// Rate MAGI opens the sink at. The H700 codec, HDMI and the usual
/// Bluetooth sinks all run in the 48 kHz family.
pub const OUTPUT_HZ: u32 = 48_000;

/// In order: the plug plugin adapts the fixed device rate to whatever the
/// codec wants; the rest are fallbacks.
const DEVICES: [&str; 4] = ["plug:default", "default", "plughw:0,0", "hw:0,0"];

pub struct Alsa {
    handle: *mut c_void,
    writei: WriteiFn,
    recover: RecoverFn,
    drain: DrainFn,
    close: CloseFn,
}

unsafe fn sym(lib: *mut c_void, name: &[u8]) -> *mut c_void {
    libc::dlsym(lib, name.as_ptr() as *const c_char)
}

unsafe fn load<T: Copy>(lib: *mut c_void, name: &[u8]) -> Option<T> {
    let p = sym(lib, name);
    if p.is_null() {
        util::log(&format!(
            "audio: missing symbol {}",
            String::from_utf8_lossy(&name[..name.len() - 1])
        ));
        return None;
    }
    Some(std::mem::transmute_copy(&p))
}

impl Alsa {
    pub fn open(rate: u32, preferred: Option<&str>) -> Option<Self> {
        unsafe {
            let lib = libc::dlopen(
                b"libasound.so.2\0".as_ptr() as *const c_char,
                libc::RTLD_NOW,
            );
            if lib.is_null() {
                util::log("audio: libasound.so.2 not found; running silent");
                return None;
            }
            let open_f: OpenFn = load(lib, b"snd_pcm_open\0")?;
            let set_params: SetParamsFn = load(lib, b"snd_pcm_set_params\0")?;
            let writei: WriteiFn = load(lib, b"snd_pcm_writei\0")?;
            let recover: RecoverFn = load(lib, b"snd_pcm_recover\0")?;
            let drain: DrainFn = load(lib, b"snd_pcm_drain\0")?;
            let close: CloseFn = load(lib, b"snd_pcm_close\0")?;
            let strerror: Option<StrErrorFn> = load(lib, b"snd_strerror\0");
            let errstr = |rc: c_int| -> String {
                match strerror {
                    Some(f) => {
                        let p = f(rc);
                        if p.is_null() {
                            rc.to_string()
                        } else {
                            std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
                        }
                    }
                    None => rc.to_string(),
                }
            };

            let mut last_err = String::from("no PCM device");

            let mut devices: Vec<&str> = Vec::new();
            if let Some(device) = preferred.filter(|d| !d.is_empty()) {
                devices.push(device);
            }
            if crate::util::headphones_in() {
                crate::util::speaker_amp_off();
                devices.push("plug:playback_hp");
                devices.push("playback_hp");
            }
            if crate::util::hdmi_active() {
                devices.push("plug:playback_hdmi");
                devices.push("playback_hdmi");
            }
            devices.extend_from_slice(&DEVICES);
            for name in devices {
                let Ok(cname) = std::ffi::CString::new(name) else {
                    continue;
                };
                let mut handle: *mut c_void = std::ptr::null_mut();
                let rc = open_f(&mut handle, cname.as_ptr(), SND_PCM_STREAM_PLAYBACK, 0);
                if rc < 0 || handle.is_null() {
                    last_err = format!("{name}: {}", errstr(rc));
                    continue;
                }
                let rc = set_params(
                    handle,
                    SND_PCM_FORMAT_S16_LE,
                    SND_PCM_ACCESS_RW_INTERLEAVED,
                    2,
                    rate,
                    1,
                    40_000,
                );
                if rc < 0 {
                    close(handle);
                    last_err = format!("{name} @{rate}Hz: {}", errstr(rc));
                    continue;
                }
                util::log(&format!("audio: {name} at {rate} Hz"));
                return Some(Self {
                    handle,
                    writei,
                    recover,
                    drain,
                    close,
                });
            }

            util::log(&format!(
                "audio: no PCM device ({last_err}); running silent"
            ));
            None
        }
    }

    /// Blocking write of interleaved stereo samples; short writes resume at
    /// the right offset and xruns are recovered.
    pub fn write(&mut self, samples: &[i16]) -> bool {
        let total = samples.len() / 2;
        let mut done = 0usize;
        let mut tries = 0u32;
        while done < total {
            let frames = (total - done) as c_ulong;
            let ptr = unsafe { samples.as_ptr().add(done * 2) } as *const c_void;
            let rc = unsafe { (self.writei)(self.handle, ptr, frames) };
            if rc < 0 {
                tries += 1;
                if tries == 1 {
                    util::log(&format!("audio: write error {rc}; recovering"));
                }
                if tries > 3 {
                    util::log(&format!(
                        "audio: write keeps failing ({rc}); stopping playback"
                    ));
                    return false;
                }
                let rr = unsafe { (self.recover)(self.handle, rc as c_int, 1) };
                if rr < 0 {
                    util::log(&format!("audio: write failed ({rc}); stopping playback"));
                    return false;
                }
                continue;
            }
            tries = 0;
            done += rc as usize;
        }
        true
    }
}

impl Drop for Alsa {
    fn drop(&mut self) {
        unsafe {
            (self.drain)(self.handle);
            (self.close)(self.handle);
        }
    }
}

/// Playback off the emulator's critical path: samples go into a ring and a
/// dedicated thread feeds ALSA with blocking writes, so rendering never waits
/// on the audio clock (waiting inline made games run below full speed).
pub struct AlsaOut {
    ring: Arc<Mutex<Vec<i16>>>,
    stop: Arc<AtomicBool>,
    ok: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

/// Ring cap in samples (stereo pairs), 8/60 s; the writer keeps the level
/// near half, so drops only happen on real stalls.
const PUSH_CAP: usize = (OUTPUT_HZ as usize / 60 * 8) * 2;
/// Frames per ALSA write.
const CHUNK_FRAMES: usize = 512;

impl AlsaOut {
    pub fn start(rate: u32, preferred: Option<String>) -> Self {
        let ring = Arc::new(Mutex::new(Vec::<i16>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let ok = Arc::new(AtomicBool::new(false));

        // Start half full with silence so the writer never begins starved.
        {
            let mut r = ring.lock().unwrap();
            r.resize(PUSH_CAP / 2, 0);
        }

        let r = ring.clone();
        let s = stop.clone();
        let o = ok.clone();
        let join = std::thread::Builder::new()
            .name("magi-audio".into())
            .spawn(move || {
                let Some(mut alsa) = Alsa::open(rate, preferred.as_deref()) else {
                    return;
                };
                o.store(true, Ordering::Relaxed);
                // Feed the device with whatever the ring holds. A shortfall
                // is never padded with silence while samples are available:
                // that turned a brief dip into an audible gap every time the
                // writer caught up with the producer. The device's 40 ms
                // buffer covers the wait; silence is only inserted if the
                // ring stays empty long enough to underrun for real.
                let mut buf = vec![0i16; CHUNK_FRAMES * 2];
                let mut starved = 0u32;
                let mut last_empty: Option<Instant> = None;
                loop {
                    let take = {
                        let mut ring = r.lock().unwrap();
                        let take = ring.len().min(buf.len());
                        buf[..take].copy_from_slice(&ring[..take]);
                        ring.drain(..take);
                        take
                    };
                    if take == 0 {
                        if s.load(Ordering::Relaxed) {
                            break;
                        }
                        let now = Instant::now();
                        if last_empty.map_or(true, |t| now.duration_since(t).as_secs() >= 2) {
                            last_empty = Some(now);
                            util::log("audio: ring empty; holding silence");
                        }
                        starved += 1;
                        if starved < 8 {
                            // Give the producer ~8 ms to catch up; the device
                            // buffer is ~40 ms, so this cannot underrun by
                            // itself.
                            std::thread::sleep(Duration::from_millis(1));
                            continue;
                        }
                        // Still empty: keep the device fed with one silent
                        // period, then try for real samples again.
                        buf.fill(0);
                        starved = 0;
                        if !alsa.write(&buf) {
                            break;
                        }
                        continue;
                    }
                    starved = 0;
                    if !alsa.write(&buf[..take]) {
                        break;
                    }
                }
                util::log("audio: closed");
            })
            .ok();

        Self {
            ring,
            stop,
            ok,
            join,
        }
    }

    /// Stop the writer and let ALSA drain. Call before cutting power so the
    /// codec does not click when the speaker amp loses its supply.
    pub fn close(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        self.ok.store(false, Ordering::Relaxed);
    }

    /// Hand a batch over; a no-op when no device opened (silent mode).
    /// Returns the ring's fill level (0..1) for gentle rate control.
    pub fn push(&self, samples: &[i16]) -> f32 {
        if !self.ok.load(Ordering::Relaxed) {
            return 0.5;
        }
        let mut ring = self.ring.lock().unwrap();
        ring.extend_from_slice(samples);
        if ring.len() > PUSH_CAP {
            let drop = ring.len() - PUSH_CAP;
            ring.drain(..drop);
        }
        ring.len() as f32 / PUSH_CAP as f32
    }
}

impl Drop for AlsaOut {
    fn drop(&mut self) {
        self.close();
    }
}

/// One-shot PCM16 WAV playback for the boot ignition, in the background;
/// resampled to the device rate and silent when anything is missing.
#[cfg(target_os = "linux")]
static BOOT_AUDIO_STOP: AtomicBool = AtomicBool::new(false);

/// Cut the boot ignition track short when the splash is skipped.
#[cfg(target_os = "linux")]
pub fn stop_boot_audio() {
    BOOT_AUDIO_STOP.store(true, Ordering::Relaxed);
}

#[cfg(target_os = "linux")]
pub fn spawn_wav(path: &std::path::Path, gain_pct: u8) {
    BOOT_AUDIO_STOP.store(false, Ordering::Relaxed);
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        if BOOT_AUDIO_STOP.load(Ordering::Relaxed) {
            return;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let Some((rate, channels, mut samples)) = parse_wav_pcm16(&bytes) else {
            crate::util::log(&format!("boot audio: not a PCM16 wav: {}", path.display()));
            return;
        };
        if channels == 1 {
            let mut st = Vec::with_capacity(samples.len() * 2);
            for s in samples {
                st.push(s);
                st.push(s);
            }
            samples = st;
        }
        if gain_pct < 100 {
            let g = (gain_pct as f32 / 100.0).powf(1.6);
            for s in &mut samples {
                *s = (*s as f32 * g) as i16;
            }
        }
        let mut out = if rate != OUTPUT_HZ {
            let mut r = crate::resample::Resampler::new(rate as f64, OUTPUT_HZ as f64);
            let mut o = Vec::new();
            r.process(&samples, &mut o);
            o
        } else {
            samples
        };
        crate::resample::fade_out(&mut out, OUTPUT_HZ, 40);
        if BOOT_AUDIO_STOP.load(Ordering::Relaxed) {
            return;
        }
        if let Some(mut sink) = Alsa::open(OUTPUT_HZ, None) {
            let chunk = (OUTPUT_HZ as usize / 50).max(1) * 2;
            for part in out.chunks(chunk) {
                if BOOT_AUDIO_STOP.load(Ordering::Relaxed) {
                    crate::util::log("boot audio: stopped");
                    return;
                }
                if !sink.write(part) {
                    return;
                }
            }
        }
    });
}

/// Minimal RIFF reader: first `data` chunk of a 16-bit PCM `fmt `.
fn parse_wav_pcm16(bytes: &[u8]) -> Option<(u32, u16, Vec<i16>)> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let le32 = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    let (mut rate, mut channels) = (0u32, 0u16);
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = le32(pos + 4) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= bytes.len() {
            let fmt = u16::from_le_bytes([bytes[body], bytes[body + 1]]);
            channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]);
            rate = le32(body + 4);
            let bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]);
            if fmt != 1 || bits != 16 {
                return None;
            }
        } else if id == b"data" {
            if rate == 0 || channels == 0 {
                return None;
            }
            let end = body.saturating_add(size).min(bytes.len());
            let mut out = Vec::with_capacity((end - body) / 2);
            let mut i = body;
            while i + 2 <= end {
                out.push(i16::from_le_bytes([bytes[i], bytes[i + 1]]));
                i += 2;
            }
            return Some((rate, channels, out));
        }
        pos = body + size + (size & 1);
    }
    None
}
