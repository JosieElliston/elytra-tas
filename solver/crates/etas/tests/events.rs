//! Recorded runs with server events: imperfect ebounces (glide-offs the recording
//! answered late, or not at all) and boosters (velocities). Two fixtures:
//!
//! - `tests/data/ebounce_events/`: run 1 of the 2026-10-19 ebounce replay (super classic
//!   downtown), 4 glide-offs, as `extract_run.py` writes it now, with a world that covers
//!   the whole run and the ebounce fixture's provisional course (finish placeholder
//!   moved to where the finish chat says it was judged).
//! - `tests/data/alpine/`: run 0 of the 2026-10-20T02_06_28 alpine replay, 7 booster
//!   velocities before the finish and a wall-hit echo after it, with a hand-made,
//!   provisional course (`course.json`'s `fitted_from.note`).

use std::path::{Path, PathBuf};

use etas::course::Course;
use etas::plan::plan_from;
use etas::server::{Arrival, ServerEvent};
use etas::tape::Tape;
use etas::trajectory::{Trajectory, from_tape, seed_perfect_bounces};
use etas::verify::{Plan, verify, verify_path};
use world::World;

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

fn load(name: &str, tape: &str) -> (Course, World, Tape) {
    let (c, w) = Course::load(&data(name)).unwrap();
    let t = Tape::read(&data(name).join(tape)).unwrap();
    (c, w, t)
}

fn ebounce() -> (Course, World, Tape) {
    load("ebounce_events", "ebounce_run1.tas")
}

fn alpine() -> (Course, World, Tape) {
    load("alpine", "alpine_run0.tas")
}

fn count(t: &Tape, glide: bool) -> usize {
    t.arrivals
        .iter()
        .filter(|a| (a.event == ServerEvent::GlideOff) == glide)
        .count()
}

/// The tapes read and write back byte for byte, `@server` lines included, and through
/// a trajectory (version 2, arrivals on line 3) too.
#[test]
fn tapes_and_trajectories_carry_arrivals() {
    for (name, file) in [
        ("ebounce_events", "ebounce_run1.tas"),
        ("alpine", "alpine_run0.tas"),
    ] {
        let (c, w, tape) = load(name, file);
        let text = std::fs::read_to_string(data(name).join(file)).unwrap();
        assert!(tape.is_record());
        assert_eq!(tape.write(), text, "{file}");
        let (traj, err) = from_tape(&tape, &w, Some(&c), "test");
        assert!(err.is_none(), "{err:?}");
        assert_eq!(traj.arrivals, tape.arrivals);
        let dir = std::env::temp_dir().join(format!("etas-events-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.traj.jsonl");
        traj.write(&path).unwrap();
        let back = Trajectory::read(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(back, traj);
        // the trajectory's poses are port's: the alpine record's differ in the last
        // place from tick 1316 (`port_replays_records_with_arrivals`)
        let tape2 = back.to_tape();
        assert_eq!(tape2.arrivals, tape.arrivals);
        let exact = if name == "alpine" {
            1315
        } else {
            tape.rows.len()
        };
        assert_eq!(tape2.rows[..exact], tape.rows[..exact]);
    }
    let (_, _, t) = ebounce();
    assert_eq!((count(&t, true), count(&t, false)), (4, 0));
    let (_, _, t) = alpine();
    assert_eq!((count(&t, true), count(&t, false)), (0, 8));
}

/// A version-1 trajectory (no arrivals line) still reads, as one with no arrivals.
#[test]
fn version_1_trajectories_read() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../courses/burrowed_grove/runs/recorded.traj.jsonl");
    let t = Trajectory::read(&p).unwrap();
    assert_eq!(t.header.version, 1);
    assert!(t.arrivals.is_empty());
}

/// `@server` lines: malformed ones, ones after their tick line, and plan directives in
/// a tape are refused; plan readers refuse a record.
#[test]
fn server_lines_are_checked() {
    let (_, _, tape) = ebounce();
    let text = tape.write();
    let bad = text.replace("@server 34 glide_off", "@server 34 glide_of");
    assert!(Tape::parse(&bad).is_err());
    let late = text.replace("@server 34 glide_off\n", "").replace(
        "@server 100 glide_off",
        "@server 100 glide_off\n@server 34 glide_off",
    );
    assert!(Tape::parse(&late).unwrap_err().contains("before tick line"));
    let a =
        Arrival::parse(&["7", "velocity", "3ff0000000000000", "8000000000000000", "0"]).unwrap();
    assert_eq!(
        a.event,
        ServerEvent::SetVelocity {
            v: [1.0, -0.0, 0.0]
        }
    );
    assert_eq!(
        Arrival::parse(&a.line().split_whitespace().collect::<Vec<_>>()[1..]).unwrap(),
        a
    );
    assert!(etas::plan::Plan::parse(&text).is_err());
}

/// Port replays both records bit for bit with their arrivals (the alpine one within
/// `RECORD_TOL` from tick 1316, the JVM's `Math.cos`), and neither without them.
#[test]
fn port_replays_records_with_arrivals() {
    for (c, w, tape) in [ebounce(), alpine()] {
        let (traj, err) = from_tape(&tape, &w, Some(&c), "test");
        assert!(err.is_none(), "{err:?}");
        assert_eq!(traj.rows.len(), tape.rows.len());
        let mut inexact = 0;
        for (r, t) in traj.rows.iter().zip(&tape.rows) {
            let d = (0..3)
                .map(|k| (r.state.position[k] - t.pos[k]).abs())
                .fold(0.0, f64::max);
            assert!(d <= etas::verify::RECORD_TOL, "tick {}: off by {d:e}", t.t);
            assert_eq!(r.state.on_ground, t.ground, "tick {}", t.t);
            inexact += (d > 0.0) as usize;
        }
        let lost = traj
            .rows
            .iter()
            .filter(|r| !r.state.fall_flying && r.t > c.start.glide_tick)
            .count();
        println!(
            "{}: {} ticks, {} after the deploy without the glide, {} inexact",
            c.name,
            traj.rows.len(),
            lost,
            inexact
        );
        let mut bare = tape.clone();
        bare.arrivals.clear();
        let r = verify(&c, &w, Plan::Tape(&bare));
        let first = tape.arrivals[0].tick;
        assert_eq!(
            r.mismatch.as_ref().map(|m| m.0),
            Some(first),
            "{c:?}",
            c = c.name
        );
    }
}

/// `etas verify` passes both records under their provisional courses.
#[test]
fn records_verify() {
    for (c, w, tape) in [ebounce(), alpine()] {
        let r = verify(&c, &w, Plan::Tape(&tape));
        print!("{}:\n{r}", c.name);
        assert!(r.ok(), "{:?}", r.failures);
        assert!(r.mismatch.is_none());
        assert_eq!(r.arrivals, tape.arrivals);
        // a record needs no reactions; its bounces are only reported
        assert!(r.bounces.iter().all(|b| b.window.is_none()));
    }
    let (c, w, tape) = ebounce();
    let r = verify(&c, &w, Plan::Tape(&tape));
    // the bounces whose glide-off the recording lost
    assert_eq!(r.bounces.len(), 5);
    assert!(r.inexact.is_none());
    let (c, w, tape) = alpine();
    let r = verify(&c, &w, Plan::Tape(&tape));
    assert_eq!(r.inexact.map(|x| x.0), Some(1316));
}

/// `verify_path`: a plan's perfect bounce, with the glide-off supplied where the plan's
/// window expects it, fires the reaction and flies the plan; outside the window the
/// glide is lost and the poses part.
#[test]
fn verify_path_fires_reactions() {
    let (c, w) = Course::load(&data("ebounce")).unwrap();
    let tape = Tape::read(&data("ebounce").join("ebounce_run1.tas")).unwrap();
    let (mut traj, _) = seed_perfect_bounces(&tape, &w, Some(&c), "test");
    traj.rows.truncate(150);
    let (plan, _) = plan_from(&c, &w, &traj);
    let first = plan.reacts[0];
    for t in first.t0..=first.t1 {
        let at = [Arrival {
            tick: t,
            event: ServerEvent::GlideOff,
        }];
        let r = verify_path(&c, &w, Plan::Reactive(&plan), &at);
        assert!(r.mismatch.is_none(), "glide-off at {t}: {:?}", r.mismatch);
        assert_eq!(r.reactions, vec![(t, first.line())]);
        assert!(
            r.failures.iter().all(|f| f.contains("timer never starts")),
            "{:?}",
            r.failures
        );
    }
    // a glide-off before the window (the tick after the bounce jump, where a press
    // is no edge) fires nothing: the glide is lost and the poses part on that tick
    let at = [Arrival {
        tick: first.t0 - 1,
        event: ServerEvent::GlideOff,
    }];
    let r = verify_path(&c, &w, Plan::Reactive(&plan), &at);
    assert!(r.reactions.is_empty());
    assert_eq!(r.mismatch.as_ref().map(|m| m.0), Some(first.t0 - 1));
    // a velocity the plan didn't fly with
    let at = [Arrival {
        tick: 100,
        event: ServerEvent::SetVelocity { v: [0.0, 1.0, 0.0] },
    }];
    let r = verify_path(&c, &w, Plan::Reactive(&plan), &at);
    assert_eq!(r.mismatch.as_ref().map(|m| m.0), Some(100));
}

/// Re-extracting the fixtures from Josie's replays gives them back byte for byte
/// (skipped when the replays or python3 aren't there; `ETAS_REPLAY_DIR` overrides the
/// Modrinth profile's replay folder).
#[test]
fn extraction_round_trip() {
    let replays = std::env::var("ETAS_REPLAY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join("Library/Application Support/ModrinthApp/profiles/main/flashback/replays")
        });
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for (replay, run, fixture) in [
        (
            "2026-10-19T23_25_42_ebounce.zip",
            "1",
            "ebounce_events/ebounce_run1.tas",
        ),
        (
            "2026-10-20T02_06_28_alpine.zip",
            "0",
            "alpine/alpine_run0.tas",
        ),
    ] {
        let src = replays.join(replay);
        if !src.exists() {
            eprintln!("skipped: no {}", src.display());
            continue;
        }
        let out =
            std::env::temp_dir().join(format!("etas-extract-{}-{run}.tas", std::process::id()));
        let st = std::process::Command::new("python3")
            .current_dir(&root)
            .args(["extract_run.py", src.to_str().unwrap(), "--run", run, "-o"])
            .arg(&out)
            .status();
        let Ok(st) = st else {
            eprintln!("skipped: no python3");
            return;
        };
        assert!(st.success(), "extract_run.py {replay}: {st}");
        let got = std::fs::read_to_string(&out).unwrap();
        std::fs::remove_file(&out).unwrap();
        let want = std::fs::read_to_string(data(fixture)).unwrap();
        assert!(got == want, "{replay} run {run} differs from {fixture}");
    }
}
