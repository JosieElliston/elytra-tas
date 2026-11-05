//! Splitting a course at gates, as `elytrasim/corner-time`'s `README-corner.md` does: a plane across
//! the route with a table of what the rest of the course is worth from each way of crossing it, so a
//! stretch can end anywhere on the gate and every way of reaching a gate adds to every way on from it.
//!
//! The table's state is where the crossing is on the plane, the velocity, and `tau`, how much of the
//! crossing tick is left after it (`past / rate`). why `tau`? velocity only changes at a tick, so the
//! later in its tick a flight crosses, the longer it coasts: 0.3-0.4 ticks of value on corner-time's
//! courses. Height is an axis here, unlike there, because floors make it matter.
//!
//! Read multilinear, as there. why not smoother? a cubic overshoots between nodes and a solve flies
//! straight at its errors (2.5 ticks there). Multilinear can't read below a cell's lowest corner, so a
//! solve is drawn to where the table is exact.

use sim::generic::num_dual::DualNum;

use super::ddp::X;

/// The state axes of a table: the plane's two free position axes, velocity, `tau`.
pub const AXES: usize = 6;
/// What it costs, in ticks, to read a cell's width outside the table, squared in that width. why
/// price it? a read outside holds the edge's value, so with no price nothing brings a flight back.
const OUTSIDE: f64 = 2.0;

/// A plane `sign * pos[axis] = at`, crossed from below `at`, inside a window on the free axes.
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    pub axis: usize,
    pub sign: f64,
    pub at: f64,
    /// The free axes' bounds. A crossing outside isn't this gate: a route can pass a plane elsewhere.
    pub lo: [f64; 2],
    pub hi: [f64; 2],
}

impl Gate {
    /// The world axes other than `axis`, in order.
    pub fn free(&self) -> [usize; 2] {
        match self.axis {
            0 => [1, 2],
            1 => [0, 2],
            _ => [0, 1],
        }
    }

    pub fn normal(&self) -> [f64; 3] {
        let mut n = [0.0; 3];
        n[self.axis] = self.sign;
        n
    }

    pub fn in_window(&self, x: &X) -> bool {
        let f = self.free();
        (0..2).all(|i| x[f[i]] >= self.lo[i] && x[f[i]] <= self.hi[i])
    }

    /// The table's coordinates of a state `tau` ticks of its own velocity past the plane.
    pub fn coords<D: DualNum<Primitive = f64> + Copy>(&self, x: &[D; 6], tau: D) -> [D; AXES] {
        let f = self.free();
        [
            x[f[0]] - x[f[0] + 3] * tau,
            x[f[1]] - x[f[1] + 3] * tau,
            x[3],
            x[4],
            x[5],
            tau,
        ]
    }

    /// The state with table coordinates `c`, its inverse.
    pub fn state(&self, c: &[f64; AXES]) -> X {
        let f = self.free();
        let tau = c[5];
        let v = [c[2], c[3], c[4]];
        let mut x = [0.0, 0.0, 0.0, v[0], v[1], v[2]];
        x[f[0]] = c[0] + v[f[0]] * tau;
        x[f[1]] = c[1] + v[f[1]] * tau;
        x[self.axis] = self.sign * self.at + v[self.axis] * tau;
        x
    }
}

/// What the course is worth from a gate: ticks from the crossing, on a grid.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueTable {
    pub gate: Gate,
    pub nodes: [Vec<f64>; AXES],
    /// In [`ValueTable::index`] order. Every value finite.
    pub values: Vec<f64>,
}

impl ValueTable {
    pub fn index(nodes: &[Vec<f64>; AXES], at: &[usize; AXES]) -> usize {
        (0..AXES).fold(0, |i, a| i * nodes[a].len() + at[a])
    }

    /// Every cell's grid position, in index order.
    pub fn cells(nodes: &[Vec<f64>; AXES]) -> Vec<[usize; AXES]> {
        let mut out = vec![[0; AXES]];
        for a in 0..AXES {
            out = out
                .iter()
                .flat_map(|c| {
                    (0..nodes[a].len()).map(move |j| {
                        let mut c = *c;
                        c[a] = j;
                        c
                    })
                })
                .collect();
        }
        out
    }

    /// The table at `c`, multilinear, holding the edge's value outside and pricing the distance.
    pub fn read<D: DualNum<Primitive = f64> + Copy>(&self, c: &[D; AXES]) -> D {
        let mut base = [0usize; AXES];
        let mut w = [D::from(0.0); AXES];
        let mut out = D::from(0.0);
        for a in 0..AXES {
            let n = &self.nodes[a];
            if n.len() < 2 {
                continue;
            }
            let v = c[a].re();
            let j = n.partition_point(|&q| q <= v).clamp(1, n.len() - 1) - 1;
            base[a] = j;
            let t = (c[a] - D::from(n[j])) / D::from(n[j + 1] - n[j]);
            if v < n[0] {
                out = out + t * t * D::from(OUTSIDE);
            } else if v > n[n.len() - 1] {
                let e = t - D::from(1.0);
                out = out + e * e * D::from(OUTSIDE);
                w[a] = D::from(1.0);
            } else {
                w[a] = t;
            }
        }
        'corners: for corner in 0..1usize << AXES {
            let mut at = base;
            let mut p = D::from(1.0);
            for a in 0..AXES {
                if corner >> a & 1 == 1 {
                    if self.nodes[a].len() < 2 {
                        continue 'corners;
                    }
                    at[a] += 1;
                    p = p * w[a];
                } else {
                    p = p * (D::from(1.0) - w[a]);
                }
            }
            out = out + p * D::from(self.values[Self::index(&self.nodes, &at)]);
        }
        out
    }
}
