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

use sim::generic::ghost::{HALF_WIDTH, HEIGHT};
use world::{Aabb, BlockPos, Cell, Effect, World};

/// Obstacle boxes in a region of the world, bucketed by block cell.
#[derive(Debug, Clone)]
pub struct Walls {
    pub boxes: Vec<Aabb>,
    /// The cells read, inclusive. A query point is answered exactly only if its
    /// search box stays inside; see [`Walls::covers`].
    pub lo: [i32; 3],
    pub hi: [i32; 3],
    /// Bucket grid over `lo - PAD ..= hi + PAD`: cell `k`'s indices into `boxes` are
    /// `filed[starts[k]..starts[k + 1]]`. why flat? a row of cells along x is then one slice, and most cells are empty.
    glo: [i32; 3],
    dims: [usize; 3],
    starts: Vec<u32>,
    filed: Vec<u32>,
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
        let ranges: Vec<([i32; 3], [i32; 3])> = boxes
            .iter()
            .map(|b| {
                let a = std::array::from_fn(|i| (b.min[i].floor() as i32).max(glo[i]));
                // Closed boxes, open cells: a query meeting a box only at a cell's edge has it at `range`, the cap.
                // A box with no width overlaps no open cell, so it takes the one above, which any nearer query reads.
                let z = std::array::from_fn(|i| {
                    let top = (b.max[i].ceil() as i32 - 1).max(b.min[i].floor() as i32);
                    top.min(glo[i] + dims[i] as i32 - 1)
                });
                (a, z)
            })
            .collect();
        let each_cell = |(a, z): &([i32; 3], [i32; 3]), f: &mut dyn FnMut(usize)| {
            for y in a[1]..=z[1] {
                for zz in a[2]..=z[2] {
                    for x in a[0]..=z[0] {
                        f(index(glo, dims, [x, y, zz]));
                    }
                }
            }
        };
        // count, sum to each cell's end, then fill from the ends down, so a cell's boxes stay in index order
        let mut starts = vec![0u32; dims[0] * dims[1] * dims[2] + 1];
        for r in &ranges {
            each_cell(r, &mut |k| starts[k] += 1);
        }
        let mut sum = 0;
        for s in &mut starts {
            sum += *s;
            *s = sum;
        }
        let mut filed = vec![0u32; sum as usize];
        for (n, r) in ranges.iter().enumerate().rev() {
            each_cell(r, &mut |k| {
                starts[k] -= 1;
                filed[starts[k] as usize] = n as u32;
            });
        }
        Walls {
            boxes,
            lo,
            hi,
            glo,
            dims,
            starts,
            filed,
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
        // `leg_sdf`, spelled out for this loop: the leg's ends are sorted once, not per box
        let lo = [-HALF_WIDTH, 0.0, -HALF_WIDTH];
        let hi = [HALF_WIDTH, HEIGHT, HALF_WIDTH];
        let near: [f64; 3] = std::array::from_fn(|i| from[i].min(to[i]));
        let far: [f64; 3] = std::array::from_fn(|i| from[i].max(to[i]));
        // past the grid along x: `y` and `z` have their own empty ranges, but a slice of the row would panic
        if ca[0] > cb[0] {
            return best;
        }
        for y in ca[1]..=cb[1] {
            for z in ca[2]..=cb[2] {
                // cells along x are adjacent, so the whole row is one slice
                let first = index(self.glo, self.dims, [ca[0], y, z]);
                let last = index(self.glo, self.dims, [cb[0], y, z]);
                let row = self.starts[first] as usize..self.starts[last + 1] as usize;
                for &n in &self.filed[row] {
                    let b = &self.boxes[n as usize];
                    let gap: [f64; 3] = std::array::from_fn(|i| {
                        ((b.min[i] - hi[i]) - far[i]).max(near[i] + (lo[i] - b.max[i]))
                    });
                    // a gap past the best is a distance past it, so skip the rest
                    let widest = gap[0].max(gap[1]).max(gap[2]);
                    if widest > best.0 {
                        continue;
                    }
                    let mut sqr = 0.0;
                    let mut outside = 0.0;
                    let mut n_pos = 0;
                    for g in gap {
                        if g > 0.0 {
                            outside = g;
                            sqr += g * g;
                            n_pos += 1;
                        }
                    }
                    let d = match n_pos {
                        0 => widest,
                        1 => outside,
                        _ => f64::sqrt(sqr),
                    };
                    // ties go to the lower index, so the answer doesn't depend on
                    // which cell saw the box first
                    if d < best.0 || (d == best.0 && best.1.is_some_and(|m| (n as usize) < m)) {
                        best = (d, Some(n as usize));
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

#[cfg(test)]
mod tests {
    use super::*;
    use sim::generic::ghost::leg_sdf;

    fn aabb(min: [f64; 3], max: [f64; 3]) -> Aabb {
        Aabb { min, max }
    }

    fn walls(boxes: Vec<Aabb>) -> Walls {
        Walls::from_boxes(boxes, [-6; 3], [6; 3], 1.0)
    }

    /// A small LCG: deterministic, no dev-dependency.
    fn lcg() -> impl FnMut(f64, f64) -> f64 {
        let mut s: u64 = 0x2545F4914F6CDD1D;
        move |lo, hi| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            lo + (hi - lo) * ((s >> 11) as f64 / (1u64 << 53) as f64)
        }
    }

    /// The least `leg_sdf` over every box, capped as `clearance` caps it, ties to the lower index.
    fn brute(w: &Walls, from: [f64; 3], to: [f64; 3]) -> (f64, Option<usize>) {
        let mut best = (w.range, None);
        for (n, b) in w.boxes.iter().enumerate() {
            let d = leg_sdf(from, to, b);
            if d < best.0 {
                best = (d, Some(n));
            }
        }
        best
    }

    #[test]
    fn a_box_is_filed_only_in_the_cells_it_overlaps() {
        let cases = [
            (aabb([2.0, 3.0, 4.0], [3.0, 4.0, 5.0]), 1),
            (aabb([-3.0, -3.0, -3.0], [-2.0, -2.0, -2.0]), 1),
            // a bottom slab
            (aabb([0.0, 0.0, 0.0], [1.0, 0.5, 1.0]), 1),
            // a fence, into the cell above
            (aabb([0.0, 0.0, 0.0], [1.0, 1.5, 1.0]), 2),
            (aabb([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]), 8),
            // no thickness, on a cell's face: still somewhere
            (aabb([1.0, 1.0, 1.0], [1.0, 2.0, 2.0]), 1),
        ];
        for (b, cells) in cases {
            let w = walls(vec![b.clone()]);
            assert_eq!(w.filed.len(), cells, "{b:?}");
        }
    }

    #[test]
    fn clearance_is_the_least_over_every_box() {
        let mut r = lcg();
        let mut boxes = Vec::new();
        for k in 0..60 {
            let c: [f64; 3] = std::array::from_fn(|_| r(-5.0, 4.0).floor());
            boxes.push(match k % 3 {
                // whole cubes, the case filed by its faces
                0 => aabb(c, c.map(|v| v + 1.0)),
                // parts of cells, and across them
                1 => aabb(c, [c[0] + r(0.0, 1.0), c[1] + r(0.0, 1.0), c[2] + r(0.0, 1.0)]),
                _ => {
                    let lo: [f64; 3] = std::array::from_fn(|i| c[i] + r(0.0, 1.0));
                    aabb(lo, std::array::from_fn(|i| lo[i] + r(0.0, 1.5)))
                }
            });
        }
        let w = walls(boxes);
        for _ in 0..5000 {
            let p: [f64; 3] = std::array::from_fn(|_| r(-4.0, 4.0));
            let mut q = p;
            q[(r(0.0, 3.0) as usize).min(2)] += r(-2.0, 2.0);
            assert_eq!(w.clearance(p, q), brute(&w, p, q), "{p:?} to {q:?}");
        }
    }

    #[test]
    fn a_box_without_width_is_filed_once() {
        let at = |x: f64, y: f64, z: f64| [x, y, z];
        for (min, max, cell) in [
            // a plane, on a cell's edge and off it
            (at(1.0, 1.5, 1.5), at(1.0, 1.75, 1.75), [1, 1, 1]),
            (at(-2.0, 1.5, 1.5), at(-2.0, 1.75, 1.75), [-2, 1, 1]),
            (at(1.5, 1.5, 1.5), at(1.5, 1.75, 1.75), [1, 1, 1]),
            // a line, a point
            (at(1.0, 2.0, 1.5), at(1.0, 2.0, 1.75), [1, 2, 1]),
            (at(1.0, 2.0, 3.0), at(1.0, 2.0, 3.0), [1, 2, 3]),
            (at(-1.0, -2.0, -3.0), at(-1.0, -2.0, -3.0), [-1, -2, -3]),
        ] {
            let w = walls(vec![aabb(min, max)]);
            assert_eq!(w.filed.len(), 1, "{min:?}");
            let k = index(w.glo, w.dims, cell);
            assert_eq!(w.filed[w.starts[k] as usize..w.starts[k + 1] as usize], [0], "{min:?}");
        }
    }

    /// A plane spanning a cell's edge along its own axes, across several cells of the others.
    #[test]
    fn a_plane_is_filed_in_each_cell_it_crosses() {
        let w = walls(vec![aabb([1.0, 0.0, 0.5], [1.0, 2.0, 1.5])]);
        assert_eq!(w.filed.len(), 4);
    }

    #[test]
    fn clearance_sees_boxes_without_width() {
        let mut r = lcg();
        let mut boxes = Vec::new();
        for k in 0..60 {
            let c: [f64; 3] = std::array::from_fn(|_| r(-5.0, 4.0).floor());
            // on a cell's edge in the flat axes, or off it
            let min: [f64; 3] = std::array::from_fn(|i| c[i] + if k % 2 == 0 { 0.0 } else { r(0.0, 1.0) });
            let flat = k % 7; // a nonzero mask of flat axes
            let max = std::array::from_fn(|i| {
                if (flat + 1) >> i & 1 == 1 { min[i] } else { min[i] + r(0.0, 1.5) }
            });
            boxes.push(aabb(min, max));
        }
        let w = walls(boxes);
        for _ in 0..5000 {
            let p: [f64; 3] = std::array::from_fn(|_| r(-4.0, 4.0));
            let mut q = p;
            q[(r(0.0, 3.0) as usize).min(2)] += r(-2.0, 2.0);
            assert_eq!(w.clearance(p, q), brute(&w, p, q), "{p:?} to {q:?}");
        }
    }

    /// A plane on a cell's edge, from both sides: the cap exactly at `range`, seen just inside it.
    #[test]
    fn a_plane_at_the_cap_reads_the_cap() {
        let (lo, hi) = ([-HALF_WIDTH, 0.0, -HALF_WIDTH], [HALF_WIDTH, HEIGHT, HALF_WIDTH]);
        for i in 0..3 {
            let (mut min, mut max) = ([-3.0; 3], [3.0; 3]);
            (min[i], max[i]) = (1.0, 1.0);
            let w = walls(vec![aabb(min, max)]);
            for at in [1.0 + w.range - lo[i], 1.0 - w.range - hi[i]] {
                let mut p = [0.0; 3];
                p[i] = at;
                assert_eq!(w.clearance(p, p), (w.range, None), "axis {i} at {at}");
                p[i] = at - (at - 1.0).signum() * 1e-9;
                assert_eq!(w.clearance(p, p).1, Some(0), "axis {i} just inside {at}");
            }
        }
    }

    /// A leg can't pass through a plane without reading it, however thin the plane is.
    #[test]
    fn a_leg_through_a_plane_reads_inside_it() {
        let w = walls(vec![aabb([-3.0, -3.0, 1.0], [3.0, 3.0, 1.0])]);
        let (c, nearest) = w.clearance([0.0, 0.0, -1.0], [0.0, 0.0, 3.0]);
        assert_eq!(nearest, Some(0));
        assert!(c <= 0.0, "{c}");
    }

    /// Off the grid on every side, along each axis: nothing there, and no panic.
    #[test]
    fn clearance_off_the_grid_reads_the_cap() {
        let w = walls(vec![aabb([0.0; 3], [1.0; 3])]);
        for i in 0..3 {
            for at in [-100.0, 100.0] {
                let mut p = [0.5; 3];
                p[i] = at;
                assert_eq!(w.clearance(p, p), (w.range, None), "axis {i} at {at}");
            }
        }
    }

    /// `--forbid`'s way in.
    #[test]
    fn with_boxes_keeps_a_box_without_width() {
        let w = walls(Vec::new()).with_boxes(&[aabb([1.0, 1.0, 1.0], [1.0, 1.0, 1.0])]);
        assert_eq!(w.clearance([1.0, 1.0, 1.0], [1.0, 1.0, 1.0]).1, Some(0));
    }

    /// A query whose search box only just meets a cube's face, where the cube is exactly `range` away.
    #[test]
    fn clearance_at_the_cap_reads_the_cap() {
        let (min, max) = ([0.0; 3], [1.0; 3]);
        let w = walls(vec![aabb(min, max)]);
        let (lo, hi) = ([-HALF_WIDTH, 0.0, -HALF_WIDTH], [HALF_WIDTH, HEIGHT, HALF_WIDTH]);
        for i in 0..3 {
            for at in [max[i] + w.range - lo[i], min[i] - w.range - hi[i]] {
                let mut p = [0.5, 0.5, 0.5];
                p[i] = at;
                assert_eq!(w.clearance(p, p), (w.range, None), "axis {i} at {at}");
                // and just inside it, the cube counts
                p[i] = at - at.signum() * 1e-9;
                assert_eq!(w.clearance(p, p).1, Some(0), "axis {i} just inside {at}");
            }
        }
    }
}
