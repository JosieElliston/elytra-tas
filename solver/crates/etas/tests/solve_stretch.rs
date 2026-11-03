//! Re-solving a stretch of the recorded Burrowed Grove run: the splice is the identity
//! on recorded inputs, and one tick saved on a stretch survives the faithful repair,
//! the splice and `verify`.

mod common;

use etas::solve::ddp::{Goal, Opts, Problem};
use etas::solve::refly::repair;
use etas::solve::stretch::{resample, splice, stretch};
use etas::solve::walls::Walls;
use etas::trajectory::{Header, from_tape};
use etas::verify::{Plan, verify};
use sim::generic::Generic;
use sim::state::Input;

#[test]
fn splicing_the_recorded_inputs_is_the_identity() {
    let (course, world, tape) = common::load();
    let (rec, err) = from_tape(&tape, &world, Some(&course), "test");
    assert!(err.is_none());
    let (a, b) = (290, 350);
    let inputs: Vec<Input<f32>> = rec.rows[a..b]
        .iter()
        .map(|r| Input {
            yaw: r.input.yaw as f32,
            pitch: r.input.pitch as f32,
        })
        .collect();
    let (t, err) = splice(&rec, a, b, &inputs, &world, rec.header.clone());
    assert!(err.is_none());
    assert_eq!(t.rows, rec.rows);
}

#[test]
fn a_tick_saved_on_a_stretch_verifies() {
    let (course, world, tape) = common::load();
    let (rec, _) = from_tape(&tape, &world, Some(&course), "test");
    let base = verify(&course, &world, Plan::Trajectory(&rec));
    let st = stretch(&rec, 400, 460).unwrap();
    let walls = Walls::around(&world, &st.path, 4.0, 1.0);
    let pb = Problem {
        kernel: Generic::default(),
        walls: &walls,
        x0: st.x0,
        goal: Goal::Reach {
            target: st.target,
            scale: [1.0, 1.0, 1.0, 0.05, 0.05, 0.05],
        },
        opts: Opts {
            tol: 1e-6,
            ..Opts::default()
        },
    };
    let s = pb.solve(&resample(&st.seed, 59));
    assert!(s.feasible, "{:?}", s.stages);
    assert!(s.plan.x.iter().all(|x| walls.covers([x[0], x[1], x[2]])));
    let rep = repair(&world, &pb, &s, st.yaw_from, 100.0, 3);
    assert!(
        rep.clean(),
        "contacts {:?} event {:?}",
        rep.contacts,
        rep.event
    );
    assert_eq!(rep.inputs.len(), 59);
    let end = rep.flown.last().unwrap();
    let err = (0..3)
        .map(|i| (end[i] - st.target[i]).abs())
        .fold(0.0, f64::max);
    assert!(err < 1e-3, "end position off by {err}");
    let header = Header::new(Some(&course), "test", "test");
    let (t, stop) = splice(&rec, st.a, st.b, &rep.inputs, &world, header);
    assert!(stop.is_none());
    let r = verify(&course, &world, Plan::Trajectory(&t));
    assert!(r.ok(), "{r}");
    let span = |r: &etas::verify::Report| r.finish_tick().unwrap() - r.timer_tick().unwrap();
    assert_eq!(span(&r) + 1, span(&base));
}
