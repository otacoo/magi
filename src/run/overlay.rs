//! Overlay layers: scan, decode, composite, blend and persist the choices.

use std::path::Path;

use crate::canvas::Canvas;
use crate::util;

use super::prefs::runtime_config_path;

/// One decoded overlay PNG (0xAARRGGBB), pre-scaled to the panel.
struct OverlayLayer {
    px: Vec<u32>,
}

/// Overlay paths by layer, in blend order: CRT, bezel.
pub(super) type OverlayLayers = [Vec<String>; 2];

/// Which layer an overlay belongs to, from its path; colour-correction
/// files are ignored (that belongs in the core options).
fn overlay_kind(rel: &str) -> Option<usize> {
    let lower = rel.to_ascii_lowercase();
    let top = lower.split('/').next().unwrap_or("");
    if top.contains("crt") || lower.contains("_crt") {
        Some(0)
    } else if top.contains("color") || top.contains("colour") || top.contains("correction") {
        None
    } else {
        Some(1)
    }
}

/// Every PNG under Overlays/ split into the two layer lists, sorted.
pub(super) fn overlay_scan_layers() -> OverlayLayers {
    fn walk(dir: &Path, base: &Path, out: &mut OverlayLayers) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("png"))
            {
                if let Ok(rel) = p.strip_prefix(base) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    if let Some(kind) = overlay_kind(&rel) {
                        out[kind].push(rel);
                    }
                }
            }
        }
    }
    let base = Path::new("Overlays");
    let mut out: OverlayLayers = std::array::from_fn(|_| Vec::new());
    walk(base, base, &mut out);
    for list in &mut out {
        list.sort();
    }
    out
}

/// Decode a PNG into the canvas' 0xAARRGGBB layout, nearest-scaled to the
/// panel (the overlay packs are authored at 640x480 already).
fn load_overlay_png(name: &str, w: u32, h: u32) -> Option<OverlayLayer> {
    let path = std::path::Path::new("Overlays").join(name);
    let img = crate::image::load_png(&path)?;
    let img = if img.w == w && img.h == h {
        img
    } else {
        img.scaled(w, h)
    };
    Some(OverlayLayer { px: img.px })
}

/// The layer choices, flattened into one image at load time so the
/// per-frame cost is a single blend pass no matter how many layers are on
/// (three passes were dropping frames on GBA/GBC cores).
pub(super) struct Overlay {
    px: Vec<u32>,
    /// Contiguous runs of non-transparent pixels, when they save real work
    /// (bezel-style overlays are mostly clear in the middle). None means a
    /// plain full scan is cheaper.
    spans: Option<Vec<(u32, u32)>>,
    any: bool,
}

/// Runs of pixels that need blending; None when spans would not pay off
/// (near-full coverage or a very fragmented pattern, e.g. CRT scanlines).
fn transparent_spans(px: &[u32]) -> Option<Vec<(u32, u32)>> {
    let mut spans: Vec<(u32, u32)> = Vec::new();
    let mut start: Option<u32> = None;
    let mut covered = 0usize;
    for (i, &p) in px.iter().enumerate() {
        if p != 0xFF00_0000 {
            covered += 1;
            if start.is_none() {
                start = Some(i as u32);
            }
        } else if let Some(s) = start.take() {
            spans.push((s, i as u32));
            if spans.len() > 8192 {
                return None;
            }
        }
    }
    if let Some(s) = start {
        spans.push((s, px.len() as u32));
    }
    (spans.len() <= 8192 && covered <= px.len() * 9 / 10).then_some(spans)
}

/// One source-over blend step on premultiplied pixels.
#[inline]
fn blend_px(dst: &mut u32, src: u32) {
    if src == 0xFF00_0000 {
        return;
    }
    let inv = src >> 24;
    let d = *dst;
    let r = ((src >> 16) & 0xff) + (((d >> 16) & 0xff) * inv + 127) / 255;
    let g = ((src >> 8) & 0xff) + (((d >> 8) & 0xff) * inv + 127) / 255;
    let b = (src & 0xff) + ((d & 0xff) * inv + 127) / 255;
    *dst = 0xFF00_0000 | (r << 16) | (g << 8) | b;
}

/// Four pixels at a time on NEON. The overlay is premultiplied, so each
/// channel is `src + dst * inv / 255`; a fully transparent pixel (the
/// 0xFF000000 marker) makes this an identity.
#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn blend4_neon(dst: *mut u32, src: *const u32) {
    use std::arch::aarch64::*;
    let s = vld1q_u32(src);
    let d = vld1q_u32(dst as *const u32);
    let mask = vdupq_n_u32(0xff);
    let sh24 = vdupq_n_s32(-24);
    let sh16 = vdupq_n_s32(-16);
    let sh8 = vdupq_n_s32(-8);
    let sh23 = vdupq_n_s32(-23);
    let left16 = vdupq_n_s32(16);
    let left8 = vdupq_n_s32(8);
    let inv = vshlq_u32(s, sh24);
    // Alpha 255 (inv 0) is a copy; alpha 0 (the transparent marker) is a
    // no-op. CRT/bezel art is mostly one or the other outside the screen.
    if vmaxvq_u32(inv) == 0 {
        vst1q_u32(dst, vorrq_u32(s, vdupq_n_u32(0xFF00_0000)));
        return;
    }
    if vminvq_u32(inv) == 255 {
        return;
    }
    let sr = vandq_u32(vshlq_u32(s, sh16), mask);
    let sg = vandq_u32(vshlq_u32(s, sh8), mask);
    let sb = vandq_u32(s, mask);
    let dr = vandq_u32(vshlq_u32(d, sh16), mask);
    let dg = vandq_u32(vshlq_u32(d, sh8), mask);
    let db = vandq_u32(d, mask);
    let round = vdupq_n_u32(127);
    let magic = vdupq_n_u32(32897);
    let div255 = |x: uint32x4_t| vshlq_u32(vmulq_u32(x, magic), sh23);
    let r = vaddq_u32(sr, div255(vaddq_u32(vmulq_u32(dr, inv), round)));
    let g = vaddq_u32(sg, div255(vaddq_u32(vmulq_u32(dg, inv), round)));
    let b = vaddq_u32(sb, div255(vaddq_u32(vmulq_u32(db, inv), round)));
    let out = vorrq_u32(
        vorrq_u32(vshlq_u32(r, left16), vshlq_u32(g, left8)),
        vorrq_u32(b, vdupq_n_u32(0xFF00_0000)),
    );
    vst1q_u32(dst, out);
}

fn blend_slice(dst: &mut [u32], src: &[u32]) {
    let n = dst.len().min(src.len());
    let mut i = 0;
    #[cfg(target_arch = "aarch64")]
    unsafe {
        while i + 4 <= n {
            blend4_neon(dst.as_mut_ptr().add(i), src.as_ptr().add(i));
            i += 4;
        }
    }
    while i < n {
        blend_px(&mut dst[i], src[i]);
        i += 1;
    }
}

/// Source-over `src` onto `dst`, in place.
fn over(dst: &mut u32, src: u32) {
    let sa = src >> 24;
    if sa == 0 {
        return;
    }
    if sa == 255 {
        *dst = src;
        return;
    }
    let da = 255 - sa;
    let r = (((src >> 16) & 0xff) * sa + ((*dst >> 16) & 0xff) * da) / 255;
    let g = (((src >> 8) & 0xff) * sa + ((*dst >> 8) & 0xff) * da) / 255;
    let b = ((src & 0xff) * sa + (*dst & 0xff) * da) / 255;
    *dst = 0xFF00_0000 | (r << 16) | (g << 8) | b;
}

/// Source-over that also composites the alpha channel. `over` is for the
/// final opaque canvas; composites that stay transparent (the DE overlay)
/// need this one, or the second layer turns the whole image opaque.
fn over_alpha(dst: &mut u32, src: u32) {
    let sa = src >> 24;
    if sa == 0 {
        return;
    }
    if sa == 255 {
        *dst = src;
        return;
    }
    let da = *dst >> 24;
    let inv = 255 - sa;
    let ao = sa + da * inv / 255;
    if ao == 0 {
        *dst = 0;
        return;
    }
    let mix = |s: u32, d: u32| ((s * sa + d * da * inv / 255) / ao).min(255);
    let r = mix((src >> 16) & 0xff, (*dst >> 16) & 0xff);
    let g = mix((src >> 8) & 0xff, (*dst >> 8) & 0xff);
    let b = mix(src & 0xff, *dst & 0xff);
    *dst = (ao << 24) | (r << 16) | (g << 8) | b;
}

/// Straight-alpha composite of the layer choices (0xAARRGGBB); the hardware
/// layer uploads this as-is.
pub(super) fn load_composite(names: &[String; 2], w: u32, h: u32) -> Option<Vec<u32>> {
    let mut iter = names.iter().filter(|n| !n.is_empty());
    let first = iter.next()?;
    let mut img = load_overlay_png(first, w, h)?;
    for name in iter {
        let Some(l) = load_overlay_png(name, w, h) else {
            continue;
        };
        if l.px.len() != img.px.len() {
            continue;
        }
        for (dst, &src) in img.px.iter_mut().zip(l.px.iter()) {
            over_alpha(dst, src);
        }
    }
    Some(img.px)
}

/// The bezel's screen region: the bounding box of its non-opaque pixels.
pub(super) fn screen_area(px: &[u32], w: u32, h: u32) -> Option<(i32, i32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, -1i32, -1i32);
    for y in 0..h as usize {
        for x in 0..w as usize {
            if px[y * w as usize + x] >> 24 < 240 {
                x0 = x0.min(x as u32);
                y0 = y0.min(y as u32);
                x1 = x1.max(x as i32);
                y1 = y1.max(y as i32);
            }
        }
    }
    let (aw, ah) = ((x1 - x0 as i32 + 1).max(0), (y1 - y0 as i32 + 1).max(0));
    if aw < (w / 4) as i32 || ah < (h / 4) as i32 {
        return None;
    }
    Some((x0 as i32, y0 as i32, aw as u32, ah as u32))
}

/// Source-over a straight-alpha composite onto the canvas (screenshots when
/// the overlay lives in a DE layer instead).
pub(super) fn blend_straight(cv: &mut Canvas, px: Option<&[u32]>) {
    let Some(px) = px else { return };
    if px.len() != cv.px.len() {
        return;
    }
    for (dst, &src) in cv.px.iter_mut().zip(px.iter()) {
        over(dst, src);
    }
}

/// The overlay DE layer's image: opaque backplate with a transparent hole at
/// the game rect, art on top. The backplate hides the fb layer underneath
/// and fills any gap between the game image and the bezel's screen.
#[cfg(target_os = "linux")]
pub(super) fn upload_overlay(
    layer: &mut Option<crate::disp2::ImageLayer>,
    art: Option<&[u32]>,
    cw: usize,
    ch: usize,
    rect: (i32, i32, u32, u32),
) {
    let Some(layer) = layer.as_mut() else { return };
    let mut px = vec![0xFF03_0303u32; cw * ch];
    for y in rect.1.max(0)..(rect.1 + rect.3 as i32).min(ch as i32) {
        for x in rect.0.max(0)..(rect.0 + rect.2 as i32).min(cw as i32) {
            px[y as usize * cw + x as usize] = 0;
        }
    }
    if let Some(art) = art {
        for (d, &s) in px.iter_mut().zip(art.iter()) {
            over(d, s);
        }
    }
    layer.upload(&px);
}

/// The bezel's screen region, from its PNG alpha (None = full canvas).
pub(super) fn load_bezel_area(names: &[String; 2], w: u32, h: u32) -> Option<(i32, i32, u32, u32)> {
    if names[1].is_empty() {
        return None;
    }
    let img = load_overlay_png(&names[1], w, h)?;
    screen_area(&img.px, w, h)
}

/// Software fallback: premultiply for the frame blend (RGB carries src*a,
/// the top byte is 255-a, so per frame it is one multiply per channel
/// instead of two; 0xFF00_0000 marks a fully transparent pixel).
pub(super) fn premultiplied(px: &[u32]) -> Overlay {
    let mut px = px.to_vec();
    let mut any = false;
    for p in &mut px {
        let a = *p >> 24;
        if a == 0 {
            *p = 0xFF00_0000;
            continue;
        }
        any = true;
        let inv = 255 - a;
        let r = ((*p >> 16) & 0xff) * a / 255;
        let g = ((*p >> 8) & 0xff) * a / 255;
        let b = (*p & 0xff) * a / 255;
        *p = (inv << 24) | (r << 16) | (g << 8) | b;
    }
    let spans = transparent_spans(&px);
    Overlay { px, spans, any }
}

/// Blend the pre-composited overlay over the finished canvas.
pub(super) fn apply_overlays(cv: &mut Canvas, overlay: &Option<Overlay>) {
    let Some(o) = overlay else { return };
    if !o.any || o.px.len() != cv.px.len() {
        return;
    }
    match &o.spans {
        Some(spans) => {
            for &(s, e) in spans {
                let (s, e) = (s as usize, e as usize);
                blend_slice(&mut cv.px[s..e], &o.px[s..e]);
            }
        }
        None => {
            let mid = (cv.px.len() / 2) & !3;
            if mid >= 4096 {
                let (dst_a, dst_b) = cv.px.split_at_mut(mid);
                let (src_a, src_b) = o.px.split_at(mid);
                std::thread::scope(|scope| {
                    scope.spawn(|| blend_slice(dst_b, src_b));
                    blend_slice(dst_a, src_a);
                });
            } else {
                blend_slice(&mut cv.px, &o.px);
            }
        }
    }
}

/// Per-system overlay choices from Config/magi.json ("" = off). A missing
/// layer uses the shipped default; the legacy global keys seed the first
/// launch, and an explicit empty string turns it off. NGP/NGPC share one
/// system, so the ROM extension picks the bezel.
fn default_overlays(system: &str, rom: &str, wide: bool) -> [&'static str; 2] {
    let crt = if wide {
        "CRT/Perfect_CRT-720x480.png"
    } else {
        "CRT/Perfect_CRT.png"
    };
    let crt240 = if wide {
        "CRT/Perfect_CRT-240p-720x480.png"
    } else {
        "CRT/Perfect_CRT-240p.png"
    };
    let crt240_noframe = if wide {
        "CRT/Perfect_CRT-240p(noframe)-720x480.png"
    } else {
        "CRT/Perfect_CRT-240p(noframe).png"
    };
    match system {
        "psx" => [crt, ""],
        "snes" | "pce" | "nes" | "neocd" => [crt240, ""],
        "genesis" => [crt240_noframe, ""],
        "gba" => {
            if wide {
                // 720x480 is exactly 3x GBA; a bezel would only shrink it.
                ["", ""]
            } else {
                ["", "GBA/mugwomp93/Perfect_GBA.png"]
            }
        }
        "gbc" => [
            "",
            if wide {
                "GBC/mugwomp93/Perfect_GBC-720x480.png"
            } else {
                "GBC/mugwomp93/Perfect_GBC.png"
            },
        ],
        "gb" => [
            "",
            if wide {
                "GB/mugwomp93/Perfect_GBP-EX-720x480.png"
            } else {
                "GB/mugwomp93/Perfect_GBP-EX.png"
            },
        ],
        "ngp" => {
            // NGP and NGPC share one catalogue entry and folder.
            let ngp = rom.to_ascii_lowercase().ends_with(".ngp");
            match (wide, ngp) {
                (true, true) => ["", "NGP/mugwomp93/ngp-720x480.png"],
                (true, false) => ["", "NGPC/mugwomp93/ngpc-720x480.png"],
                (false, true) => ["", "NGP/mugwomp93/ngp-640x480.png"],
                (false, false) => ["", "NGPC/mugwomp93/ngpc-640x480.png"],
            }
        }
        _ => ["", ""],
    }
}

fn overlay_prefs_from(v: &serde_json::Value, system: &str, rom: &str, wide: bool) -> [String; 2] {
    let has_map = v.get("overlays").is_some_and(|o| o.is_object());
    let per = v.get("overlays").and_then(|o| o.get(system));
    let get = |layer: usize, key: &str| -> String {
        let explicit = if has_map {
            per.and_then(|p| p.get(key)).and_then(|x| x.as_str())
        } else {
            v.get(format!("overlay_{key}")).and_then(|x| x.as_str())
        };
        match explicit {
            Some(value) => value.to_string(),
            None => default_overlays(system, rom, wide)[layer].to_string(),
        }
    };
    [get(0, "crt"), get(1, "bezel")]
}

pub(super) fn load_overlay_prefs(system: &str, rom: &str, wide: bool) -> [String; 2] {
    let v: serde_json::Value = std::fs::read_to_string(runtime_config_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    overlay_prefs_from(&v, system, rom, wide)
}

pub(super) fn save_overlay_prefs(system: &str, names: &[String; 2]) {
    let path = Path::new(runtime_config_path());
    let mut v: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if !v.get("overlays").is_some_and(|o| o.is_object()) {
        v["overlays"] = serde_json::json!({});
    }
    v["overlays"][system] = serde_json::json!({
        "crt": names[0],
        "bezel": names[1],
    });
    // Keep the old top-level keys current for older builds and downgrades.
    v["overlay_crt"] = serde_json::json!(names[0]);
    v["overlay_bezel"] = serde_json::json!(names[1]);
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::create_dir_all("Config");
        let _ = util::write_file_atomic(path, text.as_bytes());
    }
}

#[cfg(test)]
mod image_overlay_tests {
    use super::*;

    #[test]
    fn overlay_kind_sorts_pack_folders() {
        assert_eq!(overlay_kind("CRT/Perfect_CRT.png"), Some(0));
        assert_eq!(overlay_kind("CRT/Perfect_CRT-240p(noframe).png"), Some(0));
        assert_eq!(overlay_kind("GB/drkhrse/Perfect_DMG-EX.png"), Some(1));
        assert_eq!(overlay_kind("misc/Perfect_GBA-SP.png"), Some(1));
        // Colour correction belongs in the core options, not a layer.
        assert_eq!(overlay_kind("COLOR/scanlines.png"), None);
        assert_eq!(overlay_kind("colour/tint.png"), None);
    }

    #[test]
    fn overlay_choices_are_per_system() {
        let v = serde_json::json!({
            "overlays": {"gba": {"crt": "CRT/a.png", "bezel": "GBA/b.png"}}
        });
        assert_eq!(
            overlay_prefs_from(&v, "gba", "", false),
            ["CRT/a.png".to_string(), "GBA/b.png".to_string()]
        );
        assert_eq!(
            overlay_prefs_from(&v, "genesis", "", false),
            [
                "CRT/Perfect_CRT-240p(noframe).png".to_string(),
                String::new()
            ],
            "the global keys do not leak across systems"
        );
        let legacy = serde_json::json!({
            "overlay_crt": "CRT/x.png",
            "overlay_bezel": ""
        });
        assert_eq!(
            overlay_prefs_from(&legacy, "snes", "", false),
            ["CRT/x.png".to_string(), String::new()],
            "the legacy globals seed the first launch"
        );
    }

    #[test]
    fn fresh_configs_get_the_shipped_overlays() {
        let v = serde_json::json!({});
        assert_eq!(
            overlay_prefs_from(&v, "psx", "", false),
            ["CRT/Perfect_CRT.png".to_string(), String::new()]
        );
        for system in ["snes", "pce", "nes", "neocd"] {
            assert_eq!(
                overlay_prefs_from(&v, system, "", false),
                ["CRT/Perfect_CRT-240p.png".to_string(), String::new()],
                "{system}"
            );
            assert_eq!(
                overlay_prefs_from(&v, system, "", true),
                [
                    "CRT/Perfect_CRT-240p-720x480.png".to_string(),
                    String::new()
                ],
                "{system} wide"
            );
        }
        assert_eq!(
            overlay_prefs_from(&v, "genesis", "", false),
            [
                "CRT/Perfect_CRT-240p(noframe).png".to_string(),
                String::new()
            ]
        );
        assert_eq!(
            overlay_prefs_from(&v, "genesis", "", true),
            [
                "CRT/Perfect_CRT-240p(noframe)-720x480.png".to_string(),
                String::new()
            ]
        );
        assert_eq!(
            overlay_prefs_from(&v, "gba", "", false),
            [String::new(), "GBA/mugwomp93/Perfect_GBA.png".to_string()]
        );
        assert_eq!(
            overlay_prefs_from(&v, "gba", "", true),
            [String::new(), String::new()],
            "720x480 is exactly 3x GBA"
        );
        assert_eq!(
            overlay_prefs_from(&v, "gbc", "", false),
            [String::new(), "GBC/mugwomp93/Perfect_GBC.png".to_string()]
        );
        assert_eq!(
            overlay_prefs_from(&v, "gb", "", false),
            [String::new(), "GB/mugwomp93/Perfect_GBP-EX.png".to_string()]
        );
        assert_eq!(
            overlay_prefs_from(&v, "gb", "", true),
            [
                String::new(),
                "GB/mugwomp93/Perfect_GBP-EX-720x480.png".to_string()
            ]
        );
        assert_eq!(
            overlay_prefs_from(&v, "ngp", "Games/NGPC/Game.ngp", false),
            ["".to_string(), "NGP/mugwomp93/ngp-640x480.png".to_string()]
        );
        assert_eq!(
            overlay_prefs_from(&v, "ngp", "Games/NGPC/Game.ngc", false),
            [
                "".to_string(),
                "NGPC/mugwomp93/ngpc-640x480.png".to_string()
            ]
        );
        assert_eq!(
            overlay_prefs_from(&v, "ngp", "Games/NGPC/Game.ngp", true),
            ["".to_string(), "NGP/mugwomp93/ngp-720x480.png".to_string()]
        );
        assert_eq!(
            overlay_prefs_from(&v, "ngp", "Games/NGPC/Game.ngc", true),
            [
                "".to_string(),
                "NGPC/mugwomp93/ngpc-720x480.png".to_string()
            ]
        );
    }
}
