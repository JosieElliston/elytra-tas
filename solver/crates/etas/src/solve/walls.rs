//! The obstacles a ghost-wall solve keeps clear of, from a course world, with a bucket
//! grid for the nearest-box query.
//!
//! An obstacle is anything that would end normal flight or clip the move under the
//! faithful kernel:
//! - each block's collision boxes (`World::boxes_at`, offsets applied), ground and
//!   ceilings included, so clearance also keeps the flight off the ground;
//! - a climbable cell (one that ends the glide), as the part of its cube the gliding
//!   box overlaps exactly when the position is in the cell. why that shape? the game
//!   checks the block at the position, not the box;
//! - fluid, stuck and bouncing cells, as their whole cube (conservative: the game reads
//!   the fluid height and the bounce only on landing);
//! - unknown cells, as their cube grown by 1 block. why grown? the collision cursor
//!   reads the face-adjacent shell around the move's box, and an unknown cell there
//!   stops the faithful step.
//!
//! Boxes are taken cell by cell rather than through `World::collision_boxes`, which
//! fails as a whole on the first unknown cell; for the cells inside the region the
//! result is a superset of its boxes.

use sim::generic::ghost::{HALF_WIDTH, HEIGHT, leg_sdf, segment_sdf};
use world::{Aabb, BlockPos, Cell, Effect, World};

/// Obstacle boxes in a region of the world, bucketed by block cell.
#[derive(Debug, Clone)]
pub struct Walls {
    pub boxes: Vec<Aabb>,
    /// The cells read, inclusive. A query point is answered exactly only if its
    /// search box stays inside; see [`Walls::covers`].
    pub lo: [i32; 3],
    pub hi: [i32; 3],
    /// Bucket grid over `lo - PAD ..= hi + PAD`: indices into `boxes` per cell.
    glo: [i32; 3],
    dims: [usize; 3],
    cells: Vec<Vec<u32>>,
    /// Clearance is capped here: a leg with nothing nearer reads `range`.
    pub range: f64,
}

/// How far an obstacle box can reach out of the cell it was read from (a fence's
/// 1.5-tall box, an unknown cell's growth).
const PAD: i32 = 2;

impl Walls {
    /// The obstacles in the cells around `points`: their bounding box grown by `grow`
    /// blocks. Clearance reads at most `range` (keep it under `grow`).
    pub fn around(world: &World, points: &[[f64; 3]], grow: f64, range: f64) -> Walls {
        let mut lo = [i32::MAX; 3];
        let mut hi = [i32::MIN; 3];
        for p in points {
            for i in 0..3 {
                lo[i] = lo[i].min((p[i] - grow).floor() as i32);
                hi[i] = hi[i].max((p[i] + grow + 0.6).floor() as i32);
            }
        }
        Walls::region(world, lo, hi, range)
    }

    /// The obstacles in the cells `lo ..= hi`.
    pub fn region(world: &World, lo: [i32; 3], hi: [i32; 3], range: f64) -> Walls {
        let mut boxes = Vec::new();
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                for x in lo[0]..=hi[0] {
                    let pos = BlockPos { x, y, z };
                    let c = [x as f64, y as f64, z as f64];
                    let cube = |g: f64| Aabb {
                        min: [c[0] - g, c[1] - g, c[2] - g],
                        max: [c[0] + 1.0 + g, c[1] + 1.0 + g, c[2] + 1.0 + g],
                    };
                    let Cell::Known(id) = world.cell(pos) else {
                        boxes.push(cube(1.0));
                        continue;
                    };
                    boxes.extend(world.boxes_at(id, pos));
                    let (info, d) = (world.info(id), world.detail(id));
                    if info.effect == Effect::Climbable {
                        // the gliding box overlaps this exactly when the position is
                        // in the cell
                        boxes.push(Aabb {
                            min: [c[0] + HALF_WIDTH, c[1] + HEIGHT, c[2] + HALF_WIDTH],
                            max: [c[0] + 1.0 - HALF_WIDTH, c[1] + 1.0, c[2] + 1.0 - HALF_WIDTH],
                        });
                    } else if d.fluid != "-"
                        || matches!(d.own_effect, Effect::Stuck | Effect::Bounce)
                    {
                        boxes.push(cube(0.0));
                    }
                }
            }
        }
        Self::from_boxes(boxes, lo, hi, range)
    }

    /// These obstacles and `extra`, for a solve held to one side of something. Boxes outside the cells read don't count.
    pub fn with_boxes(&self, extra: &[Aabb]) -> Walls {
        let mut boxes = self.boxes.clone();
        boxes.extend_from_slice(extra);
        Self::from_boxes(boxes, self.lo, self.hi, self.range)
    }

    fn from_boxes(boxes: Vec<Aabb>, lo: [i32; 3], hi: [i32; 3], range: f64) -> Walls {
        let glo = [lo[0] - PAD, lo[1] - PAD, lo[2] - PAD];
        let dims: [usize; 3] = std::array::from_fn(|i| (hi[i] + PAD - glo[i] + 1) as usize);
        let mut cells = vec![Vec::new(); dims[0] * dims[1] * dims[2]];
        for (n, b) in boxes.iter().enumerate() {
            let a: [i32; 3] = std::array::from_fn(|i| (b.min[i].floor() as i32).max(glo[i]));
            let z: [i32; 3] =
                std::array::from_fn(|i| (b.max[i].floor() as i32).min(glo[i] + dims[i] as i32 - 1));
            for y in a[1]..=z[1] {
                for zz in a[2]..=z[2] {
                    for x in a[0]..=z[0] {
                        let k = index(glo, dims, [x, y, zz]);
                        cells[k].push(n as u32);
                    }
                }
            }
        }
        Walls {
            boxes,
            lo,
            hi,
            glo,
            dims,
            cells,
            range,
        }
    }

    /// Whether every obstacle within `range` of the gliding box at `p` is in this set:
    /// the search box lies in the cells read.
    pub fn covers(&self, p: [f64; 3]) -> bool {
        let (a, b) = search_box(p, p, self.range);
        (0..3).all(|i| a[i] >= self.lo[i] as f64 && b[i] < (self.hi[i] + 1) as f64)
    }

    /// The clearance of the gliding box swept from `from` to `to` (feet; one leg of
    /// the L-shaped move): the least `leg_sdf` over the obstacles, capped at `range`,
    /// and the box that attains it (none at the cap).
    pub fn clearance(&self, from: [f64; 3], to: [f64; 3]) -> (f64, Option<usize>) {
        let (a, b) = search_box(from, to, self.range);
        let ca: [i32; 3] = std::array::from_fn(|i| (a[i].floor() as i32).max(self.glo[i]));
        let cb: [i32; 3] = std::array::from_fn(|i| {
            (b[i].floor() as i32).min(self.glo[i] + self.dims[i] as i32 - 1)
        });
        let mut best = (self.range, None);
        for y in ca[1]..=cb[1] {
            for z in ca[2]..=cb[2] {
                for x in ca[0]..=cb[0] {
                    for &n in &self.cells[index(self.glo, self.dims, [x, y, z])] {
                        let d = leg_sdf(from, to, &self.boxes[n as usize]);
                        // ties go to the lower index, so the answer doesn't depend on
                        // which cell saw the box first
                        if d < best.0 || (d == best.0 && best.1.is_some_and(|m| (n as usize) < m)) {
                            best = (d, Some(n as usize));
                        }
                    }
                }
            }
        }
        best
    }

    /// [`Walls::clearance`] by `segment_sdf`, for a move off the axes.
    pub fn segment_clearance(&self, from: [f64; 3], to: [f64; 3]) -> (f64, Option<usize>) {
        let (a, b) = search_box(from, to, self.range);
        let ca: [i32; 3] = std::array::from_fn(|i| (a[i].floor() as i32).max(self.glo[i]));
        let cb: [i32; 3] = std::array::from_fn(|i| {
            (b[i].floor() as i32).min(self.glo[i] + self.dims[i] as i32 - 1)
        });
        let mut best = (self.range, None);
        for y in ca[1]..=cb[1] {
            for z in ca[2]..=cb[2] {
                for x in ca[0]..=cb[0] {
                    for &n in &self.cells[index(self.glo, self.dims, [x, y, z])] {
                        let b = &self.boxes[n as usize];
                        // the sweep's bounding box is a lower bound, and skips most boxes
                        if leg_sdf(from, to, b) > best.0 {
                            continue;
                        }
                        let d = segment_sdf(from, to, b);
                        if d < best.0 || (d == best.0 && best.1.is_some_and(|m| (n as usize) < m)) {
                            best = (d, Some(n as usize));
                        }
                    }
                }
            }
        }
        best
    }
}

/// The cells within `range` of the gliding box swept from `p` to `q`.
fn search_box(p: [f64; 3], q: [f64; 3], range: f64) -> ([f64; 3], [f64; 3]) {
    let lo = [-HALF_WIDTH, 0.0, -HALF_WIDTH];
    let hi = [HALF_WIDTH, HEIGHT, HALF_WIDTH];
    (
        std::array::from_fn(|i| p[i].min(q[i]) + lo[i] - range),
        std::array::from_fn(|i| p[i].max(q[i]) + hi[i] + range),
    )
}

fn index(glo: [i32; 3], dims: [usize; 3], c: [i32; 3]) -> usize {
    let r: [usize; 3] = std::array::from_fn(|i| (c[i] - glo[i]) as usize);
    (r[1] * dims[2] + r[2]) * dims[0] + r[0]
}
