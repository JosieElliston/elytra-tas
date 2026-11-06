//! `verify` on the recorded run, on a run with one pitch changed, and on recorded
//! states that were tampered with.

mod common;

use etas::trajectory::from_tape;
use etas::verify::{Plan, verify};

#[test]
fn recorded_tape_passes() {
    let (c, w, tape) = common::load();
    let r = verify(&c, &w, Plan::Tape(&tape));
    print!("{r}");
    assert!(r.ok(), "{:?}", r.failures);
    assert_eq!(r.timer_tick(), Some(43));
    assert_eq!(r.finish_tick(), Some(690));
    let (traj, _) = from_tape(&tape, &w, Some(&c), "test");
    let r = verify(&c, &w, Plan::Trajectory(&traj));
    assert!(r.ok(), "{:?}", r.failures);
}

/// The recorded prefix in the course file flies the tape's first ticks, up to the
/// deploy.
#[test]
fn course_start_is_the_tape_prefix() {
    let (c, w, tape) = common::load();
    let mut p = c.start_state().unwrap();
    let inputs = c.start_inputs().unwrap();
    assert_eq!(inputs.len(), c.start.glide_tick);
    for (i, (input, keys)) in inputs.iter().enumerate() {
        let r = &tape.rows[i];
        assert_eq!(keys, &r.keys);
        port::tick(&mut p, input, &w).unwrap();
        assert_eq!(
            [p.position.x, p.position.y, p.position.z],
            r.pos,
            "tick {}",
            r.t
        );
        assert_eq!(p.fall_flying, r.t >= c.start.glide_tick, "tick {}", r.t);
    }
}

/// One pitch changed by 20 degrees, re-flown by port so the recorded states are
/// consistent: the course rules have to catch it. (10 degrees at tick 100 still
/// finishes cleanly; elytra flight forgives a lot.)
#[test]
fn changed_pitch_fails() {
    let (c, w, tape) = common::load();
    for (t, expect_finish) in [(60, false), (100, true)] {
        let mut tp = tape.clone();
        tp.rows[t - 1].pitch += 20.0;
        let (traj, err) = from_tape(&tp, &w, Some(&c), "test");
        assert!(err.is_none(), "{err:?}");
        let r = verify(&c, &w, Plan::Trajectory(&traj));
        print!("pitch +20 at tick {t}:\n{r}");
        assert!(!r.ok());
        assert!(r.mismatch.is_none(), "the re-flown states should reproduce");
        assert!(
            !r.kills.is_empty(),
            "should touch the ground outside the start area"
        );
        assert_eq!(r.finish_tick().is_some(), expect_finish);
        if let Some(f) = r.finish_tick() {
            assert!(r.kills[0].tick < f);
        }
    }
}

#[test]
fn tampered_states_fail() {
    let (c, w, tape) = common::load();
    let (mut traj, _) = from_tape(&tape, &w, Some(&c), "test");
    traj.rows[299].state.position[0] += 1e-9;
    let r = verify(&c, &w, Plan::Trajectory(&traj));
    assert_eq!(
        r.mismatch.as_ref().map(|m| m.0),
        Some(300),
        "{:?}",
        r.failures
    );
    assert!(!r.ok());

    let (mut traj, _) = from_tape(&tape, &w, Some(&c), "test");
    traj.rows[499].state.fall_distance += 0.5;
    let r = verify(&c, &w, Plan::Trajectory(&traj));
    assert_eq!(r.mismatch.as_ref().map(|m| m.0), Some(500));

    let mut t2 = tape.clone();
    t2.rows[399].pos[2] = f64::from_bits(t2.rows[399].pos[2].to_bits() + 1);
    let r = verify(&c, &w, Plan::Tape(&t2));
    assert_eq!(r.mismatch.as_ref().map(|m| m.0), Some(400));
    assert!(!r.ok());
}
