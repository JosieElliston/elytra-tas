//! Trajectory artifacts: JSON lines, the common currency between tools.
//!
//! ```text
//! line 1   header: {"format": "etas-trajectory", "version": 2, "state": "port::LocalPlayer",
//!                   "course": {"name", "sha256"} | null, "world_sha256": hex | null,
//!                   "kernel": "port", "ablation": null, "objective": name,
//!                   "commit": hex | "unknown", "dirty": bool, "source": free text,
//!                   "tas_header": [[key, value], ...] | null}
//! line 2   {"initial": state}               the state at the end of tick 0
//! line 3   {"arrivals": [{"tick": n, "event": {"kind": "glide_off"}
//!                                        | {"kind": "set_velocity", "v": [x, y, z]}}, ...]}
//! then     {"t": n, "input": {"yaw": f, "pitch": f, "keys": "WR"}, "state": state}
//! ```
//!
//! `arrivals` are the server events the trajectory flew through (`server::Arrival`),
//! each applied before tick `tick`'s physics, sorted by tick; empty for a plan's path
//! with none. Version 1 had no line 3 and reads as no arrivals.
//!
//! Row `t` is the `.tas` tape's line `t`: the input of tick `t` and the state at its
//! end, after physics. `t` counts from 1 without gaps. The state is [`FullState`]
//! (port's `LocalPlayer` fields; floats widened to double). `tas_header` carries a
//! source tape's header so [`Trajectory::to_tape`] can write it back. The commit is
//! that of the code that defined the objective, dirty if its tree had changes.

use std::io::Write;
use std::path::Path;

use port::{LocalPlayer, TickInput, Unsupported, tick};
use serde::{Deserialize, Serialize};
use world::World;

use crate::course::{Course, rest_state};
use crate::server::{self, Arrival, ServerEvent};
use crate::state::{FullState, keys_of, keys_to_string};
use crate::tape::{Tape, TapeRow};

pub const FORMAT: &str = "etas-trajectory";
pub const VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CourseRef {
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub format: String,
    pub version: u32,
    pub state: String,
    pub course: Option<CourseRef>,
    pub world_sha256: Option<String>,
    pub kernel: String,
    pub ablation: Option<String>,
    pub objective: String,
    pub commit: String,
    pub dirty: bool,
    pub source: String,
    pub tas_header: Option<Vec<(String, String)>>,
}

impl Header {
    /// A header for a trajectory made by this build, under `course` if given.
    pub fn new(course: Option<&Course>, objective: &str, source: &str) -> Header {
        let (commit, dirty) = crate::git_state();
        Header {
            format: FORMAT.into(),
            version: VERSION,
            state: "port::LocalPlayer".into(),
            course: course.map(|c| CourseRef {
                name: c.name.clone(),
                sha256: c.sha256.clone(),
            }),
            world_sha256: course.map(|c| c.world.content_sha256.clone()),
            kernel: "port".into(),
            ablation: None,
            objective: objective.into(),
            commit,
            dirty,
            source: source.into(),
            tas_header: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputRow {
    /// Floats, widened to double.
    pub yaw: f64,
    pub pitch: f64,
    /// Tape keys (`state::keys_of`).
    pub keys: String,
}

impl InputRow {
    pub fn tick_input(&self) -> Result<TickInput, String> {
        Ok(TickInput {
            y_rot: self.yaw as f32,
            x_rot: self.pitch as f32,
            keys: keys_of(&self.keys)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub t: usize,
    pub input: InputRow,
    pub state: FullState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Trajectory {
    pub header: Header,
    pub initial: FullState,
    /// Applied before their tick's physics; sorted by tick.
    pub arrivals: Vec<Arrival>,
    pub rows: Vec<Row>,
}

#[derive(Serialize, Deserialize)]
struct InitialLine {
    initial: FullState,
}

#[derive(Serialize, Deserialize)]
struct ArrivalsLine {
    arrivals: Vec<Arrival>,
}

impl Trajectory {
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut w = std::io::BufWriter::new(f);
        let line = |w: &mut std::io::BufWriter<std::fs::File>, v: String| {
            writeln!(w, "{v}").map_err(|e| format!("{}: {e}", path.display()))
        };
        let j = |e: serde_json::Error| e.to_string();
        line(&mut w, serde_json::to_string(&self.header).map_err(j)?)?;
        line(
            &mut w,
            serde_json::to_string(&InitialLine {
                initial: self.initial.clone(),
            })
            .map_err(j)?,
        )?;
        line(
            &mut w,
            serde_json::to_string(&ArrivalsLine {
                arrivals: self.arrivals.clone(),
            })
            .map_err(j)?,
        )?;
        for r in &self.rows {
            line(&mut w, serde_json::to_string(r).map_err(j)?)?;
        }
        w.flush().map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn read(path: &Path) -> Result<Trajectory, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut lines = text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty());
        let at = |n: usize, e: serde_json::Error| format!("{} line {}: {e}", path.display(), n + 1);
        let (n, l) = lines.next().ok_or(format!("{}: empty", path.display()))?;
        let header: Header = serde_json::from_str(l).map_err(|e| at(n, e))?;
        if header.format != FORMAT || !(1..=VERSION).contains(&header.version) {
            return Err(format!(
                "{}: format {} version {}, reader knows {FORMAT} 1..={VERSION}",
                path.display(),
                header.format,
                header.version
            ));
        }
        let (n, l) = lines
            .next()
            .ok_or(format!("{}: no initial state", path.display()))?;
        let init: InitialLine = serde_json::from_str(l).map_err(|e| at(n, e))?;
        let mut arrivals = Vec::new();
        if header.version >= 2 {
            let (n, l) = lines
                .next()
                .ok_or(format!("{}: no arrivals line", path.display()))?;
            let a: ArrivalsLine = serde_json::from_str(l).map_err(|e| at(n, e))?;
            arrivals = a.arrivals;
        }
        let mut rows = Vec::new();
        for (n, l) in lines {
            let r: Row = serde_json::from_str(l).map_err(|e| at(n, e))?;
            if r.t != rows.len() + 1 {
                return Err(format!(
                    "{} line {}: t {}, want {}",
                    path.display(),
                    n + 1,
                    r.t,
                    rows.len() + 1
                ));
            }
            rows.push(r);
        }
        Ok(Trajectory {
            header,
            initial: init.initial,
            arrivals,
            rows,
        })
    }

    /// The tape this trajectory flies: its inputs, with its positions and onGround as
    /// the expected poses. Also the branchless plan: `tas_play` flies it. The header is
    /// the source tape's (`tas_header`) where there is one, with `start`, `ticks` and
    /// `glide_tick` from this trajectory.
    pub fn to_tape(&self) -> Tape {
        let s = &self.initial;
        let mut tape = Tape {
            header: self.header.tas_header.clone().unwrap_or_default(),
            start_pos: s.position,
            start_yaw: s.y_rot as f32,
            start_pitch: s.x_rot as f32,
            rows: self
                .rows
                .iter()
                .map(|r| TapeRow {
                    t: r.t,
                    yaw: r.input.yaw as f32,
                    pitch: r.input.pitch as f32,
                    keys: r.input.keys.clone(),
                    pos: r.state.position,
                    ground: r.state.on_ground,
                })
                .collect(),
            arrivals: self.arrivals.clone(),
        };
        if tape.get("source").is_none() {
            tape.set("source", self.header.source.clone());
        }
        tape.set("start", tape.start_value());
        tape.set("ticks", self.rows.len().to_string());
        if let Some(r) = self.rows.iter().find(|r| r.state.fall_flying) {
            tape.set("glide_tick", r.t.to_string());
        }
        tape
    }
}

/// A tape replayed with port from rest at its start pose (`rest_state`), its
/// `@server` arrivals applied before their ticks: the trajectory port flies, as far as
/// it gets, and the error that stopped it. The tape's own poses are not compared
/// here; `verify` does that.
pub fn from_tape(
    tape: &Tape,
    world: &World,
    course: Option<&Course>,
    source: &str,
) -> (Trajectory, Option<(usize, Unsupported)>) {
    fly_tape(tape, world, course, source, "recorded", true, |_, k| k)
}

/// A recorded flight, cut out of its tape. `from_tick` is the tape tick its trajectory's
/// tick 0 is; `parted` the first tape tick whose pose it misses by over
/// [`crate::verify::RECORD_TOL`], with the difference, and `stopped` where port gave up.
#[derive(Debug)]
pub struct Flight {
    pub traj: Trajectory,
    pub from_tick: usize,
    pub parted: Option<(usize, f64)>,
    pub stopped: Option<(usize, Unsupported)>,
}

/// A recorded tape's flight as a trajectory that starts in the air, at the end of tape
/// tick `from` (default: the last jump press before the first pose outside the start area,
/// i.e. the deploy it left on), with the velocity that tick's move gives. why? a glide
/// reproduces from its pose and velocity, and what comes before it (walking, jumping)
/// often doesn't, while a seed needs only the flight. Row `t` is tape tick `from + t`, and
/// arrivals shift with it.
pub fn flight_from_tape(
    tape: &Tape,
    world: &World,
    course: &Course,
    source: &str,
    from: Option<usize>,
) -> Result<Flight, String> {
    let from = match from {
        Some(f) => f,
        None => {
            let start = &course.region("start_area")?.working;
            let exit = tape
                .rows
                .iter()
                .find(|r| !start.contains(r.pos))
                .ok_or("the tape never leaves the start area")?
                .t;
            tape.rows
                .iter()
                .filter(|r| r.t < exit && r.keys.contains('J'))
                .next_back()
                .ok_or("no jump press before the tape leaves the start area")?
                .t
        }
    };
    if from < 2 || from > tape.rows.len() {
        return Err(format!("from {from}: the tape has rows 1..={}", tape.rows.len()));
    }
    let row = |t: usize| &tape.rows[t - 1];
    let (a, b) = (row(from - 1).pos, row(from).pos);
    let mut p = LocalPlayer::new(
        port::Vec3::new(b[0], b[1], b[2]),
        row(from).yaw,
        row(from).pitch,
    );
    p.delta_movement = port::Vec3::new(b[0] - a[0], b[1] - a[1], b[2] - a[2]);
    p.fall_flying = true;
    p.pose = port::Pose::FallFlying;
    p.on_ground = false;
    p.set_sprinting(row(from).keys.contains('R'));
    let initial = FullState::from(&p);
    let arrivals: Vec<Arrival> = tape
        .arrivals
        .iter()
        .filter(|a| a.tick > from)
        .map(|a| Arrival {
            tick: a.tick - from,
            event: a.event,
        })
        .collect();
    let (mut rows, mut parted, mut stopped) = (Vec::new(), None, None);
    for r in &tape.rows[from..] {
        let t = r.t - from;
        server::apply_at(&mut p, &arrivals, t, world);
        let input = InputRow {
            yaw: r.yaw as f64,
            pitch: r.pitch as f64,
            keys: r.keys.clone(),
        };
        let ti = input.tick_input()?;
        if let Err(u) = tick(&mut p, &ti, world) {
            stopped = Some((r.t, u));
            break;
        }
        server::end_tick(&mut p, &arrivals, t);
        let pos = [p.position.x, p.position.y, p.position.z];
        let miss = (0..3).map(|k| (pos[k] - r.pos[k]).abs()).fold(0.0, f64::max);
        if parted.is_none() && miss > crate::verify::RECORD_TOL {
            parted = Some((r.t, miss));
        }
        rows.push(Row {
            t,
            input,
            state: FullState::from(&p),
        });
    }
    Ok(Flight {
        traj: Trajectory {
            header: Header::new(Some(course), "recorded", source),
            initial,
            arrivals,
            rows,
        },
        from_tick: from,
        parted,
        stopped,
    })
}

/// A recorded tape with ebounces, as perfect bounces: its rotations and keys, except
/// that jump is held on every tick that starts on the ground while gliding (the
/// bounce jump) and released on every gliding tick in the air. why? a replay shows no
/// keys while gliding (`extract_run.py` fits none there), so the bounce jumps are
/// missing, and its jump presses in the air are the recording's answers to its
/// glide-offs, which a perfect bounce doesn't get; held, they would also take the
/// edge a plan's reaction needs. The seeder for ebounce starts: the recorded inputs,
/// flown with no glide-off. The tape's velocity arrivals are kept as recorded, though
/// a booster's would differ on the changed path; its glide-offs are dropped.
pub fn seed_perfect_bounces(
    tape: &Tape,
    world: &World,
    course: Option<&Course>,
    source: &str,
) -> (Trajectory, Option<(usize, Unsupported)>) {
    fly_tape(
        tape,
        world,
        course,
        source,
        "perfect_bounces",
        false,
        |p, mut k| {
            if p.fall_flying {
                k.jump = p.on_ground;
            }
            k
        },
    )
}

/// Fly a tape's rotations with port from rest, the keys of each tick passed through
/// `keys(player before the tick, tape keys)`, with the tape's arrivals (its
/// glide-offs only if `glide_offs`).
fn fly_tape(
    tape: &Tape,
    world: &World,
    course: Option<&Course>,
    source: &str,
    objective: &str,
    glide_offs: bool,
    keys: impl Fn(&LocalPlayer, port::Input) -> port::Input,
) -> (Trajectory, Option<(usize, Unsupported)>) {
    let mut p: LocalPlayer = tape
        .start_player()
        .unwrap_or_else(|_| rest_state(tape.start_pos, tape.start_yaw, tape.start_pitch));
    let mut header = Header::new(course, objective, source);
    header.tas_header = Some(tape.header.clone());
    let initial = FullState::from(&p);
    let arrivals: Vec<Arrival> = tape
        .arrivals
        .iter()
        .filter(|a| glide_offs || a.event != ServerEvent::GlideOff)
        .copied()
        .collect();
    let mut rows = Vec::new();
    let mut err = None;
    for r in &tape.rows {
        server::apply_at(&mut p, &arrivals, r.t, world);
        let Ok(k) = keys_of(&r.keys) else { break };
        let k = keys(&p, k);
        let input = InputRow {
            yaw: r.yaw as f64,
            pitch: r.pitch as f64,
            keys: if k == keys_of(&r.keys).unwrap_or(k) {
                r.keys.clone()
            } else {
                keys_to_string(k)
            },
        };
        let ti = match input.tick_input() {
            Ok(t) => t,
            Err(_) => break,
        };
        if let Err(u) = tick(&mut p, &ti, world) {
            err = Some((r.t, u));
            break;
        }
        server::end_tick(&mut p, &arrivals, r.t);
        rows.push(Row {
            t: r.t,
            input,
            state: FullState::from(&p),
        });
    }
    (
        Trajectory {
            header,
            initial,
            arrivals,
            rows,
        },
        err,
    )
}
