//! Plans, version 1 (`solver/PLAN-FORMAT.md` is the spec): a `.tas` tape with the
//! plan's header keys and `@react` directive lines. [`Plan::parse`] reads one (a
//! plain tape is a plan with no reactions), [`Plan::write`] writes one, and
//! [`plan_from`] makes one from a trajectory, with a reaction for every planned
//! perfect bounce `verify` finds.

use std::fmt::Write as _;
use std::path::Path;

use world::World;

use crate::course::Course;
use crate::tape::Tape;
use crate::trajectory::Trajectory;
use crate::verify::{self, Report};

/// The `format=` value of a version-1 plan.
pub const FORMAT: &str = "etas-plan 1";
/// The account playback refuses to fly on any other (AGENTS.md: the alt).
pub const ACCOUNT: &str = "deftenvy";
/// `diverge_tol=`'s default, blocks.
pub const DIVERGE_TOL: f64 = 1e-4;

/// What a reaction waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The server's entity data clearing the local player's fall-flying flag.
    GlideOff,
}

/// What a reaction does, in the tick the event is first handled, before its physics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Jump down for that tick.
    PressJump,
}

impl Event {
    pub fn name(self) -> &'static str {
        match self {
            Event::GlideOff => "glide_off",
        }
    }
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::PressJump => "press_jump",
        }
    }
}

/// `@react <t0> <t1> <event> <action>`: during ticks `t0..=t1`, on the tick on which
/// the client first handles `event`, do `action`. At most one firing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct React {
    pub t0: usize,
    pub t1: usize,
    pub event: Event,
    pub action: Action,
}

impl React {
    pub fn line(&self) -> String {
        format!(
            "@react {} {} {} {}",
            self.t0,
            self.t1,
            self.event.name(),
            self.action.name()
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The tape: header (the plan's keys too, in file order), start, tick lines.
    pub tape: Tape,
    /// In file order.
    pub reacts: Vec<React>,
}

impl Plan {
    /// A plan with no reactions: what a plain `.tas` tape is.
    pub fn from_tape(tape: Tape) -> Plan {
        Plan {
            tape,
            reacts: Vec::new(),
        }
    }

    pub fn read(path: &Path) -> Result<Plan, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Plan::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Parse a plan. Refuses an `@` line it doesn't know, a `format=` other than
    /// version 1, and directives in a file without `format=`. Directive lines may
    /// sit anywhere after the header; their ticks say where they apply.
    pub fn parse(text: &str) -> Result<Plan, String> {
        let mut rest = String::with_capacity(text.len());
        let mut directives = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let data = line.split('#').next().unwrap_or("").trim();
            if data.starts_with('@') {
                directives.push((n + 1, data.to_string()));
                // keep the line count for the tape's error messages
                rest.push('\n');
            } else {
                rest.push_str(line);
                rest.push('\n');
            }
        }
        let tape = Tape::parse(&rest)?;
        match tape.get("format") {
            Some(FORMAT) => {}
            Some(f) => return Err(format!("format={f}, reader knows {FORMAT}")),
            None if !directives.is_empty() => {
                return Err(format!(
                    "line {}: a directive in a plan without format={FORMAT}",
                    directives[0].0
                ));
            }
            None => {}
        }
        if let Some(v) = tape.get("diverge_tol") {
            let tol: f64 = v.parse().map_err(|e| format!("diverge_tol={v}: {e}"))?;
            if tol.is_nan() || tol < 0.0 {
                return Err(format!("diverge_tol={v}: not a tolerance"));
            }
        }
        let n = tape.rows.len();
        let mut reacts = Vec::new();
        for (line, d) in directives {
            let f: Vec<&str> = d.split_whitespace().collect();
            let at = |e: String| format!("line {line}: {e}");
            if f[0] != "@react" {
                return Err(at(format!("unknown directive {}", f[0])));
            }
            if f.len() != 5 {
                return Err(at(format!("@react has {} fields, want 4", f.len() - 1)));
            }
            let tick = |s: &str| -> Result<usize, String> {
                s.parse::<usize>()
                    .map_err(|e| at(format!("tick {s:?}: {e}")))
            };
            let (t0, t1) = (tick(f[1])?, tick(f[2])?);
            if !(1 <= t0 && t0 <= t1 && t1 <= n) {
                return Err(at(format!("window {t0}..{t1} not within ticks 1..{n}")));
            }
            let event = match f[3] {
                "glide_off" => Event::GlideOff,
                e => return Err(at(format!("unknown event {e}"))),
            };
            let action = match f[4] {
                "press_jump" => Action::PressJump,
                a => return Err(at(format!("unknown action {a}"))),
            };
            reacts.push(React {
                t0,
                t1,
                event,
                action,
            });
        }
        Ok(Plan { tape, reacts })
    }

    /// The plan as text: the tape as `Tape::write` writes it, each directive on the
    /// line before its window's first tick.
    pub fn write(&self) -> String {
        self.tape.write_with(|t| {
            let mut out = String::new();
            for r in self.reacts.iter().filter(|r| r.t0 == t) {
                let _ = writeln!(out, "{}", r.line());
            }
            out
        })
    }

    /// `diverge_tol=`, or the default.
    pub fn diverge_tol(&self) -> f64 {
        self.tape
            .get("diverge_tol")
            .and_then(|v| v.parse().ok())
            .unwrap_or(DIVERGE_TOL)
    }
}

/// The plan for a trajectory under a course: its tape, the plan header (format,
/// account, course and world hashes, `diverge_tol`), and for every planned perfect
/// bounce the window `verify` says a jump press rescues
/// ([`verify::Bounce::window_to_plan`]). Returns the trajectory's verify report too;
/// the plan is only as good as that report (and the plan's own, which the caller
/// should run).
pub fn plan_from(course: &Course, world: &World, traj: &Trajectory) -> (Plan, Report) {
    let report = verify::verify(course, world, verify::Plan::Trajectory(traj));
    let mut tape = traj.to_tape();
    let mut header = vec![
        ("format".to_string(), FORMAT.to_string()),
        ("account".to_string(), ACCOUNT.to_string()),
        ("course".to_string(), course.name.clone()),
        ("course_sha256".to_string(), course.sha256.clone()),
        (
            "world_sha256".to_string(),
            course.world.content_sha256.clone(),
        ),
        ("diverge_tol".to_string(), format!("{DIVERGE_TOL:e}")),
    ];
    let keys: Vec<String> = header.iter().map(|(k, _)| k.clone()).collect();
    header.extend(tape.header.drain(..).filter(|(k, _)| !keys.contains(k)));
    tape.header = header;
    let reacts = report
        .bounces
        .iter()
        .filter_map(|b| b.window_to_plan())
        .map(|(t0, t1)| React {
            t0,
            t1,
            event: Event::GlideOff,
            action: Action::PressJump,
        })
        .collect();
    (Plan { tape, reacts }, report)
}
