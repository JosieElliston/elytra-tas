//! `net.minecraft.world.entity.LivingEntity`: `aiStep`, `travel` and what they call.

use crate::level::Level;
use crate::mth;
use crate::phys::{AABB, Vec3};
use crate::{LocalPlayer, Pose, Unsupported};

type R<T> = Result<T, Unsupported>;

/// `Attributes.MOVEMENT_SPEED` base for players: `Player.createAttributes` adds `0.1F`.
const MOVEMENT_SPEED_BASE: f64 = 0.1f32 as f64;
/// `LivingEntity.SPEED_MODIFIER_SPRINTING`: `0.3F`, `ADD_MULTIPLIED_TOTAL`.
const SPRINTING_MODIFIER: f64 = 0.3f32 as f64;
/// `Attributes.JUMP_STRENGTH` default, `0.42F`.
const JUMP_STRENGTH: f64 = 0.42f32 as f64;
/// `Attributes.GRAVITY` default.
const GRAVITY: f64 = 0.08;
/// `Attributes.STEP_HEIGHT` default.
const STEP_HEIGHT: f64 = 0.6;
/// `Attributes.FRICTION_MODIFIER` and `AIR_DRAG_MODIFIER` defaults.
const FRICTION_MODIFIER: f64 = 1.0;
const AIR_DRAG_MODIFIER: f64 = 1.0;
/// `Attributes.BOUNCINESS` default.
const BOUNCINESS: f64 = 0.0;

/// `LivingEntity.computeModifiedFriction(float, float)`.
fn compute_modified_friction(friction: f32, modifier: f32) -> f32 {
    mth::clamp_f32(1.0 - (1.0 - friction) * modifier, 0.0, 1.0)
}

impl LocalPlayer {
    /// `LivingEntity.tick()`: `Entity.tick()` (`baseTick`), then `aiStep()`. The rest
    /// (body rotation, arrows, sleeping) doesn't touch movement.
    pub(crate) fn living_entity_tick<L: Level + ?Sized>(
        &mut self,
        keys: crate::Input,
        level: &L,
    ) -> R<crate::TickEvents> {
        self.base_tick(level)?;
        self.ai_step(keys, level)
    }

    /// `LivingEntity.aiStep()`.
    pub(crate) fn living_entity_ai_step<L: Level + ?Sized>(&mut self, level: &L) -> R<()> {
        if self.no_jump_delay > 0 {
            self.no_jump_delay -= 1;
        }
        // `isInterpolating()` is false and `canSimulateMovement()` true for the local
        // player; the head lerp and equipment tick don't move it.
        let movement = self.delta_movement;
        let mut dx = movement.x;
        let mut dy = movement.y;
        let mut dz = movement.z;
        // `this.is(EntityTypes.PLAYER)`
        if movement.horizontal_distance_sqr() < 9.0E-6 {
            dx = 0.0;
            dz = 0.0;
        }
        if movement.y.abs() < 0.003 {
            dy = 0.0;
        }
        self.set_delta_movement(Vec3::new(dx, dy, dz));
        self.apply_input();
        // `isImmobile()` is false (alive, awake); `serverAiStep` is server-side.
        if self.jumping && self.is_affected_by_fluids() {
            // `getFluidHeight`: 0 out of fluid, and fluids are errors here
            let fluid_height = 0.0;
            let in_water_and_has_fluid_height = self.is_in_water() && fluid_height > 0.0;
            let fluid_jump_threshold = self.get_fluid_jump_threshold();
            if !in_water_and_has_fluid_height
                || self.on_ground && !(fluid_height > fluid_jump_threshold)
            {
                if !self.is_in_lava() {
                    if (self.on_ground
                        || in_water_and_has_fluid_height && fluid_height <= fluid_jump_threshold)
                        && self.no_jump_delay == 0
                    {
                        self.jump_from_ground(level)?;
                        self.no_jump_delay = 10;
                    }
                }
                // else `jumpInLiquid(FluidTags.LAVA)`: unreachable without lava
            }
            // else `jumpInLiquid(FluidTags.WATER)`: unreachable without water
        } else {
            self.no_jump_delay = 0;
        }
        if self.fall_flying {
            self.update_fall_flying();
        }
        let before_travel_box: AABB = self.get_bounding_box();
        let input = Vec3::new(self.xxa as f64, self.yya as f64, self.zza as f64);
        // no slow falling or levitation; no controlled vehicle
        self.travel(input, level)?;
        // `!level.isClientSide() || isLocalInstanceAuthoritative()`: true
        self.apply_effects_from_blocks(&before_travel_box, level)?;
        Ok(())
    }

    /// `LivingEntity.isAffectedByFluids()` as `Player` overrides it: `!abilities.flying`.
    fn is_affected_by_fluids(&self) -> bool {
        true
    }

    /// `Entity.getFluidJumpThreshold()`.
    fn get_fluid_jump_threshold(&self) -> f64 {
        if (self.get_eye_height() as f64) < 0.4 {
            0.0
        } else {
            0.4
        }
    }

    /// `LivingEntity.updateFallFlying()`, client side: only the fall-distance cap. The
    /// server's `canGlide` check (which ends the glide) is not client physics.
    fn update_fall_flying(&mut self) {
        self.check_fall_distance_accumulation();
    }

    /// `LivingEntity.travel(Vec3)`. `shouldTravelInFluid` is false: fluids are
    /// [`Unsupported::Fluid`] before this runs.
    pub(crate) fn living_entity_travel<L: Level + ?Sized>(
        &mut self,
        input: Vec3,
        level: &L,
    ) -> R<()> {
        if self.fall_flying {
            self.travel_fall_flying(input, level)
        } else {
            self.travel_in_air(input, level)
        }
    }

    /// `LivingEntity.travelInAir(Vec3)`.
    fn travel_in_air<L: Level + ?Sized>(&mut self, input: Vec3, level: &L) -> R<()> {
        let pos_below = self.get_block_pos_below_that_affects_my_movement(level)?;
        let block_friction: f32 = if self.on_ground {
            compute_modified_friction(
                level.get_block_state(pos_below)?.info.friction,
                FRICTION_MODIFIER as f32,
            )
        } else {
            1.0
        };
        let movement =
            self.handle_relative_friction_and_calculate_movement(input, block_friction, level)?;
        let mut movement_y = movement.y;
        // no levitation; client side: `level.hasChunkAt(posBelow)`, which is an
        // UnknownSpace error here instead of the client's `movementY = -0.1`
        level.get_block_state(pos_below)?;
        movement_y -= self.get_effective_gravity();
        // `shouldDiscardFriction()` is false
        let entity_air_drag_modifier = AIR_DRAG_MODIFIER as f32;
        let air_drag = compute_modified_friction(0.91, entity_air_drag_modifier);
        let friction = block_friction * air_drag;
        // `omnidirectionalAirMover()` is false
        let vertical_friction = compute_modified_friction(0.98, entity_air_drag_modifier);
        self.set_delta_movement(Vec3::new(
            movement.x * friction as f64,
            movement_y * vertical_friction as f64,
            movement.z * friction as f64,
        ));
        Ok(())
    }

    /// `LivingEntity.getAirDrag()`.
    pub(crate) fn get_air_drag(&self) -> f32 {
        compute_modified_friction(0.98, AIR_DRAG_MODIFIER as f32)
    }

    /// `LivingEntity.travelFallFlying(Vec3)`.
    fn travel_fall_flying<L: Level + ?Sized>(&mut self, input: Vec3, level: &L) -> R<()> {
        if self.on_climbable(level)? {
            self.travel_in_air(input, level)?;
            self.stop_fall_flying();
        } else {
            let last_movement = self.delta_movement;
            self.set_delta_movement(self.update_fall_flying_movement(last_movement));
            self.move_(self.delta_movement, level)?;
            // `handleFallFlyingCollisions` (wall damage) runs only on the server
        }
        Ok(())
    }

    /// `LivingEntity.stopFallFlying()`.
    fn stop_fall_flying(&mut self) {
        self.fall_flying = false;
    }

    /// `LivingEntity.updateFallFlyingMovement(Vec3)`.
    pub(crate) fn update_fall_flying_movement(&self, mut movement: Vec3) -> Vec3 {
        let look_angle = self.get_look_angle();
        let lean_angle = self.x_rot * mth::DEG_TO_RAD;
        let look_hor_length = (look_angle.x * look_angle.x + look_angle.z * look_angle.z).sqrt();
        let move_hor_length = movement.horizontal_distance();
        let gravity = self.get_effective_gravity();
        let lift_force = mth::square(mth::java_cos(lean_angle as f64));
        movement = movement.add_xyz(0.0, gravity * (-1.0 + lift_force * 0.75), 0.0);
        if movement.y < 0.0 && look_hor_length > 0.0 {
            let convert = movement.y * -0.1 * lift_force;
            movement = movement.add_xyz(
                look_angle.x * convert / look_hor_length,
                convert,
                look_angle.z * convert / look_hor_length,
            );
        }
        if lean_angle < 0.0 && look_hor_length > 0.0 {
            let convert = move_hor_length * -mth::sin(lean_angle as f64) as f64 * 0.04;
            movement = movement.add_xyz(
                -look_angle.x * convert / look_hor_length,
                convert * 3.2,
                -look_angle.z * convert / look_hor_length,
            );
        }
        if look_hor_length > 0.0 {
            movement = movement.add_xyz(
                (look_angle.x / look_hor_length * move_hor_length - movement.x) * 0.1,
                0.0,
                (look_angle.z / look_hor_length * move_hor_length - movement.z) * 0.1,
            );
        }
        movement.multiply(0.99f32 as f64, 0.98f32 as f64, 0.99f32 as f64)
    }

    /// `LivingEntity.handleRelativeFrictionAndCalculateMovement(Vec3, float)`.
    fn handle_relative_friction_and_calculate_movement<L: Level + ?Sized>(
        &mut self,
        input: Vec3,
        friction: f32,
        level: &L,
    ) -> R<Vec3> {
        self.move_relative(self.get_friction_influenced_speed(friction), input);
        let d = self.handle_on_climbable(self.delta_movement, level)?;
        self.set_delta_movement(d);
        self.move_(self.delta_movement, level)?;
        let movement = self.delta_movement;
        // `(horizontalCollision || jumping) && (onClimbable() || wasInPowderSnow && ...)`:
        // a climbable errors, and powder snow is a stuck block
        if self.horizontal_collision || self.jumping {
            self.on_climbable(level)?;
        }
        Ok(movement)
    }

    /// `LivingEntity.handleOnClimbable(Vec3)`: only when `onClimbable()`, which errors.
    fn handle_on_climbable<L: Level + ?Sized>(&mut self, delta: Vec3, level: &L) -> R<Vec3> {
        self.on_climbable(level)?;
        Ok(delta)
    }

    /// `LivingEntity.onClimbable()`, as a detector: a block in the world's `climbable`
    /// tag at the block position is [`Unsupported::Climbable`], except one also in
    /// `can_glide_through` while gliding, which the game flies through (EMC sends no
    /// `can_glide_through`, so there vines end the glide). A trapdoor over a ladder,
    /// which the game also climbs, is not detected.
    pub(crate) fn on_climbable<L: Level + ?Sized>(&self, level: &L) -> R<bool> {
        let ladder_check_pos = self.block_position();
        let state = level.get_block_state(ladder_check_pos)?;
        if self.fall_flying && state.detail.glide_through {
            Ok(false)
        } else if state.detail.climbable {
            Err(Unsupported::Climbable(ladder_check_pos))
        } else {
            Ok(false)
        }
    }

    /// `LivingEntity.getFrictionInfluencedSpeed(float)`.
    fn get_friction_influenced_speed(&self, block_friction: f32) -> f32 {
        if self.on_ground {
            if block_friction as f64 > 0.6 {
                self.get_speed() * (0.21600002 / (block_friction * block_friction * block_friction))
            } else {
                self.get_speed()
            }
        } else {
            self.get_flying_speed()
        }
    }

    /// `LivingEntity.getJumpPower()`.
    fn get_jump_power<L: Level + ?Sized>(&self, level: &L) -> R<f32> {
        let multiplier = 1.0f32;
        Ok(
            JUMP_STRENGTH as f32 * multiplier * self.get_block_jump_factor(level)?
                + self.get_jump_boost_power(),
        )
    }

    /// `LivingEntity.getJumpBoostPower()`: no jump boost.
    fn get_jump_boost_power(&self) -> f32 {
        0.0
    }

    /// `LivingEntity.jumpFromGround()`.
    pub(crate) fn jump_from_ground<L: Level + ?Sized>(&mut self, level: &L) -> R<()> {
        let jump_power = self.get_jump_power(level)?;
        if !(jump_power <= 1.0E-5) {
            let movement = self.delta_movement;
            self.set_delta_movement(Vec3::new(
                movement.x,
                mth::java_max(jump_power as f64, movement.y),
                movement.z,
            ));
            if self.sprinting {
                let angle = self.y_rot * mth::DEG_TO_RAD;
                self.add_delta_movement(Vec3::new(
                    -mth::sin(angle as f64) as f64 * 0.2,
                    0.0,
                    mth::cos(angle as f64) as f64 * 0.2,
                ));
            }
        }
        Ok(())
    }

    /// `LivingEntity.setSprinting(boolean)`: the shared flag and the speed modifier.
    pub fn set_sprinting(&mut self, is_sprinting: bool) {
        self.sprinting = is_sprinting;
        self.sprint_speed_modifier = is_sprinting;
    }

    /// `(float)getAttributeValue(MOVEMENT_SPEED)` (`AttributeInstance.calculateValue`):
    /// what `Player.getSpeed()` returns.
    pub(crate) fn movement_speed_attribute(&self) -> f64 {
        let base = MOVEMENT_SPEED_BASE;
        let mut result = base;
        if self.sprint_speed_modifier {
            result *= 1.0 + SPRINTING_MODIFIER;
        }
        // `RangedAttribute.sanitizeValue`: [0, 1024]
        mth::clamp(result, 0.0, 1024.0)
    }

    /// `LivingEntity.getEffectiveGravity()`: no slow falling.
    pub(crate) fn get_effective_gravity(&self) -> f64 {
        self.get_gravity()
    }

    /// `Entity.getGravity()`: no `NoGravity`.
    fn get_gravity(&self) -> f64 {
        GRAVITY
    }

    /// `LivingEntity.getEntityBounciness()`.
    pub(crate) fn get_entity_bounciness(&self) -> f64 {
        BOUNCINESS
    }

    /// `LivingEntity.maxUpStep()`: `(float)getAttributeValue(STEP_HEIGHT)`.
    pub fn max_up_step(&self) -> f32 {
        STEP_HEIGHT as f32
    }

    /// `LivingEntity.canGlide()`.
    pub(crate) fn living_entity_can_glide(&self) -> bool {
        // not a passenger, no levitation
        !self.on_ground && self.glider_equipped
    }

    /// `LivingEntity.checkFallDamage`: `updateFluidInteraction()` when not in water
    /// (a detector here), then `Entity.checkFallDamage`.
    pub(crate) fn check_fall_damage<L: Level + ?Sized>(
        &mut self,
        ya: f64,
        on_ground: bool,
        level: &L,
    ) -> R<()> {
        if !self.is_in_water() {
            self.update_fluid_interaction(level)?;
        }
        self.entity_check_fall_damage(ya, on_ground);
        Ok(())
    }

    /// `LivingEntity.isVisuallySwimming()`.
    pub fn is_visually_swimming(&self) -> bool {
        self.pose == Pose::Swimming || !self.fall_flying && self.pose == Pose::FallFlying
    }
}
