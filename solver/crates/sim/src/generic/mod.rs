//! The generic kernel: elytra flight as plain math over [`Scalar`], with no casts and
//! no sine table. On `f64` it is the precise physics (more accurate than the game, not
//! a copy of it); on dual numbers it gives exact derivatives of that; on `f32` it is a
//! fast approximation. The faithful kernel is the one that copies the game's rounding.
//!
//! A tick is [`Generic::velocity`] then a move. Only the contact-free (`Ghost`) move is
//! here, [`Generic::step`]; [`ghost`] has the geometry a constrained optimizer needs to
//! keep that move out of walls.
//!
//! Non-smooth points, where derivatives are one-sided or undefined:
//! - `v_y = 0` after gravity (the lift-to-speed conversion switches on): a kink.
//! - pitch 0 (the nose-up conversion switches on): a kink.
//! - pitch -90: a cliff. The game's `lookHorLength` is 0 there, which skips the
//!   conversion, nose-up and turning terms, a dead tick. At +90 the game's table cos
//!   is 9.6e-5, so the terms run, and in the limit they are continuous; this kernel
//!   does the same (it gates on `pitch > -90`). The game's dead band also reaches
//!   ~0.0055 deg above -90 (one table cell), which this kernel doesn't model; a
//!   solver stays in [`PITCH_MIN`], [`PITCH_MAX`], where the two agree.
//! - zero horizontal speed (`sqrt`): the derivative of the speed is taken as 0 there.
//! - the small-velocity zeroing, when on (see [`Generic::zero_small_velocity`]).
//! - the axis order of the L-shaped move, see [`ghost::l_move`].

pub mod ghost;
mod scalar;

pub use scalar::Scalar;
/// The dual-number crate the kernel's derivatives come from, so a solver names the
/// same version.
pub use num_dual;

use crate::state::{FlightState, Input};

/// `Attributes.GRAVITY`'s default. Slow falling isn't modeled.
pub const GRAVITY: f64 = 0.08;
/// The drag factors, `0.99F` and `0.98F` widened to double as the game does. why the
/// float values? they are the game's constants; the kernel drops the game's rounding
/// of intermediate results, not its constants.
pub const DRAG_XZ: f64 = 0.99_f32 as f64;
pub const DRAG_Y: f64 = 0.98_f32 as f64;

/// `LivingEntity.aiStep`'s thresholds for a player: `|v_y| < 0.003` zeroes `v_y`,
/// `v_x² + v_z² < 9e-6` zeroes both horizontal components.
pub const ZERO_Y: f64 = 0.003;
pub const ZERO_XZ_SQR: f64 = 9.0e-6;

/// The ends of the range where flight is continuous, degrees. why not -90? the game's
/// table cos truncates to cell 0 from -90 up to here, a dead tick. +90 itself is fine:
/// nothing changes there, and `lift` reaches 0.
pub const PITCH_MIN: f64 = -89.9945;
pub const PITCH_MAX: f64 = 90.0;

/// The generic kernel. A value, not global state: two differently configured kernels
/// can run side by side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Generic {
    pub hooks: Hooks,
    /// Apply `LivingEntity.aiStep`'s small-velocity zeroing before the elytra update
    /// (see [`ZERO_Y`]). On by default. why on? every pull-up apex goes through it
    /// (Burrowed Grove ticks 555-559), and without it the kernel flies a different
    /// apex. Its derivative is zero for a component inside the band and undefined at
    /// the threshold, so a gradient method near an apex may want it off and treat the
    /// difference as a gap.
    pub zero_small_velocity: bool,
}

impl Default for Generic {
    fn default() -> Self {
        Generic {
            hooks: Hooks::default(),
            zero_small_velocity: true,
        }
    }
}

/// Ablations: named sites where the kernel departs from the game on purpose. The
/// default is the game. A new ablation is a new field here, not a flag threaded
/// through the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Hooks {
    pub lift_shape: LiftShape,
}

/// The lift factor in the lift-to-speed conversion, `convert = v_y * -0.1 * lift`. The
/// gravity relief `g * (-1 + 0.75 lift)` keeps the plain `lift`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LiftShape {
    #[default]
    Vanilla,
    /// `lift.powf(k)` at the conversion. Both ends are held (lift 0 and 1). k < 1
    /// converts more at interior pitches, k > 1 less. elytrasim's pumpdp
    /// (`--conv-lift-exp`) found the optimum stops chattering at k = 0.99.
    Pow(f64),
}

/// The rotation-dependent terms of a tick, computed once per input by
/// [`Generic::look`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look<T> {
    /// Unit horizontal look direction, x and z: `(-sin yaw, cos yaw)`. The game's
    /// `lookAngle.x / lookHorLength`, exact here because `lookHorLength = cos pitch`.
    dir: [T; 2],
    /// `cos² pitch`, the game's `liftForce`.
    lift: T,
    /// `lift` at the conversion site, after [`LiftShape`].
    conv_lift: T,
    /// `-sin pitch` when pitch < 0 (nose up), else none.
    nose_up: Option<T>,
    /// `pitch > -90`: the game's `lookHorLength > 0`.
    aero: bool,
}

impl Generic {
    /// The rotation-dependent terms for one input.
    pub fn look<T: Scalar>(&self, u: &Input<T>) -> Look<T> {
        let d2r = T::cst(std::f64::consts::PI / 180.0);
        let (sy, cy) = (u.yaw * d2r).sin_cos();
        let (sp, cp) = (u.pitch * d2r).sin_cos();
        let lift = cp * cp;
        let conv_lift = match self.hooks.lift_shape {
            LiftShape::Vanilla => lift,
            LiftShape::Pow(k) => lift.powf(k),
        };
        Look {
            dir: [-sy, cy],
            lift,
            conv_lift,
            nose_up: (u.pitch.re() < 0.0).then_some(-sp),
            aero: u.pitch.re() > -90.0,
        }
    }

    /// `LivingEntity.aiStep`'s zeroing, if [`Generic::zero_small_velocity`] is on.
    /// A zeroed component is a constant 0, with zero derivative.
    pub fn zero_small<T: Scalar>(&self, v: [T; 3]) -> [T; 3] {
        if !self.zero_small_velocity {
            return v;
        }
        let [mut x, mut y, mut z] = v;
        if (x * x + z * z).re() < ZERO_XZ_SQR {
            x = T::cst(0.0);
            z = T::cst(0.0);
        }
        if y.re().abs() < ZERO_Y {
            y = T::cst(0.0);
        }
        [x, y, z]
    }

    /// The velocity a gliding tick moves by: [`Generic::zero_small`], then
    /// `LivingEntity.updateFallFlyingMovement`.
    pub fn velocity<T: Scalar>(&self, v: [T; 3], look: &Look<T>) -> [T; 3] {
        let [mut x, mut y, mut z] = self.zero_small(v);
        let c = T::cst;
        let hor_sqr = x * x + z * z;
        // why the guard? d sqrt(0) is infinite, and a zeroed velocity would carry NaN.
        let move_hor = if hor_sqr.re() > 0.0 {
            hor_sqr.sqrt()
        } else {
            c(0.0)
        };
        let [dx, dz] = look.dir;
        y = y + c(GRAVITY) * (c(-1.0) + look.lift * c(0.75));
        if look.aero {
            if y.re() < 0.0 {
                let k = y * c(-0.1) * look.conv_lift;
                x = x + dx * k;
                y = y + k;
                z = z + dz * k;
            }
            if let Some(s) = look.nose_up {
                let k = move_hor * s * c(0.04);
                x = x - dx * k;
                y = y + k * c(3.2);
                z = z - dz * k;
            }
            x = x + (dx * move_hor - x) * c(0.1);
            z = z + (dz * move_hor - z) * c(0.1);
        }
        [x * c(DRAG_XZ), y * c(DRAG_Y), z * c(DRAG_XZ)]
    }

    /// One contact-free tick: the velocity update, then `pos += vel`.
    pub fn step<T: Scalar>(&self, s: &FlightState<T>, u: &Input<T>) -> FlightState<T> {
        let vel = self.velocity(s.vel, &self.look(u));
        FlightState {
            pos: [s.pos[0] + vel[0], s.pos[1] + vel[1], s.pos[2] + vel[2]],
            vel,
        }
    }

    /// Fly a schedule of inputs contact-free from `s0`. Returns `inputs.len() + 1`
    /// states: `s0`, then the state after each tick.
    pub fn fly<T: Scalar>(&self, s0: &FlightState<T>, inputs: &[Input<T>]) -> Vec<FlightState<T>> {
        let mut out = Vec::with_capacity(inputs.len() + 1);
        out.push(*s0);
        for u in inputs {
            let next = self.step(out.last().unwrap(), u);
            out.push(next);
        }
        out
    }
}
