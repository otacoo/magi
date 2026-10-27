//! Direct `/dev/fb0` presentation for the H700 vendor kernel.
//!
//! Opens the framebuffer, honours its geometry and bitfields, and writes
//! converted frames row by row. When the driver supports panning and the
//! virtual height allows it, MAGI double-buffers with FBIOPAN_DISPLAY.

use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;

use anyhow::{bail, Context, Result};

use crate::canvas::Canvas;
use crate::util;

// The ioctls are classic (unsized) numbers from uapi/linux/fb.h; the pan
// number is easy to get wrong (0x4604 is FBIOGETCMAP).
const FBIOPAN_DISPLAY: libc::c_ulong = 0x4606;
const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
const FBIOPUT_VSCREENINFO: libc::c_ulong = 0x4601;
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;
const FBIOBLANK: libc::c_ulong = 0x4611;
const FB_UNBLANK: libc::c_int = 0;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Bitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: Bitfield,
    green: Bitfield,
    blue: Bitfield,
    transp: Bitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

impl Default for VarScreeninfo {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FixScreeninfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

impl Default for FixScreeninfo {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

pub struct Fbdev {
    pub w: u32,
    pub h: u32,
    pub bpp: u32,
    pub id: String,
    line_length: usize,
    file: File,
    map: *mut u8,
    map_len: usize,
    red: Bitfield,
    green: Bitfield,
    blue: Bitfield,
    double: bool,
    back: u32,
    pan_failed: bool,
}

impl Fbdev {
    pub fn open() -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/fb0")
            .context("open /dev/fb0")?;
        let fd = file.as_raw_fd();

        let mut var = VarScreeninfo::default();
        ioctl_get(fd, FBIOGET_VSCREENINFO, &mut var)?;
        let mut fix = FixScreeninfo::default();
        ioctl_get(fd, FBIOGET_FSCREENINFO, &mut fix)?;

        let (w, h, bpp) = (var.xres, var.yres, var.bits_per_pixel);
        if bpp != 16 && bpp != 32 {
            bail!("unsupported framebuffer depth {bpp}bpp");
        }
        let line_length = fix.line_length as usize;
        let map_len = if fix.smem_len > 0 {
            fix.smem_len as usize
        } else {
            line_length * var.yres_virtual as usize
        };
        if map_len < line_length * h as usize {
            bail!("framebuffer buffer too small ({map_len} bytes)");
        }

        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                map_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if map == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error()).context("mmap /dev/fb0");
        }

        // Best effort: a blanked display that nobody wakes would be rude.
        unsafe {
            libc::ioctl(fd, FBIOBLANK, FB_UNBLANK);
        }

        let id = String::from_utf8_lossy(&fix.id)
            .trim_end_matches('\0')
            .trim()
            .to_string();
        let double = fix.ypanstep > 0 && var.yres_virtual >= var.yres * 2;
        util::log(&format!(
            "fbdev: {id} {w}x{h} bpp={bpp} stride={line_length} vres={} panstep={} double={double}",
            var.yres_virtual, fix.ypanstep
        ));

        Ok(Self {
            w,
            h,
            bpp,
            id,
            line_length,
            file,
            map: map as *mut u8,
            map_len,
            red: var.red,
            green: var.green,
            blue: var.blue,
            double,
            back: 0,
            pan_failed: false,
        })
    }

    #[inline]
    fn pack(&self, p: u32) -> u32 {
        fn scale(v: u32, bits: u32) -> u32 {
            if bits >= 8 {
                v << (bits - 8)
            } else if bits == 0 {
                0
            } else {
                v >> (8 - bits)
            }
        }
        (scale((p >> 16) & 0xff, self.red.length) << self.red.offset)
            | (scale((p >> 8) & 0xff, self.green.length) << self.green.offset)
            | (scale(p & 0xff, self.blue.length) << self.blue.offset)
    }

    pub fn present(&mut self, cv: &Canvas) -> Result<()> {
        if cv.w as u32 != self.w || cv.h as u32 != self.h {
            bail!(
                "canvas {}x{} does not fit fb {}x{}",
                cv.w,
                cv.h,
                self.w,
                self.h
            );
        }
        let use_double = self.double && !self.pan_failed;
        let start = if use_double {
            self.back as usize * self.h as usize
        } else {
            0
        };
        let w = self.w as usize;
        let h = self.h as usize;
        let stride = self.line_length;

        unsafe {
            let base = self.map.add(start * stride);
            match self.bpp {
                16 => {
                    for y in 0..h {
                        let row =
                            std::slice::from_raw_parts_mut(base.add(y * stride) as *mut u16, w);
                        let src = &cv.px[y * w..(y + 1) * w];
                        for (d, &s) in row.iter_mut().zip(src) {
                            *d = self.pack(s) as u16;
                        }
                    }
                }
                _ => {
                    // 8:8:8 in the usual order means the canvas needs no
                    // per-pixel work; whole rows can be copied as they are.
                    let identity = self.red.offset == 16
                        && self.red.length == 8
                        && self.green.offset == 8
                        && self.green.length == 8
                        && self.blue.offset == 0
                        && self.blue.length == 8;
                    for y in 0..h {
                        let row =
                            std::slice::from_raw_parts_mut(base.add(y * stride) as *mut u32, w);
                        let src = &cv.px[y * w..(y + 1) * w];
                        if identity {
                            std::ptr::copy_nonoverlapping(src.as_ptr(), row.as_mut_ptr(), w);
                        } else {
                            for (d, &s) in row.iter_mut().zip(src) {
                                *d = self.pack(s);
                            }
                        }
                    }
                }
            }
        }

        if use_double {
            self.pan();
        }
        Ok(())
    }

    /// Just flip pages: no pixels copied. The game host uses it as its
    /// vsync clock while the DE layers own the screen (the pan itself is
    /// hidden under the UI layer).
    pub fn pan_tick(&mut self) {
        if self.double && !self.pan_failed {
            self.pan();
        }
    }

    /// Zero the whole mapping (both pages).
    pub fn wipe(&mut self) {
        unsafe {
            std::ptr::write_bytes(self.map, 0, self.map_len);
        }
    }

    fn pan(&mut self) {
        let fd = self.file.as_raw_fd();
        let target = self.back * self.h;
        let mut v = VarScreeninfo::default();
        if ioctl_get(fd, FBIOGET_VSCREENINFO, &mut v).is_err() {
            self.pan_failed = true;
            util::log("fbdev: cannot re-read screeninfo; using direct writes");
            return;
        }
        if v.yoffset != target {
            v.yoffset = target;
            let rc = unsafe { libc::ioctl(fd, FBIOPAN_DISPLAY, &v as *const VarScreeninfo) };
            if rc < 0 {
                self.pan_failed = true;
                util::log(&format!(
                    "fbdev: FBIOPAN_DISPLAY failed ({}); using direct writes",
                    std::io::Error::last_os_error()
                ));
                return;
            }
        }
        self.back ^= 1;
    }
}

impl Drop for Fbdev {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.map as *mut libc::c_void, self.map_len);
        }
    }
}

/// Re-apply the fb geometry with FB_ACTIVATE_FORCE so the display driver
/// rebuilds the fb0 DE layer - used after a game host pointed layer 0 at
/// its own scanout buffer.
pub fn rebuild_layer() -> bool {
    use std::os::unix::io::AsRawFd;
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/fb0")
    else {
        return false;
    };
    let mut var = VarScreeninfo::default();
    if ioctl_get(file.as_raw_fd(), FBIOGET_VSCREENINFO, &mut var).is_err() {
        return false;
    }
    var.activate = 0x80; // FB_ACTIVATE_NOW | FB_ACTIVATE_FORCE
    var.xoffset = 0;
    var.yoffset = 0;
    let rc = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            FBIOPUT_VSCREENINFO,
            &var as *const VarScreeninfo,
        )
    };
    rc >= 0
}

/// The live fb0 geometry: (xres, yres, yres_virtual).
pub fn fb_geometry() -> Option<(u32, u32, u32)> {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open("/dev/fb0")
        .ok()?;
    let mut var = VarScreeninfo::default();
    ioctl_get(file.as_raw_fd(), FBIOGET_VSCREENINFO, &mut var).ok()?;
    Some((var.xres, var.yres, var.yres_virtual.max(var.yres)))
}

fn ioctl_get<T>(fd: libc::c_int, request: libc::c_ulong, out: &mut T) -> Result<()> {
    let rc = unsafe { libc::ioctl(fd, request, out as *mut T) };
    if rc < 0 {
        bail!("ioctl {request:#x}: {}", std::io::Error::last_os_error());
    }
    Ok(())
}
