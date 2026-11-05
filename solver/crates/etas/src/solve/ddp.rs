//! DDP (iLQR with the dynamics' second-order terms) through ghost walls under an
//! augmented Lagrangian, on the generic kernel, with derivatives from dual numbers.
//! Ported in method from elytrasim's corner-time `ilqr.rs`, not in structure.
//!
//! The state is position and velocity. The control is `(lead, pitch)` in radians:
//! lead is the yaw less the heading of the velocity the tick starts with. why lead?
//! corner-time found it conditions the solve better than absolute yaw, and the
//! feedback a re-solve gives is then relative to the flown velocity. [`Plan::inputs`]
//! turns a plan back into absolute rotations.
//!
//! Walls are the PHR price of the clearance of each of the L-shaped move's three legs
//! (`ghost::leg_sdf`: exact for a leg, where its points alone miss boxes thinner than
//! a leg), Gauss-Newton, one multiplier per leg. The goal ([`Goal`]) is a terminal
//! cost. Second derivatives of the dynamics enter `Q`, which a time objective needs:
//! its cost is all terminal, so the curvature that makes an optimum interior
//! (turning bleeds speed) is in the dynamics alone. Dual numbers take the branch the value takes, so at the kernel's kinks
//! (pitch 0, `v_y = 0` after gravity) the derivative is one-sided, never a stencil
//! straddling the kink, which corner-time found pins pitch at 0.

use std::f64::consts::PI;

use sim::generic::Generic;
use sim::generic::ghost::{l_move, leg_sdf};
use sim::generic::num_dual::{Dual2SVec64, DualNum, DualSVec64};
use sim::state::Input;

use super::walls::Walls;

/// Position, velocity.
pub type X = [f64; 6];
/// `[lead, pitch]`, radians.
pub type U = [f64; 2];

const D2R: f64 = PI / 180.0;
/// Pitch bound, degrees. why short of 90? the kernel's -90 is a dead tick (a cliff).
pub const PITCH_LIMIT: f64 = 89.0;
/// Constraints per tick: the legs of the L-shaped move.
pub const NP: usize = 3;
/// The plane goal's floor on the rate toward the plane, so a horizon that never
/// reaches it and turns away isn't worth minus a lot.
const RATE_MIN: f64 = 0.05;

/// Where a stretch ends.
#[derive(Debug, Clone, PartialEq)]
pub enum Goal {
    /// Cross the plane `normal . pos = at` in least time, timed by secant: with the
    /// horizon cut to the first tick past it, `N - past_N / rate_N`, which is exact
    /// because the last move is `v_N`. The horizon grows by holding the last control,
    /// up to `cap` ticks.
    Plane {
        normal: [f64; 3],
        at: f64,
        cap: usize,
    },
    /// [`Goal::Plane`]'s secant time, for a plane the route passes more than once and a
    /// finish that is a box on it: only a crossing from behind the plane to past it counts,
    /// and the state past it is held to `lo ..= hi` (a quadratic price at the walls' `rho`,
    /// so it closes as the price grows). why rising only? academy's route starts past its
    /// finish plane and crosses it the wrong way first. Use an infinite bound for a free
    /// axis.
    Gate {
        normal: [f64; 3],
        at: f64,
        cap: usize,
        lo: [f64; 3],
        hi: [f64; 3],
    },
    /// Be at `target` after exactly the horizon's ticks. Each coordinate's error over
    /// `scale` is held to 0 under the augmented Lagrangian; min time is a search over
    /// the horizon outside the solve.
    Reach { target: X, scale: X },
}

/// The augmented Lagrangian's schedule and the solve's tolerances.
#[derive(Debug, Clone, PartialEq)]
pub struct Opts {
    /// Clearance margin, blocks: every leg keeps at least this.
    pub margin: f64,
    /// First price, its growth per stage, at most this many stages. The default
    /// starts at 100. why not 1? the cost is in ticks, and at 1 the first stage
    /// tunnels through a thin wall for a tick's gain, then can't get back: the wall's
    /// gradient inside points along its thickness (the toy test did this).
    pub c0: f64,
    pub grow: f64,
    pub stages: usize,
    /// DDP iterations per stage, at most.
    pub iters: usize,
    /// Reach: the goal's price over the walls'.
    pub goal_weight: f64,
    /// Reach: done when every scaled error is under this.
    pub tol: f64,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            margin: 0.02,
            c0: 100.0,
            grow: 10.0,
            stages: 8,
            iters: 60,
            goal_weight: 100.0,
            tol: 1e-7,
        }
    }
}

/// One stretch to solve.
#[derive(Debug, Clone)]
pub struct Problem<'a> {
    pub kernel: Generic,
    pub walls: &'a Walls,
    pub x0: X,
    pub goal: Goal,
    pub opts: Opts,
}

/// The augmented Lagrangian's price and multipliers, kept for warm re-solves.
#[derive(Debug, Clone, PartialEq)]
pub struct Multipliers {
    pub rho: f64,
    /// Per tick, per leg.
    pub walls: Vec<[f64; NP]>,
    /// Reach: per coordinate.
    pub goal: X,
}

impl Multipliers {
    pub fn new(rho: f64, n: usize) -> Self {
        Multipliers {
            rho,
            walls: vec![[0.0; NP]; n],
            goal: [0.0; 6],
        }
    }

    /// Drop the first `k` ticks (a receding horizon's shift).
    pub fn shifted(&self, k: usize) -> Self {
        Multipliers {
            rho: self.rho,
            walls: self.walls[k.min(self.walls.len())..].to_vec(),
            goal: self.goal,
        }
    }
}

/// Controls and the states they fly through the walls (`x.len() == u.len() + 1`).
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub u: Vec<U>,
    pub x: Vec<X>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stage {
    pub rho: f64,
    /// Plane: the secant time; Reach: the largest scaled error.
    pub value: f64,
    pub worst: f64,
    pub iters: usize,
}

#[derive(Debug, Clone)]
pub struct Solved {
    pub plan: Plan,
    pub mult: Multipliers,
    pub stages: Vec<Stage>,
    /// Clear of every wall to the margin, and the goal met (Plane: crossed; Reach:
    /// within `tol`).
    pub feasible: bool,
}

/// The velocity heading, radians, in yaw's convention (look = `(-sin yaw, cos yaw)`).
fn heading<D: DualNum<Primitive = f64> + Copy>(v: &[D; 3]) -> D {
    DualNum::atan2(&(-v[0]), v[2])
}

/// The kernel's velocity update under a `(lead, pitch)` control.
pub fn vel_next<D: DualNum<Primitive = f64> + Copy>(k: &Generic, v: [D; 3], u: [D; 2]) -> [D; 3] {
    let r2d = D::from(1.0 / D2R);
    let input = Input {
        yaw: (heading(&v) + u[0]) * r2d,
        pitch: u[1] * r2d,
    };
    k.velocity(v, &k.look(&input))
}

/// One tick through the walls: the next state and the corners of its move (the start,
/// then [`l_move`]'s points), whose consecutive pairs are its legs.
pub fn step<D: DualNum<Primitive = f64> + Copy>(
    k: &Generic,
    x: &[D; 6],
    u: [D; 2],
) -> ([D; 6], [[D; 3]; NP + 1]) {
    let pos = [x[0], x[1], x[2]];
    let v = vel_next(k, [x[3], x[4], x[5]], u);
    let [a, b, e] = l_move(pos, v);
    ([e[0], e[1], e[2], v[0], v[1], v[2]], [pos, a, b, e])
}

/// The PHR price of clearance `c` under `margin`, and `a`, its derivative in `-c`.
fn phr(c: f64, lam: f64, rho: f64, margin: f64) -> (f64, f64) {
    let a = (lam + rho * (margin - c)).max(0.0);
    ((a * a - lam * lam) / (2.0 * rho), a)
}

fn grad<const N: usize>(d: &DualSVec64<N>) -> [f64; N] {
    match &d.eps.0 {
        Some(m) => std::array::from_fn(|i| m[i]),
        None => [0.0; N],
    }
}

/// Value, gradient and Hessian of `f` at `z`, from one pass of second-order duals.
fn hess<const N: usize>(
    f: impl Fn(&[Dual2SVec64<N>; N]) -> Dual2SVec64<N>,
    z: &[f64; N],
) -> (f64, [f64; N], [[f64; N]; N]) {
    let d: [Dual2SVec64<N>; N] =
        std::array::from_fn(|i| Dual2SVec64::<N>::from_re(z[i]).derivative(i));
    let r = f(&d);
    let g = match &r.v1.0 {
        Some(m) => std::array::from_fn(|i| m[i]),
        None => [0.0; N],
    };
    let h = match &r.v2.0 {
        Some(m) => std::array::from_fn(|i| std::array::from_fn(|j| m[(i, j)])),
        None => [[0.0; N]; N],
    };
    (r.re, g, h)
}

/// A tick's first derivatives in `z = (x, u)`: the next state's Jacobian, and per
/// leg its PHR slope `a` and clearance gradient (zero where inactive).
struct TickDerivs {
    fz: [[f64; 8]; 6],
    walls: Vec<(f64, [f64; 8])>,
}

impl Plan {
    /// The rotations this plan flies, degrees, as the game holds them. Yaw is unwrapped
    /// to within 180 of the previous tick's, starting from `yaw_from`. why unwrap? the
    /// game's yaw is unbounded and the faithful kernel sees its float bits, so the
    /// branch is part of the plan.
    pub fn inputs(&self, yaw_from: f64) -> Vec<Input<f32>> {
        let mut prev = yaw_from;
        self.u
            .iter()
            .zip(&self.x)
            .map(|(u, x)| {
                let (input, yaw) = input_at(x, u, prev);
                prev = yaw;
                input
            })
            .collect()
    }
}

/// The rotation control `u` flies from state `x`, as the game holds it, with its yaw
/// (degrees, unrounded) on the branch nearest `near`.
pub fn input_at(x: &X, u: &U, near: f64) -> (Input<f32>, f64) {
    let yaw = absolute_yaw(x, u[0], near);
    (
        Input {
            yaw: yaw as f32,
            pitch: (u[1] / D2R).clamp(-PITCH_LIMIT, PITCH_LIMIT) as f32,
        },
        yaw,
    )
}

/// Degrees: the yaw of `lead` (radians) off `x`'s velocity heading, on the branch
/// nearest `near`.
pub fn absolute_yaw(x: &X, lead: f64, near: f64) -> f64 {
    let yaw = (heading(&[x[3], x[4], x[5]]) + lead) / D2R;
    yaw + 360.0 * ((near - yaw) / 360.0).round()
}

/// Radians, in [-pi, pi): the lead of a yaw (degrees) off `x`'s velocity heading.
pub fn lead_of(x: &X, yaw: f64) -> f64 {
    let l = yaw * D2R - heading(&[x[3], x[4], x[5]]);
    (l + PI).rem_euclid(2.0 * PI) - PI
}

/// A control from degrees.
pub fn control(lead: f64, pitch_deg: f64) -> U {
    [lead, pitch_deg.clamp(-PITCH_LIMIT, PITCH_LIMIT) * D2R]
}

fn clamp_pitch(u: &mut U) {
    u[1] = u[1].clamp(-PITCH_LIMIT * D2R, PITCH_LIMIT * D2R);
}

impl<'a> Problem<'a> {
    pub fn rollout(&self, u: &[U]) -> Vec<X> {
        let mut x = Vec::with_capacity(u.len() + 1);
        x.push(self.x0);
        for &uk in u {
            let n = step(&self.kernel, x.last().unwrap(), uk).0;
            x.push(n);
        }
        x
    }

    /// The clearance of each leg of the move from `x` under `u`, and the nearest box.
    fn clearances(&self, x: &X, u: U) -> [(f64, Option<usize>); NP] {
        let (_, c) = step(&self.kernel, x, u);
        std::array::from_fn(|l| self.walls.clearance(c[l], c[l + 1]))
    }

    /// The least clearance over every leg of the plan's moves.
    pub fn worst(&self, x: &[X], u: &[U]) -> f64 {
        (0..u.len())
            .flat_map(|k| self.clearances(&x[k], u[k]))
            .map(|c| c.0)
            .fold(f64::INFINITY, f64::min)
    }

    /// Plane: how far past the plane, and the rate toward it.
    fn plane<D: DualNum<Primitive = f64> + Copy>(&self, x: &[D; 6]) -> Option<(D, D)> {
        let (n, at) = match self.goal {
            Goal::Plane { normal, at, .. } | Goal::Gate { normal, at, .. } => (normal, at),
            Goal::Reach { .. } => return None,
        };
        let past = x[0] * n[0] + x[1] * n[1] + x[2] * n[2] - D::from(at);
        let rate = x[3] * n[0] + x[4] * n[1] + x[5] * n[2];
        Some((past, rate))
    }

    /// Whether a move from `prev` to `x` ends the flight: past the plane, and for a
    /// [`Goal::Gate`] from behind it. Never for [`Goal::Reach`].
    pub fn crossed(&self, prev: &X, x: &X) -> bool {
        let past = |x: &X| self.plane(x).map_or(0.0, |p| p.0);
        match self.goal {
            Goal::Plane { .. } => past(x) > 0.0,
            Goal::Gate { .. } => past(x) > 0.0 && past(prev) <= 0.0,
            Goal::Reach { .. } => false,
        }
    }

    /// The goal's part of the cost at the horizon `n`.
    fn goal_cost<D: DualNum<Primitive = f64> + Copy>(
        &self,
        m: &Multipliers,
        x: &[D; 6],
        n: usize,
    ) -> D {
        match &self.goal {
            Goal::Plane { .. } | Goal::Gate { .. } => {
                let (past, rate) = self.plane(x).unwrap();
                let rate = if rate.re() > RATE_MIN {
                    rate
                } else {
                    D::from(RATE_MIN)
                };
                let mut c = D::from(n as f64) - past / rate;
                if let Goal::Gate { lo, hi, .. } = &self.goal {
                    for i in 0..3 {
                        for over in [x[i] - D::from(hi[i]), D::from(lo[i]) - x[i]] {
                            let over = over + D::from(self.opts.margin);
                            if over.re() > 0.0 {
                                c = c + over * over * D::from(0.5 * m.rho);
                            }
                        }
                    }
                }
                c
            }
            Goal::Reach { target, scale } => {
                let rho = m.rho * self.opts.goal_weight;
                let mut c = D::from(0.0);
                for i in 0..6 {
                    let e = (x[i] - D::from(target[i])) * D::from(1.0 / scale[i]);
                    c = c + e * D::from(m.goal[i]) + e * e * D::from(0.5 * rho);
                }
                c
            }
        }
    }

    /// Plane: the secant time of `x`'s horizon. Reach: the largest scaled error.
    pub fn value(&self, x: &[X]) -> f64 {
        let n = x.len() - 1;
        match &self.goal {
            Goal::Plane { .. } | Goal::Gate { .. } => {
                let (past, rate) = self.plane(&x[n]).unwrap();
                n as f64 - past / rate.max(RATE_MIN)
            }
            Goal::Reach { target, scale } => (0..6)
                .map(|i| ((x[n][i] - target[i]) / scale[i]).abs())
                .fold(0.0, f64::max),
        }
    }

    /// The augmented cost of a rollout.
    pub fn cost(&self, m: &Multipliers, x: &[X], u: &[U]) -> f64 {
        let n = u.len();
        let mut j = self.goal_cost(m, &x[n], n);
        for k in 0..n {
            for (p, (c, _)) in self.clearances(&x[k], u[k]).iter().enumerate() {
                j += phr(*c, m.walls[k][p], m.rho, self.opts.margin).0;
            }
        }
        j
    }

    fn derivs(&self, m: &Multipliers, k: usize, x: &X, u: U) -> TickDerivs {
        let z: [DualSVec64<8>; 8] = std::array::from_fn(|i| {
            let v = if i < 6 { x[i] } else { u[i - 6] };
            DualSVec64::<8>::from_re(v).derivative(i)
        });
        let xd: [DualSVec64<8>; 6] = std::array::from_fn(|i| z[i]);
        let (next, corners) = step(&self.kernel, &xd, [z[6], z[7]]);
        let fz = next.map(|d| grad(&d));
        let walls = (0..NP)
            .map(|l| {
                let (from, to) = (corners[l], corners[l + 1]);
                let (c, nearest) = self.walls.clearance(from.map(|d| d.re), to.map(|d| d.re));
                let (_, a) = phr(c, m.walls[k][l], m.rho, self.opts.margin);
                match nearest {
                    Some(b) if a > 0.0 => (a, grad(&leg_sdf(from, to, &self.walls.boxes[b]))),
                    _ => (0.0, [0.0; 8]),
                }
            })
            .collect();
        TickDerivs { fz, walls }
    }

    /// The augmented cost of `u` and its gradient, by the adjoint of the rollout (the
    /// first-order half of the backward pass). For checking the derivatives.
    pub fn gradient(&self, m: &Multipliers, u: &[U]) -> (f64, Vec<U>) {
        let x = self.rollout(u);
        let n = u.len();
        let (_, mut vx, _) = hess(|d| self.goal_cost(m, d, n), &x[n]);
        let mut g = vec![[0.0; 2]; n];
        for k in (0..n).rev() {
            let d = self.derivs(m, k, &x[k], u[k]);
            let mut qz = [0.0; 8];
            for (i, q) in qz.iter_mut().enumerate() {
                *q = (0..6).map(|r| d.fz[r][i] * vx[r]).sum::<f64>();
                for (a, gw) in &d.walls {
                    *q -= a * gw[i];
                }
            }
            g[k] = [qz[6], qz[7]];
            vx = std::array::from_fn(|i| qz[i]);
        }
        (self.cost(m, &x, u), g)
    }

    /// One backward pass with regularization `mu`: per tick the step and gain, and the
    /// expected reduction's linear and quadratic parts. None if some `Q_uu` isn't
    /// positive definite.
    // small dense matrices, indexed
    #[allow(clippy::type_complexity, clippy::needless_range_loop)]
    fn backward(
        &self,
        m: &Multipliers,
        x: &[X],
        u: &[U],
        mu: f64,
    ) -> Option<(Vec<U>, Vec<[[f64; 6]; 2]>, f64, f64)> {
        let n = u.len();
        let rho = m.rho;
        let (_, mut vx, mut vxx) = hess(|d| self.goal_cost(m, d, n), &x[n]);
        let mut ks = vec![[0.0; 2]; n];
        let mut gains = vec![[[0.0; 6]; 2]; n];
        let (mut dv1, mut dv2) = (0.0, 0.0);
        let lim = PITCH_LIMIT * D2R;
        for k in (0..n).rev() {
            let (xk, uk) = (&x[k], u[k]);
            let d = self.derivs(m, k, xk, uk);
            // Q_z = f_z' V_x, Q_zz = f_z' V_xx f_z (8 x 8, z = (x, u))
            let mut qz = [0.0; 8];
            let mut qzz = [[0.0; 8]; 8];
            let mut vf = [[0.0; 8]; 6]; // V_xx f_z
            for r in 0..6 {
                for j in 0..8 {
                    vf[r][j] = (0..6).map(|s| vxx[r][s] * d.fz[s][j]).sum();
                }
            }
            for i in 0..8 {
                qz[i] = (0..6).map(|r| d.fz[r][i] * vx[r]).sum();
                for j in 0..8 {
                    qzz[i][j] = (0..6).map(|r| d.fz[r][i] * vf[r][j]).sum();
                }
            }
            // second-order dynamics: V_x weighs the new velocity through both the
            // position and the velocity, and the new velocity depends on (v, u) only
            let w = [vx[0] + vx[3], vx[1] + vx[4], vx[2] + vx[5]];
            let z5 = [xk[3], xk[4], xk[5], uk[0], uk[1]];
            let (_, _, h2) = hess(
                |z| {
                    let v = vel_next(&self.kernel, [z[0], z[1], z[2]], [z[3], z[4]]);
                    v[0] * w[0] + v[1] * w[1] + v[2] * w[2]
                },
                &z5,
            );
            for i in 0..5 {
                for j in 0..5 {
                    qzz[3 + i][3 + j] += h2[i][j];
                }
            }
            // walls, Gauss-Newton
            for (a, g) in &d.walls {
                if *a > 0.0 {
                    for i in 0..8 {
                        qz[i] -= a * g[i];
                        for j in 0..8 {
                            qzz[i][j] += rho * g[i] * g[j];
                        }
                    }
                }
            }
            let qu = [qz[6], qz[7]];
            let quu = [[qzz[6][6], qzz[6][7]], [qzz[7][6], qzz[7][7]]];
            let qux: [[f64; 6]; 2] =
                std::array::from_fn(|a| std::array::from_fn(|j| qzz[6 + a][j]));
            // regularized, boxed solve for the step
            let r = [[quu[0][0] + mu, quu[0][1]], [quu[1][0], quu[1][1] + mu]];
            let det = r[0][0] * r[1][1] - r[0][1] * r[1][0];
            if !(r[0][0] > 0.0 && det > 0.0) {
                return None;
            }
            let inv = [
                [r[1][1] / det, -r[0][1] / det],
                [-r[1][0] / det, r[0][0] / det],
            ];
            let mut kk = [
                -(inv[0][0] * qu[0] + inv[0][1] * qu[1]),
                -(inv[1][0] * qu[0] + inv[1][1] * qu[1]),
            ];
            let mut gk = [[0.0; 6]; 2];
            let p_new = uk[1] + kk[1];
            if p_new > lim || p_new < -lim {
                // pitch on its bound: no pitch feedback, lead solved given the pitch step
                kk[1] = p_new.clamp(-lim, lim) - uk[1];
                kk[0] = -(qu[0] + r[0][1] * kk[1]) / r[0][0];
                for j in 0..6 {
                    gk[0][j] = -qux[0][j] / r[0][0];
                }
            } else {
                for i in 0..2 {
                    for j in 0..6 {
                        gk[i][j] = -(inv[i][0] * qux[0][j] + inv[i][1] * qux[1][j]);
                    }
                }
            }
            // the value at k
            let mut nvx: [f64; 6] = std::array::from_fn(|i| qz[i]);
            let mut nvxx: [[f64; 6]; 6] =
                std::array::from_fn(|i| std::array::from_fn(|j| qzz[i][j]));
            for i in 0..6 {
                for a in 0..2 {
                    nvx[i] += gk[a][i] * qu[a] + qux[a][i] * kk[a];
                    for b in 0..2 {
                        nvx[i] += gk[a][i] * quu[a][b] * kk[b];
                    }
                }
                for j in 0..6 {
                    for a in 0..2 {
                        nvxx[i][j] += gk[a][i] * qux[a][j] + qux[a][i] * gk[a][j];
                        for b in 0..2 {
                            nvxx[i][j] += gk[a][i] * quu[a][b] * gk[b][j];
                        }
                    }
                }
            }
            for i in 0..6 {
                for j in 0..i {
                    let s = 0.5 * (nvxx[i][j] + nvxx[j][i]);
                    nvxx[i][j] = s;
                    nvxx[j][i] = s;
                }
            }
            vx = nvx;
            vxx = nvxx;
            dv1 += kk[0] * qu[0] + kk[1] * qu[1];
            for a in 0..2 {
                for b in 0..2 {
                    dv2 += 0.5 * kk[a] * quu[a][b] * kk[b];
                }
            }
            ks[k] = kk;
            gains[k] = gk;
        }
        Some((ks, gains, dv1, dv2))
    }

    /// Fly `u + alpha k + K (x - x_ref)` from `x0`, pitch clamped.
    fn forward(
        &self,
        x: &[X],
        u: &[U],
        ks: &[U],
        gains: &[[[f64; 6]; 2]],
        alpha: f64,
    ) -> (Vec<X>, Vec<U>) {
        let mut nx = Vec::with_capacity(x.len());
        nx.push(self.x0);
        let mut nu = Vec::with_capacity(u.len());
        for k in 0..u.len() {
            let dx: [f64; 6] = std::array::from_fn(|i| nx[k][i] - x[k][i]);
            let mut uk: U = std::array::from_fn(|a| {
                u[k][a] + alpha * ks[k][a] + (0..6).map(|j| gains[k][a][j] * dx[j]).sum::<f64>()
            });
            clamp_pitch(&mut uk);
            nu.push(uk);
            let n = step(&self.kernel, &nx[k], uk).0;
            nx.push(n);
        }
        (nx, nu)
    }

    /// Plane, Gate: cut the horizon at the first tick past the plane, or grow it by holding
    /// the last control up to `cap`. Resizes the multipliers either way.
    pub fn refit(&self, m: &mut Multipliers, u: &mut Vec<U>) -> Vec<X> {
        let mut x = self.rollout(u);
        if let Goal::Plane { cap, .. } | Goal::Gate { cap, .. } = self.goal {
            if let Some(k) = (1..x.len()).find(|&k| self.crossed(&x[k - 1], &x[k])) {
                u.truncate(k);
                x.truncate(k + 1);
            } else {
                while u.len() < cap {
                    let l = *u.last().unwrap();
                    u.push(l);
                    let n = step(&self.kernel, x.last().unwrap(), l).0;
                    let done = self.crossed(x.last().unwrap(), &n);
                    x.push(n);
                    if done {
                        break;
                    }
                }
            }
        }
        m.walls.resize(u.len(), [0.0; NP]);
        x
    }

    /// DDP iterations at one price until a step gains under `tol`: the plan and the
    /// iterations spent.
    pub fn solve_stage(&self, m: &mut Multipliers, u0: &[U], tol: f64) -> (Plan, usize) {
        let mut u = u0.to_vec();
        let mut mu = 1e-6;
        let mut it = 0;
        while it < self.opts.iters {
            it += 1;
            let x = self.refit(m, &mut u);
            let j = self.cost(m, &x, &u);
            let Some((ks, gains, dv1, dv2)) = self.backward(m, &x, &u, mu) else {
                mu = (mu * 10.0).max(1e-6);
                if mu > 1e10 {
                    break;
                }
                continue;
            };
            let mut accepted = None;
            let mut alpha = 1.0;
            for _ in 0..12 {
                let (nx, nu) = self.forward(&x, &u, &ks, &gains, alpha);
                let nj = self.cost(m, &nx, &nu);
                let expected = -(alpha * dv1 + alpha * alpha * dv2);
                if nj < j && (expected <= 0.0 || (j - nj) > 1e-4 * expected) {
                    accepted = Some((nu, nj));
                    break;
                }
                alpha *= 0.5;
            }
            match accepted {
                Some((nu, nj)) => {
                    u = nu;
                    mu = (mu * 0.3).max(1e-9);
                    if j - nj < tol {
                        break;
                    }
                }
                None => {
                    mu *= 10.0;
                    if mu > 1e10 {
                        break;
                    }
                }
            }
        }
        let x = self.refit(m, &mut u);
        (Plan { u, x }, it)
    }

    /// Update the multipliers after a stage.
    fn update(&self, m: &mut Multipliers, plan: &Plan) {
        let n = plan.u.len();
        for k in 0..n {
            for (p, (c, _)) in self.clearances(&plan.x[k], plan.u[k]).iter().enumerate() {
                let l = &mut m.walls[k][p];
                *l = (*l + m.rho * (self.opts.margin - c)).max(0.0);
            }
        }
        if let Goal::Reach { target, scale } = &self.goal {
            let rho = m.rho * self.opts.goal_weight;
            for i in 0..6 {
                m.goal[i] += rho * (plan.x[n][i] - target[i]) / scale[i];
            }
        }
    }

    /// Whether a plan is clear to the margin and meets the goal.
    pub fn feasible(&self, plan: &Plan) -> bool {
        let goal = match &self.goal {
            Goal::Plane { .. } => self.plane(plan.x.last().unwrap()).unwrap().0 > 0.0,
            Goal::Gate { lo, hi, .. } => {
                let n = plan.x.len() - 1;
                let e = &plan.x[n];
                n > 0
                    && self.crossed(&plan.x[n - 1], e)
                    && (0..3).all(|i| e[i] >= lo[i] && e[i] <= hi[i])
            }
            Goal::Reach { .. } => self.value(&plan.x) < self.opts.tol,
        };
        goal && self.worst(&plan.x, &plan.u) >= self.opts.margin - 1e-9
    }

    /// The whole augmented Lagrangian from `m`, controls `u0` as the first guess: the
    /// price grows by `grow` per stage until the plan is feasible and (Plane) its time
    /// stops moving.
    pub fn solve_from(&self, mut m: Multipliers, u0: &[U]) -> Solved {
        let mut u: Vec<U> = u0.to_vec();
        for x in &mut u {
            clamp_pitch(x);
        }
        let mut stages = Vec::new();
        let mut last = f64::INFINITY;
        let mut plan = None;
        for _ in 0..self.opts.stages {
            let (p, it) = self.solve_stage(&mut m, &u, 1e-12);
            let value = self.value(&p.x);
            let worst = self.worst(&p.x, &p.u);
            stages.push(Stage {
                rho: m.rho,
                value,
                worst,
                iters: it,
            });
            let done = self.feasible(&p)
                && match self.goal {
                    Goal::Plane { .. } | Goal::Gate { .. } => (value - last).abs() < 1e-9,
                    Goal::Reach { .. } => true,
                };
            self.update(&mut m, &p);
            u = p.u.clone();
            plan = Some(p);
            if done {
                break;
            }
            last = value;
            m.rho *= self.opts.grow;
        }
        let plan = plan.expect("at least one stage");
        Solved {
            feasible: self.feasible(&plan),
            plan,
            mult: m,
            stages,
        }
    }

    /// [`Problem::solve_from`] with fresh multipliers at `c0`.
    pub fn solve(&self, u0: &[U]) -> Solved {
        self.solve_from(Multipliers::new(self.opts.c0, u0.len()), u0)
    }
}
