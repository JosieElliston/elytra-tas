//! `etas`: the toolkit's generic tools as subcommands.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use rayon::prelude::*;
use etas::course::Course;
use etas::plan::{self as eplan, plan_from};
use etas::tape::Tape;
use etas::trajectory::{flight_from_tape, Trajectory, from_tape};
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
    /// A recorded tape's flight -> trajectory starting in the air, for seeding: from tape
    /// tick `--from`, by default the deploy it left the start area on. Exit 1 if the flight
    /// doesn't reproduce the tape's poses.
    Flight {
        tape: PathBuf,
        #[arg(long)]
        course: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long)]
        from: Option<usize>,
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
    /// Render runs in their course's world to a PNG contact sheet: from above, from the
    /// side and in perspective, blocks in front of the path cut away. One run is colored
    /// by speed; several get one color each. A branching plan draws its paths.
    View {
        /// Trajectories (.jsonl) and tapes or plans (.tas).
        runs: Vec<PathBuf>,
        #[arg(long)]
        course: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        /// Frame this tick range (the run is still drawn whole).
        #[arg(long, num_args = 2, value_names = ["FROM", "TO"])]
        ticks: Vec<usize>,
        /// Draw the collision boxes the physics sees, not the block models.
        #[arg(long)]
        collision: bool,
        /// Draw every barrier, translucent. By default only those within 10 blocks of a run
        /// are drawn, solid. They collide either way, so open space in the picture isn't
        /// proof a run can fly there.
        #[arg(long)]
        barriers: bool,
        /// Join ticks with the physics' L-shaped move (y, then the larger horizontal axis,
        /// then the other) instead of straight lines. Noisy at speed; zoom with --ticks.
        #[arg(long)]
        l_step: bool,
        /// Only the course and runs, without replaying them through `verify`.
        #[arg(long)]
        no_verify: bool,
        /// Width of each panel, pixels.
        #[arg(long, default_value_t = 1100)]
        width: u32,
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
        Cmd::Flight {
            tape,
            course,
            out,
            from,
        } => {
            let (c, w) = Course::load(&course)?;
            let t = Tape::read(&tape)?;
            let f = flight_from_tape(&t, &w, &c, &tape.display().to_string(), from)?;
            f.traj.write(&out)?;
            println!(
                "wrote {} ticks from tape tick {} to {}",
                f.traj.rows.len(),
                f.from_tick,
                out.display()
            );
            if let Some((tick, u)) = &f.stopped {
                eprintln!("port stopped at tape tick {tick}: {u:?}");
            }
            if let Some((tick, miss)) = f.parted {
                eprintln!("parts from the tape at tick {tick}, by {miss:.1e}");
            }
            Ok(f.stopped.is_none() && f.parted.is_none())
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
        Cmd::View {
            runs,
            course,
            out,
            ticks,
            collision,
            barriers,
            l_step,
            no_verify,
            width,
        } => {
            let (c, w) = Course::load(&course)?;
            let mut items = Vec::new();
            for path in &runs {
                let name = path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                let name = name
                    .trim_end_matches(".jsonl")
                    .trim_end_matches(".traj")
                    .trim_end_matches(".tas")
                    .to_string();
                items.extend(etas::draw::load_runs(path, &name, &c, &w)?);
            }
            if items.is_empty() {
                return Err("nothing to draw".into());
            }
            if !no_verify {
                items.par_iter_mut().for_each(|r| {
                    r.report = Some(verify(&c, &w, Plan::Trajectory(&r.traj)));
                });
            }
            let scene = view::Scene::new(&w)?;
            for warning in scene.warnings() {
                eprintln!("view: {warning}");
            }
            let opts = etas::draw::SheetOptions {
                ticks: (ticks.len() == 2).then(|| (ticks[0], ticks[1])),
                collision,
                barriers,
                l_step,
                width,
                ..Default::default()
            };
            etas::draw::sheet(&scene, Some(&c), &items, &opts).save(&out)?;
            println!("wrote {}", out.display());
            Ok(true)
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
