//! `net.minecraft.world.entity.player.Player`: its `tick`, `aiStep` and the
//! movement methods it overrides.

use crate::level::{self, Level};
use crate::mth;
use crate::phys::{AABB, Vec3};
use crate::{LocalPlayer, Pose, TickEvents, Unsupported};

type R<T> = Result<T, Unsupported>;

impl LocalPlayer {
    /// `Player.tick()`: `LivingEntity.tick()`, the world-edge clamp, then
    /// `updatePlayerPose()`. Sleeping, XP, items and cooldowns don't move the player;
    /// `updateIsUnderwater` is covered by the fluid check in `base_tick`.
    pub(crate) fn player_tick<L: Level + ?Sized>(
        &mut self,
        keys: crate::Input,
        level: &L,
    ) -> R<TickEvents> {
        // `noPhysics = isSpectator()`: false
        let events = self.living_entity_tick(keys, level)?;
        let nx = mth::clamp(self.position.x, -2.9999999E7, 2.9999999E7);
        let nz = mth::clamp(self.position.z, -2.9999999E7, 2.9999999E7);
        if nx != self.position.x || nz != self.position.z {
            self.set_pos(Vec3::new(nx, self.position.y, nz));
        }
        self.update_player_pose(level)?;
        Ok(events)
    }

    /// `Player.updatePlayerPose()`.
    fn update_player_pose<L: Level + ?Sized>(&mut self, level: &L) -> R<()> {
        if self.can_player_fit_within_blocks_and_entities_when(Pose::Swimming, level)? {
            let desired_pose = self.get_desired_pose();
            // not a spectator or passenger
            let actual_pose = if self
                .can_player_fit_within_blocks_and_entities_when(desired_pose, level)?
            {
                desired_pose
            } else if self.can_player_fit_within_blocks_and_entities_when(Pose::Crouching, level)? {
                Pose::Crouching
            } else {
                Pose::Swimming
            };
            self.set_pose(actual_pose);
        }
        Ok(())
    }

    /// `Player.getDesiredPose()`: not sleeping, swimming or spin-attacking.
    fn get_desired_pose(&self) -> Pose {
        if self.fall_flying {
            Pose::FallFlying
        } else if self.is_shift_key_down() {
            Pose::Crouching
        } else {
            Pose::Standing
        }
    }

    /// `Player.canPlayerFitWithinBlocksAndEntitiesWhen(Pose)`.
    pub(crate) fn can_player_fit_within_blocks_and_entities_when<L: Level + ?Sized>(
        &self,
        new_pose: Pose,
        level: &L,
    ) -> R<bool> {
        level::no_collision(
            level,
            &new_pose.make_bounding_box(self.position).deflate(1.0E-7),
        )
    }

    /// `Player.aiStep()`: `LivingEntity.aiStep()`. The rest (regeneration, inventory,
    /// pickup, `setSpeed` of a field `Player.getSpeed()` ignores) doesn't move it.
    pub(crate) fn player_ai_step<L: Level + ?Sized>(&mut self, level: &L) -> R<()> {
        self.living_entity_ai_step(level)
    }

    /// `Player.travel(Vec3)`: not a passenger, swimming or flying.
    pub(crate) fn travel<L: Level + ?Sized>(&mut self, input: Vec3, level: &L) -> R<()> {
        self.living_entity_travel(input, level)
    }

    /// `Player.maybeBackOffFromEdge(Vec3, MoverType)` for `MoverType.SELF`: sneaking
    /// stops at ledges taller than the step height.
    pub(crate) fn maybe_back_off_from_edge<L: Level + ?Sized>(
        &self,
        delta: Vec3,
        level: &L,
    ) -> R<Vec3> {
        let max_down_step = self.max_up_step();
        if !(delta.y > 0.0)
            && self.is_staying_on_ground_surface()
            && self.is_above_ground(max_down_step, level)?
        {
            let mut delta_x = delta.x;
            let mut delta_z = delta.z;
            let step_x = mth::signum(delta_x) * 0.05;
            let step_z = mth::signum(delta_z) * 0.05;
            while delta_x != 0.0
                && self.can_fall_at_least(delta_x, 0.0, max_down_step as f64, level)?
            {
                if delta_x.abs() <= 0.05 {
                    delta_x = 0.0;
                    break;
                }
                delta_x -= step_x;
            }
            while delta_z != 0.0
                && self.can_fall_at_least(0.0, delta_z, max_down_step as f64, level)?
            {
                if delta_z.abs() <= 0.05 {
                    delta_z = 0.0;
                    break;
                }
                delta_z -= step_z;
            }
            while delta_x != 0.0
                && delta_z != 0.0
                && self.can_fall_at_least(delta_x, delta_z, max_down_step as f64, level)?
            {
                if delta_x.abs() <= 0.05 {
                    delta_x = 0.0;
                } else {
                    delta_x -= step_x;
                }
                if delta_z.abs() <= 0.05 {
                    delta_z = 0.0;
                } else {
                    delta_z -= step_z;
                }
            }
            Ok(Vec3::new(delta_x, delta.y, delta_z))
        } else {
            Ok(delta)
        }
    }

    /// `Player.isAboveGround(float)`.
    fn is_above_ground<L: Level + ?Sized>(&self, max_down_step: f32, level: &L) -> R<bool> {
        Ok(self.on_ground
            || self.fall_distance < max_down_step as f64
                && !self.can_fall_at_least(
                    0.0,
                    0.0,
                    max_down_step as f64 - self.fall_distance,
                    level,
                )?)
    }

    /// `Player.canFallAtLeast(double, double, double)`.
    fn can_fall_at_least<L: Level + ?Sized>(
        &self,
        delta_x: f64,
        delta_z: f64,
        min_height: f64,
        level: &L,
    ) -> R<bool> {
        let bb = self.get_bounding_box();
        level::no_collision(
            level,
            &AABB::new(
                bb.min_x + 1.0E-7 + delta_x,
                bb.min_y - min_height - 1.0E-7,
                bb.min_z + 1.0E-7 + delta_z,
                bb.max_x - 1.0E-7 + delta_x,
                bb.min_y,
                bb.max_z - 1.0E-7 + delta_z,
            ),
        )
    }

    /// `Player.isStayingOnGroundSurface()`.
    fn is_staying_on_ground_surface(&self) -> bool {
        self.is_shift_key_down()
    }

    /// `Player.canGlide()`: not creative-flying.
    fn can_glide(&self) -> bool {
        self.living_entity_can_glide()
    }

    /// `Player.getSpeed()`.
    pub(crate) fn get_speed(&self) -> f32 {
        self.movement_speed_attribute() as f32
    }

    /// `Player.tryToStartFallFlying()`.
    pub(crate) fn try_to_start_fall_flying(&mut self) -> bool {
        if !self.fall_flying && self.can_glide() && !self.is_in_water() {
            self.start_fall_flying();
            true
        } else {
            false
        }
    }

    /// `Player.startFallFlying()`: shared flag 7.
    fn start_fall_flying(&mut self) {
        self.fall_flying = true;
    }

    /// `Player.getBlockSpeedFactor()`: 1 while gliding.
    pub(crate) fn get_block_speed_factor<L: Level + ?Sized>(&self, level: &L) -> R<f32> {
        if !self.fall_flying {
            self.entity_get_block_speed_factor(level)
        } else {
            Ok(1.0)
        }
    }

    /// `Player.getFlyingSpeed()`, not creative-flying.
    pub(crate) fn get_flying_speed(&self) -> f32 {
        if self.sprinting { 0.025999999 } else { 0.02 }
    }
}
