//! Phase `flights`: every seed tape in `runs/seeds/` (`segments.py --tapes`) as a
//! trajectory starting in the air, the deploy it left the start area on, written to
//! `runs/flights/` and verified. Prints one line per tape, slowest last; fails only on an
//! I/O error, since a recording that broke a course rule is still a usable seed.
//!
//! `cargo run --release -p academy --bin flights`

use std::path::Path;
use std::process::ExitCode;

use etas::course::Course;
use etas::tape::Tape;
use etas::trajectory::{Trajectory, flight_from_tape};
use etas::verify::{Plan, verify};

fn run() -> Result<(), String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (course, world) = Course::load(dir)?;
    let out = dir.join("runs/flights");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut tapes: Vec<_> = std::fs::read_dir(dir.join("runs/seeds"))
        .map_err(|e| format!("runs/seeds: {e}; run segments.py --tapes first"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "tas"))
        .collect();
    tapes.sort();
    for path in tapes {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let tape = Tape::read(&path)?;
        let f = match flight_from_tape(&tape, &world, &course, &name, None) {
            Ok(f) => f,
            Err(e) => {
                println!("{name}: {e}");
                continue;
            }
        };
        let file = out.join(format!("{name}.traj.jsonl"));
        f.traj.write(&file)?;
        let report = verify(&course, &world, Plan::Trajectory(&Trajectory::read(&file)?));
        let timer = report.timer.first().and_then(|(_, c)| c.as_ref().map(|c| c.tick));
        let finish = report.finish.first().and_then(|(_, c)| c.as_ref().map(|c| c.tick));
        let ticks = timer.zip(finish).map(|(a, b)| b - a);
        println!(
            "{name}: from tape tick {}, {} ticks; timed {}; parts {}; {}{}",
            f.from_tick,
            f.traj.rows.len(),
            ticks.map_or("-".into(), |t| format!("{t} ({:.2} s)", t as f64 * 0.05)),
            f.parted.map_or("never".into(), |(t, m)| format!("at tape tick {t} by {m:.1e}")),
            if report.ok() { "PASS" } else { "FAIL" },
            report.failures.first().map_or(String::new(), |e| format!(": {e}")),
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("flights: {e}");
            ExitCode::FAILURE
        }
    }
}
