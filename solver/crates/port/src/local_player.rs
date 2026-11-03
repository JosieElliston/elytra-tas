//! `net.minecraft.client.player.LocalPlayer`: `tick`, `aiStep` (jump edge, glide
//! start, sprint) and the input it feeds `LivingEntity`.

use world::BlockPos;

use crate::entity::containing;
use crate::level::{self, Level};
use crate::mth;
use crate::phys::{AABB, Axis, Vec2, Vec3};
use crate::{Input, LocalPlayer, Pose, TickEvents, Unsupported};

type R<T> = Result<T, Unsupported>;

/// `Options.sprintWindow` default.
const SPRINT_WINDOW: i32 = 7;
/// `Attributes.SNEAKING_SPEED` default.
const SNEAKING_SPEED: f64 = 0.3;

impl LocalPlayer {
    /// `LocalPlayer.tick()`: `Player.tick()` (through `AbstractClientPlayer.tick()`),
    /// with the client loaded. The packets it sends after don't move the player.
    pub fn tick<L: Level + ?Sized>(&mut self, keys: Input, level: &L) -> R<TickEvents> {
        self.player_tick(keys, level)
    }

    /// `LocalPlayer.aiStep()`, with `keys` as what `KeyboardInput.tick()` reads.
    /// Portal effects, creative flight, water vision and riding are left out (none
    /// apply).
    pub(crate) fn ai_step<L: Level + ?Sized>(&mut self, keys: Input, level: &L) -> R<TickEvents> {
        let mut events = TickEvents::default();
        if self.sprint_trigger_time > 0 {
            self.sprint_trigger_time -= 1;
        }
        // last tick's keys: `input.tick()` hasn't run yet
        let was_jumping = self.input.key_presses.jump;
        let was_shift_key_down = self.input.key_presses.shift;
        let has_forward_impulse = self.input.has_forward_impulse();
        // not flying, not a passenger, not sleeping
        self.crouching = !self.is_swimming()
            && self.can_player_fit_within_blocks_and_entities_when(Pose::Crouching, level)?
            && (self.is_shift_key_down()
                || !self.can_player_fit_within_blocks_and_entities_when(Pose::Standing, level)?);
        self.input.tick(keys);
        let mut was_auto_jump = false;
        if self.auto_jump_time > 0 {
            self.auto_jump_time -= 1;
            was_auto_jump = true;
            self.input.make_jump();
        }
        let _ = was_auto_jump; // read only by creative flight
        // `!this.noPhysics`
        let w = self.get_bb_width() as f64;
        self.move_towards_closest_space(
            self.position.x - w * 0.35,
            self.position.z + w * 0.35,
            level,
        )?;
        self.move_towards_closest_space(
            self.position.x - w * 0.35,
            self.position.z - w * 0.35,
            level,
        )?;
        self.move_towards_closest_space(
            self.position.x + w * 0.35,
            self.position.z - w * 0.35,
            level,
        )?;
        self.move_towards_closest_space(
            self.position.x + w * 0.35,
            self.position.z + w * 0.35,
            level,
        )?;
        // no item in use
        if was_shift_key_down || self.input.key_presses.backward {
            self.sprint_trigger_time = 0;
        }
        if self.can_start_sprinting() {
            if !has_forward_impulse {
                if self.sprint_trigger_time > 0 {
                    self.set_sprinting(true);
                } else {
                    self.sprint_trigger_time = SPRINT_WINDOW;
                }
            }
            if self.input.key_presses.sprint {
                self.set_sprinting(true);
            }
        }
        if self.sprinting {
            if self.is_swimming() {
                // `shouldStopSwimSprinting`: unreachable without water
            } else if self.should_stop_run_sprinting() {
                self.set_sprinting(false);
            }
        }
        // `abilities.mayfly` is false, so no creative-flight toggle
        let just_toggled_creative_flight = false;
        if self.input.key_presses.jump
            && !just_toggled_creative_flight
            && !was_jumping
            && !self.on_climbable(level)?
            && self.try_to_start_fall_flying()
        {
            events.start_fall_flying = true;
        }
        // `isInWater() && shift`: `goDownInWater`, unreachable without water
        self.player_ai_step(level)?;
        Ok(events)
    }

    /// `LocalPlayer.shouldStopRunSprinting()`.
    fn should_stop_run_sprinting(&self) -> bool {
        !self.is_sprinting_possible(false)
            || !self.input.has_forward_impulse()
            || self.horizontal_collision && !self.minor_horizontal_collision
    }

    /// `LocalPlayer.moveTowardsClosestSpace(double, double)`: velocity 0.1 toward the
    /// nearest free side when a box corner column holds a suffocating block.
    fn move_towards_closest_space<L: Level + ?Sized>(
        &mut self,
        x: f64,
        z: f64,
        level: &L,
    ) -> R<()> {
        let pos = containing(x, self.position.y, z);
        if self.suffocates_at(pos, level)? {
            let xd = x - pos.x as f64;
            let zd = z - pos.z as f64;
            let mut dir: Option<(Axis, i32)> = None;
            let mut closest = f64::MAX;
            // WEST, EAST, NORTH, SOUTH as (axis, step)
            for (axis, step) in [(Axis::X, -1), (Axis::X, 1), (Axis::Z, -1), (Axis::Z, 1)] {
                let axis_distance = axis.choose(xd, 0.0, zd);
                let distance_to_edge = if step > 0 {
                    1.0 - axis_distance
                } else {
                    axis_distance
                };
                let neighbor = match axis {
                    Axis::X => BlockPos {
                        x: pos.x + step,
                        ..pos
                    },
                    _ => BlockPos {
                        z: pos.z + step,
                        ..pos
                    },
                };
                if distance_to_edge < closest && !self.suffocates_at(neighbor, level)? {
                    closest = distance_to_edge;
                    dir = Some((axis, step));
                }
            }
            if let Some((axis, step)) = dir {
                let old_movement = self.delta_movement;
                if axis == Axis::X {
                    self.set_delta_movement(Vec3::new(
                        0.1 * step as f64,
                        old_movement.y,
                        old_movement.z,
                    ));
                } else {
                    self.set_delta_movement(Vec3::new(
                        old_movement.x,
                        old_movement.y,
                        0.1 * step as f64,
                    ));
                }
            }
        }
        Ok(())
    }

    /// `LocalPlayer.suffocatesAt(BlockPos)`.
    fn suffocates_at<L: Level + ?Sized>(&self, pos: BlockPos, level: &L) -> R<bool> {
        let bb = self.get_bounding_box();
        let test_area = AABB::new(
            pos.x as f64,
            bb.min_y,
            pos.z as f64,
            pos.x as f64 + 1.0,
            bb.max_y,
            pos.z as f64 + 1.0,
        )
        .deflate(1.0E-7);
        level::collides_with_suffocating_block(level, &test_area)
    }

    /// `LocalPlayer.isShiftKeyDown()`.
    pub fn is_shift_key_down(&self) -> bool {
        self.input.key_presses.shift
    }

    /// `LocalPlayer.isCrouching()`.
    pub fn is_crouching(&self) -> bool {
        self.crouching
    }

    /// `LocalPlayer.isMovingSlowly()`.
    pub fn is_moving_slowly(&self) -> bool {
        self.is_crouching() || self.is_visually_crawling()
    }

    /// `LocalPlayer.isUnderWater()`: `wasUnderwater`, false without water.
    fn is_under_water(&self) -> bool {
        false
    }

    /// `LocalPlayer.applyInput()` (the camera is the player).
    pub(crate) fn apply_input(&mut self) {
        let modified_input = self.modify_input(self.input.move_vector);
        self.xxa = modified_input.x;
        self.zza = modified_input.y;
        self.jumping = self.input.key_presses.jump;
    }

    /// `LocalPlayer.modifyInput(Vec2)`: no item in use.
    fn modify_input(&self, input: Vec2) -> Vec2 {
        if input.length_squared() == 0.0 {
            return input;
        }
        let mut new_input = input.scale(0.98);
        if self.is_moving_slowly() {
            let sneaking_movement_factor = SNEAKING_SPEED as f32;
            new_input = new_input.scale(sneaking_movement_factor);
        }
        modify_input_speed_for_square_movement(new_input)
    }

    /// `LocalPlayer.move(MoverType, Vec3)`: `Entity.move`; auto-jump is off and the
    /// walked distance only animates.
    pub(crate) fn move_<L: Level + ?Sized>(&mut self, delta: Vec3, level: &L) -> R<()> {
        self.entity_move(delta, level)
    }

    /// `LocalPlayer.isHorizontalCollisionMinor(Vec3)`: the actual move is within 8
    /// degrees of the input direction.
    pub(crate) fn is_horizontal_collision_minor(&self, movement: Vec3) -> bool {
        let y_rot_in_radians = self.y_rot * mth::DEG_TO_RAD;
        let y_rot_sin = mth::sin(y_rot_in_radians as f64) as f64;
        let y_rot_cos = mth::cos(y_rot_in_radians as f64) as f64;
        let global_xa = self.xxa as f64 * y_rot_cos - self.zza as f64 * y_rot_sin;
        let global_za = self.zza as f64 * y_rot_cos + self.xxa as f64 * y_rot_sin;
        let a_length_squared = mth::square(global_xa) + mth::square(global_za);
        let movement_length_squared = mth::square(movement.x) + mth::square(movement.z);
        if !(a_length_squared < 1.0E-5f32 as f64) && !(movement_length_squared < 1.0E-5f32 as f64) {
            let dot_product = global_xa * movement.x + global_za * movement.z;
            let angle = (dot_product / (a_length_squared * movement_length_squared).sqrt()).acos();
            angle < 0.13962634f32 as f64
        } else {
            false
        }
    }

    /// `LocalPlayer.isSprintingPossible(boolean)`: no blindness, enough food.
    fn is_sprinting_possible(&self, allowed_in_shallow_water: bool) -> bool {
        allowed_in_shallow_water || !self.is_in_shallow_water()
    }

    /// `Entity.isInShallowWater()`.
    fn is_in_shallow_water(&self) -> bool {
        self.is_in_water() && !self.is_under_water()
    }

    /// `LocalPlayer.canStartSprinting()`: no item in use.
    fn can_start_sprinting(&self) -> bool {
        !self.sprinting
            && self.input.has_forward_impulse()
            && self.is_sprinting_possible(false)
            && (!self.fall_flying || self.is_under_water())
            && (!self.is_moving_slowly() || self.is_under_water())
    }
}

/// `LocalPlayer.modifyInputSpeedForSquareMovement(Vec2)`.
fn modify_input_speed_for_square_movement(input: Vec2) -> Vec2 {
    let length = input.length();
    if length <= 0.0 {
        return input;
    }
    let direction = input.scale(1.0 / length);
    let distance_to_unit_square = distance_to_unit_square(direction);
    let modified_length = mth::java_min_f32(length * distance_to_unit_square, 1.0);
    direction.scale(modified_length)
}

/// `LocalPlayer.distanceToUnitSquare(Vec2)`.
fn distance_to_unit_square(direction: Vec2) -> f32 {
    let direction_x = direction.x.abs();
    let direction_y = direction.y.abs();
    let tan = if direction_y > direction_x {
        direction_x / direction_y
    } else {
        direction_y / direction_x
    };
    mth::sqrt(1.0 + mth::square_f32(tan))
}
