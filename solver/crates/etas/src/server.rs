//! What the server did to the local player, as the client saw it: the shared currency
//! for recorded runs (extracted from replays), plan branches (what playback observes)
//! and verify (what it applies when replaying a path).
//!
//! In a `.tas` tape an arrival is a line before the tick line it acts on (format in
//! `extract_run.py`'s docstring): `@server <t> glide_off`, `@server <t> velocity
//! <vx> <vy> <vz>` or `@server <t> teleport <x> <y> <z>`, the numbers as double bits
//! in hex.

use port::{LocalPlayer, Pose, Vec3};
use serde::{Deserialize, Serialize};
use world::World;

/// A packet from the server that changes the local player's movement state.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerEvent {
    /// `set_entity_data` clearing the fall-flying flag (0x80 of shared flags).
    GlideOff,
    /// `set_entity_motion` for the local player: velocity replaced by this. Boosters
    /// and the server's wall-hit echo both arrive this way.
    SetVelocity { v: [f64; 3] },
    /// `player_position`, a checkpoint reset or the server's correction of a move:
    /// position replaced, velocity zeroed (all 258 in the logs checked, 2026-11-04, had
    /// a zero velocity and no relative flags). Flashback drops this packet, so `pos` is
    /// what a replay does show, the pose at the end of the tick: the tick's physics runs
    /// from rest at `pos` and the tick ends at `pos` again. The packet's target differs
    /// from that by the one move from rest, which the tick's end pose hides. A record's
    /// event only: a plan can't say it (`Plan` refuses it).
    Teleport { pos: [f64; 3] },
}

/// A server event handled by the client before tick `tick`'s physics. `tick` is the
/// `.tas` row numbering: the event acts on row `tick`'s physics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Arrival {
    pub tick: usize,
    pub event: ServerEvent,
}

impl Arrival {
    /// The tape line, without a newline.
    pub fn line(&self) -> String {
        match self.event {
            ServerEvent::GlideOff => format!("@server {} glide_off", self.tick),
            ServerEvent::SetVelocity { v } => format!(
                "@server {} velocity {:016x} {:016x} {:016x}",
                self.tick,
                v[0].to_bits(),
                v[1].to_bits(),
                v[2].to_bits()
            ),
            ServerEvent::Teleport { pos } => format!(
                "@server {} teleport {:016x} {:016x} {:016x}",
                self.tick,
                pos[0].to_bits(),
                pos[1].to_bits(),
                pos[2].to_bits()
            ),
        }
    }

    /// An `@server` line's fields after `@server`.
    pub fn parse(fields: &[&str]) -> Result<Arrival, String> {
        let tick: usize = fields
            .first()
            .ok_or("@server without a tick")?
            .parse()
            .map_err(|e| format!("@server tick: {e}"))?;
        let event = match fields.get(1).copied() {
            Some("glide_off") if fields.len() == 2 => ServerEvent::GlideOff,
            Some("velocity") if fields.len() == 5 => {
                let d = |s: &str| {
                    u64::from_str_radix(s, 16)
                        .map(f64::from_bits)
                        .map_err(|e| format!("@server velocity bits {s:?}: {e}"))
                };
                ServerEvent::SetVelocity {
                    v: [d(fields[2])?, d(fields[3])?, d(fields[4])?],
                }
            }
            Some("teleport") if fields.len() == 5 => {
                let d = |s: &str| {
                    u64::from_str_radix(s, 16)
                        .map(f64::from_bits)
                        .map_err(|e| format!("@server teleport bits {s:?}: {e}"))
                };
                ServerEvent::Teleport {
                    pos: [d(fields[2])?, d(fields[3])?, d(fields[4])?],
                }
            }
            Some(k) => return Err(format!("@server {tick} {k}: unknown event or field count")),
            None => return Err(format!("@server {tick}: no event")),
        };
        Ok(Arrival { tick, event })
    }
}

/// The pose the server's glide-off carries: what its `updatePlayerPose` picks for a
/// player without the glide at `pos` (standing if it fits, else crouching, else
/// swimming), or the player's own pose if not even swimming fits. The server judges
/// at its copy of the position, the last move it received; the client's position
/// before the tick stands in for it, which matters only within a block of a
/// ceiling.
pub fn server_pose(world: &World, pos: Vec3, own: Pose) -> Pose {
    let fits = |pose: Pose| {
        port::level::no_collision(world, &pose.make_bounding_box(pos).deflate(1.0E-7))
            .unwrap_or(false)
    };
    if !fits(Pose::Swimming) {
        own
    } else if fits(Pose::Standing) {
        Pose::Standing
    } else if fits(Pose::Crouching) {
        Pose::Crouching
    } else {
        Pose::Swimming
    }
}

/// Apply one event to the port player, as the client's packet handler does, before
/// the tick's physics.
pub fn apply(p: &mut LocalPlayer, event: &ServerEvent, world: &World) {
    match *event {
        ServerEvent::GlideOff => {
            let pose = server_pose(world, p.position, p.pose);
            p.handle_glide_off(pose);
        }
        ServerEvent::SetVelocity { v } => p.handle_set_entity_motion(Vec3::new(v[0], v[1], v[2])),
        ServerEvent::Teleport { pos } => {
            p.position = Vec3::new(pos[0], pos[1], pos[2]);
            p.delta_movement = Vec3::ZERO;
        }
    }
}

/// Apply every arrival for tick `t` (in order: the later velocity wins, as on the
/// client). `arrivals` is sorted by tick.
pub fn apply_at(p: &mut LocalPlayer, arrivals: &[Arrival], t: usize, world: &World) {
    for a in arrivals.iter().filter(|a| a.tick == t) {
        apply(p, &a.event, world);
    }
}

/// After tick `t`'s physics: a teleport's tick ends at the teleport's pose
/// ([`ServerEvent::Teleport`]).
pub fn end_tick(p: &mut LocalPlayer, arrivals: &[Arrival], t: usize) {
    for a in arrivals.iter().filter(|a| a.tick == t) {
        if let ServerEvent::Teleport { pos } = a.event {
            p.position = Vec3::new(pos[0], pos[1], pos[2]);
        }
    }
}
