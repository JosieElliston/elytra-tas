//! The full state artifacts carry: for now `port::LocalPlayer`'s fields, one for one.
//! why port's? it is every field the port reads, so a trajectory cut anywhere resumes
//! bit-exactly under port; a kernel-neutral state can replace it once a second kernel
//! has to resume from these files.
//!
//! Floats are JSON numbers: doubles as themselves, floats widened to double (exact both
//! ways, as the logger does). Python reads them with `json`, and `numpy.float32(v)` of
//! a float field is the exact float.

use port::{BlockPos, ClientInput, Input, LocalPlayer, Pose, Vec2, Vec3};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FullState {
    pub position: [f64; 3],
    pub delta_movement: [f64; 3],
    pub y_rot: f64,
    pub x_rot: f64,
    pub on_ground: bool,
    pub horizontal_collision: bool,
    pub vertical_collision: bool,
    pub vertical_collision_below: bool,
    pub minor_horizontal_collision: bool,
    pub main_supporting_block_pos: Option<[i32; 3]>,
    pub on_ground_no_blocks: bool,
    pub fall_distance: f64,
    pub pose: String,
    pub fall_flying: bool,
    pub sprinting: bool,
    pub no_jump_delay: i32,
    pub jumping: bool,
    pub xxa: f64,
    pub yya: f64,
    pub zza: f64,
    pub sprint_speed_modifier: bool,
    pub glider_equipped: bool,
    /// `input.key_presses`, as tape keys (see [`keys_to_string`]).
    pub input_keys: String,
    pub input_move_vector: [f64; 2],
    pub sprint_trigger_time: i32,
    pub auto_jump_time: i32,
    pub crouching: bool,
}

fn v3(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

fn pose_name(p: Pose) -> &'static str {
    match p {
        Pose::Standing => "standing",
        Pose::FallFlying => "fall_flying",
        Pose::Swimming => "swimming",
        Pose::SpinAttack => "spin_attack",
        Pose::Crouching => "crouching",
    }
}

fn pose_of(s: &str) -> Result<Pose, String> {
    Ok(match s {
        "standing" => Pose::Standing,
        "fall_flying" => Pose::FallFlying,
        "swimming" => Pose::Swimming,
        "spin_attack" => Pose::SpinAttack,
        "crouching" => Pose::Crouching,
        _ => return Err(format!("unknown pose {s}")),
    })
}

/// Keys as a `.tas` tape writes them: a subset of `WASDJR` (R = sprint) in that order,
/// plus `C` for shift (tapes have none), or `-` for none.
pub fn keys_to_string(k: Input) -> String {
    let mut s = String::new();
    for (on, c) in [
        (k.forward, 'W'),
        (k.left, 'A'),
        (k.backward, 'S'),
        (k.right, 'D'),
        (k.jump, 'J'),
        (k.sprint, 'R'),
        (k.shift, 'C'),
    ] {
        if on {
            s.push(c);
        }
    }
    if s.is_empty() { "-".into() } else { s }
}

/// The inverse of [`keys_to_string`], in any order. Rejects other letters.
pub fn keys_of(s: &str) -> Result<Input, String> {
    if let Some(c) = s.chars().find(|c| !"WASDJRC-".contains(*c)) {
        return Err(format!("key {c:?} in {s:?}"));
    }
    let mut k = Input::from_tape(s);
    k.shift = s.contains('C');
    Ok(k)
}

impl From<&LocalPlayer> for FullState {
    fn from(p: &LocalPlayer) -> FullState {
        FullState {
            position: v3(p.position),
            delta_movement: v3(p.delta_movement),
            y_rot: p.y_rot as f64,
            x_rot: p.x_rot as f64,
            on_ground: p.on_ground,
            horizontal_collision: p.horizontal_collision,
            vertical_collision: p.vertical_collision,
            vertical_collision_below: p.vertical_collision_below,
            minor_horizontal_collision: p.minor_horizontal_collision,
            main_supporting_block_pos: p.main_supporting_block_pos.map(|b| [b.x, b.y, b.z]),
            on_ground_no_blocks: p.on_ground_no_blocks,
            fall_distance: p.fall_distance,
            pose: pose_name(p.pose).into(),
            fall_flying: p.fall_flying,
            sprinting: p.sprinting,
            no_jump_delay: p.no_jump_delay,
            jumping: p.jumping,
            xxa: p.xxa as f64,
            yya: p.yya as f64,
            zza: p.zza as f64,
            sprint_speed_modifier: p.sprint_speed_modifier,
            glider_equipped: p.glider_equipped,
            input_keys: keys_to_string(p.input.key_presses),
            input_move_vector: [p.input.move_vector.x as f64, p.input.move_vector.y as f64],
            sprint_trigger_time: p.sprint_trigger_time,
            auto_jump_time: p.auto_jump_time,
            crouching: p.crouching,
        }
    }
}

impl FullState {
    /// The port player this state describes. Fields `LocalPlayer` gained after this
    /// file was written keep `LocalPlayer::new`'s values; the round-trip test catches
    /// that.
    pub fn to_player(&self) -> Result<LocalPlayer, String> {
        let p3 = |a: [f64; 3]| Vec3::new(a[0], a[1], a[2]);
        let mut p = LocalPlayer::new(p3(self.position), self.y_rot as f32, self.x_rot as f32);
        p.delta_movement = p3(self.delta_movement);
        p.on_ground = self.on_ground;
        p.horizontal_collision = self.horizontal_collision;
        p.vertical_collision = self.vertical_collision;
        p.vertical_collision_below = self.vertical_collision_below;
        p.minor_horizontal_collision = self.minor_horizontal_collision;
        p.main_supporting_block_pos = self.main_supporting_block_pos.map(|b| BlockPos {
            x: b[0],
            y: b[1],
            z: b[2],
        });
        p.on_ground_no_blocks = self.on_ground_no_blocks;
        p.fall_distance = self.fall_distance;
        p.pose = pose_of(&self.pose)?;
        p.fall_flying = self.fall_flying;
        p.sprinting = self.sprinting;
        p.no_jump_delay = self.no_jump_delay;
        p.jumping = self.jumping;
        p.xxa = self.xxa as f32;
        p.yya = self.yya as f32;
        p.zza = self.zza as f32;
        p.sprint_speed_modifier = self.sprint_speed_modifier;
        p.glider_equipped = self.glider_equipped;
        p.input = ClientInput {
            key_presses: keys_of(&self.input_keys)?,
            move_vector: Vec2::new(
                self.input_move_vector[0] as f32,
                self.input_move_vector[1] as f32,
            ),
        };
        p.sprint_trigger_time = self.sprint_trigger_time;
        p.auto_jump_time = self.auto_jump_time;
        p.crouching = self.crouching;
        Ok(p)
    }

    /// The first field that differs from `other`, as `name: self vs other`.
    pub fn first_difference(&self, other: &FullState) -> Option<String> {
        let a = serde_json::to_value(self).ok()?;
        let b = serde_json::to_value(other).ok()?;
        let (a, b) = (a.as_object()?, b.as_object()?);
        a.iter()
            .find(|(k, v)| b.get(*k) != Some(v))
            .map(|(k, v)| format!("{k}: {v} vs {}", b.get(k).cloned().unwrap_or_default()))
    }
}
