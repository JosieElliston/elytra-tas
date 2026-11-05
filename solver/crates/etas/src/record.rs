//! Replay a recorded run (a `.tas` tape with its `@server` lines) under port and the
//! faithful kernel, with no course: does the model reproduce what the game did?
//! `check_runs.py` sweeps every recorded run through this.

use std::path::Path;

use port::{LocalPlayer, Pose as PPose, TickInput, tick};
use serde::Serialize;
use sim::faithful::Faithful;
use sim::state::{FullState, Input, Keys, Pose};
use world::World;

use crate::course::Course;
use crate::server::{self, ServerEvent};
use crate::state::keys_of;
use crate::tape::Tape;
use crate::verify::{self, Plan, RECORD_TOL};

#[derive(Debug, Clone, Default, Serialize)]
pub struct Check {
    pub tape: String,
    pub ticks: usize,
    pub glide_offs: usize,
    pub velocities: usize,
    pub teleports: usize,
    /// Why the tape couldn't be read at all.
    pub error: Option<String>,
    /// The first tick port doesn't reproduce (tick, how), or stops at.
    pub port: Option<(usize, String)>,
    /// The largest pose difference port had up to `port`'s tick.
    pub port_max_dev: f64,
    /// The first tick the faithful kernel, flown alone, parts from port or stops at.
    pub faithful: Option<(usize, String)>,
    /// Tape lines whose keys [`refit`] changed: `extract_run.py` fits keys without the
    /// blocks, so it can't see friction, walls or delayed effects.
    pub refit: usize,
    /// `verify`'s failures under the course, when given one.
    pub course: Option<Vec<String>>,
    pub timer_tick: Option<usize>,
    pub finish_tick: Option<usize>,
    /// Hits as `verify` reports them: (tick, vanilla's damage on the client's copy).
    pub hits: Vec<(usize, f32)>,
}

fn pose_of(p: PPose) -> Pose {
    match p {
        PPose::Standing => Pose::Standing,
        PPose::FallFlying => Pose::FallFlying,
        PPose::Swimming => Pose::Swimming,
        PPose::SpinAttack => Pose::SpinAttack,
        PPose::Crouching => Pose::Crouching,
    }
}

fn sim_keys(k: port::Input) -> Keys {
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

/// The kernel's state for a port player.
pub fn full_state(p: &LocalPlayer) -> FullState {
    let a = |v: port::Vec3| [v.x, v.y, v.z];
    FullState {
        pos: a(p.position),
        vel: a(p.delta_movement),
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
        pose: pose_of(p.pose),
        fall_flying: p.fall_flying,
        sprinting: p.sprinting,
        sprint_speed_modifier: p.sprint_speed_modifier,
        glider_equipped: p.glider_equipped,
        no_jump_delay: p.no_jump_delay,
        jumping: p.jumping,
        move_input: [p.xxa, p.yya, p.zza],
        keys: sim_keys(p.input.key_presses),
        move_vector: [p.input.move_vector.x, p.input.move_vector.y],
        sprint_trigger_time: p.sprint_trigger_time,
        auto_jump_time: p.auto_jump_time,
        crouching: p.crouching,
    }
}

/// Replay the tape at `path`. Never panics on a bad tape; see [`Check::error`].
pub fn check(
    path: &Path,
    world: &World,
    course: Option<&Course>,
    trace: Option<std::ops::RangeInclusive<usize>>,
) -> Check {
    let mut c = Check {
        tape: path.display().to_string(),
        ..Check::default()
    };
    let mut tape = match Tape::read(path) {
        Ok(t) => t,
        Err(e) => {
            c.error = Some(e);
            return c;
        }
    };
    c.refit = refit(&mut tape, world);
    if c.refit > 0
        && let Err(e) = std::fs::write(path, tape.write())
    {
        c.error = Some(format!("{}: {e}", path.display()));
    }
    replay(&tape, world, &mut c, trace);
    if let Some(course) = course {
        let r = verify::verify(course, world, Plan::Tape(&tape));
        c.timer_tick = r.timer_tick();
        c.finish_tick = r.finish_tick();
        c.hits = r
            .contacts
            .iter()
            .filter(|k| k.damage > 0.0)
            .map(|k| (k.tick, k.damage))
            .collect();
        c.course = Some(r.failures);
    }
    c
}

/// How far back [`refit`] looks for keys to change when a tick has no keys that fit.
/// why back at all? sneak is read a tick late, and a sprint or jump key shows only later.
const REFIT_BACK: usize = 6;
/// Ticks tried per stuck tick, a limit on [`refit`]'s search.
const REFIT_BUDGET: usize = 40_000;

/// Every key combination `.tas` can write, as (keys, string).
fn all_keys() -> Vec<(port::Input, String)> {
    let mut out = Vec::new();
    for mv in ["", "W", "S", "A", "D", "WA", "WD", "SA", "SD"] {
        for j in ["", "J"] {
            for r in ["", "R"] {
                for c in ["", "C"] {
                    let k = format!("{mv}{j}{r}{c}");
                    let s = if k.is_empty() { "-".to_string() } else { k };
                    if let Ok(i) = keys_of(&s) {
                        out.push((i, s));
                    }
                }
            }
        }
    }
    out
}

/// Whether tick `r` from `p` with `keys` ends where the tape says; the state after it.
fn try_tick(
    p: &LocalPlayer,
    tape: &Tape,
    i: usize,
    keys: port::Input,
    world: &World,
) -> Option<LocalPlayer> {
    let r = &tape.rows[i];
    let mut q = p.clone();
    server::apply_at(&mut q, &tape.arrivals, r.t, world);
    let input = TickInput {
        y_rot: r.yaw,
        x_rot: r.pitch,
        keys,
    };
    tick(&mut q, &input, world).ok()?;
    server::end_tick(&mut q, &tape.arrivals, r.t);
    let got = [q.position.x, q.position.y, q.position.z];
    let d = (0..3).fold(0.0f64, |m, k| m.max((r.pos[k] - got[k]).abs()));
    (d <= RECORD_TOL && r.ground == q.on_ground).then_some(q)
}

/// Change the tape's keys, where they don't fly its poses, to keys that do: a depth-first
/// search from the tape's own keys, with port and the blocks. Keys can't fake a pose
/// (every pose must still come out within `RECORD_TOL`), and a replay shows no keys, so
/// any that fit are as good as the ones the fit chose. Returns the lines changed, and
/// leaves the tape's keys alone from the first tick nothing fits on.
pub fn refit(tape: &mut Tape, world: &World) -> usize {
    let Ok(start) = tape.start_player() else {
        return 0;
    };
    let keys = all_keys();
    let own: Vec<Option<port::Input>> = tape.rows.iter().map(|r| keys_of(&r.keys).ok()).collect();
    // per tick reached: the state before it, the next candidate (0 is the tape's keys)
    let mut stack: Vec<(LocalPlayer, usize, String)> = vec![(start, 0, String::new())];
    let mut furthest = 0;
    let mut budget = REFIT_BUDGET;
    let mut best: Vec<String> = Vec::new();
    while stack.len() <= tape.rows.len() {
        let i = stack.len() - 1;
        let (p, c) = {
            let top = stack.last_mut().expect("never empty");
            top.1 += 1;
            (top.0.clone(), top.1 - 1)
        };
        let cand = if c == 0 {
            own[i].map(|k| (k, tape.rows[i].keys.clone()))
        } else {
            keys.get(c - 1).cloned()
        };
        let Some((k, name)) = cand.or_else(|| (c == 0).then(|| keys[0].clone())) else {
            // out of candidates here: back up, unless that is too far
            if stack.len() <= 1 || i + REFIT_BACK <= furthest {
                break;
            }
            stack.pop();
            continue;
        };
        if budget == 0 {
            break;
        }
        budget -= 1;
        if let Some(q) = try_tick(&p, tape, i, k, world) {
            stack.last_mut().expect("never empty").2 = name;
            stack.push((q, 0, String::new()));
            if stack.len() - 1 > furthest {
                furthest = stack.len() - 1;
                budget = REFIT_BUDGET;
                best = stack[..furthest].iter().map(|s| s.2.clone()).collect();
            }
        }
    }
    let mut changed = 0;
    for (r, k) in tape.rows.iter_mut().zip(&best) {
        if r.keys != *k {
            r.keys = k.clone();
            changed += 1;
        }
    }
    changed
}

fn replay(
    tape: &Tape,
    world: &World,
    c: &mut Check,
    trace: Option<std::ops::RangeInclusive<usize>>,
) {
    c.ticks = tape.rows.len();
    for a in &tape.arrivals {
        match a.event {
            ServerEvent::GlideOff => c.glide_offs += 1,
            ServerEvent::SetVelocity { .. } => c.velocities += 1,
            ServerEvent::Teleport { .. } => c.teleports += 1,
        }
    }
    let mut p = match tape.start_player() {
        Ok(p) => p,
        Err(e) => {
            c.error = Some(e);
            return;
        }
    };
    let mut alone = full_state(&p);
    let mut faithful_ok = true;
    for r in &tape.rows {
        let t = r.t;
        let keys = match keys_of(&r.keys) {
            Ok(k) => k,
            Err(e) => {
                c.error = Some(format!("tick {t}: {e}"));
                return;
            }
        };
        let input = TickInput {
            y_rot: r.yaw,
            x_rot: r.pitch,
            keys,
        };
        server::apply_at(&mut p, &tape.arrivals, t, world);
        if faithful_ok {
            for a in tape.arrivals.iter().filter(|a| a.tick == t) {
                match a.event {
                    ServerEvent::GlideOff => match Faithful.server_pose(world, &alone) {
                        Ok(pose) => alone.apply_glide_off(pose),
                        Err(e) => {
                            c.faithful = Some((t, format!("{e:?}")));
                            faithful_ok = false;
                        }
                    },
                    ServerEvent::SetVelocity { v } => alone.apply_velocity(v),
                    ServerEvent::Teleport { pos } => {
                        alone.pos = pos;
                        alone.vel = [0.0; 3];
                    }
                }
            }
        }
        if let Err(u) = tick(&mut p, &input, world) {
            c.port = Some((t, format!("stopped: {u:?}")));
            return;
        }
        server::end_tick(&mut p, &tape.arrivals, t);
        if trace.as_ref().is_some_and(|r| r.contains(&t)) {
            eprintln!(
                "{t} keys {} port p=({:.5},{:.5},{:.5}) v=({:.5},{:.5},{:.5}) g={} ff={} sprint={} pose={:?} hc={} | tape ({:.5},{:.5},{:.5}) g={}",
                r.keys,
                p.position.x,
                p.position.y,
                p.position.z,
                p.delta_movement.x,
                p.delta_movement.y,
                p.delta_movement.z,
                p.on_ground as u8,
                p.fall_flying as u8,
                p.sprinting as u8,
                p.pose,
                p.horizontal_collision as u8,
                r.pos[0],
                r.pos[1],
                r.pos[2],
                r.ground as u8,
            );
        }
        let got = [p.position.x, p.position.y, p.position.z];
        let d = (0..3).fold(0.0f64, |m, k| m.max((r.pos[k] - got[k]).abs()));
        if d.is_nan() || d > RECORD_TOL || r.ground != p.on_ground {
            c.port = Some((
                t,
                format!(
                    "pose {got:?} ground {} fly {} vs tape {:?} ground {} (off by {d:.2e})",
                    p.on_ground, p.fall_flying, r.pos, r.ground
                ),
            ));
            return;
        }
        c.port_max_dev = c.port_max_dev.max(d);
        if faithful_ok {
            let u = Input {
                yaw: r.yaw,
                pitch: r.pitch,
            };
            match Faithful.step_full(world, &alone, &u, sim_keys(keys)) {
                Ok(n) => {
                    alone = n.state;
                    for a in tape.arrivals.iter().filter(|a| a.tick == t) {
                        if let ServerEvent::Teleport { pos } = a.event {
                            alone.pos = pos;
                        }
                    }
                    let want = full_state(&p);
                    // record precision: port and the kernel share the arithmetic, so
                    // any difference at all is a kernel bug
                    if alone != want {
                        c.faithful = Some((t, format!("{alone:?} vs port {want:?}")));
                        faithful_ok = false;
                    }
                }
                Err(e) => {
                    c.faithful = Some((t, format!("{e:?}")));
                    faithful_ok = false;
                }
            }
        }
    }
}
