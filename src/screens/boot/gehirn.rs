//! Procedural GEHIRN wireframe: a rotating grid sphere with depth-shaded
//! lines. The splash reuses the mesh without the scan band.

use std::f32::consts::{PI, TAU};

use crate::canvas::Canvas;
use crate::theme::{mix, pal, Color};

const MERIDIANS: usize = 26;
const PARALLELS: usize = 14;
const MERIDIAN_SEG: usize = 40;
const PARALLEL_SEG: usize = 56;
const CAMERA: f32 = 4.2;

/// Probe nodes scattered around the mesh, as (latitude, longitude, radius).
const NODES: [(f32, f32, f32); 16] = [
    (0.62, 0.20, 1.18),
    (-0.55, 0.80, 1.22),
    (0.20, 1.70, 1.14),
    (-0.15, 2.50, 1.26),
    (0.75, 3.10, 1.20),
    (-0.70, 3.80, 1.16),
    (0.10, 4.50, 1.24),
    (-0.35, 5.20, 1.18),
    (0.45, 5.90, 1.22),
    (-0.80, 0.50, 1.15),
    (0.35, 1.20, 1.25),
    (-0.25, 2.10, 1.20),
    (0.05, 3.50, 1.16),
    (0.55, 4.20, 1.23),
    (-0.60, 4.90, 1.17),
    (0.30, 5.60, 1.21),
];

/// Draw the animated globe; rows outside the clip are skipped.
pub fn draw(
    cv: &mut Canvas,
    cx: f32,
    cy: f32,
    r: f32,
    t: f32,
    alpha: f32,
    clip_top: i32,
    clip_bottom: i32,
) {
    draw_mesh(cv, cx, cy, r, t, alpha, clip_top, clip_bottom, true);
}

/// The static splash uses the same mesh, with the scan band left off.
pub fn draw_splash(cv: &mut Canvas, cx: f32, cy: f32, r: f32, alpha: f32) {
    draw_mesh(cv, cx, cy, r, 0.0, alpha, 0, cv.h, false);
}

fn draw_mesh(
    cv: &mut Canvas,
    cx: f32,
    cy: f32,
    r: f32,
    t: f32,
    alpha: f32,
    clip_top: i32,
    clip_bottom: i32,
    scan: bool,
) {
    if clip_bottom <= clip_top {
        return;
    }
    let yaw = t * 0.42;
    let pitch = 0.38 + 0.07 * (t * 0.63).sin();
    let roll = 0.04 * (t * 0.37).sin();
    let view = View {
        cx,
        cy,
        r,
        yaw,
        pitch,
        roll,
        clip_top: clip_top as f32,
        clip_bottom: clip_bottom as f32,
    };

    axis(cv, &view, alpha);
    grid(cv, &view, alpha);
    if scan {
        scan_band(cv, &view, t, alpha);
    }
    nodes(cv, &view, alpha);
}

struct View {
    cx: f32,
    cy: f32,
    r: f32,
    yaw: f32,
    pitch: f32,
    roll: f32,
    clip_top: f32,
    clip_bottom: f32,
}

impl View {
    /// Rotate a point and project it. Returns `(x, y, depth)`.
    fn point(&self, x: f32, y: f32, z: f32) -> (f32, f32, f32) {
        let (sy, cy) = self.yaw.sin_cos();
        let x1 = x * cy + z * sy;
        let z1 = -x * sy + z * cy;
        let (sp, cp) = self.pitch.sin_cos();
        let y2 = y * cp - z1 * sp;
        let z2 = y * sp + z1 * cp;
        let (sr, cr) = self.roll.sin_cos();
        let x3 = x1 * cr - y2 * sr;
        let y3 = x1 * sr + y2 * cr;
        let f = CAMERA / (CAMERA - z2);
        (self.cx + x3 * self.r * f, self.cy + y3 * self.r * f, z2)
    }
}

fn grid(cv: &mut Canvas, view: &View, alpha: f32) {
    let p = pal();

    for m in 0..MERIDIANS {
        let lon = m as f32 / MERIDIANS as f32 * TAU;
        let (sl, cl) = lon.sin_cos();
        let base = if m == 0 { p.accent } else { p.text_dim };
        let boost = if m == 0 { 1.0 } else { 0.92 };
        let mut prev = None;
        for i in 0..=MERIDIAN_SEG {
            let lat = -PI * 0.5 + PI * i as f32 / MERIDIAN_SEG as f32;
            let (sp, cp) = lat.sin_cos();
            let q = view.point(cp * cl, sp, cp * sl);
            if let Some(prev) = prev {
                segment(cv, view, prev, q, base, alpha * boost, 0.20, 1.0);
            }
            prev = Some(q);
        }
    }

    for j in 1..PARALLELS {
        let lat = -PI * 0.5 + PI * j as f32 / PARALLELS as f32;
        let (sp, cp) = lat.sin_cos();
        let equator = j * 2 == PARALLELS;
        let base = if equator { p.accent } else { p.text_dim };
        let boost = if equator { 1.0 } else { 0.86 };
        let mut prev = None;
        for i in 0..=PARALLEL_SEG {
            let lon = i as f32 / PARALLEL_SEG as f32 * TAU;
            let (sl, cl) = lon.sin_cos();
            let q = view.point(cp * cl, sp, cp * sl);
            if let Some(prev) = prev {
                segment(cv, view, prev, q, base, alpha * boost, 0.20, 1.0);
            }
            prev = Some(q);
        }
    }
}

fn scan_band(cv: &mut Canvas, view: &View, t: f32, alpha: f32) {
    let p = pal();
    let lat = (t * 1.55).sin() * 1.05;
    for (offset, a, weight) in [(0.0, 0.95, 1.6), (-0.075, 0.22, 1.0)] {
        let at = (lat + offset).clamp(-1.35, 1.35);
        let (sp, cp) = at.sin_cos();
        let mut prev = None;
        for i in 0..=PARALLEL_SEG {
            let lon = i as f32 / PARALLEL_SEG as f32 * TAU;
            let (sl, cl) = lon.sin_cos();
            let q = view.point(cp * cl, sp, cp * sl);
            if let Some(prev) = prev {
                segment(cv, view, prev, q, p.glow, alpha * a, 0.35, weight);
            }
            prev = Some(q);
        }
    }
}

fn nodes(cv: &mut Canvas, view: &View, alpha: f32) {
    let p = pal();
    for (lat, lon, radius) in NODES {
        let (sp, cp) = lat.sin_cos();
        let (sl, cl) = lon.sin_cos();
        let q = view.point(radius * cp * cl, radius * sp, radius * cp * sl);
        let front = ((q.2 + 1.0) * 0.5).clamp(0.0, 1.0);
        let a = (alpha * (0.45 + 0.55 * front) * 2.1).clamp(0.0, 1.0);
        dot(cv, q.0, q.1, 2.0, mix(p.bg0, p.text, a));
        dot(
            cv,
            q.0,
            q.1,
            0.9,
            mix(p.bg0, p.glow, (a * 1.15).clamp(0.0, 1.0)),
        );
    }
}

fn axis(cv: &mut Canvas, view: &View, alpha: f32) {
    let p = pal();
    let color = mix(p.bg0, p.text_dim, (alpha * 0.34).clamp(0.0, 1.0));
    let north = view.point(0.0, 1.12, 0.0);
    let south = view.point(0.0, -1.12, 0.0);
    line(
        cv,
        north.0,
        north.1,
        south.0,
        south.1,
        color,
        view.clip_top,
        view.clip_bottom,
    );
    for q in [north, south] {
        line(
            cv,
            q.0 - 3.0,
            q.1,
            q.0 + 3.0,
            q.1,
            color,
            view.clip_top,
            view.clip_bottom,
        );
    }
}

fn segment(
    cv: &mut Canvas,
    view: &View,
    a: (f32, f32, f32),
    b: (f32, f32, f32),
    base: Color,
    alpha: f32,
    floor: f32,
    weight: f32,
) {
    let depth = (a.2 + b.2) * 0.5;
    let d = floor + (1.0 - floor) * ((depth + 1.0) * 0.5).clamp(0.0, 1.0);
    let color = mix(pal().bg0, base, (alpha * d).clamp(0.0, 1.0));
    line(
        cv,
        a.0,
        a.1,
        b.0,
        b.1,
        color,
        view.clip_top,
        view.clip_bottom,
    );
    if weight > 1.2 {
        line(
            cv,
            a.0 + 1.0,
            a.1,
            b.0 + 1.0,
            b.1,
            color,
            view.clip_top,
            view.clip_bottom,
        );
    }
}

/// A clipped floating-point line.
fn line(
    cv: &mut Canvas,
    mut x0: f32,
    mut y0: f32,
    mut x1: f32,
    mut y1: f32,
    c: Color,
    clip_top: f32,
    clip_bottom: f32,
) {
    if (y0 < clip_top && y1 < clip_top) || (y0 >= clip_bottom && y1 >= clip_bottom) {
        return;
    }
    if y0 < clip_top {
        let t = (clip_top - y0) / (y1 - y0);
        x0 += (x1 - x0) * t;
        y0 = clip_top;
    }
    if y1 < clip_top {
        let t = (clip_top - y1) / (y0 - y1);
        x1 += (x0 - x1) * t;
        y1 = clip_top;
    }
    if y0 >= clip_bottom {
        let t = (clip_bottom - y0) / (y1 - y0);
        x0 += (x1 - x0) * t;
        y0 = clip_bottom;
    }
    if y1 >= clip_bottom {
        let t = (clip_bottom - y1) / (y0 - y1);
        x1 += (x0 - x1) * t;
        y1 = clip_bottom;
    }
    let dx = x1 - x0;
    let dy = y1 - y0;
    let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as i32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        cv.put(
            (x0 + dx * t).round() as i32,
            (y0 + dy * t).round() as i32,
            c,
        );
    }
}

fn dot(cv: &mut Canvas, x: f32, y: f32, r: f32, color: Color) {
    let ri = r.round().max(0.0) as i32;
    let (cx, cy) = (x.round() as i32, y.round() as i32);
    for dy in -ri..=ri {
        for dx in -ri..=ri {
            if dx * dx + dy * dy <= ri * ri {
                cv.put(cx + dx, cy + dy, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_globe_draws_inside_its_clip() {
        let bg = pal().bg0.to_u32();
        let mut cv = Canvas::new(220, 220);
        cv.clear(pal().bg0);
        draw(&mut cv, 110.0, 110.0, 78.0, 1.0, 1.0, 0, 220);
        let ink = cv.px.iter().filter(|&&p| p != bg).count();
        assert!(ink > 500, "ink {ink}");

        let mut empty = Canvas::new(220, 220);
        empty.clear(pal().bg0);
        draw(&mut empty, 110.0, 110.0, 78.0, 1.0, 1.0, 0, 0);
        assert!(empty.px.iter().all(|&p| p == bg));
    }
}
