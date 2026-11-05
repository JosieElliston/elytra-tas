//! The perfect-ebounce mode: a gliding tick that may touch the ground. The client never
//! ends a glide (only the server's entity data does), so after a landing it keeps
//! gliding: on the ground the elytra update runs and the move clips at the floor
//! again, and a held jump key jumps off it (`LivingEntity.jumpFromGround`, with the
//! sprint boost) before the elytra update. A bounce the server never undeploys flies
//! exactly this, and so does one whose glide-off the client answers with a jump press
//! on the tick it arrives (`PLAN-FORMAT.md`).
//!
//! The tick is port's `LocalPlayer.tick` restricted to a glide: `LocalPlayer.aiStep`'s
//! sprint stop, `LivingEntity.aiStep`'s jump delay, zeroing and jump, then
//! `travelFallFlying` and `Entity.move` with step-up, the supporting block, the
//! collision flags and the block effects. Sneak is not modeled: the keys must not
//! include it (it backs off ledges on the ground).

// `!(a < b)` is not `a >= b` for NaN; the Java's comparisons are kept.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use port::mth;
use world::{BlockPos, Effect, World};

use super::collide::{self, player_box};
use super::{Contact, Event, Faithful, GRAVITY, Look};
use crate::state::{GlideState, Keys};

/// `Attributes.JUMP_STRENGTH` default, `0.42F`.
const JUMP_STRENGTH: f32 = 0.42;

/// One [`Faithful::step_glide`] tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlideTick {
    pub state: GlideState,
    /// A block clipped the move. On the ground the floor clips every tick; that is
    /// not a contact unless a wall or ceiling also did. Damage is as in
    /// [`Contact::damage`].
    pub contact: Option<Contact>,
    /// The move ended on the ground from the air: the landing tick.
    pub landed: bool,
    /// The tick jumped off the ground (`jumpFromGround`).
    pub jumped: bool,
}

/// `ClientInput.moveVector` as `KeyboardInput.tick()` sets it.
pub(super) fn move_vector(k: Keys) -> [f32; 2] {
    let imp = |p: bool, n: bool| {
        if p == n {
            0.0f32
        } else if p {
            1.0
        } else {
            -1.0
        }
    };
    let (x, y) = (imp(k.left, k.right), imp(k.forward, k.backward));
    // `Vec2.normalized()`
    let len = mth::sqrt(x * x + y * y);
    if len < 1.0E-4 {
        [0.0, 0.0]
    } else {
        [x / len, y / len]
    }
}

/// `LocalPlayer.modifyInput` (no item in use), the applied `xxa`, `zza`.
pub(super) fn modify_input(v: [f32; 2], slowly: bool) -> [f32; 2] {
    if v[0] * v[0] + v[1] * v[1] == 0.0 {
        return v;
    }
    let mut n = [v[0] * 0.98, v[1] * 0.98];
    if slowly {
        let f = 0.3f64 as f32;
        n = [n[0] * f, n[1] * f];
    }
    // `modifyInputSpeedForSquareMovement`
    let length = mth::sqrt(n[0] * n[0] + n[1] * n[1]);
    if length <= 0.0 {
        return n;
    }
    let d = [n[0] * (1.0 / length), n[1] * (1.0 / length)];
    let (ax, ay) = (d[0].abs(), d[1].abs());
    let tan = if ay > ax { ax / ay } else { ay / ax };
    let to_square = mth::sqrt(1.0 + tan * tan);
    let m = mth::java_min_f32(length * to_square, 1.0);
    [d[0] * m, d[1] * m]
}

/// `LocalPlayer.isHorizontalCollisionMinor`: the move is within 8 degrees of the
/// applied input's direction.
pub(super) fn collision_minor(yaw: f32, input: [f32; 2], movement: [f64; 3]) -> bool {
    let r = yaw * mth::DEG_TO_RAD;
    let (s, c) = (mth::sin(r as f64) as f64, mth::cos(r as f64) as f64);
    let (xa, za) = (input[0] as f64, input[1] as f64);
    let gx = xa * c - za * s;
    let gz = za * c + xa * s;
    let a = mth::square(gx) + mth::square(gz);
    let m = mth::square(movement[0]) + mth::square(movement[2]);
    if !(a < 1.0E-5f32 as f64) && !(m < 1.0E-5f32 as f64) {
        let dot = gx * movement[0] + gz * movement[2];
        (dot / (a * m).sqrt()).acos() < 0.13962634f32 as f64
    } else {
        false
    }
}

/// The box of a pose `height` tall at `pos`, deflated by 1e-7, as
/// `canPlayerFitWithinBlocksAndEntitiesWhen` tests it.
fn pose_box(pos: [f64; 3], height: f32) -> world::Aabb {
    let w = (0.6f32 / 2.0) as f64;
    let b = world::Aabb {
        min: [pos[0] - w, pos[1], pos[2] - w],
        max: [pos[0] + w, pos[1] + height as f64, pos[2] + w],
    };
    collide::deflate(&b, 1.0E-7)
}

impl Faithful {
    /// One gliding tick from `s`, ground allowed: in the air it is [`Faithful::step`]
    /// (same arithmetic, same events), with the bookkeeping a later bounce reads; a
    /// landing is not an event but a tick with `landed` set; on the ground the glide
    /// slides on, and jumps when `keys.jump` is held and the jump delay is out.
    /// `keys` are the tick's (`KeyboardInput`'s); sneak is not modeled.
    pub fn step_glide(
        &self,
        world: &World,
        s: &GlideState,
        k: &Look,
        keys: Keys,
    ) -> Result<GlideTick, Event> {
        debug_assert!(!keys.sneak, "sneak is not modeled while gliding");
        let pos = s.pos;
        let bb = player_box(pos);
        // `Entity.baseTick`
        collide::find_fluid(world, &collide::deflate(&bb, 0.001))?;
        // `LocalPlayer.aiStep`: the input, then sprint can only stop while gliding
        let mv = move_vector(keys);
        let forward = mv[1] > 1.0E-5;
        let mut sprinting = s.sprinting;
        if sprinting && (!forward || s.horizontal_collision && !s.minor_horizontal_collision) {
            sprinting = false;
        }
        // (the jump edge's `tryToStartFallFlying` does nothing while gliding; its
        // climbable check is the one `travelFallFlying` makes below)
        // `LivingEntity.aiStep`
        let mut delay = s.no_jump_delay;
        if delay > 0 {
            delay -= 1;
        }
        let mut v = self.zero_small(s.vel);
        let mut jumped = false;
        if keys.jump {
            if s.on_ground && delay == 0 {
                v = jump(world, s, v, k.yaw, sprinting)?;
                jumped = true;
                delay = 10;
            }
        } else {
            delay = 0;
        }
        // `travelFallFlying`
        collide::check_climbable(world, pos)?;
        let (v, h_before) = self.elytra(v, k);
        // `Entity.move`
        let movement = collide::collide_step(world, pos, &bb, v, s.on_ground)?;
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
        let on_ground = vertical && v[1] < 0.0;
        let new_bb = player_box(new_pos);
        let (supporting_block, on_ground_no_blocks) = if on_ground {
            collide::check_supporting_block(
                world,
                new_pos,
                &new_bb,
                movement,
                s.on_ground_no_blocks,
            )?
        } else {
            (None, false)
        };
        let minor = horizontal && {
            // `isMovingSlowly`: crouching, from the start of the tick (`aiStep`
            // computes it before the input); not crawling while gliding
            let crouching = collide::no_collision(world, &pose_box(pos, 1.5))?
                && !collide::no_collision(world, &pose_box(pos, 1.8))?;
            collision_minor(k.yaw, modify_input(mv, crouching), movement)
        };
        let effect_pos = collide::on_pos(world, new_pos, supporting_block, 0.2)?;
        let effect = collide::state_at(world, effect_pos)?;
        // `checkFallDamage` (fluid), then `restituteMovementAfterCollisions`:
        // bounciness 0 zeroes the clipped axes as `-v * 0.0`
        collide::find_fluid(world, &collide::deflate(&new_bb, 0.001))?;
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
        // a bouncing block under the feet: the restitution (falling at least at
        // gravity) or `stepOn` (any tick on the ground) bounces
        if on_ground && world.detail(effect).own_effect == Effect::Bounce {
            return Err(Event::Bounce(effect_pos));
        }
        // `applyEffectsFromBlocks`
        collide::find_stuck(world, &bb, &new_bb)?;
        // `Player.tick`'s world-edge clamp
        new_pos[0] = mth::clamp(new_pos[0], -2.9999999E7, 2.9999999E7);
        new_pos[2] = mth::clamp(new_pos[2], -2.9999999E7, 2.9999999E7);
        // the floor under a ground tick is not a contact
        let clipped = [
            v[0] != movement[0],
            vertical && !on_ground,
            v[2] != movement[2],
        ];
        let contact = clipped.iter().any(|&c| c).then(|| {
            let h_after = (vel[0] * vel[0] + vel[2] * vel[2]).sqrt();
            let dmg = ((h_before - h_after) * 10.0 - 3.0) as f32;
            Contact {
                clipped,
                horizontal_collision: horizontal,
                damage: (horizontal && dmg > 0.0).then_some(dmg),
            }
        });
        Ok(GlideTick {
            state: GlideState {
                pos: new_pos,
                vel,
                on_ground,
                horizontal_collision: horizontal,
                minor_horizontal_collision: minor,
                supporting_block,
                on_ground_no_blocks,
                sprinting,
                no_jump_delay: delay,
                keys,
            },
            contact,
            landed: on_ground && !s.on_ground,
            jumped,
        })
    }
}

/// `LivingEntity.jumpFromGround` from velocity `v` (after the zeroing): `v_y` at least
/// the jump power, and the sprint boost along the yaw.
fn jump(
    world: &World,
    s: &GlideState,
    v: [f64; 3],
    yaw: f32,
    sprinting: bool,
) -> Result<[f64; 3], Event> {
    let power = jump_power(world, s.pos, s.supporting_block)?;
    if power <= 1.0E-5 {
        return Ok(v);
    }
    let mut v = [v[0], mth::java_max(power as f64, v[1]), v[2]];
    if sprinting {
        let angle = yaw * mth::DEG_TO_RAD;
        v = [
            v[0] + -mth::sin(angle as f64) as f64 * 0.2,
            v[1] + 0.0,
            v[2] + mth::cos(angle as f64) as f64 * 0.2,
        ];
    }
    Ok(v)
}

/// `LivingEntity.getJumpPower()`: the jump strength times `getBlockJumpFactor` (the
/// block at the position, else the one under it).
pub(super) fn jump_power(
    world: &World,
    pos: [f64; 3],
    supporting_block: Option<BlockPos>,
) -> Result<f32, Event> {
    let here = BlockPos {
        x: mth::floor(pos[0]),
        y: mth::floor(pos[1]),
        z: mth::floor(pos[2]),
    };
    let mut factor = world.info(collide::state_at(world, here)?).jump_factor;
    let below = collide::on_pos(world, pos, supporting_block, 0.500001)?;
    let below = world.info(collide::state_at(world, below)?).jump_factor;
    if factor as f64 == 1.0 {
        factor = below;
    }
    Ok(JUMP_STRENGTH * 1.0 * factor + 0.0)
}

/// Gravity is the game's default here as in the rest of the kernel.
const _: () = assert!(GRAVITY == 0.08);
