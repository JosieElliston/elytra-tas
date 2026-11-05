//! Tools built from the primitives; nothing here reaches inside a [`Frame`]. Copy one
//! and change it if it doesn't fit.

use crate::camera::{Camera, V3, add, dot, look_vector, scale, sub};
use crate::color::{Color, INK, Ramp, WHITE};
use crate::frame::{Align, Frame, Marker, Stroke, TextStyle};
use crate::scene::Scene;
use crate::style::WorldStyle;

const FLIGHT_WIDTH: f64 = 0.6;
const FLIGHT_HEIGHT: f64 = 0.6;

/// The title strip, top left.
pub fn title(f: &mut Frame, text: &str) {
    f.text([8.0, 8.0], text, TextStyle::default());
}

/// A gliding player's 0.6 box at a feet position.
pub fn hitbox(f: &mut Frame, feet: V3, stroke: Stroke) {
    let r = FLIGHT_WIDTH * 0.5;
    f.aabb(
        [feet[0] - r, feet[1], feet[2] - r],
        [feet[0] + r, feet[1] + FLIGHT_HEIGHT, feet[2] + r],
        stroke,
        None,
    );
}

/// The look direction from a gliding player's eye, `length` blocks long.
pub fn look(f: &mut Frame, feet: V3, yaw: f64, pitch: f64, length: f64, stroke: Stroke) {
    let eye = [feet[0], feet[1] + 0.4, feet[2]];
    f.arrow(eye, add(eye, scale(look_vector(yaw, pitch), length)), stroke);
}

/// A velocity arrow from a feet position, reaching where the player would be in `ticks`.
pub fn velocity(f: &mut Frame, feet: V3, v: V3, ticks: f64, stroke: Stroke) {
    f.arrow(feet, add(feet, scale(v, ticks)), stroke);
}

/// A polyline colored by a value per vertex over `lo..hi`. Add [`colorbar`] to say what
/// the colors mean.
pub fn gradient(f: &mut Frame, pts: &[V3], values: &[f64], lo: f64, hi: f64, ramp: &Ramp, stroke: Stroke) {
    let span = (hi - lo).abs().max(1e-12);
    let colors: Vec<Color> = values.iter().map(|v| ramp.at((v - lo) / span)).collect();
    f.polyline_colored(pts, &colors, stroke);
}

/// A nice step: 1, 2 or 5 times a power of ten, at least `min`.
pub fn nice_step(min: f64) -> f64 {
    let p = 10f64.powf(min.max(1e-9).log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if m * p >= min {
            return m * p;
        }
    }
    10.0 * p
}

/// Grid lines on the plane `y`, over a box in x and z.
pub fn grid_xz(f: &mut Frame, y: f64, min: [f64; 2], max: [f64; 2], spacing: f64, color: Color) {
    let st = Stroke::new(color, 1.0).halo(0.0).occluded(0.15);
    let mut x = (min[0] / spacing).ceil() * spacing;
    while x <= max[0] {
        f.line([x, y, min[1]], [x, y, max[1]], st);
        x += spacing;
    }
    let mut z = (min[1] / spacing).ceil() * spacing;
    while z <= max[1] {
        f.line([min[0], y, z], [max[0], y, z], st);
        z += spacing;
    }
}

fn axis_name(v: V3) -> Option<(&'static str, usize, f64)> {
    for (k, n) in ["x", "y", "z"].into_iter().enumerate() {
        if v[k].abs() > 0.999 {
            return Some((n, k, v[k].signum()));
        }
    }
    None
}

/// Coordinate rulers on the left and bottom edges of an orthographic view, with faint
/// grid lines, so positions can be read off the image. Does nothing in perspective.
/// An axis that isn't a world axis is labeled `s`, its distance along the screen.
pub fn rulers(f: &mut Frame) {
    let v = f.view().clone();
    let Some(k) = v.ortho else { return };
    let [w, h] = f.size();
    let eye_r = dot(v.eye, v.right);
    let eye_u = dot(v.eye, v.up);
    let step = nice_step(70.0 / k);
    let font = 12.0;
    let line = WHITE.alpha(0.22);
    let tag = TextStyle::default().size(font);
    // horizontal axis: value at pixel x is eye_r + (x - cx) / k
    let (hname, _, hsign) = axis_name(v.right).unwrap_or(("s", 0, 1.0));
    let lo = eye_r + (0.0 - v.cx) / k;
    let hi = eye_r + (w as f64 - v.cx) / k;
    let mut r = (lo / step).ceil() * step;
    while r <= hi {
        let x = (v.cx + (r - eye_r) * k) as f32;
        f.line2d([x, 0.0], [x, h], line, 1.0);
        f.line2d([x, h - 7.0], [x, h], WHITE, 1.5);
        let val = r * hsign;
        // clear of the vertical axis' labels and the compass
        if x > 74.0 && x < w - 150.0 {
            f.text(
                [x, h - 9.0],
                format!("{hname}={}", fmt_coord(val)),
                tag.align(Align::BottomLeft).background(Some(INK.alpha(0.6))),
            );
        }
        r += step;
    }
    // vertical axis
    let (vname, _, vsign) = axis_name(v.up).unwrap_or(("s", 0, 1.0));
    let lo = eye_u + (v.cy - h as f64) / k;
    let hi = eye_u + v.cy / k;
    let mut u = (lo / step).ceil() * step;
    while u <= hi {
        let y = (v.cy - (u - eye_u) * k) as f32;
        f.line2d([0.0, y], [w, y], line, 1.0);
        f.line2d([0.0, y], [7.0, y], WHITE, 1.5);
        let val = u * vsign;
        if y < h - 44.0 {
            f.text(
                [9.0, y],
                format!("{vname}={}", fmt_coord(val)),
                tag.align(Align::TopLeft).background(Some(INK.alpha(0.6))),
            );
        }
        u += step;
    }
}

fn fmt_coord(v: f64) -> String {
    if (v - v.round()).abs() < 1e-6 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

/// A small gizmo of the world axes, bottom right unless placed.
pub fn compass(f: &mut Frame, at: Option<[f32; 2]>) {
    let [w, h] = f.size();
    let c = at.unwrap_or([w - 52.0, h - 56.0]);
    f.disc2d(c, 40.0, INK.alpha(0.55));
    let v = f.view().clone();
    let len = 28.0;
    for (axis, name, col) in [
        ([1.0, 0.0, 0.0], "+x", Color::hex(0xFF5A5A)),
        ([0.0, 1.0, 0.0], "+y", Color::hex(0x6BE07A)),
        ([0.0, 0.0, 1.0], "+z", Color::hex(0x6AAEFF)),
    ] {
        let (r, u) = (dot(axis, v.right) as f32, dot(axis, v.up) as f32);
        // an axis along the view direction shrinks to a dot, which is the honest picture
        let end = [c[0] + r * len, c[1] - u * len];
        f.line2d(c, end, col, 2.2);
        f.text(
            [end[0] + r * 4.0, end[1] - u * 4.0],
            name,
            TextStyle::default()
                .size(11.0)
                .color(WHITE)
                .background(None)
                .align(Align::Center),
        );
    }
}

/// A legend box, top right: a color chip and a name per entry.
pub fn legend(f: &mut Frame, entries: &[(&str, Color)]) {
    let [w, _] = f.size();
    let size = 13.0;
    let line = 19.0;
    let tw = entries
        .iter()
        .map(|(n, _)| f.measure(n, size)[0])
        .fold(0.0, f32::max);
    let (bw, bh) = (tw + 36.0, entries.len() as f32 * line + 10.0);
    let (x, y) = (w - bw - 8.0, 8.0);
    f.rect2d([x, y], [x + bw, y + bh], Some(INK.alpha(0.78)), None);
    for (i, (name, col)) in entries.iter().enumerate() {
        let ly = y + 5.0 + i as f32 * line;
        f.rect2d([x + 7.0, ly + 3.0], [x + 25.0, ly + 13.0], Some(col.alpha(1.0)), Some(WHITE.alpha(0.5)));
        f.text(
            [x + 30.0, ly],
            *name,
            TextStyle::default().size(size).background(None),
        );
    }
}

/// A vertical color bar for a ramp over `lo..hi`, left of the right edge.
pub fn colorbar(f: &mut Frame, ramp: &Ramp, lo: f64, hi: f64, label: &str) {
    let [w, h] = f.size();
    let (bh, bw) = (h * 0.32, 14.0);
    let (x, y) = (w - bw - 12.0, h * 0.34);
    f.rect2d([x - 62.0, y - 28.0], [w - 4.0, y + bh + 12.0], Some(INK.alpha(0.72)), None);
    let n = bh as i32;
    for i in 0..n {
        let t = 1.0 - i as f64 / (n - 1).max(1) as f64;
        f.rect2d([x, y + i as f32], [x + bw, y + i as f32 + 1.0], Some(ramp.at(t).alpha(1.0)), None);
    }
    let tag = TextStyle::default().size(12.0).background(None).align(Align::TopRight);
    f.text([x - 4.0, y - 2.0], format!("{hi:.2}"), tag);
    f.text([x - 4.0, y + bh - 14.0], format!("{lo:.2}"), tag);
    f.text([w - 8.0, y - 24.0], label, tag);
}

/// A marker every `every`th point, and a label from `name` if given.
pub fn every(f: &mut Frame, pts: &[V3], every: usize, marker: Marker) {
    for p in pts.iter().step_by(every.max(1)) {
        f.point(*p, marker);
    }
}

/// A perspective camera fitted to `pts` from the direction that shows the most of them,
/// by raycasting from candidate eyes. The share of points it can see comes back too.
pub fn clearest_view(
    scene: &Scene,
    pts: &[V3],
    style: &WorldStyle,
    w: u32,
    h: u32,
    fov: f64,
) -> (Camera, f64) {
    let step = (pts.len() / 64).max(1);
    let mut best: Option<(f64, Camera, f64)> = None;
    for pitch in [25.0f64, 40.0, 55.0, 70.0] {
        for k in 0..16 {
            let yaw = k as f64 * 22.5;
            let cam = Camera::fit(pts).from(yaw, pitch).fov(fov);
            let view = scene.frame(&cam, w, h).view().clone();
            let (mut seen, mut n) = (0, 0);
            for p in pts.iter().step_by(step) {
                n += 1;
                let to = sub(*p, view.eye);
                let dist = dot(to, to).sqrt();
                let dir = scale(to, 1.0 / dist.max(1e-9));
                let hit = scene.raycast(style, view.eye, dir, (dist - 0.75).max(0.0));
                seen += hit.is_none() as usize;
            }
            let share = seen as f64 / n.max(1) as f64;
            // how much of the image the points fill
            let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
            for p in pts.iter().step_by(step) {
                if let Some(q) = view.project(*p) {
                    (x0, x1) = (x0.min(q[0]), x1.max(q[0]));
                    (y0, y1) = (y0.min(q[1]), y1.max(q[1]));
                }
            }
            let fill = ((x1 - x0) * (y1 - y0) / (w as f64 * h as f64)).clamp(0.0, 1.0);
            // lower pitches read better, so a steep view has to earn it
            let score = share + fill * 0.15 - (pitch - 25.0) * 0.001;
            if best.as_ref().is_none_or(|b| score > b.0) {
                best = Some((score, cam, share));
            }
        }
    }
    let (_, cam, share) = best.expect("candidates");
    (cam, share)
}
