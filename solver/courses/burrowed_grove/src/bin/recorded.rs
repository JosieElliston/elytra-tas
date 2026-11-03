//! Phase `recorded`: the recorded run (`tas/burrowed_grove.tas`) as a trajectory,
//! replayed with port in the course's world, written to `runs/recorded.traj.jsonl` and
//! verified. The smoke test of the chain tape -> port -> trajectory -> verify: it fails
//! unless verify passes and its predicted timer and finish agree with the tape's
//! recorded `timer_tick`, `finish_tick` and `finish_text`.
//!
//! `cargo run --release -p burrowed_grove --bin recorded`

use std::path::Path;
use std::process::ExitCode;

use etas::course::Course;
use etas::tape::Tape;
use etas::trajectory::{Trajectory, from_tape};
use etas::verify::{Plan, verify};
use sha2::{Digest, Sha256};

fn run() -> Result<bool, String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (course, world) = Course::load(dir)?;
    let tape_path = course.repo_root().join(&course.start.source.path);
    let bytes = std::fs::read(&tape_path).map_err(|e| format!("{}: {e}", tape_path.display()))?;
    let sha: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if sha != course.start.source.sha256 {
        return Err(format!(
            "{}: sha256 {sha}, the course's start was taken from {}",
            tape_path.display(),
            course.start.source.sha256
        ));
    }
    let tape = Tape::read(&tape_path)?;
    let (traj, err) = from_tape(&tape, &world, Some(&course), &course.start.source.path);
    if let Some((t, u)) = err {
        return Err(format!("port stopped at tick {t}: {u:?}"));
    }
    let out = dir.join("runs/recorded.traj.jsonl");
    std::fs::create_dir_all(out.parent().unwrap()).map_err(|e| e.to_string())?;
    traj.write(&out)?;
    let traj = Trajectory::read(&out)?;
    let report = verify(&course, &world, Plan::Trajectory(&traj));
    println!("{}: {} ticks", out.display(), traj.rows.len());
    print!("{report}");

    // the prediction against what EMC said for this run
    let mut ok = report.ok();
    let [l0, l1] = course.observation.tick_chat_lag;
    let header = |k: &str| tape.get(k).ok_or(format!("tape has no {k}="));
    let timer_chat: usize = header("timer_tick")?
        .parse()
        .map_err(|e| format!("timer_tick: {e}"))?;
    let finish_chat: usize = header("finish_tick")?
        .parse()
        .map_err(|e| format!("finish_tick: {e}"))?;
    let shown: f64 = header("finish_text")?
        .split("Your time:")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse().ok())
        .ok_or("finish_text has no time")?;
    let (Some(ts), Some(tf), Some(secs)) = (
        report.timer_tick(),
        report.finish_tick(),
        report.timer_seconds(),
    ) else {
        println!("MISMATCH: no predicted timer or finish");
        return Ok(false);
    };
    let checks = [
        (
            "timer chat",
            (ts + l0..=ts + l1).contains(&timer_chat),
            format!("tape {timer_chat}, predicted {}..{}", ts + l0, ts + l1),
        ),
        (
            "finish chat",
            (tf + l0..=tf + l1).contains(&finish_chat),
            format!("tape {finish_chat}, predicted {}..{}", tf + l0, tf + l1),
        ),
        (
            "EMC time",
            (secs - shown).abs() <= 0.025,
            format!("shown {shown:.3} s, predicted {secs:.2} s"),
        ),
    ];
    for (what, good, detail) in checks {
        println!(
            "{} {what}: {detail}",
            if good { "agrees" } else { "MISMATCH" }
        );
        ok &= good;
    }
    Ok(ok)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("recorded: {e}");
            ExitCode::from(2)
        }
    }
}
