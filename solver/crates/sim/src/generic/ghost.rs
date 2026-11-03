//! Geometry for the `Ghost` collision model: the move ignores walls, and a constrained
//! optimizer keeps it out of them through the signed distance of each point it visits.
//!
//! Distances are between the gliding player's box and world boxes, over [`Scalar`] so
//! they differentiate along with the kernel. Constraining the points of [`l_move`]
//! keeps its corners clear, not its segments: a move longer than a box is thin can
//! pass through it with every point outside.

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
