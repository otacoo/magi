//! disp2 hardware layers: the game frame is scanned out by the display
//! engine, the UI canvas and screen overlays ride on top as separate layers.
//!
//! DE33 gives each channel one scaler, on the VI layer (layer 0 - the same
//! one the kernel's fb0 uses); the other layers are 1:1 only. So while a
//! game runs in scanout mode layer 0 displays the core frame (scaled to the
//! fitted rect by the engine), layer 1 the UI canvas (pixel alpha, pushed
//! only when the UI changes) and layer 2 the composited overlay, both at
//! panel resolution. The fb0 layer is rebuilt when the game exits.
//!
//! Everything here is best-effort: any failure logs and leaves the caller
//! on its software path. `magi --disp2probe` exercises the same ioctls.

#![cfg(target_os = "linux")]

use std::os::unix::io::{AsRawFd, FromRawFd};

use crate::util;

const DISP_LAYER_SET_CONFIG2: libc::c_ulong = 0x49;
const DISP_LAYER_GET_CONFIG2: libc::c_ulong = 0x4a;

pub const DISP_FORMAT_RGB_565: i32 = 0x0a;
pub const DISP_FORMAT_XRGB_8888: i32 = 0x04;
const DISP_FORMAT_ARGB_8888: i32 = 0x00;
const DISP_BT601: i32 = 0x104;
const LAYER_MODE_BUFFER: i32 = 0;
const LAYER_MODE_COLOR: i32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct DispRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct DispRectSz {
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct DispRect64 {
    x: i64,
    y: i64,
    width: i64,
    height: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FbInfo2 {
    fd: i32,
    size: [DispRectSz; 3],
    align: [u32; 3],
    format: i32,
    color_space: i32,
    trd_right_fd: i32,
    pre_multiply: bool,
    crop: DispRect64,
    flags: i32,
    scan: i32,
    eotf: i32,
    depth: i32,
    fbd_en: u32,
    metadata_fd: i32,
    metadata_size: u32,
    metadata_flag: u32,
}

impl Default for FbInfo2 {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AtwInfo {
    used: bool,
    mode: i32,
    b_row: u32,
    b_col: u32,
    cof_fd: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SnrInfo {
    en: u8,
    demo_en: u8,
    demo_win: DispRect,
    y_strength: u8,
    u_strength: u8,
    v_strength: u8,
    th_ver_line: u8,
    th_hor_line: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LayerInfo2 {
    mode: i32,
    zorder: u8,
    alpha_mode: u8,
    alpha_value: u8,
    screen_win: DispRect,
    b_trd_out: bool,
    out_trd_mode: i32,
    fb: FbInfo2,
    id: u32,
    atw: AtwInfo,
    transform: i32,
    snr: SnrInfo,
}

impl Default for LayerInfo2 {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LayerConfig2 {
    info: LayerInfo2,
    enable: bool,
    channel: u32,
    layer_id: u32,
}

impl Default for LayerConfig2 {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

const _: () = assert!(std::mem::size_of::<FbInfo2>() == 120);
const _: () = assert!(std::mem::size_of::<LayerConfig2>() == 224);

// ---- ION (Allwinner 4.9 UAPI) ----

#[repr(C)]
#[derive(Default)]
struct IonAllocationData {
    len: usize,
    align: usize,
    heap_id_mask: u32,
    flags: u32,
    handle: i32,
}

#[repr(C)]
#[derive(Default)]
struct IonFdData {
    handle: i32,
    fd: i32,
}

#[repr(C)]
#[derive(Default)]
struct IonHandleData {
    handle: i32,
}

fn iowr(nr: u64, size: u64) -> libc::c_ulong {
    (3 << 30) | (size << 16) | ((b'I' as u64) << 8) | nr
}

fn ion_alloc(fd: libc::c_int, len: usize, heap_mask: u32) -> Option<(i32, i32)> {
    let mut a = IonAllocationData {
        len,
        align: 4096,
        heap_id_mask: heap_mask,
        flags: 0,
        handle: -1,
    };
    let rc = unsafe {
        libc::ioctl(
            fd,
            iowr(0, std::mem::size_of::<IonAllocationData>() as u64),
            &mut a,
        )
    };
    if rc < 0 {
        return None;
    }
    let mut m = IonFdData {
        handle: a.handle,
        fd: -1,
    };
    let rc = unsafe { libc::ioctl(fd, iowr(2, std::mem::size_of::<IonFdData>() as u64), &mut m) };
    if rc < 0 || m.fd < 0 {
        ion_free(fd, a.handle);
        return None;
    }
    Some((a.handle, m.fd))
}

fn ion_free(ion: libc::c_int, handle: i32) {
    let h = IonHandleData { handle };
    unsafe {
        libc::ioctl(
            ion,
            iowr(1, std::mem::size_of::<IonHandleData>() as u64),
            &h,
        );
    }
}

// ---- layer plumbing ----

fn disp_get2(disp: libc::c_int, ch: u32, lyr: u32) -> Option<LayerConfig2> {
    let mut cfg = LayerConfig2::default();
    cfg.channel = ch;
    cfg.layer_id = lyr;
    let mut param: [libc::c_ulong; 4] = [0, (&mut cfg as *mut LayerConfig2) as libc::c_ulong, 1, 0];
    let rc = unsafe { libc::ioctl(disp, DISP_LAYER_GET_CONFIG2, param.as_mut_ptr()) };
    (rc >= 0).then_some(cfg)
}

fn disp_set2(disp: libc::c_int, cfg: &mut LayerConfig2) -> i32 {
    let mut param: [libc::c_ulong; 4] = [0, (cfg as *mut LayerConfig2) as libc::c_ulong, 1, 0];
    unsafe { libc::ioctl(disp, DISP_LAYER_SET_CONFIG2, param.as_mut_ptr()) }
}

/// Disable the kernel fb layer (ch1 lyr0): channel 1 mixes above channel 0,
/// so the shelf's frozen frame would cover the game otherwise. The fb stays
/// open in the host and its pan still clocks the loop when the DE allows it.
pub fn fb_layer_hide() -> bool {
    let Ok(disp) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/disp")
    else {
        return false;
    };
    let fd = disp.as_raw_fd();
    let Some(mut cfg) = disp_get2(fd, 1, 0) else {
        return false;
    };
    if !cfg.enable {
        return true;
    }
    cfg.enable = false;
    if disp_set2(fd, &mut cfg) < 0 {
        return false;
    }
    disp_get2(fd, 1, 0).is_some_and(|c| !c.enable)
}

/// Put the fb layer back the way the shelf expects it: enabled, global,
/// opaque. Called when scanout mode ends (and if hiding fails).
pub fn fb_restore() -> bool {
    let Ok(disp) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/disp")
    else {
        return false;
    };
    let fd = disp.as_raw_fd();
    let Some(mut cfg) = disp_get2(fd, 1, 0) else {
        return false;
    };
    if cfg.enable && cfg.info.alpha_mode == 1 && cfg.info.alpha_value == 255 {
        return true;
    }
    cfg.enable = true;
    cfg.info.alpha_mode = 1;
    cfg.info.alpha_value = 255;
    disp_set2(fd, &mut cfg) >= 0
}

/// The screen window for a canvas-sized layer: the panel 1:1, or the same
/// 4:3 pillarbox the fb layer uses on HDMI.
fn window_for(w: u32, h: u32) -> DispRect {
    if util::hdmi_active() {
        DispRect {
            x: 240,
            y: 0,
            width: 1440,
            height: 1080,
        }
    } else {
        DispRect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }
}

/// The game frame as a DE layer on the scaler layer (channel 1, layer 0,
/// which the kernel fbdev normally owns; `present::fbdev::rebuild_layer`
/// puts it back). Two halves in one ION buffer allow tear-free flips.
pub struct GameLayer {
    disp: std::fs::File,
    ion: std::fs::File,
    handle: i32,
    buf_fd: std::fs::File,
    map: *mut u8,
    len: usize,
    buf_w: u32,
    half_h: u32,
    bpp: usize,
    format: i32,
    crop_w: u32,
    crop_h: u32,
    back: u32,
    window: DispRect,
    visible: bool,
}

impl GameLayer {
    pub fn create(max_w: u32, max_h: u32, format: i32, bpp: usize) -> Option<Self> {
        let disp = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/disp")
            .ok()?;
        let ion = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/ion")
            .ok()?;
        let len = (max_w as usize) * (max_h as usize) * 2 * bpp;
        let mut alloc = None;
        for id in 0..16u32 {
            if let Some(a) = ion_alloc(ion.as_raw_fd(), len, 1 << id) {
                alloc = Some((id, a));
                break;
            }
        }
        let Some((heap, (handle, buf_fd))) = alloc else {
            util::log("disp2: ion alloc failed for the game layer");
            return None;
        };
        let buf_fd = unsafe { std::fs::File::from_raw_fd(buf_fd) };
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                buf_fd.as_raw_fd(),
                0,
            )
        };
        if map == libc::MAP_FAILED {
            util::log("disp2: mmap failed for the game layer");
            ion_free(ion.as_raw_fd(), handle);
            return None;
        }
        util::log(&format!(
            "disp2: game layer ch0 lyr0 (scaler), ion heap {heap}, {len} bytes, fmt {format:#x}"
        ));
        let layer = Self {
            disp,
            ion,
            handle,
            buf_fd,
            map: map as *mut u8,
            len,
            buf_w: max_w,
            half_h: max_h,
            bpp,
            format,
            crop_w: max_w,
            crop_h: max_h,
            back: 0,
            window: DispRect {
                x: 0,
                y: 0,
                width: max_w,
                height: max_h,
            },
            visible: false,
        };
        // Leave the layer disabled: the scanout state machine enables it on
        // the first frame, and setups that keep the software path (warmth,
        // HDMI) never displace the kernel's fb layer at all. The buffer is
        // zeroed so the first frames before an upload are black, not junk.
        unsafe {
            std::ptr::write_bytes(layer.map, 0, layer.len);
        }
        Some(layer)
    }

    /// Copy one core frame into the back half; false when it does not fit.
    pub fn upload(&mut self, src: *const u8, pitch: usize, w: u32, h: u32) -> bool {
        if src.is_null() || w > self.buf_w || h > self.half_h {
            return false;
        }
        let row = w as usize * self.bpp;
        let dst_half = self.back as usize * self.half_h as usize * self.buf_w as usize * self.bpp;
        unsafe {
            let dst = self.map.add(dst_half);
            if row == pitch && w == self.buf_w {
                std::ptr::copy_nonoverlapping(src, dst, h as usize * pitch);
            } else {
                for y in 0..h as usize {
                    std::ptr::copy_nonoverlapping(
                        src.add(y * pitch),
                        dst.add(y * self.buf_w as usize * self.bpp),
                        row,
                    );
                }
            }
        }
        self.crop_w = w;
        self.crop_h = h;
        true
    }

    /// Show the freshly uploaded half. The caller ticks the fb pan right
    /// after, which is the only vsync-aligned flip this kernel offers.
    pub fn flip(&mut self) {
        self.back ^= 1;
        if self.visible {
            self.commit();
        }
    }

    pub fn set_window(&mut self, x: i32, y: i32, w: u32, h: u32) {
        let win = DispRect {
            x,
            y,
            width: w,
            height: h,
        };
        let changed = win.x != self.window.x
            || win.y != self.window.y
            || win.width != self.window.width
            || win.height != self.window.height;
        self.window = win;
        if changed && self.visible {
            self.commit();
        }
    }

    pub fn set_visible(&mut self, on: bool) {
        if on == self.visible {
            return;
        }
        if on {
            self.visible = true;
            self.commit();
        } else {
            self.visible = false;
            let rc = disp_set2(
                self.disp.as_raw_fd(),
                &mut LayerConfig2 {
                    info: LayerInfo2::default(),
                    enable: false,
                    channel: 0,
                    layer_id: 0,
                },
            );
            if rc < 0 {
                util::log(&format!(
                    "disp2: game layer disable failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
    }

    fn commit(&mut self) {
        let mut cfg = LayerConfig2::default();
        cfg.info.mode = LAYER_MODE_BUFFER;
        // Above the kernel fb layer (z 0): equal zorders tie in the fb's
        // favour and the frozen shelf covers the game.
        cfg.info.zorder = 1;
        cfg.info.alpha_mode = 1; // global alpha, opaque
        cfg.info.alpha_value = 255;
        cfg.info.screen_win = self.window;
        cfg.info.fb.fd = self.buf_fd.as_raw_fd();
        cfg.info.fb.size[0] = DispRectSz {
            width: self.buf_w,
            height: self.half_h * 2,
        };
        cfg.info.fb.format = self.format;
        cfg.info.fb.color_space = DISP_BT601;
        cfg.info.fb.crop = DispRect64 {
            x: 0,
            y: (self.back as i64 * self.half_h as i64) << 32,
            width: (self.crop_w as i64) << 32,
            height: (self.crop_h as i64) << 32,
        };
        cfg.enable = true;
        cfg.channel = 0;
        cfg.layer_id = 0;
        let rc = disp_set2(self.disp.as_raw_fd(), &mut cfg);
        if rc < 0 {
            util::log(&format!(
                "disp2: game layer commit failed: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
}

impl Drop for GameLayer {
    fn drop(&mut self) {
        self.set_visible(false);
        unsafe {
            libc::munmap(self.map as *mut libc::c_void, self.len);
        }
        ion_free(self.ion.as_raw_fd(), self.handle);
    }
}

/// A 1:1 pixel-alpha layer at panel resolution: the UI canvas (channel 1
/// layer 1) and the composited overlay (layer 2).
pub struct ImageLayer {
    disp: std::fs::File,
    ion: std::fs::File,
    handle: i32,
    buf_fd: std::fs::File,
    map: *mut u32,
    len: usize,
    w: u32,
    h: u32,
    channel: u32,
    layer_id: u32,
    zorder: u8,
    window: DispRect,
    visible: bool,
}

impl ImageLayer {
    pub fn create(channel: u32, layer_id: u32, zorder: u8, w: u32, h: u32) -> Option<Self> {
        let disp = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/disp")
            .ok()?;
        let ion = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/ion")
            .ok()?;
        let len = (w as usize) * (h as usize) * 4;
        let mut alloc = None;
        for id in 0..16u32 {
            if let Some(a) = ion_alloc(ion.as_raw_fd(), len, 1 << id) {
                alloc = Some((id, a));
                break;
            }
        }
        let Some((heap, (handle, buf_fd))) = alloc else {
            util::log(&format!(
                "disp2: ion alloc failed for layer ch{channel} lyr{layer_id}"
            ));
            return None;
        };
        let buf_fd = unsafe { std::fs::File::from_raw_fd(buf_fd) };
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                buf_fd.as_raw_fd(),
                0,
            )
        };
        if map == libc::MAP_FAILED {
            util::log(&format!(
                "disp2: mmap failed for layer ch{channel} lyr{layer_id}"
            ));
            ion_free(ion.as_raw_fd(), handle);
            return None;
        }
        util::log(&format!(
            "disp2: layer ch{channel} lyr{layer_id} z{zorder}, ion heap {heap}, {len} bytes"
        ));
        Some(Self {
            disp,
            ion,
            handle,
            buf_fd,
            map: map as *mut u32,
            len,
            w,
            h,
            channel,
            layer_id,
            zorder,
            window: window_for(w, h),
            visible: false,
        })
    }

    /// Copy a straight-alpha ARGB0 image into the buffer.
    pub fn upload(&mut self, px: &[u32]) -> bool {
        if px.len() * 4 != self.len {
            return false;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(px.as_ptr(), self.map, px.len());
        }
        true
    }

    pub fn set_visible(&mut self, on: bool) {
        if on == self.visible {
            return;
        }
        let mut cfg = LayerConfig2::default();
        cfg.channel = self.channel;
        cfg.layer_id = self.layer_id;
        cfg.enable = on;
        if on {
            cfg.info.mode = LAYER_MODE_BUFFER;
            cfg.info.zorder = self.zorder;
            cfg.info.alpha_mode = 0; // per-pixel alpha
            cfg.info.alpha_value = 255;
            cfg.info.screen_win = self.window;
            cfg.info.fb.fd = self.buf_fd.as_raw_fd();
            cfg.info.fb.size[0] = DispRectSz {
                width: self.w,
                height: self.h,
            };
            cfg.info.fb.format = DISP_FORMAT_ARGB_8888;
            cfg.info.fb.color_space = DISP_BT601;
            cfg.info.fb.crop = DispRect64 {
                x: 0,
                y: 0,
                width: (self.w as i64) << 32,
                height: (self.h as i64) << 32,
            };
        } else {
            cfg.info.mode = LAYER_MODE_COLOR;
        }
        let rc = disp_set2(self.disp.as_raw_fd(), &mut cfg);
        if rc < 0 {
            util::log(&format!(
                "disp2: layer ch{} lyr{} {} failed: {}",
                self.channel,
                self.layer_id,
                if on { "enable" } else { "disable" },
                std::io::Error::last_os_error()
            ));
            return;
        }
        self.visible = on;
    }
}

impl Drop for ImageLayer {
    fn drop(&mut self) {
        self.set_visible(false);
        unsafe {
            libc::munmap(self.map as *mut libc::c_void, self.len);
        }
        ion_free(self.ion.as_raw_fd(), self.handle);
    }
}

/// The DE background behind every layer (the letterbox bars).
pub fn set_background(color: u32) -> bool {
    let Ok(disp) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/disp")
    else {
        return false;
    };
    let mut args: [libc::c_ulong; 4] = [0, color as libc::c_ulong, 0, 0];
    (unsafe { libc::ioctl(disp.as_raw_fd(), 0x03, args.as_mut_ptr()) }) >= 0
}

/// The shelf runs this at startup: a crashed game host may have left layers
/// 1..2 enabled with stale buffers and layer 0 pointed at its scanout
/// buffer instead of fb0.
pub fn clear_stale_layer() {
    if let Ok(disp) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/disp")
    {
        for (ch, lyr) in [(0u32, 0u32), (0, 1), (0, 2), (0, 3), (1, 1), (1, 2), (1, 3)] {
            let Some(mut cfg) = disp_get2(disp.as_raw_fd(), ch, lyr) else {
                continue;
            };
            if !cfg.enable {
                continue;
            }
            util::log(&format!("disp2: clearing stale layer ch{ch} lyr{lyr}"));
            cfg.enable = false;
            cfg.info.mode = LAYER_MODE_COLOR;
            let _ = disp_set2(disp.as_raw_fd(), &mut cfg);
        }
        // Rebuild layer 0 from the fb geometry.
        let _ = crate::present::fbdev::rebuild_layer();
    }
}

/// `magi --disp2probe`: prove the layer path on this kernel (see RESUME).
pub fn probe() -> anyhow::Result<()> {
    util::log("disp2probe: start");
    let disp = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/disp")?;
    let disp = disp.as_raw_fd();

    let mut fb_z = 0u8;
    for ch in 0..2u32 {
        for lyr in 0..4u32 {
            match disp_get2(disp, ch, lyr) {
                Some(c) => {
                    util::log(&format!(
                        "disp2probe: get2 ch{ch} lyr{lyr} ok enable={} mode={} z={} alpha={}/{} win={}x{}+{}+{} fb={}x{} fd={}",
                        c.enable,
                        c.info.mode,
                        c.info.zorder,
                        c.info.alpha_mode,
                        c.info.alpha_value,
                        c.info.screen_win.width,
                        c.info.screen_win.height,
                        c.info.screen_win.x,
                        c.info.screen_win.y,
                        c.info.fb.size[0].width,
                        c.info.fb.size[0].height,
                        c.info.fb.fd,
                    ));
                    if ch == 1 && lyr == 0 {
                        fb_z = c.info.zorder;
                    }
                }
                None => util::log(&format!("disp2probe: get2 ch{ch} lyr{lyr} unavailable")),
            }
        }
    }
    util::log(&format!("disp2probe: fb0 layer zorder={fb_z}"));

    let ion = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/ion")?;
    let (w, h) = (640usize, 480usize);
    let len = w * h * 4;
    let mut alloc = None;
    for id in 0..16u32 {
        if let Some(a) = ion_alloc(ion.as_raw_fd(), len, 1 << id) {
            util::log(&format!("disp2probe: ion heap id {id} allocated"));
            alloc = Some(a);
            break;
        }
    }
    let Some((handle, buf_fd)) = alloc else {
        util::log("disp2probe: ion alloc failed on every heap");
        return Ok(());
    };
    let map = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            buf_fd,
            0,
        )
    };
    if map == libc::MAP_FAILED {
        util::log("disp2probe: mmap failed");
        ion_free(ion.as_raw_fd(), handle);
        return Ok(());
    }
    unsafe {
        let px = std::slice::from_raw_parts_mut(map as *mut u32, w * h);
        for p in px.iter_mut() {
            *p = 0x80FF_FFFF; // 50% white, straight alpha
        }
    }

    // Step tests. Each holds a visible pattern for 4 s and logs a marker;
    // the observer reports what appeared for each step.
    fn step(
        disp: libc::c_int,
        label: &str,
        ch: u32,
        lyr: u32,
        buf_fd: libc::c_int,
        buf_w: u32,
        buf_h: u32,
        zorder: u8,
        win: (i32, i32, u32, u32),
    ) -> bool {
        let mut cfg = LayerConfig2::default();
        cfg.info.mode = LAYER_MODE_BUFFER;
        cfg.info.zorder = zorder;
        cfg.info.alpha_mode = 0;
        cfg.info.alpha_value = 255;
        cfg.info.screen_win = DispRect {
            x: win.0,
            y: win.1,
            width: win.2,
            height: win.3,
        };
        cfg.info.fb.fd = buf_fd;
        cfg.info.fb.size[0] = DispRectSz {
            width: buf_w,
            height: buf_h,
        };
        cfg.info.fb.format = DISP_FORMAT_ARGB_8888;
        cfg.info.fb.color_space = DISP_BT601;
        cfg.info.fb.crop = DispRect64 {
            x: 0,
            y: 0,
            width: (buf_w as i64) << 32,
            height: (buf_h as i64) << 32,
        };
        cfg.enable = true;
        cfg.channel = ch;
        cfg.layer_id = lyr;
        let rc = disp_set2(disp, &mut cfg);
        util::log(&format!(
            "disp2probe: STEP {label} ch{ch} lyr{lyr} win {}x{}+{}+{} set rc={rc}{}",
            win.2,
            win.3,
            win.0,
            win.1,
            if rc < 0 {
                format!(" ({})", std::io::Error::last_os_error())
            } else {
                String::new()
            }
        ));
        if rc < 0 {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
        let back = disp_get2(disp, ch, lyr);
        util::log(&format!(
            "disp2probe: STEP {label} readback enable={} z={} win={}x{}+{}+{}",
            back.map(|b| b.enable).unwrap_or(false),
            back.map(|b| b.info.zorder).unwrap_or(0),
            back.map(|b| b.info.screen_win.width).unwrap_or(0),
            back.map(|b| b.info.screen_win.height).unwrap_or(0),
            back.map(|b| b.info.screen_win.x).unwrap_or(0),
            back.map(|b| b.info.screen_win.y).unwrap_or(0),
        ));
        util::log(&format!(
            "disp2probe: STEP {label} visible 4s - note what you see"
        ));
        std::thread::sleep(std::time::Duration::from_secs(4));
        cfg.enable = false;
        let rc = disp_set2(disp, &mut cfg);
        util::log(&format!("disp2probe: STEP {label} off rc={rc}"));
        true
    }

    let mut placed = false;

    // A: the known-good case - a 1:1 white wash on ch1 lyr2.
    step(
        disp,
        "A",
        1,
        2,
        buf_fd,
        w as u32,
        h as u32,
        1,
        (0, 0, w as u32, h as u32),
    );

    // A 240x160 red buffer for the scaling tests.
    let (sw, sh) = (240usize, 160usize);
    let slen = sw * sh * 4;
    let mut small = None;
    for id in 0..16u32 {
        if let Some(a) = ion_alloc(ion.as_raw_fd(), slen, 1 << id) {
            small = Some(a);
            break;
        }
    }
    let Some((shandle, sfd)) = small else {
        util::log("disp2probe: small ion alloc failed");
        unsafe {
            libc::munmap(map, len);
        }
        ion_free(ion.as_raw_fd(), handle);
        util::log("disp2probe: done (placed=false)");
        return Ok(());
    };
    let smap = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            slen,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            sfd,
            0,
        )
    };
    if smap != libc::MAP_FAILED {
        unsafe {
            let px = std::slice::from_raw_parts_mut(smap as *mut u32, sw * sh);
            for p in px.iter_mut() {
                *p = 0xFFFF_0000; // solid red, opaque
            }
        }
    }

    // B/C/D/E: does a 240x160 red frame scale to 440x300 at (100,100)?
    let win = (100, 100, 440, 300);
    for (label, ch, lyr) in [("B", 1u32, 2u32), ("C", 1, 1), ("D", 1, 0), ("E", 0, 0)] {
        if smap == libc::MAP_FAILED || disp_get2(disp, ch, lyr).is_none() {
            util::log(&format!(
                "disp2probe: STEP {label} ch{ch} lyr{lyr} unavailable"
            ));
            continue;
        }
        if step(disp, label, ch, lyr, sfd, sw as u32, sh as u32, 1, win) {
            placed = true;
        }
        if ch == 1 && lyr == 0 {
            // Our test displaced the kernel's fb layer; put it back.
            let _ = crate::present::fbdev::rebuild_layer();
        }
    }
    if smap != libc::MAP_FAILED {
        unsafe {
            libc::munmap(smap, slen);
        }
    }
    ion_free(ion.as_raw_fd(), shandle);

    unsafe {
        libc::munmap(map, len);
    }
    ion_free(ion.as_raw_fd(), handle);
    util::log(&format!("disp2probe: done (placed={placed})"));
    Ok(())
}
