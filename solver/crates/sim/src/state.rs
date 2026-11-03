//! Player state at two levels (ARCHITECTURE.md, "State"): the full state, every field
//! the game's movement reads, so a cut anywhere can be resumed; and the flight state,
//! position and velocity, for normal flight. [`FlightState::lift`] and
//! [`FullState::flight`] move between them.
//!
//! **Normal flight** is a tick that the elytra path flies and nothing else touches.
//! What the state alone decides is [`FullState::flight`]: gliding, off the ground, in
//! the gliding pose. The tick's keys must not include sneak. why? sneaking backs off
//! ledges (`Player.maybeBackOffFromEdge`) when the ground is near; the other keys only
//! change sprint and flag bookkeeping while gliding, never the move. What the world
//! decides (fluid, a climbable, the box inside a block, landing, unknown space) the
//! faithful kernel's step checks every tick and reports as a
//! [`crate::faithful::Event`].

use world::BlockPos;

/// Position and velocity at the end of a tick, after its move.
///
/// `pos` is the entity position (feet, box center in x and z), `vel` the
/// `deltaMovement` the next tick starts from. In contact-free flight
/// `pos[t] = pos[t - 1] + vel[t]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlightState<T> {
    pub pos: [T; 3],
    pub vel: [T; 3],
}

/// One tick's rotation, in degrees, as the game holds it (`Entity.yRot`, `xRot`).
///
/// `yaw` is unbounded (the client never wraps it). `pitch` is in [-90, 90], positive
/// looking down; the game clamps it there (`Entity.setXRot`), and so does the faithful
/// kernel; the generic kernel doesn't. The faithful kernel takes `Input<f32>`, the
/// game's type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Input<T> {
    pub yaw: T,
    pub pitch: T,
}

/// The movement keys held (`net.minecraft.world.entity.player.Input`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Keys {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
}

/// `net.minecraft.world.entity.Pose`, the poses player movement uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pose {
    #[default]
    Standing,
    FallFlying,
    Swimming,
    SpinAttack,
    Crouching,
}

/// Every field the port's client tick reads, by what it is for. Fixed facts (survival,
/// no effects, default attributes, auto-jump off) are not fields; see the port's
/// README for the list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FullState {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    /// The rotation the last tick flew with. Each tick sets its own before reading it,
    /// so this matters only for a non-finite input, which the game ignores.
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    /// The last move's collision flags. `horizontal_collision` and
    /// `minor_horizontal_collision` decide whether sprint stops next tick.
    pub horizontal_collision: bool,
    pub vertical_collision: bool,
    pub vertical_collision_below: bool,
    pub minor_horizontal_collision: bool,
    /// `Entity.mainSupportingBlockPos` and `onGroundNoBlocks`.
    pub supporting_block: Option<BlockPos>,
    pub on_ground_no_blocks: bool,
    pub fall_distance: f64,
    pub pose: Pose,
    /// Shared flag 7. Only the server ends a glide.
    pub fall_flying: bool,
    /// Shared flag 3, and the `MOVEMENT_SPEED` modifier only `setSprinting` changes.
    pub sprinting: bool,
    pub sprint_speed_modifier: bool,
    /// An elytra (or another glider) is equipped.
    pub glider_equipped: bool,
    pub no_jump_delay: i32,
    /// `LivingEntity.jumping`, `xxa`, `yya`, `zza`: the last tick's applied input.
    pub jumping: bool,
    pub move_input: [f32; 3],
    /// `LocalPlayer.input` after the last tick: its keys and move vector. A tick reads
    /// them as the previous keys (jump edge, sprint double tap) before loading its own.
    pub keys: Keys,
    pub move_vector: [f32; 2],
    pub sprint_trigger_time: i32,
    pub auto_jump_time: i32,
    pub crouching: bool,
}

impl FlightState<f64> {
    /// The full state of this flight state: what normal flight implies, and neutral
    /// values for the rest.
    ///
    /// Implied: gliding (`fall_flying`, glider equipped), off the ground with no
    /// supporting block, the gliding pose, no downward collision.
    ///
    /// Neutral, because normal flight doesn't determine them and they never change a
    /// normal-flight move: rotation 0, the other collision flags clear, fall distance
    /// 0, not sprinting, no keys or applied input, timers 0, not crouching. They do
    /// matter after normal flight ends (sprint at a jump, fall distance at a landing),
    /// so resuming a recorded run takes the recorded full state, not a lift.
    pub fn lift(&self) -> FullState {
        FullState {
            pos: self.pos,
            vel: self.vel,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            horizontal_collision: false,
            vertical_collision: false,
            vertical_collision_below: false,
            minor_horizontal_collision: false,
            supporting_block: None,
            on_ground_no_blocks: false,
            fall_distance: 0.0,
            pose: Pose::FallFlying,
            fall_flying: true,
            sprinting: false,
            sprint_speed_modifier: false,
            glider_equipped: true,
            no_jump_delay: 0,
            jumping: false,
            move_input: [0.0; 3],
            keys: Keys::default(),
            move_vector: [0.0; 2],
            sprint_trigger_time: 0,
            auto_jump_time: 0,
            crouching: false,
        }
    }
}

impl FullState {
    /// The flight state, if the state is in normal flight: gliding, off the ground,
    /// in the gliding pose. None otherwise.
    ///
    /// why each: `fall_flying` selects the elytra path; on the ground, a horizontal
    /// collision tries a step-up; in another pose the box isn't 0.6 tall (the tick a
    /// glide starts still moves standing). `glider_equipped` is not a condition: the
    /// client reads it only to start a glide.
    ///
    /// Everything else a normal-flight move reads is position, velocity and the
    /// tick's input; the fields [`FlightState::lift`] fills with neutral values are
    /// written before they are read, or only feed sprint and flags.
    pub fn flight(&self) -> Option<FlightState<f64>> {
        (self.fall_flying && !self.on_ground && self.pose == Pose::FallFlying).then_some(
            FlightState {
                pos: self.pos,
                vel: self.vel,
            },
        )
    }
}
