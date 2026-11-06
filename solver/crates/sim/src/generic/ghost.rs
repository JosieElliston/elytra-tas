//! Geometry for the `Ghost` collision model: the move ignores walls, and a constrained
//! optimizer keeps it out of them through the signed distance of each point it visits.
//!
//! Distances are between the gliding player's box and world boxes, over [`Scalar`] so
//! they differentiate along with the kernel. Constraining the points of [`l_move`]
//! keeps its corners clear, not its segments: a move longer than a box is thin can
//! pass through it with every point outside. [`leg_sdf`] measures a whole leg.

use super::Scalar;
use super::scalar::max;
use world::Aabb;

/// Half the gliding box's width, `0.6F / 2.0F` (`Avatar.POSES` `FALL_FLYING`,
/// `EntityDimensions.makeBoundingBox`), widened as the game does.
pub const HALF_WIDTH: f64 = 0.3_f32 as f64;
/// The gliding box's height, `0.6F`.
pub const HEIGHT: f64 = 0.6_f32 as f64;

/// Signed distance from the gliding player's box at `pos` (feet) to `b`: the Euclidean
/// gap when apart, 0 when touching, minus the shallowest axis' penetration when
/// overlapping.
///
/// Smoothness: C¹ outside and across faces, with curvature that jumps (0 beside a face,
/// `1/d` beside an edge or corner) where the nearest feature changes. The gradient
/// jumps at zero distance on an edge or corner, and inside along the planes where the
/// shallowest axis changes.
pub fn box_sdf<T: Scalar>(pos: [T; 3], b: &Aabb) -> T {
    let lo = [-HALF_WIDTH, 0.0, -HALF_WIDTH];
    let hi = [HALF_WIDTH, HEIGHT, HALF_WIDTH];
    // gap[i] > 0: apart along axis i by that much; < 0: the intervals overlap by -gap.
    let gap: [T; 3] = std::array::from_fn(|i| {
        max(
            T::cst(b.min[i] - hi[i]) - pos[i],
            pos[i] + T::cst(lo[i] - b.max[i]),
        )
    });
    let mut outside: Option<T> = None;
    let mut sqr = T::cst(0.0);
    let mut n_pos = 0;
    for g in gap {
        if g.re() > 0.0 {
            outside = Some(g);
            sqr = sqr + g * g;
            n_pos += 1;
        }
    }
    match n_pos {
        0 => max(max(gap[0], gap[1]), gap[2]),
        // why not sqrt(g²)? the same value, but exact, and no sqrt near 0.
        1 => outside.unwrap(),
        _ => sqr.sqrt(),
    }
}

/// Signed distance, as [`box_sdf`] measures it, from `b` to the gliding box swept from
/// feet position `from` to `to`. Each leg of [`l_move`] moves along one axis, and the
/// sweep of a box along an axis is a box, so for a leg this is exact: the leg clears
/// `b` exactly when it is positive, where checking the leg's points can miss a box
/// thinner than the leg. For a segment off the axes it is the distance to the sweep's
/// bounding box, a lower bound. `leg_sdf(p, p, b) == box_sdf(p, b)`.
///
/// Smoothness: as [`box_sdf`]'s, plus a kink in a moving coordinate where the leg
/// changes direction (`from[i] == to[i]`).
pub fn leg_sdf<T: Scalar>(from: [T; 3], to: [T; 3], b: &Aabb) -> T {
    let lo = [-HALF_WIDTH, 0.0, -HALF_WIDTH];
    let hi = [HALF_WIDTH, HEIGHT, HALF_WIDTH];
    let gap: [T; 3] = std::array::from_fn(|i| {
        let (a, z) = if from[i].re() <= to[i].re() {
            (from[i], to[i])
        } else {
            (to[i], from[i])
        };
        max(T::cst(b.min[i] - hi[i]) - z, a + T::cst(lo[i] - b.max[i]))
    });
    let mut outside: Option<T> = None;
    let mut sqr = T::cst(0.0);
    let mut n_pos = 0;
    for g in gap {
        if g.re() > 0.0 {
            outside = Some(g);
            sqr = sqr + g * g;
            n_pos += 1;
        }
    }
    match n_pos {
        0 => max(max(gap[0], gap[1]), gap[2]),
        1 => outside.unwrap(),
        _ => sqr.sqrt(),
    }
}

/// The least [`box_sdf`] over the segment from `from` to `to`: the signed distance from `b`
/// to the gliding box swept along any segment, on or off the axes. Exact where [`leg_sdf`]
/// is only a lower bound. On an axis it is [`leg_sdf`], whose value inside a box is the
/// sweep's depth rather than the deepest point's. Apart, the segment's parameter is searched
/// on the values and held constant for the derivatives. why is that right? at a minimum over
/// the segment the value doesn't change to first order as the parameter moves. Overlapping,
/// that minimum sits on a kink and the argument fails, so the depth is [`overlap_depth`]'s.
pub fn segment_sdf<T: Scalar>(from: [T; 3], to: [T; 3], b: &Aabb) -> T {
    if (0..3).filter(|&i| from[i].re() != to[i].re()).count() < 2 {
        return leg_sdf(from, to, b);
    }
    let (p, q) = (from.map(|x| x.re()), to.map(|x| x.re()));
    let at = |t: f64| box_sdf::<f64>(std::array::from_fn(|i| p[i] + (q[i] - p[i]) * t), b);
    // box_sdf is convex in the position and the segment is affine, so this is unimodal.
    const R: f64 = 0.618_033_988_749_894_9;
    let (mut lo, mut hi) = (0.0, 1.0);
    let (mut c, mut d) = (hi - R * (hi - lo), lo + R * (hi - lo));
    let (mut fc, mut fd) = (at(c), at(d));
    for _ in 0..40 {
        if fc <= fd {
            (hi, d, fd) = (d, c, fc);
            c = hi - R * (hi - lo);
            fc = at(c);
        } else {
            (lo, c, fc) = (c, d, fd);
            d = lo + R * (hi - lo);
            fd = at(d);
        }
    }
    let inner = if fc <= fd { c } else { d };
    let t = [(0.0, at(0.0)), (1.0, at(1.0)), (inner, fc.min(fd))]
        .into_iter()
        .fold((0.0, f64::INFINITY), |a, b| if b.1 < a.1 { b } else { a })
        .0;
    if at(t) < 0.0 {
        return overlap_depth(from, to, b);
    }
    let t = T::cst(t);
    box_sdf(std::array::from_fn(|i| from[i] + (to[i] - from[i]) * t), b)
}

/// How far the segment must move to clear `b`, negative, by separating axes: the box's own
/// and each one across the segment and a box axis. Meets [`box_sdf`]'s 0 at touching.
fn overlap_depth<T: Scalar>(from: [T; 3], to: [T; 3], b: &Aabb) -> T {
    let lo = [-HALF_WIDTH, 0.0, -HALF_WIDTH];
    let hi = [HALF_WIDTH, HEIGHT, HALF_WIDTH];
    let mut best = None::<T>;
    let mut take = |g: T| best = Some(best.map_or(g, |x| max(x, g)));
    for i in 0..3 {
        let (a, z) = if from[i].re() <= to[i].re() { (from[i], to[i]) } else { (to[i], from[i]) };
        take(max(T::cst(b.min[i] - hi[i]) - z, a + T::cst(lo[i] - b.max[i])));
    }
    let d: [T; 3] = std::array::from_fn(|i| to[i] - from[i]);
    // the box the feet can't enter
    let c: [f64; 3] = std::array::from_fn(|i| 0.5 * (b.min[i] - hi[i] + b.max[i] - lo[i]));
    let h: [f64; 3] = std::array::from_fn(|i| 0.5 * (b.max[i] - lo[i] - b.min[i] + hi[i]));
    for i in 0..3 {
        let (j, k) = ((i + 1) % 3, (i + 2) % 3);
        // d x e_i has no i part; the segment is a point along it
        let (nj, nk) = (d[k], T::cst(0.0) - d[j]);
        let sq = nj.re() * nj.re() + nk.re() * nk.re();
        if sq < 1e-18 {
            continue;
        }
        let len = (nj * nj + nk * nk).sqrt();
        let (nj, nk) = (nj / len, nk / len);
        let along = nj * (from[j] - T::cst(c[j])) + nk * (from[k] - T::cst(c[k]));
        let abs = |x: T| if x.re() < 0.0 { T::cst(0.0) - x } else { x };
        take(abs(along) - abs(nj) * T::cst(h[j]) - abs(nk) * T::cst(h[k]));
    }
    best.expect("the box's own axes")
}

/// The least [`box_sdf`] over `boxes`, or none for no boxes. Kinks where the nearest
/// box changes; constrain each box with [`box_sdf`] instead where that matters.
pub fn sdf<T: Scalar>(pos: [T; 3], boxes: &[Aabb]) -> Option<T> {
    boxes
        .iter()
        .map(|b| box_sdf(pos, b))
        .reduce(|a, d| if d.re() < a.re() { d } else { a })
}

/// The points `Entity.collide` visits moving by `vel` from `pos`, in order: after the
/// y leg, after the first horizontal leg, and the end. The first horizontal leg is the
/// larger of |x| and |z|, x on a tie (`Direction.axisStepOrder`). The order switches
/// at |x| = |z|, where the middle point jumps.
pub fn l_move<T: Scalar>(pos: [T; 3], vel: [T; 3]) -> [[T; 3]; 3] {
    let a = [pos[0], pos[1] + vel[1], pos[2]];
    let b = if vel[0].re().abs() < vel[2].re().abs() {
        [a[0], a[1], a[2] + vel[2]]
    } else {
        [a[0] + vel[0], a[1], a[2]]
    };
    [a, b, [a[0] + vel[0], a[1], a[2] + vel[2]]]
}

/// [`l_move`] with its corners slid toward the straight move, for a solve under a relaxed
/// collision model: `s = [s_xz, s_y]`, each 0 for the L and 1 for straight. `[1, 0]` keeps
/// the y leg and goes straight across xz; `[1, 1]` is straight on all three axes, the corners
/// at thirds of the move. Between them the path is continuous in `s`, and at `s_xz = 1` it
/// doesn't jump where [`l_move`]'s order switches. The legs are off the axes: measure them
/// with [`segment_sdf`]. Not a path the game takes unless `s` is 0.
pub fn relaxed_move<T: Scalar>(pos: [T; 3], vel: [T; 3], s: [f64; 2]) -> [[T; 3]; 3] {
    let [a, b, e] = l_move(pos, vel);
    let mix = |x: T, y: T, w: f64| x + (y - x) * T::cst(w);
    let third = T::cst(1.0 / 3.0);
    let a2: [T; 3] = std::array::from_fn(|i| mix(a[i], pos[i] + vel[i] * third, s[1]));
    let half = T::cst(0.5);
    let b2 = std::array::from_fn(|i| mix(b[i], a2[i] + (e[i] - a2[i]) * half, s[0]));
    [a2, b2, e]
}
