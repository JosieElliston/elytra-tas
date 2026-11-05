//! The full mode: any client tick from a [`FullState`], glide or not, with the keys
//! mattering. What [`Faithful::step`] and [`Faithful::step_glide`] leave out lives
//! here: ticks without the glide (walking, falling, jumping, air control, sprint,
//! sneak at ledges), the jump edge that starts a glide, poses other than the gliding
//! one (the standing box after the server's glide-off), and the server's events
//! (`apply_glide_off`, `apply_velocity`), so a recorded run whose bounces lost the
//! glide flies through.
//!
//! One tick, in the game's order but in this kernel's terms:
//! - the input: the last tick's keys (jump edge, sneak, forward impulse for the
//!   sprint double tap) against this tick's; crouching from the fit of the crouching
//!   and standing boxes; the sprint start and stop; a jump edge in the air starts the
//!   glide;
//! - the velocity: the jump delay, the small-velocity zeroing, the jump off the
//!   ground, then the elytra update, or the walk: input along the yaw at the ground's
//!   (or the air's) speed, the move, then gravity and the ground's friction;
//! - the move: [`collide::collide_step`] for the pose's box, sneak's ledge back-off,
//!   the supporting block, fall distance, the clipped axes zeroed, the block speed
//!   factor;
//! - the pose for the next tick.
//!
//! Same events as the other modes. Fixed facts are port's (survival, no effects,
//! default attributes, no item in use, auto-jump off).

// `!(a < b)` is not `a >= b` for NaN; the Java's comparisons are kept.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use port::mth;
use world::{Aabb, BlockPos, Effect, World};

use super::collide;
use super::glide::{collision_minor, jump_power, modify_input, move_vector};
use super::{Contact, Event, Faithful, GRAVITY};
use crate::state::{FullState, Input, Keys, Pose};

/// `Attributes.MOVEMENT_SPEED` base for players, `0.1F`, and the sprinting modifier's
/// `0.3F` (`ADD_MULTIPLIED_TOTAL`).
const SPEED: f64 = 0.1f32 as f64;
const SPRINT_MODIFIER: f64 = 0.3f32 as f64;
/// `Options.sprintWindow`: 0, as in port's `local_player.rs`, which says why.
const SPRINT_WINDOW: i32 = 0;
/// `LivingEntity.maxUpStep()`.
const MAX_UP_STEP: f32 = 0.6;

/// One [`Faithful::step_full`] tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FullTick {
    pub state: FullState,
    /// A wall or ceiling clipped the move (a floor under a ground tick is not a
    /// contact). Damage only while gliding, as in [`Contact::damage`].
    pub contact: Option<Contact>,
    /// The jump edge started the glide this tick (`START_FALL_FLYING` went out).
    pub glide_started: bool,
}

/// `Avatar.POSES` heights (scale 1); the width is 0.6 for all.
fn height(p: Pose) -> f32 {
    match p {
        Pose::Standing => 1.8,
        Pose::Crouching => 1.5,
        Pose::FallFlying | Pose::Swimming | Pose::SpinAttack => 0.6,
    }
}

/// `EntityDimensions.makeBoundingBox` for a pose at `pos` (half width a float).
pub(super) fn pose_box(pos: [f64; 3], pose: Pose) -> Aabb {
    let w = (0.6f32 / 2.0) as f64;
    Aabb {
        min: [pos[0] - w, pos[1], pos[2] - w],
        max: [pos[0] + w, pos[1] + height(pose) as f64, pos[2] + w],
    }
}

/// `canPlayerFitWithinBlocksAndEntitiesWhen(pose)`.
fn fits(world: &World, pos: [f64; 3], pose: Pose) -> Result<bool, Event> {
    collide::no_collision(world, &collide::deflate(&pose_box(pos, pose), 1.0E-7))
}

/// `LivingEntity.computeModifiedFriction(friction, 1)`.
fn modified_friction(f: f32) -> f32 {
    mth::clamp_f32(1.0 - (1.0 - f) * 1.0, 0.0, 1.0)
}

/// `LivingEntity.onClimbable()` as a detector: a climbable block at the position is
/// an event, except one in `can_glide_through` while gliding.
fn check_climbable(world: &World, pos: [f64; 3], gliding: bool) -> Result<(), Event> {
    let at = BlockPos {
        x: mth::floor(pos[0]),
        y: mth::floor(pos[1]),
        z: mth::floor(pos[2]),
    };
    let d = world.detail(collide::state_at(world, at)?);
    if d.climbable && !(gliding && d.glide_through) {
        Err(Event::Climbable(at))
    } else {
        Ok(())
    }
}

impl FullState {
    /// The server's glide-off (`set_entity_data`, handled before a tick): the flag
    /// clear and the pose the server sends with it ([`Faithful::server_pose`]).
    pub fn apply_glide_off(&mut self, pose: Pose) {
        self.fall_flying = false;
        self.pose = pose;
    }

    /// The server's `set_entity_motion`: the velocity replaced.
    pub fn apply_velocity(&mut self, v: [f64; 3]) {
        if v.iter().all(|x| x.is_finite()) {
            self.vel = v;
        }
    }
}

impl Faithful {
    /// The pose the server's glide-off carries, judged at `s`'s position: standing if
    /// it fits, else crouching, else swimming (the server's `updatePlayerPose`), or
    /// `s`'s own pose if not even swimming fits.
    pub fn server_pose(&self, world: &World, s: &FullState) -> Result<Pose, Event> {
        Ok(if !fits(world, s.pos, Pose::Swimming)? {
            s.pose
        } else if fits(world, s.pos, Pose::Standing)? {
            Pose::Standing
        } else if fits(world, s.pos, Pose::Crouching)? {
            Pose::Crouching
        } else {
            Pose::Swimming
        })
    }

    /// One client tick from `s` with rotation `u` and keys `keys` (`KeyboardInput`'s),
    /// gliding or not. Port's tick, exactly, wherever it is defined.
    pub fn step_full(
        &self,
        world: &World,
        s: &FullState,
        u: &Input<f32>,
        keys: Keys,
    ) -> Result<FullTick, Event> {
        let k = self.look(u);
        let (yaw, pos) = (k.yaw, s.pos);
        let mut n = *s;
        n.yaw = k.yaw;
        n.pitch = k.pitch;
        let bb = pose_box(pos, s.pose);
        // `Entity.baseTick`
        collide::find_fluid(world, &collide::deflate(&bb, 0.001))?;

        // `LocalPlayer.aiStep`: the last tick's keys, then this tick's
        if n.sprint_trigger_time > 0 {
            n.sprint_trigger_time -= 1;
        }
        let (was_jumping, was_sneaking) = (s.keys.jump, s.keys.sneak);
        let had_forward = s.move_vector[1] > 1.0E-5;
        // not swimming (no water)
        n.crouching = fits(world, pos, Pose::Crouching)?
            && (was_sneaking || !fits(world, pos, Pose::Standing)?);
        let mut keys = keys;
        n.move_vector = move_vector(keys);
        if n.auto_jump_time > 0 {
            n.auto_jump_time -= 1;
            keys.jump = true;
        }
        n.keys = keys;
        // `moveTowardsClosestSpace`, as an event
        collide::check_inside(world, pos, &bb)?;
        if was_sneaking || keys.backward {
            n.sprint_trigger_time = 0;
        }
        let forward = n.move_vector[1] > 1.0E-5;
        // `isMovingSlowly`: crouching, or crawling (visually swimming out of water)
        let crawling = s.pose == Pose::Swimming || !s.fall_flying && s.pose == Pose::FallFlying;
        let slowly = n.crouching || crawling;
        let set_sprint = |n: &mut FullState, on: bool| {
            n.sprinting = on;
            n.sprint_speed_modifier = on;
        };
        if !n.sprinting && forward && !n.fall_flying && !slowly {
            if !had_forward {
                if n.sprint_trigger_time > 0 {
                    set_sprint(&mut n, true);
                } else {
                    n.sprint_trigger_time = SPRINT_WINDOW;
                }
            }
            if keys.sprint {
                set_sprint(&mut n, true);
            }
        }
        if n.sprinting && (!forward || s.horizontal_collision && !s.minor_horizontal_collision) {
            set_sprint(&mut n, false);
        }
        let mut glide_started = false;
        if keys.jump && !was_jumping {
            check_climbable(world, pos, n.fall_flying)?;
            // `tryToStartFallFlying`: `canGlide` (off the ground, a glider), not in water
            if !n.fall_flying && !s.on_ground && n.glider_equipped {
                n.fall_flying = true;
                glide_started = true;
            }
        }

        // `LivingEntity.aiStep`
        if n.no_jump_delay > 0 {
            n.no_jump_delay -= 1;
        }
        let mut v = self.zero_small(s.vel);
        n.move_input = {
            let m = modify_input(n.move_vector, slowly);
            [m[0], 0.0, m[1]]
        };
        n.jumping = keys.jump;
        if n.jumping {
            if s.on_ground && n.no_jump_delay == 0 {
                let power = jump_power(world, pos, s.supporting_block)?;
                if !(power <= 1.0E-5) {
                    v = [v[0], mth::java_max(power as f64, v[1]), v[2]];
                    if n.sprinting {
                        let angle = yaw * mth::DEG_TO_RAD;
                        v = [
                            v[0] + -mth::sin(angle as f64) as f64 * 0.2,
                            v[1] + 0.0,
                            v[2] + mth::cos(angle as f64) as f64 * 0.2,
                        ];
                    }
                }
                n.no_jump_delay = 10;
            }
        } else {
            n.no_jump_delay = 0;
        }
        if n.fall_flying && v[1] > -0.5 && n.fall_distance > 1.0 {
            // `checkFallDistanceAccumulation`
            n.fall_distance = 1.0;
        }

        // `LivingEntity.travel`
        let mut h_before = None;
        let (delta, walk) = if n.fall_flying {
            check_climbable(world, pos, true)?;
            let (v, h) = self.elytra(v, &k);
            h_before = Some(h);
            (v, None)
        } else {
            // `travelInAir`: the friction and speed from the block below, before the move
            let below = collide::on_pos(world, pos, s.supporting_block, 0.500001)?;
            let block_friction = if s.on_ground {
                modified_friction(world.info(collide::state_at(world, below)?).friction)
            } else {
                1.0
            };
            let speed = if s.on_ground {
                let attr = mth::clamp(
                    if n.sprint_speed_modifier {
                        SPEED * (1.0 + SPRINT_MODIFIER)
                    } else {
                        SPEED
                    },
                    0.0,
                    1024.0,
                ) as f32;
                if block_friction as f64 > 0.6 {
                    attr * (0.21600002 / (block_friction * block_friction * block_friction))
                } else {
                    attr
                }
            } else if n.sprinting {
                0.025999999
            } else {
                0.02
            };
            let iv = input_vector(n.move_input, speed, yaw);
            v = [v[0] + iv[0], v[1] + iv[1], v[2] + iv[2]];
            check_climbable(world, pos, false)?;
            (v, Some((below, block_friction)))
        };

        // `Entity.move`: the backed-off move collides; the velocity it restitutes is
        // the one before the back-off (`getDeltaMovement()`)
        let orig = delta;
        let delta = self.back_off_from_edge(world, &n, &bb, delta)?;
        let movement = collide::collide_step(world, pos, &bb, delta, s.on_ground)?;
        let len = movement[0] * movement[0] + movement[1] * movement[1] + movement[2] * movement[2];
        let mut new_pos = pos;
        if len > 1.0E-7
            || (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]) - len < 1.0E-7
        {
            new_pos = [
                pos[0] + movement[0],
                pos[1] + movement[1],
                pos[2] + movement[2],
            ];
        }
        let x_coll = !mth::equal(delta[0], movement[0]);
        let z_coll = !mth::equal(delta[2], movement[2]);
        let horizontal = x_coll || z_coll;
        let moved_vertically = delta[1].abs() > 0.0;
        let vertical = delta[1] != movement[1];
        let below_coll = vertical && delta[1] < 0.0;
        n.pos = new_pos;
        n.on_ground = below_coll;
        n.horizontal_collision = horizontal;
        n.vertical_collision = vertical;
        n.vertical_collision_below = below_coll;
        let new_bb = pose_box(new_pos, s.pose);
        if below_coll {
            let (sb, no_blocks) = collide::check_supporting_block(
                world,
                new_pos,
                &new_bb,
                movement,
                s.on_ground_no_blocks,
            )?;
            n.supporting_block = sb;
            n.on_ground_no_blocks = no_blocks;
        } else {
            n.supporting_block = None;
            n.on_ground_no_blocks = false;
        }
        n.minor_horizontal_collision =
            horizontal && collision_minor(yaw, [n.move_input[0], n.move_input[2]], movement);
        let effect_pos = collide::on_pos(world, new_pos, n.supporting_block, 0.2)?;
        let effect = collide::state_at(world, effect_pos)?;
        // `LivingEntity.checkFallDamage`: the fluid check, then the fall distance
        collide::find_fluid(world, &collide::deflate(&new_bb, 0.001))?;
        if movement[1] < 0.0 {
            n.fall_distance -= (movement[1] as f32) as f64;
        }
        if n.on_ground {
            n.fall_distance = 0.0;
        }
        let mut vel = orig;
        if moved_vertically && vertical || horizontal {
            if x_coll {
                vel[0] = -orig[0] * 0.0;
            }
            if z_coll {
                vel[2] = -orig[2] * 0.0;
            }
            if vertical {
                // a bouncing block under a fall at least at gravity, not sneaking
                if below_coll
                    && !(-orig[1] < GRAVITY)
                    && !keys.sneak
                    && world.detail(effect).own_effect == Effect::Bounce
                {
                    return Err(Event::Bounce(effect_pos));
                }
                vel[1] = (0.0 - orig[1]) * 1.0 * 0.0;
            }
        }
        // `getBlockSpeedFactor`: 1 while gliding
        if !n.fall_flying {
            let here = BlockPos {
                x: mth::floor(new_pos[0]),
                y: mth::floor(new_pos[1]),
                z: mth::floor(new_pos[2]),
            };
            let id = collide::state_at(world, here)?;
            let mut f = world.info(id).speed_factor;
            let water = matches!(
                world.info(id).state.split('[').next(),
                Some("minecraft:water" | "minecraft:bubble_column")
            );
            if !water && f as f64 == 1.0 {
                let b = collide::on_pos(world, new_pos, n.supporting_block, 0.500001)?;
                f = world.info(collide::state_at(world, b)?).speed_factor;
            }
            vel = [vel[0] * f as f64, vel[1] * 1.0, vel[2] * f as f64];
        }
        if let Some((below, block_friction)) = walk {
            if horizontal || n.jumping {
                check_climbable(world, new_pos, false)?;
            }
            collide::state_at(world, below)?;
            let my = vel[1] - GRAVITY;
            let friction = block_friction * modified_friction(0.91);
            let vertical_friction = modified_friction(0.98);
            vel = [
                vel[0] * friction as f64,
                my * vertical_friction as f64,
                vel[2] * friction as f64,
            ];
        }
        n.vel = vel;
        // `applyEffectsFromBlocks`
        if n.on_ground && world.detail(effect).own_effect == Effect::Bounce {
            return Err(Event::Bounce(effect_pos));
        }
        collide::find_stuck(world, &bb, &new_bb)?;
        // `Player.tick`: the world-edge clamp, then `updatePlayerPose`
        n.pos[0] = mth::clamp(n.pos[0], -2.9999999E7, 2.9999999E7);
        n.pos[2] = mth::clamp(n.pos[2], -2.9999999E7, 2.9999999E7);
        if fits(world, n.pos, Pose::Swimming)? {
            let desired = if n.fall_flying {
                Pose::FallFlying
            } else if keys.sneak {
                Pose::Crouching
            } else {
                Pose::Standing
            };
            n.pose = if fits(world, n.pos, desired)? {
                desired
            } else if fits(world, n.pos, Pose::Crouching)? {
                Pose::Crouching
            } else {
                Pose::Swimming
            };
        }
        let clipped = [
            delta[0] != movement[0],
            vertical && !below_coll,
            delta[2] != movement[2],
        ];
        let contact = clipped.iter().any(|&c| c).then(|| {
            let damage = h_before.and_then(|h| {
                let h_after = (vel[0] * vel[0] + vel[2] * vel[2]).sqrt();
                let dmg = ((h - h_after) * 10.0 - 3.0) as f32;
                (horizontal && dmg > 0.0).then_some(dmg)
            });
            Contact {
                clipped,
                horizontal_collision: horizontal,
                damage,
            }
        });
        Ok(FullTick {
            state: n,
            contact,
            glide_started,
        })
    }

    /// `Player.maybeBackOffFromEdge` for `MoverType.SELF`: sneaking (this tick's key,
    /// in `s.keys`) near the ground walks no further off a ledge taller than the step
    /// height. `s` is mid-tick: the keys and fall distance are this tick's, the
    /// position and `on_ground` the last tick's.
    fn back_off_from_edge(
        &self,
        world: &World,
        s: &FullState,
        bb: &Aabb,
        delta: [f64; 3],
    ) -> Result<[f64; 3], Event> {
        if delta[1] > 0.0 || !s.keys.sneak {
            return Ok(delta);
        }
        let down = MAX_UP_STEP as f64;
        let can_fall = |dx: f64, dz: f64, h: f64| -> Result<bool, Event> {
            collide::no_collision(
                world,
                &Aabb {
                    min: [
                        bb.min[0] + 1.0E-7 + dx,
                        bb.min[1] - h - 1.0E-7,
                        bb.min[2] + 1.0E-7 + dz,
                    ],
                    max: [bb.max[0] - 1.0E-7 + dx, bb.min[1], bb.max[2] - 1.0E-7 + dz],
                },
            )
        };
        let above_ground =
            s.on_ground || s.fall_distance < down && !can_fall(0.0, 0.0, down - s.fall_distance)?;
        if !above_ground {
            return Ok(delta);
        }
        let (mut dx, mut dz) = (delta[0], delta[2]);
        let (sx, sz) = (mth::signum(dx) * 0.05, mth::signum(dz) * 0.05);
        while dx != 0.0 && can_fall(dx, 0.0, down)? {
            if dx.abs() <= 0.05 {
                dx = 0.0;
                break;
            }
            dx -= sx;
        }
        while dz != 0.0 && can_fall(0.0, dz, down)? {
            if dz.abs() <= 0.05 {
                dz = 0.0;
                break;
            }
            dz -= sz;
        }
        while dx != 0.0 && dz != 0.0 && can_fall(dx, dz, down)? {
            if dx.abs() <= 0.05 {
                dx = 0.0;
            } else {
                dx -= sx;
            }
            if dz.abs() <= 0.05 {
                dz = 0.0;
            } else {
                dz -= sz;
            }
        }
        Ok([dx, delta[1], dz])
    }
}

/// `Entity.getInputVector(input, speed, yRot)`.
fn input_vector(input: [f32; 3], speed: f32, yaw: f32) -> [f64; 3] {
    let i = [input[0] as f64, input[1] as f64, input[2] as f64];
    let length = i[0] * i[0] + i[1] * i[1] + i[2] * i[2];
    if length < 1.0E-7 {
        return [0.0; 3];
    }
    let i = if length > 1.0 {
        // `Vec3.normalize`
        let l = length.sqrt();
        if l < 1.0E-5 {
            [0.0; 3]
        } else {
            [i[0] / l, i[1] / l, i[2] / l]
        }
    } else {
        i
    };
    let m = [
        i[0] * speed as f64,
        i[1] * speed as f64,
        i[2] * speed as f64,
    ];
    let r = (yaw * mth::DEG_TO_RAD) as f64;
    let (sin, cos) = (mth::sin(r) as f64, mth::cos(r) as f64);
    [m[0] * cos - m[2] * sin, m[1], m[2] * cos + m[0] * sin]
}
