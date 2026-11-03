//! The faithful kernel: normal flight with the game's arithmetic (float rotations,
//! `Mth`'s sine table, fdlibm's `Math.cos` for the lift, the float constants widened
//! as the Java widens them) and vanilla collision against a [`World`]. Exact against
//! `port` on normal flight; see the crate README for how that is tested and where it
//! is not.
//!
//! A tick is [`Faithful::step`]: the start checks, [`Faithful::velocity`] (the
//! small-velocity zeroing, then the elytra update), the L-shaped move against the
//! blocks around the swept box, the end checks. What lies outside normal flight is an
//! [`Event`], and the step stops there; wall and ceiling hits are normal flight and
//! come back as a [`Contact`].
//!
//! [`Faithful::step_glide`] is the perfect-ebounce mode (the `glide` module): the same
//! tick with the ground allowed, from a [`crate::state::GlideState`].

mod collide;
mod glide;

pub use glide::GlideTick;

use port::mth;
use world::{BlockPos, World};

use crate::state::{FlightState, Input};

/// `Attributes.GRAVITY`'s default.
pub const GRAVITY: f64 = 0.08;

/// The faithful kernel. It has no settings: it is the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Faithful;

/// The rotation-dependent terms of a tick, computed once per input by
/// [`Faithful::look`], bit for bit as `updateFallFlyingMovement` computes them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// The rotation as `Entity.setYRot`/`setXRot` store it (pitch clamped).
    pub yaw: f32,
    pub pitch: f32,
    /// `getLookAngle()` x and z (table sine and cosine, float products, widened).
    look: [f64; 2],
    /// `lookHorLength`.
    hor: f64,
    /// `lookAngle / lookHorLength`, the third term's direction.
    dir: [f64; 2],
    /// `liftForce`, `square(Math.cos(leanAngle))`.
    lift: f64,
    /// `gravity * (-1 + liftForce * 0.75)`.
    lift_gravity: f64,
    /// `-Mth.sin(leanAngle)` widened, when `leanAngle < 0` (nose up).
    nose_up: Option<f64>,
}

/// Why a tick is not normal flight. The step stops at the first one, in the game's
/// order, and returns no state. Positions are the block that caused it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The box is inside a block: a suffocating block under one of its corners,
    /// which the game pushes the player out of (`LocalPlayer.moveTowardsClosestSpace`),
    /// or a box of a shape whose inner grid the game's collision would clip at (see
    /// the `collide` module). A flight that starts outside every box doesn't get here
    /// in practice.
    InsideBlock(BlockPos),
    /// The block at the position (the start of the tick) is climbable and not
    /// `can_glide_through`: `travelFallFlying` ends the glide this tick.
    Climbable(BlockPos),
    /// Water or lava overlaps the box (deflated by 0.001), at the start or after the
    /// move. A detector, as port's: the game also checks the fluid height.
    Fluid(BlockPos),
    /// Cobweb, berry bush, powder snow between the box before and after the move.
    Stuck(BlockPos),
    /// The move landed on a bouncing block (slime, bed).
    Bounce(BlockPos),
    /// The move landed: the downward move was clipped, so `onGround` is set. The
    /// state it landed in is [`Faithful::step_glide`]'s tick from the same state
    /// (`FlightState::lift_glide`, or the carried `GlideState`), which allows the
    /// ground. why not carry it here? events are small `Copy` values that callers
    /// match on; a landing that matters is a bounce, and that mode steps on.
    Landing,
    /// The tick read a cell the world doesn't cover.
    UnknownSpace(BlockPos),
}

impl From<world::UnknownSpace> for Event {
    fn from(u: world::UnknownSpace) -> Self {
        Event::UnknownSpace(u.0)
    }
}

/// A move the blocks clipped. Normal flight goes on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contact {
    /// Per axis, the move was shorter than the velocity (exact comparison). The y axis
    /// is a ceiling (a floor is [`Event::Landing`]).
    pub clipped: [bool; 3],
    /// The game's `horizontalCollision`: x or z clipped by at least 1e-5
    /// (`Mth.equal`). A smaller clip still moves the box less, but leaves the
    /// velocity and this flag alone.
    pub horizontal_collision: bool,
    /// Fly-into-wall damage, vanilla's `(float)(10 (h_before - h_after) - 3)` when
    /// positive and `horizontal_collision`, with `h_before` the horizontal speed
    /// before the elytra update and `h_after` after the collision. A prediction: the
    /// server judges with its own copy of the physics.
    pub damage: Option<f32>,
}

/// One normal-flight tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    pub state: FlightState<f64>,
    pub contact: Option<Contact>,
}

/// [`Faithful::fly`]'s result.
#[derive(Debug, Clone, PartialEq)]
pub struct Flown {
    /// `ticks[i]` is input `i`'s tick.
    pub ticks: Vec<Tick>,
    /// The event that stopped the flight early, at input `ticks.len()`.
    pub event: Option<Event>,
}

impl Faithful {
    /// The rotation-dependent terms for one input. The input must be finite (the
    /// game ignores a non-finite rotation and keeps the last one, which a flight
    /// state doesn't carry).
    pub fn look(&self, u: &Input<f32>) -> Look {
        debug_assert!(
            u.yaw.is_finite() && u.pitch.is_finite(),
            "non-finite input {u:?}"
        );
        // `Entity.setXRot`: `Math.clamp(xRot % 360.0F, -90.0F, 90.0F)`
        let (yaw, pitch) = (u.yaw, (u.pitch % 360.0).clamp(-90.0, 90.0));
        // `Entity.calculateViewVector`
        let x_rad = pitch * mth::DEG_TO_RAD;
        let y_rad = -yaw * mth::DEG_TO_RAD;
        let (y_cos, y_sin) = (mth::cos(y_rad as f64), mth::sin(y_rad as f64));
        let x_cos = mth::cos(x_rad as f64);
        let look = [(y_sin * x_cos) as f64, (y_cos * x_cos) as f64];
        let hor = (look[0] * look[0] + look[1] * look[1]).sqrt();
        let lift = mth::square(mth::java_cos(x_rad as f64));
        Look {
            yaw,
            pitch,
            look,
            hor,
            dir: [look[0] / hor, look[1] / hor],
            lift,
            lift_gravity: GRAVITY * (-1.0 + lift * 0.75),
            nose_up: (x_rad < 0.0).then(|| (-mth::sin(x_rad as f64)) as f64),
        }
    }

    /// `LivingEntity.aiStep`'s small-velocity zeroing (always on: it is the game),
    /// then `updateFallFlyingMovement`. Returns the new velocity and the horizontal
    /// speed before the update (what wall damage compares against).
    pub fn velocity(&self, v: [f64; 3], k: &Look) -> ([f64; 3], f64) {
        self.elytra(self.zero_small(v), k)
    }

    /// `LivingEntity.aiStep`'s small-velocity zeroing.
    pub fn zero_small(&self, v: [f64; 3]) -> [f64; 3] {
        let [mut x, mut y, mut z] = v;
        if x * x + z * z < 9.0E-6 {
            x = 0.0;
            z = 0.0;
        }
        if y.abs() < 0.003 {
            y = 0.0;
        }
        [x, y, z]
    }

    /// `updateFallFlyingMovement`, and the horizontal speed before it.
    pub fn elytra(&self, v: [f64; 3], k: &Look) -> ([f64; 3], f64) {
        let [mut x, mut y, mut z] = v;
        let move_hor = (x * x + z * z).sqrt();
        // The adds of 0.0 are the Java's `add(0, dy, 0)`: they turn -0.0 into 0.0.
        x += 0.0;
        y += k.lift_gravity;
        z += 0.0;
        let aero = k.hor > 0.0;
        if y < 0.0 && aero {
            let convert = y * -0.1 * k.lift;
            x += k.look[0] * convert / k.hor;
            y += convert;
            z += k.look[1] * convert / k.hor;
        }
        if let Some(s) = k.nose_up
            && aero
        {
            let convert = move_hor * s * 0.04;
            x += -k.look[0] * convert / k.hor;
            y += convert * 3.2;
            z += -k.look[1] * convert / k.hor;
        }
        if aero {
            x += (k.dir[0] * move_hor - x) * 0.1;
            y += 0.0;
            z += (k.dir[1] * move_hor - z) * 0.1;
        }
        (
            [x * 0.99f32 as f64, y * 0.98f32 as f64, z * 0.99f32 as f64],
            move_hor,
        )
    }

    /// One tick of normal flight from `s` with the rotation terms `k`.
    pub fn step(&self, world: &World, s: &FlightState<f64>, k: &Look) -> Result<Tick, Event> {
        let pos = s.pos;
        let bb = collide::player_box(pos);
        // `Entity.baseTick` (fluid), `travelFallFlying` (climbable)
        collide::find_fluid(world, &collide::deflate(&bb, 0.001))?;
        collide::check_climbable(world, pos)?;
        let (v, h_before) = self.velocity(s.vel, k);
        // `Entity.move`
        let movement = collide::collide(world, pos, &bb, v)?;
        let len = movement[0] * movement[0] + movement[1] * movement[1] + movement[2] * movement[2];
        let mut new_pos = pos;
        if len > 1.0E-7 || (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) - len < 1.0E-7 {
            new_pos = [
                pos[0] + movement[0],
                pos[1] + movement[1],
                pos[2] + movement[2],
            ];
        }
        let x_coll = !mth::equal(v[0], movement[0]);
        let z_coll = !mth::equal(v[2], movement[2]);
        let horizontal = x_coll || z_coll;
        let vertical = v[1] != movement[1];
        let landing = vertical && v[1] < 0.0;
        if landing && (v[0] != movement[0] || v[2] != movement[2]) {
            // the game tries a step-up here, and the checks below would see the
            // unstepped position
            return Err(Event::Landing);
        }
        let new_bb = collide::player_box(new_pos);
        // `getOnPosLegacy()`, read for its block. In flight there is no supporting
        // block, so it is the block 0.2 under the position; a landing sets one.
        let effect_pos = if landing {
            collide::landing_on_pos(world, new_pos, &new_bb, movement)?
        } else {
            BlockPos {
                x: mth::floor(new_pos[0]),
                y: mth::floor(new_pos[1] - 0.2f32 as f64),
                z: mth::floor(new_pos[2]),
            }
        };
        let effect = collide::state_at(world, effect_pos)?;
        collide::find_fluid(world, &collide::deflate(&new_bb, 0.001))?;
        // `restituteMovementAfterCollisions`: bounciness 0, so the clipped axes'
        // velocity becomes `-v * 0.0`, keeping the Java's signed zeros.
        let mut vel = v;
        if v[1].abs() > 0.0 && vertical || horizontal {
            if x_coll {
                vel[0] = -v[0] * 0.0;
            }
            if z_coll {
                vel[2] = -v[2] * 0.0;
            }
            if vertical {
                vel[1] = (0.0 - v[1]) * 1.0 * 0.0;
            }
        }
        // a bouncing block under a landing: the restitution (falling at least at
        // gravity) or `stepOn` (any landing) bounces
        if landing && world.detail(effect).own_effect == world::Effect::Bounce {
            return Err(Event::Bounce(effect_pos));
        }
        collide::find_stuck(world, &bb, &new_bb)?;
        if landing {
            return Err(Event::Landing);
        }
        // `Player.tick`'s world-edge clamp
        new_pos[0] = mth::clamp(new_pos[0], -2.9999999E7, 2.9999999E7);
        new_pos[2] = mth::clamp(new_pos[2], -2.9999999E7, 2.9999999E7);
        let clipped = [v[0] != movement[0], vertical, v[2] != movement[2]];
        let contact = clipped.iter().any(|&c| c).then(|| {
            let h_after = (vel[0] * vel[0] + vel[2] * vel[2]).sqrt();
            let dmg = ((h_before - h_after) * 10.0 - 3.0) as f32;
            Contact {
                clipped,
                horizontal_collision: horizontal,
                damage: (horizontal && dmg > 0.0).then_some(dmg),
            }
        });
        Ok(Tick {
            state: FlightState { pos: new_pos, vel },
            contact,
        })
    }

    /// [`Faithful::step`] with the rotation terms computed here.
    pub fn step_input(
        &self,
        world: &World,
        s: &FlightState<f64>,
        u: &Input<f32>,
    ) -> Result<Tick, Event> {
        self.step(world, s, &self.look(u))
    }

    /// Fly a schedule of inputs from `s0` until it ends or an event stops it.
    pub fn fly(&self, world: &World, s0: &FlightState<f64>, inputs: &[Input<f32>]) -> Flown {
        let mut ticks = Vec::with_capacity(inputs.len());
        let mut s = *s0;
        for u in inputs {
            match self.step_input(world, &s, u) {
                Ok(t) => {
                    s = t.state;
                    ticks.push(t);
                }
                Err(e) => {
                    return Flown {
                        ticks,
                        event: Some(e),
                    };
                }
            }
        }
        Flown { ticks, event: None }
    }
}
