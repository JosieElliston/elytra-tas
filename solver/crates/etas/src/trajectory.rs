//! Trajectory artifacts: JSON lines, the common currency between tools.
//!
//! ```text
//! line 1   header: {"format": "etas-trajectory", "version": 1, "state": "port::LocalPlayer",
//!                   "course": {"name", "sha256"} | null, "world_sha256": hex | null,
//!                   "kernel": "port", "ablation": null, "objective": name,
//!                   "commit": hex | "unknown", "dirty": bool, "source": free text,
//!                   "tas_header": [[key, value], ...] | null}
//! line 2   {"initial": state}               the state at the end of tick 0
//! then     {"t": n, "input": {"yaw": f, "pitch": f, "keys": "WR"}, "state": state}
//! ```
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
use crate::state::{FullState, keys_of, keys_to_string};
use crate::tape::{Tape, TapeRow};

pub const FORMAT: &str = "etas-trajectory";
pub const VERSION: u32 = 1;

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
    pub rows: Vec<Row>,
}

#[derive(Serialize, Deserialize)]
struct InitialLine {
    initial: FullState,
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
        if header.format != FORMAT || header.version != VERSION {
            return Err(format!(
                "{}: format {} version {}, reader knows {FORMAT} {VERSION}",
                path.display(),
                header.format,
                header.version
            ));
        }
        let (n, l) = lines
            .next()
            .ok_or(format!("{}: no initial state", path.display()))?;
        let init: InitialLine = serde_json::from_str(l).map_err(|e| at(n, e))?;
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

/// A tape replayed with port from rest at its start pose (`rest_state`): the
/// trajectory port flies, as far as it gets, and the error that stopped it. The tape's
/// own poses are not compared here; `verify` does that.
pub fn from_tape(
    tape: &Tape,
    world: &World,
    course: Option<&Course>,
    source: &str,
) -> (Trajectory, Option<(usize, Unsupported)>) {
    fly_tape(tape, world, course, source, "recorded", |_, k| k)
}

/// A recorded tape with ebounces, as perfect bounces: its rotations and keys, except
/// that jump is held on every tick that starts on the ground while gliding (the
/// bounce jump) and released on every gliding tick in the air. why? a replay shows no
/// keys while gliding (`extract_run.py` fits none there), so the bounce jumps are
/// missing, and its jump presses in the air are the recording's answers to its
/// glide-offs, which a perfect bounce doesn't get; held, they would also take the
/// edge a plan's reaction needs. The seeder for ebounce starts: the recorded inputs,
/// flown with no glide-off.
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
        |p, mut k| {
            if p.fall_flying {
                k.jump = p.on_ground;
            }
            k
        },
    )
}

/// Fly a tape's rotations with port from rest, the keys of each tick passed through
/// `keys(player before the tick, tape keys)`.
fn fly_tape(
    tape: &Tape,
    world: &World,
    course: Option<&Course>,
    source: &str,
    objective: &str,
    keys: impl Fn(&LocalPlayer, port::Input) -> port::Input,
) -> (Trajectory, Option<(usize, Unsupported)>) {
    let mut p: LocalPlayer = rest_state(tape.start_pos, tape.start_yaw, tape.start_pitch);
    let mut header = Header::new(course, objective, source);
    header.tas_header = Some(tape.header.clone());
    let initial = FullState::from(&p);
    let mut rows = Vec::new();
    let mut err = None;
    for r in &tape.rows {
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
            rows,
        },
        err,
    )
}
