//! `net.minecraft.world.entity.Entity`: the methods movement runs through.

use world::{BlockPos, Effect};

use crate::level::{self, BlockState, Level};
use crate::mth;
use crate::phys::{AABB, Axis, Vec3, axis_step_order};
use crate::shapes::{self, VoxelShape};
use crate::{LocalPlayer, Pose, Unsupported};

type R<T> = Result<T, Unsupported>;

/// `BlockPos.containing(x, y, z)`.
pub(crate) fn containing(x: f64, y: f64, z: f64) -> BlockPos {
    BlockPos {
        x: mth::floor(x),
        y: mth::floor(y),
        z: mth::floor(z),
    }
}

impl LocalPlayer {
    /// `Entity.setYRot`: non-finite values are discarded.
    pub fn set_y_rot(&mut self, y_rot: f32) {
        if y_rot.is_finite() {
            self.y_rot = y_rot;
        }
    }

    /// `Entity.setXRot`: `Math.clamp(xRot % 360.0F, -90.0F, 90.0F)`.
    pub fn set_x_rot(&mut self, x_rot: f32) {
        if x_rot.is_finite() {
            self.x_rot = (x_rot % 360.0).clamp(-90.0, 90.0);
        }
    }

    /// `Entity.getBoundingBox()`: the game keeps `bb` equal to
    /// `dimensions.makeBoundingBox(position)` (`setPos`, `refreshDimensions` on a pose
    /// change), so it is computed here.
    pub fn get_bounding_box(&self) -> AABB {
        self.pose.make_bounding_box(self.position)
    }

    /// `Entity.getBbWidth()`.
    pub fn get_bb_width(&self) -> f32 {
        self.pose.dimensions().0
    }

    /// `Entity.getEyeHeight()`.
    pub fn get_eye_height(&self) -> f32 {
        self.pose.dimensions().2
    }

    /// `Entity.blockPosition()`: `setPosRaw` keeps it at the floor of the position.
    pub fn block_position(&self) -> BlockPos {
        containing(self.position.x, self.position.y, self.position.z)
    }

    /// `Entity.setPos(double, double, double)`; the bounding box follows.
    pub fn set_pos(&mut self, pos: Vec3) {
        self.position = pos;
    }

    /// `Entity.setDeltaMovement(Vec3)`: non-finite values are discarded.
    pub fn set_delta_movement(&mut self, delta_movement: Vec3) {
        if delta_movement.is_finite() {
            self.delta_movement = delta_movement;
        }
    }

    /// `Entity.addDeltaMovement(Vec3)`.
    pub fn add_delta_movement(&mut self, momentum: Vec3) {
        if momentum.is_finite() {
            self.set_delta_movement(self.delta_movement.add(momentum));
        }
    }

    /// `Entity.setPose`.
    pub fn set_pose(&mut self, pose: Pose) {
        self.pose = pose;
    }

    /// `Entity.baseTick()`: of it, only `updateFluidInteraction` touches movement, and
    /// fluids aren't simulated: water or lava in the box is [`Unsupported::Fluid`].
    pub(crate) fn base_tick<L: Level + ?Sized>(&mut self, level: &L) -> R<()> {
        self.update_fluid_interaction(level)
    }

    /// `Entity.updateFluidInteraction()`, as a detector: errors where the game would
    /// find the player in water or lava (any fluid cell overlapping the box deflated
    /// by 0.001; the game also checks the fluid's height, so this can fire early).
    pub(crate) fn update_fluid_interaction<L: Level + ?Sized>(&self, level: &L) -> R<()> {
        let bx = self.get_bounding_box().deflate(0.001);
        match level::find_block(level, &bx, |s| s.has_fluid())? {
            Some(pos) => Err(Unsupported::Fluid(pos)),
            None => Ok(()),
        }
    }

    /// `Entity.isInWater()`: `wasTouchingWater`, always false here (see
    /// [`Self::update_fluid_interaction`]).
    pub fn is_in_water(&self) -> bool {
        false
    }

    /// `Entity.isInLava()`: always false here, as [`Self::is_in_water`].
    pub fn is_in_lava(&self) -> bool {
        false
    }

    /// `Entity.isSwimming()`: shared flag 4, which needs water.
    pub fn is_swimming(&self) -> bool {
        false
    }

    /// `Entity.isVisuallyCrawling()`.
    pub fn is_visually_crawling(&self) -> bool {
        self.is_visually_swimming() && !self.is_in_water()
    }

    /// `Entity.setOnGroundWithMovement(boolean, boolean, Vec3)`.
    pub(crate) fn set_on_ground_with_movement<L: Level + ?Sized>(
        &mut self,
        on_ground: bool,
        horizontal_collision: bool,
        movement: Vec3,
        level: &L,
    ) -> R<()> {
        self.on_ground = on_ground;
        self.horizontal_collision = horizontal_collision;
        self.check_supporting_block(on_ground, Some(movement), level)
    }

    /// `Entity.checkSupportingBlock(boolean, Vec3)`.
    pub(crate) fn check_supporting_block<L: Level + ?Sized>(
        &mut self,
        on_ground: bool,
        movement: Option<Vec3>,
        level: &L,
    ) -> R<()> {
        if on_ground {
            let bb = self.get_bounding_box();
            let test_area = AABB::new(
                bb.min_x,
                bb.min_y - 1.0E-6,
                bb.min_z,
                bb.max_x,
                bb.min_y,
                bb.max_z,
            );
            let mut supporting_block =
                level::find_supporting_block(level, self.position, &test_area)?;
            if supporting_block.is_some() || self.on_ground_no_blocks {
                self.main_supporting_block_pos = supporting_block;
            } else if let Some(movement) = movement {
                let on_ground_collision_test_area = test_area.move_(-movement.x, 0.0, -movement.z);
                supporting_block = level::find_supporting_block(
                    level,
                    self.position,
                    &on_ground_collision_test_area,
                )?;
                self.main_supporting_block_pos = supporting_block;
            }
            self.on_ground_no_blocks = supporting_block.is_none();
        } else {
            self.on_ground_no_blocks = false;
            if self.main_supporting_block_pos.is_some() {
                self.main_supporting_block_pos = None;
            }
        }
        Ok(())
    }

    /// `Entity.move(MoverType, Vec3)` for `MoverType.SELF`, without the pieces that
    /// can't apply here: `noPhysics` (spectator), the stuck multiplier (stuck blocks
    /// are [`Unsupported::Stuck`]), the fall-distance reset from `level.clip` on long
    /// moves, movement sounds.
    pub(crate) fn entity_move<L: Level + ?Sized>(&mut self, delta: Vec3, level: &L) -> R<()> {
        let delta = self.maybe_back_off_from_edge(delta, level)?;
        let movement = self.collide(delta, level)?;
        let movement_length = movement.length_sqr();
        if movement_length > 1.0E-7 || delta.length_sqr() - movement_length < 1.0E-7 {
            let pos = self.position;
            let new_position = pos.add(movement);
            self.set_pos(new_position);
        }
        let x_collision = !mth::equal(delta.x, movement.x);
        let z_collision = !mth::equal(delta.z, movement.z);
        self.horizontal_collision = x_collision || z_collision;
        let moved_vertically = delta.y.abs() > 0.0;
        // `if (movedVertically || this.isLocalInstanceAuthoritative())`: always, for the
        // local player
        self.vertical_collision = delta.y != movement.y;
        self.vertical_collision_below = self.vertical_collision && delta.y < 0.0;
        self.set_on_ground_with_movement(
            self.vertical_collision_below,
            self.horizontal_collision,
            movement,
            level,
        )?;
        if self.horizontal_collision {
            self.minor_horizontal_collision = self.is_horizontal_collision_minor(movement);
        } else {
            self.minor_horizontal_collision = false;
        }
        let effect_pos = self.get_on_pos_legacy(level)?;
        let effect_state = level.get_block_state(effect_pos)?;
        self.check_fall_damage(movement.y, self.on_ground, level)?;
        if moved_vertically && self.vertical_collision || self.horizontal_collision {
            self.restitute_movement_after_collisions(
                &effect_state,
                effect_pos,
                x_collision,
                z_collision,
                movement,
            )?;
        }
        let block_speed_factor = self.get_block_speed_factor(level)?;
        self.set_delta_movement(self.delta_movement.multiply(
            block_speed_factor as f64,
            1.0,
            block_speed_factor as f64,
        ));
        Ok(())
    }

    /// `Entity.restituteMovementAfterCollisions`. The player's bounciness attribute is
    /// 0, so this zeroes the clipped axes' velocity (as `-v * 0.0`, keeping the Java's
    /// signed zeros), unless the landing block bounces: [`Unsupported::Bounce`].
    fn restitute_movement_after_collisions(
        &mut self,
        effect_state: &BlockState,
        effect_pos: BlockPos,
        x_collision: bool,
        z_collision: bool,
        movement: Vec3,
    ) -> R<()> {
        let mut restitution = if self.is_suppressing_bounce() {
            0.0
        } else {
            self.get_entity_bounciness()
        };
        let current_movement = self.delta_movement;
        let mut movement_after_bounce = current_movement;
        if x_collision {
            movement_after_bounce =
                movement_after_bounce.with(Axis::X, -current_movement.x * restitution);
        }
        if z_collision {
            movement_after_bounce =
                movement_after_bounce.with(Axis::Z, -current_movement.z * restitution);
        }
        if self.vertical_collision {
            if self.vertical_collision_below {
                restitution = if !(-current_movement.y < self.get_effective_gravity())
                    && !self.is_suppressing_bounce()
                {
                    // `Math.max(restitution, getBlockBounciness(block))`: the table has
                    // no bounce restitution, only the effect class
                    if effect_state.detail.own_effect == Effect::Bounce {
                        return Err(Unsupported::Bounce(effect_pos));
                    }
                    mth::java_max(restitution, 0.0)
                } else {
                    0.0
                };
            }
            let gravity_compensation;
            let effective_drag;
            if restitution > 0.0 {
                let portion_with_movement = movement.y / current_movement.y;
                gravity_compensation = portion_with_movement * self.get_effective_gravity();
                effective_drag = mth::lerp(portion_with_movement, 1.0, self.get_air_drag() as f64);
            } else {
                gravity_compensation = 0.0;
                effective_drag = 1.0;
            }
            movement_after_bounce = movement_after_bounce.with(
                Axis::Y,
                (gravity_compensation - current_movement.y) * effective_drag * restitution,
            );
        }
        self.set_delta_movement(movement_after_bounce);
        Ok(())
    }

    /// `Entity.isSuppressingBounce()`.
    fn is_suppressing_bounce(&self) -> bool {
        self.is_shift_key_down()
    }

    /// `Entity.checkFallDamage`: the fall distance (the block's `fallOn` only deals
    /// damage).
    pub(crate) fn entity_check_fall_damage(&mut self, ya: f64, on_ground: bool) {
        if !self.is_in_water() && ya < 0.0 {
            self.fall_distance -= (ya as f32) as f64;
        }
        if on_ground {
            self.reset_fall_distance();
        }
    }

    /// `Entity.resetFallDistance()`.
    pub fn reset_fall_distance(&mut self) {
        self.fall_distance = 0.0;
    }

    /// `Entity.checkFallDistanceAccumulation()`.
    pub(crate) fn check_fall_distance_accumulation(&mut self) {
        if self.delta_movement.y > -0.5 && self.fall_distance > 1.0 {
            self.fall_distance = 1.0;
        }
    }

    /// `Entity.applyEffectsFromBlocks()` (from `LivingEntity.aiStep`), as a detector:
    /// `stepOn` of a bouncing block under the player, and `entityInside` of a stuck
    /// block anywhere between the box before travel and after (the game checks cells
    /// along each axis step of the move; this box contains them).
    pub(crate) fn apply_effects_from_blocks<L: Level + ?Sized>(
        &mut self,
        before_travel_box: &AABB,
        level: &L,
    ) -> R<()> {
        if self.on_ground {
            let effect_pos = self.get_on_pos_legacy(level)?;
            if level.get_block_state(effect_pos)?.detail.own_effect == Effect::Bounce {
                return Err(Unsupported::Bounce(effect_pos));
            }
        }
        let swept = before_travel_box
            .minmax(&self.get_bounding_box())
            .deflate(1.0E-5f32 as f64);
        if let Some(pos) =
            level::find_block(level, &swept, |s| s.detail.own_effect == Effect::Stuck)?
        {
            return Err(Unsupported::Stuck(pos));
        }
        Ok(())
    }

    /// `Entity.getOnPosLegacy()`.
    pub(crate) fn get_on_pos_legacy<L: Level + ?Sized>(&self, level: &L) -> R<BlockPos> {
        self.get_on_pos(0.2, level)
    }

    /// `Entity.getBlockPosBelowThatAffectsMyMovement()`.
    pub(crate) fn get_block_pos_below_that_affects_my_movement<L: Level + ?Sized>(
        &self,
        level: &L,
    ) -> R<BlockPos> {
        self.get_on_pos(0.500001, level)
    }

    /// `Entity.getOnPos(float)`.
    pub(crate) fn get_on_pos<L: Level + ?Sized>(&self, offset: f32, level: &L) -> R<BlockPos> {
        if let Some(get_on_pos) = self.main_supporting_block_pos {
            if !(offset > 1.0E-5) {
                return Ok(get_on_pos);
            }
            let below_state = level.get_block_state(get_on_pos)?;
            if (!(offset as f64 <= 0.5) || !below_state.is_fence())
                && !below_state.is_wall()
                && !below_state.is_fence_gate()
            {
                Ok(BlockPos {
                    y: mth::floor(self.position.y - offset as f64),
                    ..get_on_pos
                })
            } else {
                Ok(get_on_pos)
            }
        } else {
            let x_truncated = mth::floor(self.position.x);
            let y_truncated_below = mth::floor(self.position.y - offset as f64);
            let z_truncated = mth::floor(self.position.z);
            Ok(BlockPos {
                x: x_truncated,
                y: y_truncated_below,
                z: z_truncated,
            })
        }
    }

    /// `Entity.getBlockJumpFactor()`.
    pub(crate) fn get_block_jump_factor<L: Level + ?Sized>(&self, level: &L) -> R<f32> {
        let jump_factor_here = level
            .get_block_state(self.block_position())?
            .info
            .jump_factor;
        let jump_factor_below = level
            .get_block_state(self.get_block_pos_below_that_affects_my_movement(level)?)?
            .info
            .jump_factor;
        Ok(if jump_factor_here as f64 == 1.0 {
            jump_factor_below
        } else {
            jump_factor_here
        })
    }

    /// `Entity.getBlockSpeedFactor()`.
    pub(crate) fn entity_get_block_speed_factor<L: Level + ?Sized>(&self, level: &L) -> R<f32> {
        let state = level.get_block_state(self.block_position())?;
        let speed_factor_here = state.info.speed_factor;
        if !matches!(state.name(), "minecraft:water" | "minecraft:bubble_column") {
            Ok(if speed_factor_here as f64 == 1.0 {
                level
                    .get_block_state(self.get_block_pos_below_that_affects_my_movement(level)?)?
                    .info
                    .speed_factor
            } else {
                speed_factor_here
            })
        } else {
            Ok(speed_factor_here)
        }
    }

    /// `Entity.collide(Vec3)`: the L-shaped move against blocks, then step-up.
    /// No entity colliders (boats, shulkers) and no world border.
    pub(crate) fn collide<L: Level + ?Sized>(&self, movement: Vec3, level: &L) -> R<Vec3> {
        let aabb = self.get_bounding_box();
        let movement_step = if movement.length_sqr() == 0.0 {
            movement
        } else {
            collide_bounding_box(movement, &aabb, level)?
        };
        let x_collision = movement.x != movement_step.x;
        let y_collision = movement.y != movement_step.y;
        let z_collision = movement.z != movement_step.z;
        let on_ground_after_collision = y_collision && movement.y < 0.0;
        if self.max_up_step() > 0.0
            && (on_ground_after_collision || self.on_ground)
            && (x_collision || z_collision)
        {
            let grounded_aabb = if on_ground_after_collision {
                aabb.move_(0.0, movement_step.y, 0.0)
            } else {
                aabb
            };
            let mut step_up_aabb =
                grounded_aabb.expand_towards(movement.x, self.max_up_step() as f64, movement.z);
            if !on_ground_after_collision {
                step_up_aabb = step_up_aabb.expand_towards(0.0, -1.0E-5f32 as f64, 0.0);
            }
            let colliders = collect_colliders_ignoring_world_border(level, &step_up_aabb)?;
            let step_height_to_skip = movement_step.y as f32;
            let candidate_step_up_heights = collect_candidate_step_up_heights(
                &grounded_aabb,
                &colliders,
                self.max_up_step(),
                step_height_to_skip,
            );
            for candidate_step_up_height in candidate_step_up_heights {
                let step_from_ground = collide_with_shapes(
                    Vec3::new(movement.x, candidate_step_up_height as f64, movement.z),
                    &grounded_aabb,
                    &colliders,
                );
                if step_from_ground.horizontal_distance_sqr()
                    > movement_step.horizontal_distance_sqr()
                {
                    let distance_to_ground = aabb.min_y - grounded_aabb.min_y;
                    return Ok(step_from_ground.subtract_xyz(0.0, distance_to_ground, 0.0));
                }
            }
        }
        Ok(movement_step)
    }

    /// `Entity.moveRelative(float, Vec3)`.
    pub(crate) fn move_relative(&mut self, speed: f32, input: Vec3) {
        let delta = get_input_vector(input, speed, self.y_rot);
        self.set_delta_movement(self.delta_movement.add(delta));
    }

    /// `Entity.calculateViewVector(float, float)`.
    pub fn calculate_view_vector(&self, x_rot: f32, y_rot: f32) -> Vec3 {
        let real_x_rot = x_rot * mth::DEG_TO_RAD;
        let real_y_rot = -y_rot * mth::DEG_TO_RAD;
        let y_cos = mth::cos(real_y_rot as f64);
        let y_sin = mth::sin(real_y_rot as f64);
        let x_cos = mth::cos(real_x_rot as f64);
        let x_sin = mth::sin(real_x_rot as f64);
        Vec3::new(
            (y_sin * x_cos) as f64,
            (-x_sin) as f64,
            (y_cos * x_cos) as f64,
        )
    }

    /// `Entity.getLookAngle()`.
    pub fn get_look_angle(&self) -> Vec3 {
        self.calculate_view_vector(self.x_rot, self.y_rot)
    }
}

/// `Entity.getInputVector(Vec3, float, float)`.
pub(crate) fn get_input_vector(input: Vec3, speed: f32, y_rot: f32) -> Vec3 {
    let length = input.length_sqr();
    if length < 1.0E-7 {
        return Vec3::ZERO;
    }
    let movement = (if length > 1.0 {
        input.normalize()
    } else {
        input
    })
    .scale(speed as f64);
    let sin = mth::sin((y_rot * mth::DEG_TO_RAD) as f64);
    let cos = mth::cos((y_rot * mth::DEG_TO_RAD) as f64);
    Vec3::new(
        movement.x * cos as f64 - movement.z * sin as f64,
        movement.y,
        movement.z * cos as f64 + movement.x * sin as f64,
    )
}

/// `Entity.collectCandidateStepUpHeights`: the y coordinates of the colliders'
/// grids, relative to the box bottom, from 0 to `max_step_height`, deduplicated
/// (`FloatArraySet`, by bits) and sorted (`Float.compare` order).
fn collect_candidate_step_up_heights(
    bounding_box: &AABB,
    colliders: &[VoxelShape],
    max_step_height: f32,
    step_height_to_skip: f32,
) -> Vec<f32> {
    let mut candidates: Vec<f32> = Vec::with_capacity(4);
    for collider in colliders {
        for &coord in collider.get_coords(Axis::Y) {
            let relative_coord = (coord - bounding_box.min_y) as f32;
            if !(relative_coord < 0.0) && relative_coord != step_height_to_skip {
                if relative_coord > max_step_height {
                    break;
                }
                if !candidates
                    .iter()
                    .any(|c| c.to_bits() == relative_coord.to_bits())
                {
                    candidates.push(relative_coord);
                }
            }
        }
    }
    candidates.sort_by(|a, b| a.total_cmp(b));
    candidates
}

/// `Entity.collideBoundingBox(Entity, Vec3, AABB, Level, List)`.
pub(crate) fn collide_bounding_box<L: Level + ?Sized>(
    movement: Vec3,
    bounding_box: &AABB,
    level: &L,
) -> R<Vec3> {
    let colliders =
        collect_colliders_ignoring_world_border(level, &bounding_box.expand_towards_vec(movement))?;
    Ok(collide_with_shapes(movement, bounding_box, &colliders))
}

/// `Entity.collectCollidersIgnoringWorldBorder`: block shapes only (no entities;
/// the world border is far from any course).
fn collect_colliders_ignoring_world_border<L: Level + ?Sized>(
    level: &L,
    bounding_box: &AABB,
) -> R<Vec<VoxelShape>> {
    level::get_block_collisions(level, bounding_box)
}

/// `Entity.collideWithShapes(Vec3, AABB, List)`: one axis at a time in
/// `Direction.axisStepOrder`, each from where the previous axes left the box.
pub(crate) fn collide_with_shapes(
    movement: Vec3,
    bounding_box: &AABB,
    shapes: &[VoxelShape],
) -> Vec3 {
    if shapes.is_empty() {
        return movement;
    }
    let mut resolved_movement = Vec3::ZERO;
    for axis in axis_step_order(movement) {
        let axis_movement = movement.get(axis);
        if axis_movement != 0.0 {
            let collision = shapes::collide(
                axis,
                &bounding_box.move_vec(resolved_movement),
                shapes,
                axis_movement,
            );
            resolved_movement = resolved_movement.with(axis, collision);
        }
    }
    resolved_movement
}
