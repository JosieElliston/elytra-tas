//! Test helpers for the faithful kernel: conversions between sim's full state and
//! `port::LocalPlayer`, port ticks for a flight input, the Burrowed Grove tape and
//! world, and comparison of a faithful tick with a port tick.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use port::{ClientInput, LocalPlayer, TickInput, Unsupported, Vec2, Vec3};
use sim::faithful::{Event, Faithful, Tick};
use sim::state::{FlightState, FullState, Input, Keys, Pose};
use world::World;

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn pose_to_port(p: Pose) -> port::Pose {
    match p {
        Pose::Standing => port::Pose::Standing,
        Pose::FallFlying => port::Pose::FallFlying,
        Pose::Swimming => port::Pose::Swimming,
        Pose::SpinAttack => port::Pose::SpinAttack,
        Pose::Crouching => port::Pose::Crouching,
    }
}

fn pose_from_port(p: port::Pose) -> Pose {
    match p {
        port::Pose::Standing => Pose::Standing,
        port::Pose::FallFlying => Pose::FallFlying,
        port::Pose::Swimming => Pose::Swimming,
        port::Pose::SpinAttack => Pose::SpinAttack,
        port::Pose::Crouching => Pose::Crouching,
    }
}

pub fn keys_to_port(k: Keys) -> port::Input {
    port::Input {
        forward: k.forward,
        backward: k.backward,
        left: k.left,
        right: k.right,
        jump: k.jump,
        shift: k.sneak,
        sprint: k.sprint,
    }
}

pub fn keys_from_port(k: port::Input) -> Keys {
    Keys {
        forward: k.forward,
        backward: k.backward,
        left: k.left,
        right: k.right,
        jump: k.jump,
        sneak: k.shift,
        sprint: k.sprint,
    }
}

fn v3(a: [f64; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}

fn a3(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

pub fn to_port(s: &FullState) -> LocalPlayer {
    let mut p = LocalPlayer::new(v3(s.pos), s.yaw, s.pitch);
    p.delta_movement = v3(s.vel);
    p.on_ground = s.on_ground;
    p.horizontal_collision = s.horizontal_collision;
    p.vertical_collision = s.vertical_collision;
    p.vertical_collision_below = s.vertical_collision_below;
    p.minor_horizontal_collision = s.minor_horizontal_collision;
    p.main_supporting_block_pos = s.supporting_block;
    p.on_ground_no_blocks = s.on_ground_no_blocks;
    p.fall_distance = s.fall_distance;
    p.pose = pose_to_port(s.pose);
    p.fall_flying = s.fall_flying;
    p.sprinting = s.sprinting;
    p.sprint_speed_modifier = s.sprint_speed_modifier;
    p.glider_equipped = s.glider_equipped;
    p.no_jump_delay = s.no_jump_delay;
    p.jumping = s.jumping;
    p.xxa = s.move_input[0];
    p.yya = s.move_input[1];
    p.zza = s.move_input[2];
    p.input = ClientInput {
        key_presses: keys_to_port(s.keys),
        move_vector: Vec2::new(s.move_vector[0], s.move_vector[1]),
    };
    p.sprint_trigger_time = s.sprint_trigger_time;
    p.auto_jump_time = s.auto_jump_time;
    p.crouching = s.crouching;
    p
}

pub fn from_port(p: &LocalPlayer) -> FullState {
    FullState {
        pos: a3(p.position),
        vel: a3(p.delta_movement),
        yaw: p.y_rot,
        pitch: p.x_rot,
        on_ground: p.on_ground,
        horizontal_collision: p.horizontal_collision,
        vertical_collision: p.vertical_collision,
        vertical_collision_below: p.vertical_collision_below,
        minor_horizontal_collision: p.minor_horizontal_collision,
        supporting_block: p.main_supporting_block_pos,
        on_ground_no_blocks: p.on_ground_no_blocks,
        fall_distance: p.fall_distance,
        pose: pose_from_port(p.pose),
        fall_flying: p.fall_flying,
        sprinting: p.sprinting,
        sprint_speed_modifier: p.sprint_speed_modifier,
        glider_equipped: p.glider_equipped,
        no_jump_delay: p.no_jump_delay,
        jumping: p.jumping,
        move_input: [p.xxa, p.yya, p.zza],
        keys: keys_from_port(p.input.key_presses),
        move_vector: [p.input.move_vector.x, p.input.move_vector.y],
        sprint_trigger_time: p.sprint_trigger_time,
        auto_jump_time: p.auto_jump_time,
        crouching: p.crouching,
    }
}

/// A flight input as port's tick input, no keys held.
pub fn tick_input(u: &Input<f32>) -> TickInput {
    TickInput {
        y_rot: u.yaw,
        x_rot: u.pitch,
        keys: port::Input::EMPTY,
    }
}

/// Port's tick of a lifted flight state, and what came of it.
pub fn port_step(
    world: &World,
    s: &FlightState<f64>,
    u: &Input<f32>,
) -> (LocalPlayer, Result<(), Unsupported>) {
    let mut p = to_port(&s.lift());
    let r = port::tick(&mut p, &tick_input(u), world).map(|_| ());
    (p, r)
}

pub fn bits(a: [f64; 3]) -> [u64; 3] {
    a.map(f64::to_bits)
}

/// How a faithful tick and a port tick from the same state compare.
#[derive(Debug, PartialEq, Eq)]
pub enum Agree {
    /// Both flew normal flight, bit for bit; `contact` says a block clipped the move.
    Flight { contact: bool },
    /// Port left normal flight and faithful raised a matching event.
    Left,
}

/// Check a faithful tick against port's tick from the same full state (port state
/// `after`, result `r`). Panics with a description on disagreement.
///
/// Matching events: the same kind, and the same block except for unknown space (port
/// reads more cells than the faithful kernel, for the crouch and pose probes). A
/// landing ends normal flight, so it matches port landing (`onGround`) or any port
/// error raised after the move (fluid, bounce, stuck, unknown space); a faithful
/// bounce matches a port bounce or port landing (port may find another supporting
/// block after a step-up).
pub fn agree(
    f: &Result<Tick, Event>,
    after: &LocalPlayer,
    r: &Result<(), Unsupported>,
    what: &str,
) -> Agree {
    match (f, r) {
        (Ok(t), Ok(())) => {
            let full = from_port(after);
            assert!(
                full.flight().is_some(),
                "{what}: faithful flew on, port left normal flight: {full:?}"
            );
            assert_eq!(
                (bits(t.state.pos), bits(t.state.vel)),
                (bits(full.pos), bits(full.vel)),
                "{what}: faithful {:?}, port pos {:?} vel {:?}",
                t.state,
                full.pos,
                full.vel
            );
            let c = t.contact;
            assert_eq!(
                c.is_some_and(|c| c.horizontal_collision),
                after.horizontal_collision,
                "{what}: horizontalCollision"
            );
            assert_eq!(
                c.is_some_and(|c| c.clipped[1]),
                after.vertical_collision,
                "{what}: verticalCollision"
            );
            Agree::Flight {
                contact: c.is_some(),
            }
        }
        (Err(Event::Landing), Ok(())) => {
            assert!(
                after.on_ground,
                "{what}: faithful landed, port didn't: {after:?}"
            );
            Agree::Left
        }
        (Err(Event::Bounce(_)), Ok(())) => {
            assert!(
                after.on_ground,
                "{what}: faithful bounced, port didn't land"
            );
            Agree::Left
        }
        (Err(e), Err(u)) => {
            let ok = match (e, u) {
                (Event::UnknownSpace(_), Unsupported::UnknownSpace(_)) => true,
                (Event::Climbable(a), Unsupported::Climbable(b))
                | (Event::Fluid(a), Unsupported::Fluid(b))
                | (Event::Stuck(a), Unsupported::Stuck(b)) => a == b,
                (Event::Landing, Unsupported::Fluid(_) | Unsupported::Bounce(_))
                | (Event::Landing, Unsupported::Stuck(_) | Unsupported::UnknownSpace(_)) => true,
                (Event::Bounce(_), Unsupported::Bounce(_)) => true,
                _ => false,
            };
            assert!(ok, "{what}: faithful {e:?}, port {u:?}");
            Agree::Left
        }
        (Err(e), Ok(())) => panic!("{what}: faithful raised {e:?}, port flew on: {after:?}"),
        (Ok(t), Err(u)) => panic!("{what}: faithful flew to {t:?}, port raised {u:?}"),
    }
}

/// One tick both ways from a flight state.
pub fn compare_tick(
    world: &World,
    s: &FlightState<f64>,
    u: &Input<f32>,
    what: &str,
) -> (Result<Tick, Event>, Agree) {
    let f = Faithful.step_input(world, s, u);
    let (after, r) = port_step(world, s, u);
    let a = agree(&f, &after, &r, what);
    (f, a)
}

/// The Burrowed Grove world fixture (the world crate's).
pub fn burrowed_grove() -> World {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../world/tests/data/burrowed_grove.world.gz");
    World::load(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// One tape line: tick `t`'s rotation and keys.
pub struct TapeTick {
    pub yaw: f32,
    pub pitch: f32,
    pub keys: port::Input,
}

pub struct Tape {
    pub start_pos: Vec3,
    pub start_yaw: f32,
    pub start_pitch: f32,
    /// `ticks[0]` is tick 1.
    pub ticks: Vec<TapeTick>,
}

fn f32_hex(s: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(s, 16).unwrap())
}

fn f64_hex(s: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(s, 16).unwrap())
}

/// `tas/burrowed_grove.tas` (format: `extract_run.py`'s docstring).
pub fn tape() -> Tape {
    let text = std::fs::read_to_string(repo_root().join("tas/burrowed_grove.tas")).unwrap();
    let mut start = None;
    let mut ticks = Vec::new();
    for line in text.lines() {
        let data = line.split('#').next().unwrap().trim();
        if data.is_empty() {
            continue;
        }
        if let Some(v) = data.strip_prefix("start=") {
            let s: Vec<&str> = v.split_whitespace().collect();
            start = Some((
                Vec3::new(f64_hex(s[0]), f64_hex(s[1]), f64_hex(s[2])),
                f32_hex(s[3]),
                f32_hex(s[4]),
            ));
            continue;
        }
        if data.contains('=') {
            continue;
        }
        let f: Vec<&str> = data.split_whitespace().collect();
        assert_eq!(f[0].parse::<usize>().unwrap(), ticks.len() + 1);
        ticks.push(TapeTick {
            yaw: f32_hex(f[1]),
            pitch: f32_hex(f[2]),
            keys: port::Input::from_tape(f[3]),
        });
    }
    let (start_pos, start_yaw, start_pitch) = start.unwrap();
    Tape {
        start_pos,
        start_yaw,
        start_pitch,
        ticks,
    }
}

impl Tape {
    /// Port at the start pose, at rest on the ground, as port's own tape test starts.
    pub fn standing_start(&self) -> LocalPlayer {
        let mut p = LocalPlayer::new(self.start_pos, self.start_yaw, self.start_pitch);
        p.delta_movement = Vec3::new(0.0, (0.0 - 0.08) * 0.98f32 as f64, 0.0);
        p
    }

    pub fn input(&self, t: usize) -> TickInput {
        let k = &self.ticks[t - 1];
        TickInput {
            y_rot: k.yaw,
            x_rot: k.pitch,
            keys: k.keys,
        }
    }

    pub fn flight_input(&self, t: usize) -> Input<f32> {
        let k = &self.ticks[t - 1];
        Input {
            yaw: k.yaw,
            pitch: k.pitch,
        }
    }
}
