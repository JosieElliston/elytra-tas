//! Player state at two levels (ARCHITECTURE.md, "State"): the full state, every field
//! the game's movement reads, so a cut anywhere can be resumed; and the flight state,
//! position and velocity, for normal flight. [`FlightState::lift`] and
//! [`FullState::flight`] move between them. Between the two is [`GlideState`], a
//! glide that may touch the ground: what the faithful kernel's perfect-ebounce mode
//! steps.
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

/// A glide that may be on the ground: the state the faithful kernel's perfect-ebounce
/// mode ([`crate::faithful::Faithful::step_glide`]) steps. Gliding (`fall_flying`, the
/// gliding pose, glider equipped) is implied; the client never ends it, only the
/// server does, and a perfect bounce is one where it doesn't.
///
/// The fields past `vel` never change a move in the air. They decide the jump off the
/// ground (`on_ground`, `no_jump_delay`, `sprinting` for the sprint boost, the
/// supporting block for the jump factor), sprint's end (`keys`, the collision flags),
/// and the next landing's supporting block (`on_ground_no_blocks`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlideState {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub on_ground: bool,
    pub horizontal_collision: bool,
    pub minor_horizontal_collision: bool,
    pub supporting_block: Option<BlockPos>,
    pub on_ground_no_blocks: bool,
    /// Shared flag 3. While gliding it can only end (`canStartSprinting` needs the glide
    /// off), so a bounce keeps the sprint boost only if sprint survived the flight.
    pub sprinting: bool,
    pub no_jump_delay: i32,
    /// The last tick's keys: the jump edge and the forward impulse read them.
    pub keys: Keys,
}

impl FlightState<f64> {
    /// The glide state of this flight state, with [`FlightState::lift`]'s neutral
    /// values: not sprinting, no jump delay, no keys, no collision. A bounce from it
    /// jumps without the sprint boost.
    pub fn lift_glide(&self) -> GlideState {
        GlideState {
            pos: self.pos,
            vel: self.vel,
            on_ground: false,
            horizontal_collision: false,
            minor_horizontal_collision: false,
            supporting_block: None,
            on_ground_no_blocks: false,
            sprinting: false,
            no_jump_delay: 0,
            keys: Keys::default(),
        }
    }
}

impl GlideState {
    /// The flight state, when back in normal flight: off the ground. The other fields
    /// are dropped, so carry the glide state, not the flight state, through a flight
    /// that bounces again (its jump needs `sprinting` and `no_jump_delay`).
    pub fn flight(&self) -> Option<FlightState<f64>> {
        (!self.on_ground).then_some(FlightState {
            pos: self.pos,
            vel: self.vel,
        })
    }

    /// The full state: these fields, gliding, and [`FlightState::lift`]'s neutral
    /// values for the rest. Exact for the next tick: the fields left neutral are
    /// written before they are read while gliding, or only feed starting a sprint,
    /// which gliding rules out.
    pub fn lift(&self) -> FullState {
        let mut f = FlightState {
            pos: self.pos,
            vel: self.vel,
        }
        .lift();
        f.on_ground = self.on_ground;
        f.horizontal_collision = self.horizontal_collision;
        f.minor_horizontal_collision = self.minor_horizontal_collision;
        f.supporting_block = self.supporting_block;
        f.on_ground_no_blocks = self.on_ground_no_blocks;
        f.sprinting = self.sprinting;
        f.sprint_speed_modifier = self.sprinting;
        f.no_jump_delay = self.no_jump_delay;
        f.keys = self.keys;
        f.jumping = self.keys.jump;
        f
    }
}

impl FullState {
    /// The glide state, if the state glides (on the ground or not): `fall_flying`, in
    /// the gliding pose, glider equipped. None otherwise.
    pub fn glide(&self) -> Option<GlideState> {
        (self.fall_flying && self.pose == Pose::FallFlying && self.glider_equipped).then_some(
            GlideState {
                pos: self.pos,
                vel: self.vel,
                on_ground: self.on_ground,
                horizontal_collision: self.horizontal_collision,
                minor_horizontal_collision: self.minor_horizontal_collision,
                supporting_block: self.supporting_block,
                on_ground_no_blocks: self.on_ground_no_blocks,
                sprinting: self.sprinting,
                no_jump_delay: self.no_jump_delay,
                keys: self.keys,
            },
        )
    }
}
