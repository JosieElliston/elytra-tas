//! Mechanical port of MC 26.2 client-side local player movement: `LocalPlayer`,
//! `Player`, `LivingEntity` and `Entity`, transcribed method by method with the
//! Java's float/double casts and `Mth`'s sine table. See README.md for scope.
//!
//! One struct, [`LocalPlayer`], holds the Java fields these methods read and write.
//! Its methods are split into one file per Java class (`entity.rs`,
//! `living_entity.rs`, `player.rs`, `local_player.rs`) and named after the Java
//! method; where a subclass overrides a method, the override keeps the Java name and
//! the superclass version is prefixed with its class (`entity_get_block_speed_factor`).
//! [`tick`] is the entry point, in game order.

// The Java's shape is kept over these lints. why? `!(a < b)` is not `a >= b` for NaN,
// and nested or late-initialized branches mirror the Java line by line.
#![allow(
    clippy::neg_cmp_op_on_partial_ord,
    clippy::collapsible_if,
    clippy::needless_late_init
)]
// `Vec3::add` etc. are the Java method names.
#![allow(clippy::should_implement_trait)]

mod client_packet_listener;
mod entity;
pub mod level;
mod living_entity;
mod local_player;
pub mod mth;
pub mod phys;
mod player;
pub mod shapes;

pub use level::{BlockMap, BlockState, Level};
pub use phys::{AABB, Axis, Vec2, Vec3};
pub use world::BlockPos;

/// A tick the port does not simulate faithfully, with the block that caused it. The
/// player state is unspecified after one: the tick stopped partway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    /// A query touched space the world doesn't cover.
    UnknownSpace(BlockPos),
    /// Water or lava overlaps the player (`Entity.updateFluidInteraction` would find it).
    Fluid(BlockPos),
    /// `LivingEntity.onClimbable` (ladders, vines, scaffolding, ...).
    Climbable(BlockPos),
    /// Cobweb, berry bush, powder snow: `Entity.makeStuckInBlock`.
    Stuck(BlockPos),
    /// Landing on or walking over a slime block or bed (restitution, `stepOn`).
    Bounce(BlockPos),
}

impl From<world::UnknownSpace> for Unsupported {
    fn from(u: world::UnknownSpace) -> Self {
        Unsupported::UnknownSpace(u.0)
    }
}

/// `net.minecraft.world.entity.player.Input`: the movement keys held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Input {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub shift: bool,
    pub sprint: bool,
}

impl Input {
    pub const EMPTY: Input = Input {
        forward: false,
        backward: false,
        left: false,
        right: false,
        jump: false,
        shift: false,
        sprint: false,
    };

    /// A `.tas` tape's keys: a subset of `WASDJR` (R = sprint), or `-`.
    pub fn from_tape(keys: &str) -> Input {
        Input {
            forward: keys.contains('W'),
            backward: keys.contains('S'),
            left: keys.contains('A'),
            right: keys.contains('D'),
            jump: keys.contains('J'),
            shift: false,
            sprint: keys.contains('R'),
        }
    }

    /// The logger's `T` record input bits: 1 fwd 2 back 4 left 8 right 16 jump 32 shift
    /// 64 sprint.
    pub fn from_bits(bits: u32) -> Input {
        Input {
            forward: bits & 1 != 0,
            backward: bits & 2 != 0,
            left: bits & 4 != 0,
            right: bits & 8 != 0,
            jump: bits & 16 != 0,
            shift: bits & 32 != 0,
            sprint: bits & 64 != 0,
        }
    }
}

/// `net.minecraft.client.player.ClientInput` as `KeyboardInput` fills it: the keys
/// and the move vector from the last `tick()`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ClientInput {
    pub key_presses: Input,
    pub move_vector: Vec2,
}

impl ClientInput {
    /// `KeyboardInput.tick()`, with `keys` standing for the key mappings' `isDown()`.
    pub fn tick(&mut self, keys: Input) {
        fn calculate_impulse(positive: bool, negative: bool) -> f32 {
            if positive == negative {
                0.0
            } else if positive {
                1.0
            } else {
                -1.0
            }
        }
        self.key_presses = keys;
        let forward_impulse = calculate_impulse(keys.forward, keys.backward);
        let left_impulse = calculate_impulse(keys.left, keys.right);
        self.move_vector = Vec2::new(left_impulse, forward_impulse).normalized();
    }

    /// `ClientInput.hasForwardImpulse()`.
    pub fn has_forward_impulse(&self) -> bool {
        self.move_vector.y > 1.0E-5
    }

    /// `ClientInput.makeJump()`.
    pub fn make_jump(&mut self) {
        self.key_presses.jump = true;
    }
}

/// `net.minecraft.world.entity.Pose`, the ones a gliding or walking player reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pose {
    #[default]
    Standing,
    FallFlying,
    Swimming,
    SpinAttack,
    Crouching,
}

impl Pose {
    /// `Avatar.POSES` (scale 1): width, height, eye height.
    pub fn dimensions(self) -> (f32, f32, f32) {
        match self {
            Pose::Standing => (0.6, 1.8, 1.62),
            Pose::FallFlying | Pose::Swimming | Pose::SpinAttack => (0.6, 0.6, 0.4),
            Pose::Crouching => (0.6, 1.5, 1.27),
        }
    }

    /// `EntityDimensions.makeBoundingBox(x, y, z)`: the half width is a float.
    pub fn make_bounding_box(self, pos: Vec3) -> AABB {
        let (width, height, _) = self.dimensions();
        let w = width / 2.0;
        let h = height;
        AABB::new(
            pos.x - w as f64,
            pos.y,
            pos.z - w as f64,
            pos.x + w as f64,
            pos.y + h as f64,
            pos.z + w as f64,
        )
    }
}

/// One tick's input: the rotation set before the tick (mouse, or `tas_play`'s tick
/// handler) and the keys `KeyboardInput.tick()` reads inside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickInput {
    pub y_rot: f32,
    pub x_rot: f32,
    pub keys: Input,
}

/// What a tick sent that the caller may care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TickEvents {
    /// `ServerboundPlayerCommandPacket(START_FALL_FLYING)` went out.
    pub start_fall_flying: bool,
}

/// The local player's movement state: the Java fields the ported methods read or
/// write, by owning class. The bounding box isn't stored: the game recomputes it from
/// position and pose on every `setPos` and pose change, so it is
/// [`LocalPlayer::get_bounding_box`].
///
/// Fixed, not state: survival or adventure (no `abilities.flying`/`mayfly`), not a
/// passenger, no effects, no item in use, food above 6, elytra in the chest slot
/// unless `glider_equipped` is false, default attributes, auto-jump off, sprint
/// window 7 (the option default).
#[derive(Debug, Clone, PartialEq)]
pub struct LocalPlayer {
    // Entity
    pub position: Vec3,
    pub delta_movement: Vec3,
    pub y_rot: f32,
    pub x_rot: f32,
    pub on_ground: bool,
    pub horizontal_collision: bool,
    pub vertical_collision: bool,
    pub vertical_collision_below: bool,
    pub minor_horizontal_collision: bool,
    pub main_supporting_block_pos: Option<BlockPos>,
    pub on_ground_no_blocks: bool,
    pub fall_distance: f64,
    pub pose: Pose,
    /// Shared flag 7. Only the server ends a glide: the caller clears this when the
    /// server's flags say so (the client clears it only on a climbable, which is
    /// [`Unsupported::Climbable`] here).
    pub fall_flying: bool,
    /// Shared flag 3.
    pub sprinting: bool,
    // LivingEntity
    pub no_jump_delay: i32,
    pub jumping: bool,
    pub xxa: f32,
    pub yya: f32,
    pub zza: f32,
    /// The sprinting modifier on `MOVEMENT_SPEED`. Locally only `setSprinting` changes
    /// it, but the server's `update_attributes` echo rewrites it a tick late, as its
    /// `set_entity_data` does the sprint flag (oracle, 2026-11-03); a caller replaying
    /// real play applies those between ticks.
    pub sprint_speed_modifier: bool,
    /// `canGlideUsing` holds for some equipped item.
    pub glider_equipped: bool,
    // LocalPlayer
    pub input: ClientInput,
    pub sprint_trigger_time: i32,
    pub auto_jump_time: i32,
    pub crouching: bool,
}

impl LocalPlayer {
    /// A player standing still on the ground at `position` (`on_ground` set, no
    /// supporting block found yet: the first move finds it).
    pub fn new(position: Vec3, y_rot: f32, x_rot: f32) -> LocalPlayer {
        LocalPlayer {
            position,
            delta_movement: Vec3::ZERO,
            y_rot,
            x_rot,
            on_ground: true,
            horizontal_collision: false,
            vertical_collision: false,
            vertical_collision_below: false,
            minor_horizontal_collision: false,
            main_supporting_block_pos: None,
            on_ground_no_blocks: false,
            fall_distance: 0.0,
            pose: Pose::Standing,
            fall_flying: false,
            sprinting: false,
            no_jump_delay: 0,
            jumping: false,
            xxa: 0.0,
            yya: 0.0,
            zza: 0.0,
            sprint_speed_modifier: false,
            glider_equipped: true,
            input: ClientInput::default(),
            sprint_trigger_time: 0,
            auto_jump_time: 0,
            crouching: false,
        }
    }
}

/// One client tick of the local player, in game order: the rotation is set
/// (`Entity.setYRot`/`setXRot`), then `LocalPlayer.tick()` runs (`Entity.baseTick`,
/// `LocalPlayer.aiStep` down to `LivingEntity.travel`, then `Player.updatePlayerPose`).
pub fn tick<L: Level + ?Sized>(
    player: &mut LocalPlayer,
    input: &TickInput,
    level: &L,
) -> Result<TickEvents, Unsupported> {
    player.set_y_rot(input.y_rot);
    player.set_x_rot(input.x_rot);
    player.tick(input.keys, level)
}
