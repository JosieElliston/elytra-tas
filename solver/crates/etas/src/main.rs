//! `etas`: the toolkit's generic tools as subcommands.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use rayon::prelude::*;
use etas::course::Course;
use etas::plan::{self as eplan, plan_from};
use etas::tape::Tape;
use etas::trajectory::{Trajectory, from_tape};
use etas::verify::{Plan, verify};

#[derive(Parser)]
#[command(about = "elytra TAS toolkit")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Replay a trajectory (.jsonl) or a .tas plan with port against a course; exit 1
    /// if a recorded state doesn't reproduce, a rule is broken, or a planned bounce
    /// lacks a working @react. A .tas with @server lines is a record: its server
    /// events are applied and no reactions are required.
    Verify {
        file: PathBuf,
        #[arg(long)]
        course: PathBuf,
        /// Print the report as JSON instead.
        #[arg(long)]
        json: bool,
    },
    /// Replay recorded tapes under port and the faithful kernel in one world (a course's,
    /// or `--world`), one JSON line per tape, tapes in parallel. With `--course`, also
    /// `verify` each. Exit 0 whatever they do: `check_runs.py` reads the lines.
    Replay {
        tapes: Vec<PathBuf>,
        #[arg(long)]
        course: Option<PathBuf>,
        #[arg(long)]
        world: Option<PathBuf>,
        /// Also print port's state per tick in this range to stderr (a failure's context).
        #[arg(long, num_args = 2)]
        trace: Vec<usize>,
    },
    /// .tas tape -> trajectory, replayed with port in the course's world.
    Tas2traj {
        tape: PathBuf,
        #[arg(long)]
        course: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Trajectory (or .tas) -> plan: its tape, the plan header, and an @react for
    /// every planned perfect bounce; then verifies the plan (exit 1 if it fails).
    Plan {
        traj: PathBuf,
        #[arg(long)]
        course: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Trajectory -> .tas tape (the branchless plan tas_play flies).
    Traj2tas {
        traj: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
}

fn is_tape(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "tas")
}

/// A tape with `@server` lines (`tape::Tape::is_record`).
fn is_record(path: &Path) -> Result<bool, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(text.lines().any(|l| l.trim_start().starts_with("@server ")))
}

fn run(cli: Cli) -> Result<bool, String> {
    match cli.cmd {
        Cmd::Verify { file, course, json } => {
            let (c, w) = Course::load(&course)?;
            let report = if is_tape(&file) && is_record(&file)? {
                verify(&c, &w, Plan::Tape(&Tape::read(&file)?))
            } else if is_tape(&file) {
                let plan = eplan::Plan::read(&file)?;
                if plan.branches() {
                    // every path that reaches the plan's end must pass; paths that end
                    // in an abort leaf are playback's problem, not a physical claim
                    let paths = eplan::verify_paths(&c, &w, &plan);
                    let mut ok = true;
                    for (p, r) in &paths {
                        if json {
                            println!("{}", serde_json::to_string(&r).map_err(|e| e.to_string())?);
                        } else {
                            println!("== path {}", p.segments.join(" > "));
                            print!("{r}");
                        }
                        ok &= r.ok();
                    }
                    println!(
                        "{} paths to the end, {}",
                        paths.len(),
                        if ok { "all PASS" } else { "FAIL" }
                    );
                    return Ok(ok && !paths.is_empty());
                }
                verify(&c, &w, Plan::Reactive(&plan))
            } else {
                verify(&c, &w, Plan::Trajectory(&Trajectory::read(&file)?))
            };
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
                );
            } else {
                print!("{report}");
            }
            Ok(report.ok())
        }
        Cmd::Replay {
            tapes,
            course,
            world,
            trace,
        } => {
            let c = course.as_deref().map(Course::load).transpose()?;
            let w = match (&c, &world) {
                (_, Some(p)) => world::World::load(p).map_err(|e| e.to_string())?,
                (Some((_, w)), None) => w.clone(),
                (None, None) => return Err("replay needs --course or --world".into()),
            };
            let lines: Vec<String> = tapes
                .par_iter()
                .map(|p| {
                    let t = (trace.len() == 2).then(|| trace[0]..=trace[1]);
                    let r = etas::record::check(p, &w, c.as_ref().map(|(c, _)| c), t);
                    serde_json::to_string(&r).unwrap_or_default()
                })
                .collect();
            for l in lines {
                println!("{l}");
            }
            Ok(true)
        }
        Cmd::Tas2traj { tape, course, out } => {
            let (c, w) = Course::load(&course)?;
            let t = Tape::read(&tape)?;
            let (traj, err) = from_tape(&t, &w, Some(&c), &tape.display().to_string());
            traj.write(&out)?;
            println!("wrote {} ticks to {}", traj.rows.len(), out.display());
            if let Some((tick, u)) = err {
                eprintln!("port stopped at tick {tick}: {u:?}; the trajectory ends before it");
                return Ok(false);
            }
            Ok(true)
        }
        Cmd::Plan { traj, course, out } => {
            let (c, w) = Course::load(&course)?;
            let t = if is_tape(&traj) {
                let (t, err) = from_tape(
                    &Tape::read(&traj)?,
                    &w,
                    Some(&c),
                    &traj.display().to_string(),
                );
                if let Some((tick, u)) = err {
                    return Err(format!("port stopped at tick {tick}: {u:?}"));
                }
                t
            } else {
                Trajectory::read(&traj)?
            };
            if !t.arrivals.is_empty() {
                return Err(format!(
                    "{}: a record with {} server events, not a plan (plans can't say what the server sends)",
                    traj.display(),
                    t.arrivals.len()
                ));
            }
            let (p, _) = plan_from(&c, &w, &t);
            std::fs::write(&out, p.write()).map_err(|e| format!("{}: {e}", out.display()))?;
            println!(
                "wrote {} ticks and {} reactions to {}",
                p.tape.rows.len(),
                p.reacts.len(),
                out.display()
            );
            let report = verify(&c, &w, Plan::Reactive(&eplan::Plan::read(&out)?));
            print!("{report}");
            Ok(report.ok())
        }
        Cmd::Traj2tas { traj, out } => {
            let t = Trajectory::read(&traj)?;
            std::fs::write(&out, t.to_tape().write())
                .map_err(|e| format!("{}: {e}", out.display()))?;
            println!("wrote {} ticks to {}", t.rows.len(), out.display());
            Ok(true)
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("etas: {e}");
            ExitCode::from(2)
        }
    }
}
