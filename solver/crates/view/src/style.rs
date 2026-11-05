//! What the world part of a frame shows. Deliberately small: annotations are where the
//! control lives.

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

    /// Draw a cell only if this returns true, e.g. with a [`crate::CellMask`] so a
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

/// A column-wise mask around points: cells within `radius` horizontally of a point, and
/// between `below` under and `above` over the lowest and highest such point in that
/// column. Cheap to test, for [`WorldStyle::keep`].
pub struct CellMask {
    min: [i32; 2],
    dim: [i32; 2],
    span: Vec<(i32, i32)>,
}

impl CellMask {
    pub fn corridor(points: &[[f64; 3]], radius: f64, below: f64, above: f64) -> CellMask {
        let r = radius.ceil() as i32 + 1;
        let (mut lo, mut hi) = ([i32::MAX; 2], [i32::MIN; 2]);
        for p in points {
            let (x, z) = (p[0].floor() as i32, p[2].floor() as i32);
            lo = [lo[0].min(x - r), lo[1].min(z - r)];
            hi = [hi[0].max(x + r), hi[1].max(z + r)];
        }
        if points.is_empty() {
            return CellMask {
                min: [0, 0],
                dim: [0, 0],
                span: Vec::new(),
            };
        }
        let dim = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1];
        let mut span = vec![(i32::MAX, i32::MIN); (dim[0] * dim[1]) as usize];
        let r2 = radius * radius;
        for p in points {
            let (ylo, yhi) = ((p[1] - below).floor() as i32, (p[1] + above).ceil() as i32);
            let (cx, cz) = (p[0].floor() as i32, p[2].floor() as i32);
            for z in cz - r..=cz + r {
                for x in cx - r..=cx + r {
                    let (dx, dz) = (x as f64 + 0.5 - p[0], z as f64 + 0.5 - p[2]);
                    if dx * dx + dz * dz > r2 {
                        continue;
                    }
                    let i = ((z - lo[1]) * dim[0] + (x - lo[0])) as usize;
                    span[i] = (span[i].0.min(ylo), span[i].1.max(yhi));
                }
            }
        }
        CellMask {
            min: lo,
            dim,
            span,
        }
    }

    /// A trench along points: within `radius` of a point horizontally, only cells up to
    /// `headroom` above the lowest point nearby pass [`CellMask::not_above`]. why the
    /// lowest? a path that spirals down passes under itself, and a cut to its highest
    /// pass would bury the lower ones.
    pub fn trench(points: &[[f64; 3]], radius: f64, headroom: f64) -> CellMask {
        let mut m = CellMask::corridor(points, radius, 0.0, headroom);
        let r = radius.ceil() as i32 + 1;
        m.span.iter_mut().for_each(|s| *s = (i32::MAX, i32::MIN));
        let r2 = radius * radius;
        for p in points {
            let top = (p[1] + headroom).ceil() as i32;
            let (cx, cz) = (p[0].floor() as i32, p[2].floor() as i32);
            for z in cz - r..=cz + r {
                for x in cx - r..=cx + r {
                    let (dx, dz) = (x as f64 + 0.5 - p[0], z as f64 + 0.5 - p[2]);
                    if dx * dx + dz * dz > r2 {
                        continue;
                    }
                    let i = ((z - m.min[1]) * m.dim[0] + (x - m.min[0])) as usize;
                    let s = &mut m.span[i];
                    *s = (i32::MIN, if s.1 == i32::MIN { top } else { s.1.min(top) });
                }
            }
        }
        m
    }

    /// Whether a cell isn't above this mask: true outside its columns. For cutting the
    /// ceiling over a path while keeping everything around it.
    pub fn not_above(&self, p: BlockPos) -> bool {
        let (x, z) = (p.x - self.min[0], p.z - self.min[1]);
        if x < 0 || z < 0 || x >= self.dim[0] || z >= self.dim[1] {
            return true;
        }
        let (a, b) = self.span[(z * self.dim[0] + x) as usize];
        a > b || p.y <= b
    }

    pub fn contains(&self, p: BlockPos) -> bool {
        let (x, z) = (p.x - self.min[0], p.z - self.min[1]);
        if x < 0 || z < 0 || x >= self.dim[0] || z >= self.dim[1] {
            return false;
        }
        let (a, b) = self.span[(z * self.dim[0] + x) as usize];
        a <= p.y && p.y <= b
    }
}
