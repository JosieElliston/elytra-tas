//! The immediate-mode part. A [`Frame`] records what's asked for; [`Frame::finish`]
//! traces the world and draws it all. Draw order is the world, world-space primitives,
//! labels, then screen-space shapes, in the order submitted within each.

use std::sync::OnceLock;

use rayon::prelude::*;

use crate::camera::{Camera, NEAR, V3, View, cross, norm, scale, sub, add};
use crate::color::{Color, INK, WHITE};
use crate::image::Image;
use crate::scene::{Scene, Tracer};
use crate::style::WorldStyle;

/// A line's look. `occluded` is its opacity behind blocks, as a fraction of its own:
/// 0 hides it, 1 is x-ray, and the default keeps a path visible inside a canyon.
#[derive(Debug, Clone, Copy)]
pub struct Stroke {
    pub color: Color,
    /// Pixels.
    pub width: f32,
    pub occluded: f32,
    /// A darker casing this many pixels wider, so the line reads on any texture.
    pub halo: f32,
}

impl Stroke {
    pub fn new(color: Color, width: f32) -> Stroke {
        Stroke {
            color,
            width,
            occluded: 0.35,
            halo: 1.5,
        }
    }
    pub fn occluded(mut self, a: f32) -> Stroke {
        self.occluded = a;
        self
    }
    pub fn halo(mut self, px: f32) -> Stroke {
        self.halo = px;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Circle,
    Square,
    Diamond,
    Cross,
    /// Upward triangle.
    Triangle,
}

/// A point marker, with a size in pixels and a ring in `ring` to lift it off the world.
#[derive(Debug, Clone, Copy)]
pub struct Marker {
    pub shape: Shape,
    pub color: Color,
    pub size: f32,
    pub ring: Color,
    pub occluded: f32,
}

impl Marker {
    pub fn new(shape: Shape, color: Color, size: f32) -> Marker {
        Marker {
            shape,
            color,
            size,
            ring: INK,
            occluded: 0.35,
        }
    }
    pub fn circle(color: Color, size: f32) -> Marker {
        Marker::new(Shape::Circle, color, size)
    }
    pub fn ring(mut self, c: Color) -> Marker {
        self.ring = c;
        self
    }
    pub fn occluded(mut self, a: f32) -> Marker {
        self.occluded = a;
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Align {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

/// Screen-space text. `size` is in pixels.
#[derive(Debug, Clone, Copy)]
pub struct TextStyle {
    pub size: f32,
    pub color: Color,
    pub background: Option<Color>,
    pub align: Align,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            size: 15.0,
            color: WHITE,
            background: Some(INK.alpha(0.78)),
            align: Align::TopLeft,
        }
    }
}

impl TextStyle {
    pub fn size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = c;
        self
    }
    pub fn background(mut self, c: Option<Color>) -> Self {
        self.background = c;
        self
    }
    pub fn align(mut self, a: Align) -> Self {
        self.align = a;
        self
    }
}

/// A world-space text box: a box near an anchor with a leader line to it, placed to
/// avoid other labels and annotations unless `offset` pins it.
#[derive(Debug, Clone, Copy)]
pub struct LabelStyle {
    pub size: f32,
    /// If set, text is this many blocks tall instead of `size` pixels, clamped to stay
    /// readable.
    pub world_size: Option<f64>,
    pub color: Color,
    pub background: Color,
    /// Border and leader color.
    pub accent: Color,
    /// Fixed offset of the box from the anchor, in pixels.
    pub offset: Option<[f32; 2]>,
    /// Draw at this fraction of opacity when the anchor is behind blocks.
    pub occluded: f32,
}

impl Default for LabelStyle {
    fn default() -> Self {
        LabelStyle {
            size: 13.0,
            world_size: None,
            color: WHITE,
            background: INK.alpha(0.82),
            accent: WHITE,
            offset: None,
            occluded: 0.6,
        }
    }
}

impl LabelStyle {
    pub fn size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
    pub fn world_size(mut self, blocks: f64) -> Self {
        self.world_size = Some(blocks);
        self
    }
    pub fn accent(mut self, c: Color) -> Self {
        self.accent = c;
        self
    }
    pub fn offset(mut self, dx: f32, dy: f32) -> Self {
        self.offset = Some([dx, dy]);
        self
    }
}

enum WorldOp {
    Line {
        pts: Vec<V3>,
        colors: Option<Vec<Color>>,
        stroke: Stroke,
    },
    Marker {
        p: V3,
        m: Marker,
    },
    Tri {
        v: [V3; 3],
        color: Color,
        occluded: f32,
    },
}

struct LabelOp {
    p: V3,
    text: String,
    style: LabelStyle,
}

enum ScreenOp {
    Line {
        a: [f32; 2],
        b: [f32; 2],
        color: Color,
        width: f32,
    },
    Rect {
        min: [f32; 2],
        max: [f32; 2],
        fill: Option<Color>,
        border: Option<Color>,
    },
    Disc {
        c: [f32; 2],
        r: f32,
        color: Color,
    },
    Text {
        at: [f32; 2],
        text: String,
        style: TextStyle,
    },
}

/// One image being described.
pub struct Frame<'a> {
    scene: &'a Scene<'a>,
    view: View,
    style: WorldStyle<'a>,
    samples: u32,
    world_ops: Vec<WorldOp>,
    labels: Vec<LabelOp>,
    screen: Vec<ScreenOp>,
}

impl<'w> Scene<'w> {
    /// A new frame. `w` and `h` are in pixels.
    pub fn frame<'a>(&'a self, camera: &Camera, w: u32, h: u32) -> Frame<'a>
    where
        'w: 'a,
    {
        // Scene is covariant in its world borrow
        let scene: &'a Scene<'a> = self;
        Frame {
            scene,
            view: camera.resolve(w, h, self.bounds),
            style: WorldStyle::new(),
            samples: 2,
            world_ops: Vec::new(),
            labels: Vec::new(),
            screen: Vec::new(),
        }
    }
}

impl<'a> Frame<'a> {
    pub fn style(&mut self, style: WorldStyle<'a>) {
        self.style = style;
    }

    /// Antialiasing: this many samples per pixel per axis (default 2). Time goes with its
    /// square.
    pub fn samples(&mut self, n: u32) {
        self.samples = n.max(1);
    }

    pub fn view(&self) -> &View {
        &self.view
    }

    pub fn size(&self) -> [f32; 2] {
        [self.view.w as f32, self.view.h as f32]
    }

    /// Pixel x, pixel y and depth of a world point; None behind the camera.
    pub fn project(&self, p: V3) -> Option<[f64; 3]> {
        self.view.project(p)
    }

    // ---- world space --------------------------------------------------------------

    pub fn line(&mut self, a: V3, b: V3, stroke: Stroke) {
        self.polyline(&[a, b], stroke);
    }

    pub fn polyline(&mut self, pts: &[V3], stroke: Stroke) {
        self.world_ops.push(WorldOp::Line {
            pts: pts.to_vec(),
            colors: None,
            stroke,
        });
    }

    /// A polyline with a color per vertex, interpolated along it.
    pub fn polyline_colored(&mut self, pts: &[V3], colors: &[Color], stroke: Stroke) {
        assert_eq!(pts.len(), colors.len());
        self.world_ops.push(WorldOp::Line {
            pts: pts.to_vec(),
            colors: Some(colors.to_vec()),
            stroke,
        });
    }

    pub fn point(&mut self, p: V3, marker: Marker) {
        self.world_ops.push(WorldOp::Marker { p, m: marker });
    }

    pub fn fill_tri(&mut self, a: V3, b: V3, c: V3, color: Color) {
        self.world_ops.push(WorldOp::Tri {
            v: [a, b, c],
            color,
            occluded: 0.35,
        });
    }

    /// A filled quad, corners in order.
    pub fn fill_quad(&mut self, a: V3, b: V3, c: V3, d: V3, color: Color) {
        self.fill_tri(a, b, c, color);
        self.fill_tri(a, c, d, color);
    }

    /// A box's edges, and optionally its faces.
    pub fn aabb(&mut self, min: V3, max: V3, stroke: Stroke, fill: Option<Color>) {
        let c = |x: bool, y: bool, z: bool| -> V3 {
            [
                if x { max[0] } else { min[0] },
                if y { max[1] } else { min[1] },
                if z { max[2] } else { min[2] },
            ]
        };
        if let Some(col) = fill {
            for k in 0..2 {
                let s = k == 1;
                self.fill_quad(c(s, false, false), c(s, true, false), c(s, true, true), c(s, false, true), col);
                self.fill_quad(c(false, s, false), c(true, s, false), c(true, s, true), c(false, s, true), col);
                self.fill_quad(c(false, false, s), c(true, false, s), c(true, true, s), c(false, true, s), col);
            }
        }
        self.polyline(
            &[
                c(false, false, false),
                c(true, false, false),
                c(true, false, true),
                c(false, false, true),
                c(false, false, false),
                c(false, true, false),
                c(true, true, false),
                c(true, true, true),
                c(false, true, true),
                c(false, true, false),
            ],
            stroke,
        );
        for (x, z) in [(true, false), (true, true), (false, true)] {
            self.line(c(x, false, z), c(x, true, z), stroke);
        }
    }

    /// A line with an arrowhead at `b`, its head a constant size on screen.
    pub fn arrow(&mut self, a: V3, b: V3, stroke: Stroke) {
        self.line(a, b, stroke);
        let Some(pb) = self.view.project(b) else {
            return;
        };
        let head = (stroke.width as f64 * 4.0 + 5.0) / self.view.px_per_block(pb[2]);
        let dir = norm(sub(b, a));
        let to_eye = match self.view.ortho {
            Some(_) => scale(self.view.fwd, -1.0),
            None => norm(sub(self.view.eye, b)),
        };
        let side = norm(cross(dir, to_eye));
        let back = sub(b, scale(dir, head));
        let w = head * 0.5;
        self.polyline(&[add(back, scale(side, w)), b, add(back, scale(side, -w))], stroke);
    }

    /// A world-space text box with a leader line to `p`.
    pub fn label(&mut self, p: V3, text: impl Into<String>, style: LabelStyle) {
        self.labels.push(LabelOp {
            p,
            text: text.into(),
            style,
        });
    }

    // ---- screen space -------------------------------------------------------------

    pub fn text(&mut self, at: [f32; 2], text: impl Into<String>, style: TextStyle) {
        self.screen.push(ScreenOp::Text {
            at,
            text: text.into(),
            style,
        });
    }

    pub fn line2d(&mut self, a: [f32; 2], b: [f32; 2], color: Color, width: f32) {
        self.screen.push(ScreenOp::Line { a, b, color, width });
    }

    pub fn rect2d(&mut self, min: [f32; 2], max: [f32; 2], fill: Option<Color>, border: Option<Color>) {
        self.screen.push(ScreenOp::Rect { min, max, fill, border });
    }

    pub fn disc2d(&mut self, c: [f32; 2], r: f32, color: Color) {
        self.screen.push(ScreenOp::Disc { c, r, color });
    }

    /// The width and height of a text in pixels.
    pub fn measure(&self, text: &str, size: f32) -> [f32; 2] {
        measure(text, size)
    }

    // ---- drawing ------------------------------------------------------------------

    /// Trace the world, draw it all, and return the image.
    pub fn finish(self) -> Image {
        let s = self.samples as usize;
        let (w, h) = (self.view.w as usize, self.view.h as usize);
        let (sw, sh) = (w * s, h * s);
        let mut canvas = Canvas::new(sw, sh, self.style.background);
        // the world
        if !self.style.hidden {
            let v = &self.view;
            let (foot_z, foot_0) = match v.ortho {
                Some(k) => (0.0, 1.0 / (k as f32 * s as f32)),
                None => (1.0 / (v.focal as f32 * s as f32), 0.0),
            };
            let tr = Tracer::new(self.scene, &self.style, foot_z, foot_0);
            let fs = s as f64;
            canvas
                .color
                .par_chunks_mut(sw)
                .zip(canvas.depth.par_chunks_mut(sw))
                .enumerate()
                .for_each(|(y, (crow, drow))| {
                    for x in 0..sw {
                        let (o, d) = v.ray((x as f64 + 0.5) / fs, (y as f64 + 0.5) / fs);
                        let (c, t) = tr.trace(o, d);
                        crow[x] = c;
                        drow[x] = if t.is_finite() {
                            // along the view axis, as projected annotations measure it
                            (t as f64 * crate::camera::dot(d, v.fwd)) as f32
                        } else {
                            f32::INFINITY
                        };
                    }
                });
        }
        // world-space primitives
        let fs = s as f32;
        for op in &self.world_ops {
            match op {
                WorldOp::Line { pts, colors, stroke } => {
                    if stroke.halo > 0.0 {
                        let halo = Stroke {
                            color: INK.alpha(stroke.color.a * 0.7),
                            width: stroke.width + stroke.halo * 2.0,
                            ..*stroke
                        };
                        canvas.polyline(&self.view, pts, None, &halo, fs);
                    }
                    canvas.polyline(&self.view, pts, colors.as_deref(), stroke, fs);
                }
                WorldOp::Marker { p, m } => canvas.marker(&self.view, *p, m, fs),
                WorldOp::Tri { v, color, occluded } => {
                    canvas.triangle(&self.view, *v, *color, *occluded, fs)
                }
            }
        }
        let occupied = canvas.occupied.clone();
        let mut img = canvas.downsample(w, h, s);
        // labels, then screen space
        draw_labels(&mut img, &self.view, &self.labels, &canvas.depth, s, &occupied, canvas.occ_w);
        for op in &self.screen {
            match op {
                ScreenOp::Line { a, b, color, width } => line2d(&mut img, *a, *b, *color, *width),
                ScreenOp::Rect { min, max, fill, border } => {
                    if let Some(c) = fill {
                        rect(&mut img, *min, *max, *c);
                    }
                    if let Some(c) = border {
                        rect_border(&mut img, *min, *max, *c);
                    }
                }
                ScreenOp::Disc { c, r, color } => disc(&mut img, *c, *r, *color),
                ScreenOp::Text { at, text, style } => draw_text_box(&mut img, *at, text, style),
            }
        }
        img
    }
}

// ---- the supersampled canvas --------------------------------------------------------

const OCC_CELL: usize = 8;

struct Canvas {
    w: usize,
    h: usize,
    color: Vec<[f32; 3]>,
    depth: Vec<f32>,
    /// Per-polyline state: which polyline last touched a sample, and its opacity there,
    /// so a joint isn't blended twice.
    stamp: Vec<u32>,
    cover: Vec<f32>,
    id: u32,
    /// Coarse map of where annotations went, for label placement.
    occupied: Vec<bool>,
    occ_w: usize,
    occ_s: usize,
}

impl Canvas {
    fn new(w: usize, h: usize, bg: Color) -> Canvas {
        let n = w * h;
        let occ_w = w / OCC_CELL + 1;
        Canvas {
            w,
            h,
            color: vec![[bg.r, bg.g, bg.b]; n],
            depth: vec![f32::INFINITY; n],
            stamp: vec![0; n],
            cover: vec![0.0; n],
            id: 0,
            occupied: vec![false; occ_w * (h / OCC_CELL + 1)],
            occ_w,
            occ_s: OCC_CELL,
        }
    }

    #[inline]
    fn put(&mut self, i: usize, c: [f32; 3], a: f32) {
        // what this polyline already laid here counts toward `a`
        let prev = if self.stamp[i] == self.id {
            self.cover[i]
        } else {
            0.0
        };
        if a <= prev {
            return;
        }
        let d = if prev > 0.0 { (a - prev) / (1.0 - prev) } else { a };
        self.stamp[i] = self.id;
        self.cover[i] = a;
        let p = &mut self.color[i];
        p[0] += (c[0] - p[0]) * d;
        p[1] += (c[1] - p[1]) * d;
        p[2] += (c[2] - p[2]) * d;
        if a > 0.3 {
            let (x, y) = (i % self.w, i / self.w);
            self.occupied[y / self.occ_s * self.occ_w + x / self.occ_s] = true;
        }
    }

    fn polyline(&mut self, v: &View, pts: &[V3], colors: Option<&[Color]>, st: &Stroke, s: f32) {
        self.id += 1;
        let w = st.width * s;
        for i in 0..pts.len().saturating_sub(1) {
            let (c0, c1) = match colors {
                Some(c) => (c[i], c[i + 1]),
                None => (st.color, st.color),
            };
            self.segment(v, pts[i], pts[i + 1], c0, c1, w, st.occluded, s);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn segment(&mut self, v: &View, a: V3, b: V3, c0: Color, c1: Color, w: f32, occ: f32, s: f32) {
        // clip to the near plane
        let (za, zb) = (
            crate::camera::dot(sub(a, v.eye), v.fwd),
            crate::camera::dot(sub(b, v.eye), v.fwd),
        );
        if za < NEAR && zb < NEAR {
            return;
        }
        let at = |t: f64| add(a, scale(sub(b, a), t));
        let (mut pa, mut pb, mut ca, mut cb) = (a, b, c0, c1);
        if za < NEAR {
            let t = (NEAR - za) / (zb - za);
            pa = at(t);
            ca = c0.lerp(c1, t as f32);
        } else if zb < NEAR {
            let t = (NEAR - za) / (zb - za);
            pb = at(t);
            cb = c0.lerp(c1, t as f32);
        }
        let (Some(p0), Some(p1)) = (v.project(pa), v.project(pb)) else {
            return;
        };
        let fs = s as f64;
        let (x0, y0, x1, y1) = (
            (p0[0] * fs) as f32,
            (p0[1] * fs) as f32,
            (p1[0] * fs) as f32,
            (p1[1] * fs) as f32,
        );
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len2 = dx * dx + dy * dy;
        let r = w * 0.5 + 1.0;
        let persp = v.ortho.is_none();
        let (iz0, iz1) = if persp {
            (1.0 / p0[2] as f32, 1.0 / p1[2] as f32)
        } else {
            (p0[2] as f32, p1[2] as f32)
        };
        let (w_i, h_i) = (self.w as i32, self.h as i32);
        // walk the longer axis, so a long diagonal doesn't scan its bounding box
        let horizontal = dx.abs() >= dy.abs();
        let (lo, hi) = if horizontal {
            (x0.min(x1) - r, x0.max(x1) + r)
        } else {
            (y0.min(y1) - r, y0.max(y1) + r)
        };
        let limit = if horizontal { w_i } else { h_i };
        let (lo, hi) = (lo.floor().max(0.0) as i32, hi.ceil().min(limit as f32 - 1.0) as i32);
        let slope = if horizontal {
            if dx.abs() > 1e-6 { dy / dx } else { 0.0 }
        } else if dy.abs() > 1e-6 {
            dx / dy
        } else {
            0.0
        };
        let cross_r = r * (1.0 + slope * slope).sqrt();
        let lin0 = [ca.r, ca.g, ca.b, ca.a];
        let lin1 = [cb.r, cb.g, cb.b, cb.a];
        for m in lo..=hi {
            let mc = m as f32 + 0.5;
            let center = if horizontal {
                y0 + (mc - x0).clamp(x0.min(x1) - x0, x0.max(x1) - x0) * slope
            } else {
                x0 + (mc - y0).clamp(y0.min(y1) - y0, y0.max(y1) - y0) * slope
            };
            let limit2 = if horizontal { h_i } else { w_i };
            let n0 = (center - cross_r).floor().max(0.0) as i32;
            let n1 = (center + cross_r).ceil().min(limit2 as f32 - 1.0) as i32;
            for n in n0..=n1 {
                let (x, y) = if horizontal { (m, n) } else { (n, m) };
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let t = if len2 > 1e-6 {
                    (((px - x0) * dx + (py - y0) * dy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (ex, ey) = (px - (x0 + dx * t), py - (y0 + dy * t));
                let dist = (ex * ex + ey * ey).sqrt();
                let cov = (w * 0.5 + 0.5 - dist).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let z = if persp {
                    1.0 / (iz0 + (iz1 - iz0) * t)
                } else {
                    iz0 + (iz1 - iz0) * t
                };
                let i = y as usize * self.w + x as usize;
                let hidden = z > self.depth[i] + depth_bias(z);
                let col = [
                    lin0[0] + (lin1[0] - lin0[0]) * t,
                    lin0[1] + (lin1[1] - lin0[1]) * t,
                    lin0[2] + (lin1[2] - lin0[2]) * t,
                ];
                let al = lin0[3] + (lin1[3] - lin0[3]) * t;
                let a = cov * al * if hidden { occ } else { 1.0 };
                if a > 0.0 {
                    self.put(i, col, a);
                }
            }
        }
    }

    fn marker(&mut self, v: &View, p: V3, m: &Marker, s: f32) {
        let Some(q) = v.project(p) else { return };
        self.id += 1;
        let (cx, cy) = (q[0] as f32 * s, q[1] as f32 * s);
        let z = q[2] as f32;
        let r = m.size * 0.5 * s;
        let ring = 1.5 * s;
        let reach = (r + ring + 2.0).ceil() as i32;
        let (ix, iy) = (cx as i32, cy as i32);
        // signed distance to the shape: negative inside
        let shape = m.shape;
        let sd = |dx: f32, dy: f32| -> f32 {
            match shape {
                Shape::Circle => (dx * dx + dy * dy).sqrt() - r,
                Shape::Square => (dx.abs().max(dy.abs())) - r * 0.88,
                Shape::Diamond => (dx.abs() + dy.abs()) * std::f32::consts::FRAC_1_SQRT_2 - r * 0.85,
                Shape::Cross => {
                    let (a, b) = ((dx + dy).abs(), (dx - dy).abs());
                    let (a, b) = (a * std::f32::consts::FRAC_1_SQRT_2, b * std::f32::consts::FRAC_1_SQRT_2);
                    let arm = r * 0.3;
                    let along = r * 1.05;
                    let d1 = (a - arm).max(b - along);
                    let d2 = (b - arm).max(a - along);
                    d1.min(d2)
                }
                Shape::Triangle => {
                    let y = dy + r * 0.2;
                    let k = 0.866f32;
                    ((dx.abs() * k + y * -0.5).max(y)) - r * 0.6
                }
            }
        };
        for y in (iy - reach).max(0)..=(iy + reach).min(self.h as i32 - 1) {
            for x in (ix - reach).max(0)..=(ix + reach).min(self.w as i32 - 1) {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let d = sd(dx, dy);
                let i = y as usize * self.w + x as usize;
                let hidden = z > self.depth[i] + depth_bias(z);
                let vis = if hidden { m.occluded } else { 1.0 };
                let ring_cov = (ring + 0.5 - d).clamp(0.0, 1.0);
                if ring_cov > 0.0 && d > -0.5 {
                    self.put(i, [m.ring.r, m.ring.g, m.ring.b], ring_cov * m.ring.a * vis);
                }
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.id += 1;
                    self.put(i, [m.color.r, m.color.g, m.color.b], cov * m.color.a * vis);
                    self.id -= 1;
                }
            }
        }
    }

    fn triangle(&mut self, v: &View, t: [V3; 3], c: Color, occ: f32, s: f32) {
        // clip to the near plane
        let mut poly: Vec<V3> = Vec::with_capacity(4);
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            let (za, zb) = (
                crate::camera::dot(sub(a, v.eye), v.fwd),
                crate::camera::dot(sub(b, v.eye), v.fwd),
            );
            if za >= NEAR {
                poly.push(a);
            }
            if (za >= NEAR) != (zb >= NEAR) {
                poly.push(add(a, scale(sub(b, a), (NEAR - za) / (zb - za))));
            }
        }
        if poly.len() < 3 {
            return;
        }
        self.id += 1;
        let persp = v.ortho.is_none();
        for i in 1..poly.len() - 1 {
            let (Some(p0), Some(p1), Some(p2)) = (
                v.project(poly[0]),
                v.project(poly[i]),
                v.project(poly[i + 1]),
            ) else {
                continue;
            };
            let q = |p: [f64; 3]| [p[0] as f32 * s, p[1] as f32 * s];
            let (a, b, d) = (q(p0), q(p1), q(p2));
            let area = (b[0] - a[0]) * (d[1] - a[1]) - (b[1] - a[1]) * (d[0] - a[0]);
            if area.abs() < 1e-3 {
                continue;
            }
            let (zs0, zs1, zs2) = if persp {
                (1.0 / p0[2] as f32, 1.0 / p1[2] as f32, 1.0 / p2[2] as f32)
            } else {
                (p0[2] as f32, p1[2] as f32, p2[2] as f32)
            };
            let x0 = a[0].min(b[0]).min(d[0]).floor().max(0.0) as i32;
            let x1 = a[0].max(b[0]).max(d[0]).ceil().min(self.w as f32 - 1.0) as i32;
            let y0 = a[1].min(b[1]).min(d[1]).floor().max(0.0) as i32;
            let y1 = a[1].max(b[1]).max(d[1]).ceil().min(self.h as f32 - 1.0) as i32;
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((b[0] - px) * (d[1] - py) - (b[1] - py) * (d[0] - px)) / area;
                    let w1 = ((d[0] - px) * (a[1] - py) - (d[1] - py) * (a[0] - px)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let zi = w0 * zs0 + w1 * zs1 + w2 * zs2;
                    let z = if persp { 1.0 / zi } else { zi };
                    let i = y as usize * self.w + x as usize;
                    let hidden = z > self.depth[i] + depth_bias(z);
                    let al = c.a * if hidden { occ } else { 1.0 };
                    if al > 0.0 {
                        self.put(i, [c.r, c.g, c.b], al);
                    }
                }
            }
        }
    }

    fn downsample(&self, w: usize, h: usize, s: usize) -> Image {
        let mut out = vec![0u8; w * h * 4];
        let k = 1.0 / (s * s) as f32;
        out.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let mut c = [0.0f32; 3];
                for sy in 0..s {
                    for sx in 0..s {
                        let p = self.color[(y * s + sy) * self.w + x * s + sx];
                        c[0] += p[0];
                        c[1] += p[1];
                        c[2] += p[2];
                    }
                }
                for i in 0..3 {
                    row[x * 4 + i] = (c[i] * k * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
                }
                row[x * 4 + 3] = 255;
            }
        });
        Image {
            w: w as u32,
            h: h as u32,
            rgba: out,
        }
    }
}

/// Keeps a line lying on a surface from flickering in and out of it.
#[inline]
fn depth_bias(z: f32) -> f32 {
    0.02 + z * 0.0015
}

// ---- screen-space drawing ------------------------------------------------------------

#[inline]
fn blend(img: &mut Image, x: i32, y: i32, c: Color, a: f32) {
    if x < 0 || y < 0 || x >= img.w as i32 || y >= img.h as i32 || a <= 0.0 {
        return;
    }
    let i = (y as usize * img.w as usize + x as usize) * 4;
    let a = a.min(1.0);
    for (k, v) in [c.r, c.g, c.b].into_iter().enumerate() {
        let p = img.rgba[i + k] as f32 / 255.0;
        img.rgba[i + k] = ((p + (v - p) * a) * 255.0 + 0.5) as u8;
    }
}

pub(crate) fn line2d(img: &mut Image, a: [f32; 2], b: [f32; 2], c: Color, w: f32) {
    let r = w * 0.5 + 1.0;
    let (x0, x1) = (
        (a[0].min(b[0]) - r).floor().max(0.0) as i32,
        (a[0].max(b[0]) + r).ceil().min(img.w as f32) as i32,
    );
    let (y0, y1) = (
        (a[1].min(b[1]) - r).floor().max(0.0) as i32,
        (a[1].max(b[1]) + r).ceil().min(img.h as f32) as i32,
    );
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let t = if l2 > 1e-6 {
                (((px - a[0]) * dx + (py - a[1]) * dy) / l2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (ex, ey) = (px - (a[0] + dx * t), py - (a[1] + dy * t));
            let cov = (w * 0.5 + 0.5 - (ex * ex + ey * ey).sqrt()).clamp(0.0, 1.0);
            blend(img, x, y, c, cov * c.a);
        }
    }
}

fn rect(img: &mut Image, min: [f32; 2], max: [f32; 2], c: Color) {
    for y in min[1].floor() as i32..max[1].ceil() as i32 {
        for x in min[0].floor() as i32..max[0].ceil() as i32 {
            blend(img, x, y, c, c.a);
        }
    }
}

fn rect_border(img: &mut Image, min: [f32; 2], max: [f32; 2], c: Color) {
    let (x0, y0, x1, y1) = (
        min[0].floor() as i32,
        min[1].floor() as i32,
        max[0].ceil() as i32 - 1,
        max[1].ceil() as i32 - 1,
    );
    for x in x0..=x1 {
        blend(img, x, y0, c, c.a);
        blend(img, x, y1, c, c.a);
    }
    for y in y0 + 1..y1 {
        blend(img, x0, y, c, c.a);
        blend(img, x1, y, c, c.a);
    }
}

fn disc(img: &mut Image, c: [f32; 2], r: f32, col: Color) {
    let k = (r + 2.0).ceil() as i32;
    let (ix, iy) = (c[0] as i32, c[1] as i32);
    for y in iy - k..=iy + k {
        for x in ix - k..=ix + k {
            let (dx, dy) = (x as f32 + 0.5 - c[0], y as f32 + 0.5 - c[1]);
            let cov = (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            blend(img, x, y, col, cov * col.a);
        }
    }
}

// ---- text ------------------------------------------------------------------------------

fn font() -> &'static fontdue::Font {
    static FONT: OnceLock<fontdue::Font> = OnceLock::new();
    FONT.get_or_init(|| {
        fontdue::Font::from_bytes(
            epaint_default_fonts::HACK_REGULAR,
            fontdue::FontSettings::default(),
        )
        .expect("embedded font")
    })
}

fn line_height(size: f32) -> f32 {
    (size * 1.25).ceil()
}

pub(crate) fn measure(text: &str, size: f32) -> [f32; 2] {
    let f = font();
    let mut w = 0.0f32;
    let mut n = 0;
    for l in text.split('\n') {
        n += 1;
        let lw: f32 = l.chars().map(|c| f.metrics(c, size).advance_width).sum();
        w = w.max(lw);
    }
    [w.ceil(), n as f32 * line_height(size)]
}

fn draw_text(img: &mut Image, at: [f32; 2], text: &str, size: f32, c: Color) {
    let f = font();
    let ascent = f
        .horizontal_line_metrics(size)
        .map_or(size * 0.8, |m| m.ascent);
    let mut y = at[1];
    for l in text.split('\n') {
        let mut x = at[0];
        let base = y + ascent;
        for ch in l.chars() {
            let (m, bm) = f.rasterize(ch, size);
            let gx = (x + m.xmin as f32).round() as i32;
            let gy = (base - m.height as f32 - m.ymin as f32).round() as i32;
            for j in 0..m.height {
                for i in 0..m.width {
                    let a = bm[j * m.width + i] as f32 / 255.0;
                    blend(img, gx + i as i32, gy + j as i32, c, a * c.a);
                }
            }
            x += m.advance_width;
        }
        y += line_height(size);
    }
}

fn draw_text_box(img: &mut Image, at: [f32; 2], text: &str, st: &TextStyle) {
    let [tw, th] = measure(text, st.size);
    let pad = if st.background.is_some() { 5.0 } else { 0.0 };
    let (bw, bh) = (tw + 2.0 * pad, th + 2.0 * pad);
    let (x, y) = match st.align {
        Align::TopLeft => (at[0], at[1]),
        Align::TopRight => (at[0] - bw, at[1]),
        Align::BottomLeft => (at[0], at[1] - bh),
        Align::BottomRight => (at[0] - bw, at[1] - bh),
        Align::Center => (at[0] - bw * 0.5, at[1] - bh * 0.5),
    };
    if let Some(b) = st.background {
        rect(img, [x, y], [x + bw, y + bh], b);
    }
    draw_text(img, [x + pad, y + pad], text, st.size, st.color);
}

// ---- labels ----------------------------------------------------------------------------

fn draw_labels(
    img: &mut Image,
    v: &View,
    labels: &[LabelOp],
    depth: &[f32],
    s: usize,
    occupied: &[bool],
    occ_w: usize,
) {
    let mut placed: Vec<[f32; 4]> = Vec::new();
    let (w, h) = (v.w as f32, v.h as f32);
    let anchors: Vec<Option<[f64; 3]>> = labels.iter().map(|l| v.project(l.p)).collect();
    let occ_scale = OCC_CELL as f32 / s as f32;
    for (li, l) in labels.iter().enumerate() {
        let Some(q) = anchors[li] else { continue };
        let (ax, ay) = (q[0] as f32, q[1] as f32);
        if ax < 0.0 || ay < 0.0 || ax >= w || ay >= h {
            continue;
        }
        let size = match l.style.world_size {
            Some(b) => ((b * v.px_per_block(q[2])) as f32).clamp(9.0, 42.0),
            None => l.style.size,
        };
        let [tw, th] = measure(&l.text, size);
        let (bw, bh) = (tw + 8.0, th + 4.0);
        let score = |x: f32, y: f32, dist: f32| -> f32 {
            let mut cost = dist * 0.05;
            if x < 2.0 || y < 2.0 || x + bw > w - 2.0 || y + bh > h - 2.0 {
                cost += 1e6;
            }
            for b in &placed {
                if x < b[2] && x + bw > b[0] && y < b[3] && y + bh > b[1] {
                    cost += 1e4;
                }
            }
            // other anchors
            for (j, a) in anchors.iter().enumerate() {
                if j != li && let Some(a) = a {
                    let (px, py) = (a[0] as f32, a[1] as f32);
                    if px > x - 3.0 && px < x + bw + 3.0 && py > y - 3.0 && py < y + bh + 3.0 {
                        cost += 300.0;
                    }
                }
            }
            // annotations already drawn
            let (c0, c1) = ((x / occ_scale) as i32, ((x + bw) / occ_scale) as i32);
            let (r0, r1) = ((y / occ_scale) as i32, ((y + bh) / occ_scale) as i32);
            for r in r0..=r1 {
                for c in c0..=c1 {
                    if c >= 0 && r >= 0 && (c as usize) < occ_w {
                        let i = r as usize * occ_w + c as usize;
                        if occupied.get(i).copied().unwrap_or(false) {
                            cost += 6.0;
                        }
                    }
                }
            }
            cost
        };
        let (bx, by) = match l.style.offset {
            Some(o) => (ax + o[0], ay + o[1]),
            None => {
                let mut best = (f32::INFINITY, ax + 14.0, ay - bh - 10.0);
                for radius in [14.0f32, 26.0, 44.0, 70.0, 110.0, 170.0] {
                    for k in 0..16 {
                        let th_ = k as f32 / 16.0 * std::f32::consts::TAU;
                        let (dx, dy) = (th_.cos() * radius, th_.sin() * radius);
                        // the box's near edge sits at this offset from the anchor
                        let x = if dx >= 0.0 { ax + dx } else { ax + dx - bw };
                        let y = if dy >= 0.0 { ay + dy } else { ay + dy - bh };
                        let c = score(x, y, radius);
                        if c < best.0 {
                            best = (c, x, y);
                        }
                    }
                }
                (best.1, best.2)
            }
        };
        placed.push([bx, by, bx + bw, by + bh]);
        // is the anchor behind blocks?
        let (sx, sy) = ((ax * s as f32) as usize, (ay * s as f32) as usize);
        let i = sy * (v.w as usize * s) + sx;
        let z = q[2] as f32;
        let hidden = depth.get(i).is_some_and(|d| z > *d + depth_bias(z));
        let vis = if hidden { l.style.occluded } else { 1.0 };
        let accent = l.style.accent.alpha(l.style.accent.a * vis);
        // leader to the box's nearest edge point
        let (ex, ey) = (ax.clamp(bx, bx + bw), ay.clamp(by, by + bh));
        line2d(img, [ax, ay], [ex, ey], INK.alpha(0.6 * vis), 3.0);
        line2d(img, [ax, ay], [ex, ey], accent, 1.3);
        disc(img, [ax, ay], 2.5, accent);
        let bg = l.style.background;
        rect(img, [bx, by], [bx + bw, by + bh], bg.alpha(bg.a * vis));
        rect_border(img, [bx, by], [bx + bw, by + bh], accent);
        let col = l.style.color.alpha(l.style.color.a * vis.max(0.8));
        draw_text(img, [bx + 4.0, by + 2.0], &l.text, size, col);
    }
}
