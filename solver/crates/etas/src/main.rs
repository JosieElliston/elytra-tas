//! `etas`: the toolkit's generic tools as subcommands.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
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
    /// lacks a working @react.
    Verify {
        file: PathBuf,
        #[arg(long)]
        course: PathBuf,
        /// Print the report as JSON instead.
        #[arg(long)]
        json: bool,
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

fn run(cli: Cli) -> Result<bool, String> {
    match cli.cmd {
        Cmd::Verify { file, course, json } => {
            let (c, w) = Course::load(&course)?;
            let report = if is_tape(&file) {
                verify(&c, &w, Plan::Reactive(&eplan::Plan::read(&file)?))
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
