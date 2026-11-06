//! What the world part of a frame shows. Deliberately small: annotations are where the
//! control lives.

use crate::camera::View;
use world::{BlockInfo, BlockPos};

use crate::color::Color;

/// Which shapes the world is drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Geometry {
    /// Block models and textures.
    Visual,
    /// The collision boxes the physics sees, in flat block colors, with their edges
    /// drawn. why? a fence is 1.5 high, plants have none, and bamboo's box moves with
    /// its position, none of which the visual model shows.
    Collision,
}

type Keep<'a> = Box<dyn Fn(BlockPos, &BlockInfo) -> bool + Sync + 'a>;
type Tint<'a> = Box<dyn Fn(BlockPos, &BlockInfo) -> Option<Color> + Sync + 'a>;

pub struct WorldStyle<'a> {
    pub geometry: Geometry,
    /// Half-spaces `n . p <= d` that are kept.
    pub(crate) clips: Vec<([f64; 3], f64)>,
    pub(crate) keep: Option<Keep<'a>>,
    pub(crate) tint: Option<Tint<'a>>,
    /// None hides barriers, in collision views too. why by default? many maps sit in a
    /// hull of them, which would tint every view and roof every collision view. Hidden
    /// ones still collide: open space in a picture doesn't mean a run can fly there.
    pub barriers: Option<Color>,
    /// Of the barriers `barriers` would draw, the ones to draw.
    pub(crate) barrier_keep: Option<Keep<'a>>,
    pub unknown: Option<Color>,
    pub background: Color,
    /// Distance at which fog is about 63% opaque.
    pub fog: Option<f64>,
    pub shadows: bool,
    /// Mixes the world toward gray, so annotations stand out.
    pub desaturate: f32,
    /// Skip the world altogether (annotations on the background).
    pub hidden: bool,
}

impl Default for WorldStyle<'_> {
    fn default() -> Self {
        WorldStyle {
            geometry: Geometry::Visual,
            clips: Vec::new(),
            keep: None,
            tint: None,
            barriers: None,
            barrier_keep: None,
            unknown: None,
            background: Color::hex(0xB9CAD8),
            fog: None,
            shadows: true,
            desaturate: 0.3,
            hidden: false,
        }
    }
}

impl<'a> WorldStyle<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn collision(mut self) -> Self {
        self.geometry = Geometry::Collision;
        self
    }

    /// Keep the half-space `n . p <= d`. Solid blocks the plane cuts show a hatched face.
    pub fn clip(mut self, n: [f64; 3], d: f64) -> Self {
        self.clips.push((n, d));
        self
    }

    pub fn below_y(self, y: f64) -> Self {
        self.clip([0.0, 1.0, 0.0], y)
    }

    pub fn above_y(self, y: f64) -> Self {
        self.clip([0.0, -1.0, 0.0], -y)
    }

    /// Keep only the box.
    pub fn inside(self, min: [f64; 3], max: [f64; 3]) -> Self {
        self.clip([1.0, 0.0, 0.0], max[0])
            .clip([0.0, 1.0, 0.0], max[1])
            .clip([0.0, 0.0, 1.0], max[2])
            .clip([-1.0, 0.0, 0.0], -min[0])
            .clip([0.0, -1.0, 0.0], -min[1])
            .clip([0.0, 0.0, -1.0], -min[2])
    }

    /// A slice: keep `lo <= p[axis] <= hi` (axis 0 x, 1 y, 2 z).
    pub fn slice(self, axis: usize, lo: f64, hi: f64) -> Self {
        let mut n = [0.0; 3];
        n[axis] = 1.0;
        let m = n.map(|v| -v);
        self.clip(n, hi).clip(m, -lo)
    }

    /// Draw a cell only if this returns true, e.g. with a [`crate::TubeCut`] so a
    /// ceiling doesn't hide a path.
    pub fn keep(mut self, f: impl Fn(BlockPos, &BlockInfo) -> bool + Sync + 'a) -> Self {
        self.keep = Some(Box::new(f));
        self
    }

    /// Color a cell: the returned color is mixed over the block by its alpha.
    pub fn tint(mut self, f: impl Fn(BlockPos, &BlockInfo) -> Option<Color> + Sync + 'a) -> Self {
        self.tint = Some(Box::new(f));
        self
    }

    /// Barrier color; None hides them.
    pub fn barriers(mut self, c: Option<Color>) -> Self {
        self.barriers = c;
        self
    }

    /// Draw barriers: tinted faces, or their solid boxes in a collision view.
    pub fn show_barriers(self) -> Self {
        self.barriers(Some(Color::rgba(0.95, 0.12, 0.18, 0.28)))
    }

    /// Draw barriers opaque, so they hide what's behind them and block
    /// [`crate::Scene::raycast`] like any block.
    pub fn solid_barriers(self) -> Self {
        self.barriers(Some(Color::rgba(0.95, 0.12, 0.18, 1.0)))
    }

    /// Of the barriers this style draws, only those this returns true for, e.g. with a
    /// [`NearMask`].
    pub fn barriers_where(mut self, f: impl Fn(BlockPos, &BlockInfo) -> bool + Sync + 'a) -> Self {
        self.barrier_keep = Some(Box::new(f));
        self
    }

    /// Fill unknown space (what `verify` rejects) with this translucent color, per cell.
    pub fn unknown(mut self, c: Color) -> Self {
        self.unknown = Some(c);
        self
    }

    pub fn background(mut self, c: Color) -> Self {
        self.background = c;
        self
    }

    pub fn fog(mut self, distance: f64) -> Self {
        self.fog = Some(distance);
        self
    }

    pub fn shadows(mut self, on: bool) -> Self {
        self.shadows = on;
        self
    }

    pub fn desaturate(mut self, amount: f32) -> Self {
        self.desaturate = amount;
        self
    }

    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }
}

/// The cells within `radius` of any point, measured to the nearest part of the cell.
pub struct NearMask {
    min: [i32; 3],
    dim: [i32; 3],
    bits: Vec<u64>,
}

impl NearMask {
    pub fn new(points: &[[f64; 3]], radius: f64) -> NearMask {
        let (mut lo, mut hi) = ([i32::MAX; 3], [i32::MIN; 3]);
        for p in points {
            for k in 0..3 {
                lo[k] = lo[k].min((p[k] - radius).floor() as i32);
                hi[k] = hi[k].max((p[k] + radius).floor() as i32);
            }
        }
        if points.is_empty() {
            return NearMask {
                min: [0; 3],
                dim: [0; 3],
                bits: Vec::new(),
            };
        }
        let dim = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];
        let n = dim[0] as usize * dim[1] as usize * dim[2] as usize;
        let mut bits = vec![0u64; n.div_ceil(64)];
        let gap = |c: i32, v: f64| (c as f64 - v).max(v - (c as f64 + 1.0)).max(0.0);
        for p in points {
            let (x0, x1) = ((p[0] - radius).floor() as i32, (p[0] + radius).floor() as i32);
            let (y0, y1) = ((p[1] - radius).floor() as i32, (p[1] + radius).floor() as i32);
            let (z0, z1) = ((p[2] - radius).floor() as i32, (p[2] + radius).floor() as i32);
            for z in z0..=z1 {
                let dz = gap(z, p[2]);
                for y in y0..=y1 {
                    let dy = gap(y, p[1]);
                    for x in x0..=x1 {
                        let dx = gap(x, p[0]);
                        if dx * dx + dy * dy + dz * dz <= radius * radius {
                            let i = (z - lo[2]) as usize * dim[1] as usize + (y - lo[1]) as usize;
                            let i = i * dim[0] as usize + (x - lo[0]) as usize;
                            bits[i / 64] |= 1 << (i % 64);
                        }
                    }
                }
            }
        }
        NearMask {
            min: lo,
            dim,
            bits,
        }
    }

    pub fn contains(&self, p: BlockPos) -> bool {
        let q = [p.x - self.min[0], p.y - self.min[1], p.z - self.min[2]];
        if (0..3).any(|k| q[k] < 0 || q[k] >= self.dim[k]) {
            return false;
        }
        let i = q[2] as usize * self.dim[1] as usize + q[1] as usize;
        let i = i * self.dim[0] as usize + q[0] as usize;
        self.bits[i / 64] >> (i % 64) & 1 == 1
    }
}

/// Bins of a [`TubeCut`], pixels.
const TUBE_BIN: f64 = 2.0;

/// How much shallower a [`TubeCut`] gets per block away from the path, on the image.
/// why not flat? a floor block on the camera's side of a path is nearer than the path and
/// lands within its radius, so a flat cut bites holes in floors that hide nothing. A slope
/// of 1 spares any floor seen from 45 degrees or steeper.
const TUBE_SLOPE: f64 = 1.0;

/// Hides what stands between a camera and a tube around points. A cell is cut if it lands on
/// the image within `radius` of a point (measured at that point's depth) and is nearer than
/// that point by more than `headroom` plus [`TUBE_SLOPE`] per block away from it, so the cut
/// is a funnel opening toward the camera. Works from any rotation, since "nearer" is along
/// the camera's own line of sight; for [`WorldStyle::keep`]. The deepest cut at a spot wins.
/// why? a path that spirals passes behind itself, and a cut to its nearest pass would bury
/// the others. Cells outside the image are kept, so they still cast shadows.
///
/// A cell counts by all of its footprint on the image, not its center. why? a block next
/// to the eye covers much of the image, and a camera buried in terrain would otherwise see
/// nothing. The footprint is rounded out to bins, so the cut can run a block or two wide.
pub struct TubeCut {
    view: View,
    /// The depth cut down to per bin, then the same at half the resolution, and so on.
    levels: Vec<TubeLevel>,
    headroom: f64,
}

struct TubeLevel {
    bin: f64,
    size: [i64; 2],
    far: Vec<f64>,
}

impl TubeLevel {
    fn at(&self, i: i64, j: i64) -> f64 {
        self.far[(j * self.size[0] + i) as usize]
    }
}

impl TubeCut {
    pub fn new(view: &View, points: &[[f64; 3]], radius: f64, headroom: f64) -> TubeCut {
        let size = [
            (view.w as f64 / TUBE_BIN).ceil() as i64,
            (view.h as f64 / TUBE_BIN).ceil() as i64,
        ];
        let mut far = vec![f64::NEG_INFINITY; (size[0] * size[1]) as usize];
        for p in points {
            let Some([x, y, z]) = view.project(*p) else { continue };
            let r = radius * view.px_per_block(z);
            let bx = (((x - r) / TUBE_BIN).floor() as i64).max(0)..=(((x + r) / TUBE_BIN) as i64).min(size[0] - 1);
            let by = (((y - r) / TUBE_BIN).floor() as i64).max(0)..=(((y + r) / TUBE_BIN) as i64).min(size[1] - 1);
            for j in by {
                for i in bx.clone() {
                    let (dx, dy) = ((i as f64 + 0.5) * TUBE_BIN - x, (j as f64 + 0.5) * TUBE_BIN - y);
                    if dx * dx + dy * dy <= r * r {
                        let e = &mut far[(j * size[0] + i) as usize];
                        let off = (dx * dx + dy * dy).sqrt() / view.px_per_block(z);
                        *e = e.max(z - TUBE_SLOPE * off);
                    }
                }
            }
        }
        let mut levels = vec![TubeLevel { bin: TUBE_BIN, size, far }];
        while let Some(l) = levels.last().filter(|l| l.size[0] > 1 || l.size[1] > 1) {
            let size = [(l.size[0] + 1) / 2, (l.size[1] + 1) / 2];
            let mut far = vec![f64::NEG_INFINITY; (size[0] * size[1]) as usize];
            for j in 0..l.size[1] {
                for i in 0..l.size[0] {
                    let e = &mut far[((j / 2) * size[0] + i / 2) as usize];
                    *e = e.max(l.at(i, j));
                }
            }
            levels.push(TubeLevel { bin: l.bin * 2.0, size, far });
        }
        TubeCut {
            view: view.clone(),
            levels,
            headroom,
        }
    }

    /// Whether a cell survives the cut.
    pub fn keeps(&self, p: BlockPos) -> bool {
        let center = [p.x as f64 + 0.5, p.y as f64 + 0.5, p.z as f64 + 0.5];
        // half a cell's diagonal: how far the cell reaches from its center
        const REACH: f64 = 0.87;
        let Some([x, y, z]) = self.view.project(center) else {
            // at or behind the eye: only a cell around the eye itself is in the way
            let top = self.levels.last().expect("levels").far[0];
            let v = [center[0] - self.view.eye[0], center[1] - self.view.eye[1], center[2] - self.view.eye[2]];
            return top == f64::NEG_INFINITY || v[0] * v[0] + v[1] * v[1] + v[2] * v[2] > REACH * REACH;
        };
        // a cell closer than its own reach could hold the eye, so it counts as that close
        let reach = REACH * self.view.px_per_block(z - REACH);
        // the first level whose bins are as big as the footprint: at most 3 x 3 to look at
        let l = self
            .levels
            .iter()
            .find(|l| l.bin >= reach)
            .unwrap_or_else(|| self.levels.last().expect("levels"));
        let clamp = |v: f64, size: i64| ((v / l.bin).floor() as i64).clamp(0, size - 1);
        let on_image = x + reach >= 0.0 && y + reach >= 0.0 && x - reach <= self.view.w as f64 && y - reach <= self.view.h as f64;
        if !on_image {
            return true;
        }
        for j in clamp(y - reach, l.size[1])..=clamp(y + reach, l.size[1]) {
            for i in clamp(x - reach, l.size[0])..=clamp(x + reach, l.size[0]) {
                if z < l.at(i, j) - self.headroom {
                    return false;
                }
            }
        }
        true
    }
}
