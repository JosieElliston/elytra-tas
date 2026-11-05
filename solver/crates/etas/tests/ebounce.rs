//! Perfect ebounces on real data: run 1 of the 2026-10-19 ebounce replay (super
//! classic downtown's start area, `tests/data/ebounce/`), whose bounces port flies as
//! perfect ones up to the server's first glide-off; then verify and the plan writer on
//! the perfect-bounce seed of it.
//!
//! The fixture: `extract_run.py <replays>/2026-10-19T23_25_42_ebounce.zip --run 1`
//! (`ebounce_run1.tas`), the world from `extract_world.py` (the command is in
//! `course.json`), and a hand-written course: `docs/emc-course-rules.md`'s start-area box as the
//! ground-allowed region, a placeholder finish (fit_course.py can't fit this map's).

use std::path::{Path, PathBuf};

use etas::course::Course;
use etas::plan::{Plan as PlanFile, plan_from};
use etas::state::keys_of;
use etas::tape::Tape;
use etas::trajectory::seed_perfect_bounces;
use etas::verify::{Plan, verify};
use port::{Pose, TickInput};
use sim::faithful::Faithful;
use sim::state::{GlideState, Input, Keys};
use world::{BlockPos, World};

/// The first bounce's landing tick, and the tick the server's glide-off for it was
/// first handled (the recording's first imperfection).
const LANDING: usize = 30;
const GLIDE_OFF: usize = 34;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ebounce")
}

fn load() -> (Course, World, Tape) {
    let (c, w) = Course::load(&dir()).unwrap();
    let t = Tape::read(&dir().join("ebounce_run1.tas")).unwrap();
    (c, w, t)
}

/// Port flies the recorded bounce exactly as a perfect one, with the bounce jump the
/// replay can't show: the landing at 30 and every tick to 33. At 34 the recording
/// parts: the glide-off arrived 4 ticks after the landing (the move is the non-gliding
/// air move with the sprint flying speed, so W and sprint were held, though the
/// extractor wrote no keys), and the jump press that restarted the glide came a tick
/// late, at 35. A press at 34 would have kept the perfect bounce.
#[test]
fn recorded_bounce_is_perfect_until_the_glide_off() {
    let (c, w, tape) = load();
    let (traj, err) = seed_perfect_bounces(&tape, &w, Some(&c), "test");
    assert!(err.is_none(), "{err:?}");
    let pos = |t: usize| traj.rows[t - 1].state.position;
    for t in 1..GLIDE_OFF {
        assert_eq!(pos(t), tape.rows[t - 1].pos, "tick {t}");
    }
    assert_ne!(pos(GLIDE_OFF), tape.rows[GLIDE_OFF - 1].pos);
    let at = |t: usize| &traj.rows[t - 1].state;
    assert!(at(LANDING).on_ground && at(LANDING).fall_flying);
    assert!(!at(LANDING - 1).on_ground);
    assert!(
        traj.rows[LANDING].input.keys.contains('J'),
        "the bounce jump"
    );
    assert!(!at(LANDING + 1).on_ground && at(LANDING + 1).sprinting);

    // the glide-off before tick 34: the flag off, the server's standing pose
    let mut p = at(GLIDE_OFF - 1).to_player().unwrap();
    p.fall_flying = false;
    p.pose = Pose::Standing;
    let r = &tape.rows[GLIDE_OFF - 1];
    let wr = keys_of("WR").unwrap();
    port::tick(
        &mut p,
        &TickInput {
            y_rot: r.yaw,
            x_rot: r.pitch,
            keys: wr,
        },
        &w,
    )
    .unwrap();
    assert_eq!([p.position.x, p.position.y, p.position.z], r.pos);
    assert!(!p.fall_flying);
    // the press a tick late restarts the glide, and the recording goes on from there
    let r = &tape.rows[GLIDE_OFF];
    assert_eq!(r.keys, "WRJ");
    port::tick(
        &mut p,
        &TickInput {
            y_rot: r.yaw,
            x_rot: r.pitch,
            keys: keys_of(&r.keys).unwrap(),
        },
        &w,
    )
    .unwrap();
    assert!(p.fall_flying);
    assert_eq!([p.position.x, p.position.y, p.position.z], r.pos);

    // the press on time (jump at 34) flies the perfect bounce instead
    let mut q = at(GLIDE_OFF - 1).to_player().unwrap();
    q.fall_flying = false;
    q.pose = Pose::Standing;
    let r = &tape.rows[GLIDE_OFF - 1];
    port::tick(
        &mut q,
        &TickInput {
            y_rot: r.yaw,
            x_rot: r.pitch,
            keys: keys_of("WRJ").unwrap(),
        },
        &w,
    )
    .unwrap();
    assert_eq!([q.position.x, q.position.y, q.position.z], pos(GLIDE_OFF));
    assert!(q.fall_flying && q.pose == Pose::FallFlying);
}

fn glide_of(s: &etas::state::FullState) -> GlideState {
    let k = keys_of(&s.input_keys).unwrap();
    GlideState {
        pos: s.position,
        vel: s.delta_movement,
        on_ground: s.on_ground,
        horizontal_collision: s.horizontal_collision,
        minor_horizontal_collision: s.minor_horizontal_collision,
        supporting_block: s.main_supporting_block_pos.map(|b| BlockPos {
            x: b[0],
            y: b[1],
            z: b[2],
        }),
        on_ground_no_blocks: s.on_ground_no_blocks,
        sprinting: s.sprinting,
        no_jump_delay: s.no_jump_delay,
        keys: Keys {
            forward: k.forward,
            backward: k.backward,
            left: k.left,
            right: k.right,
            jump: k.jump,
            sneak: k.shift,
            sprint: k.sprint,
        },
    }
}

/// The faithful kernel's perfect-ebounce mode flies the recording too, from the deploy
/// through the landing and the jump, to the glide-off, bit for bit.
#[test]
fn faithful_flies_the_recorded_bounce() {
    let (c, w, tape) = load();
    let (traj, _) = seed_perfect_bounces(&tape, &w, Some(&c), "test");
    let deploy = traj.rows.iter().position(|r| r.state.fall_flying).unwrap() + 1;
    let mut s = glide_of(&traj.rows[deploy - 1].state);
    let mut landed = None;
    for t in deploy + 1..GLIDE_OFF {
        let row = &traj.rows[t - 1];
        let k = keys_of(&row.input.keys).unwrap();
        let keys = Keys {
            forward: k.forward,
            backward: k.backward,
            left: k.left,
            right: k.right,
            jump: k.jump,
            sneak: false,
            sprint: k.sprint,
        };
        let u = Input {
            yaw: row.input.yaw as f32,
            pitch: row.input.pitch as f32,
        };
        let tick = Faithful
            .step_glide(&w, &s, &Faithful.look(&u), keys)
            .unwrap();
        assert_eq!(tick.state.pos, tape.rows[t - 1].pos, "tick {t}");
        assert_eq!(tick.state, glide_of(&row.state), "tick {t}");
        if tick.landed {
            landed.get_or_insert(t);
        }
        s = tick.state;
    }
    assert_eq!(landed, Some(LANDING));
}

/// Verify finds the seed's bounces in the start area and the windows a press rescues;
/// the plain tape fails for lacking reactions, the written plan passes them.
#[test]
fn verify_and_plan_the_perfect_bounces() {
    let (c, w, tape) = load();
    let (mut traj, _) = seed_perfect_bounces(&tape, &w, Some(&c), "test");
    // the start area only: the timer starts at 163 in the recording
    traj.rows.truncate(150);
    let r = verify(&c, &w, Plan::Trajectory(&traj));
    print!("{r}");
    assert!(!r.bounces.is_empty());
    let b = &r.bounces[0];
    assert_eq!(b.landing, LANDING);
    assert_eq!(b.jump, Some(LANDING + 1));
    // jump is held at 31 for the bounce, so a press at 32 is no edge. From 35 on a
    // press starts a sprint: the tape's keys stop sprint at 34 (the extractor's guess
    // of none, see above) and hold W and sprint from 35, and a tick without the glide
    // may start one (`canStartSprinting`). The next bounce's jump at 61 then gets the
    // sprint boost the plan's doesn't.
    assert_eq!(
        b.window_to_plan(),
        Some((LANDING + 3, LANDING + 4)),
        "{b:?}"
    );
    let (t, why) = &b.not_rescuable[1];
    assert_eq!(*t, LANDING + 5);
    assert!(
        why.starts_with("tick 61:") && why.contains("started a sprint at 35"),
        "{why}"
    );
    for b in &r.bounces {
        let (t0, t1) = b.window_to_plan().unwrap();
        assert!(t0 <= b.landing + 3 && t1 >= b.landing + 4, "{b:?}");
    }
    // trajectories carry no reactions: only the timer fails (it never starts here)
    assert!(
        r.failures.iter().all(|f| f.contains("timer never starts")),
        "{:?}",
        r.failures
    );

    let plain = traj.to_tape();
    let r = verify(&c, &w, Plan::Tape(&plain));
    assert!(
        r.failures.iter().any(|f| f.contains("no @react")),
        "{:?}",
        r.failures
    );

    let (plan, _) = plan_from(&c, &w, &traj);
    assert_eq!(plan.reacts.len(), r.bounces.len());
    let text = plan.write();
    let back = PlanFile::parse(&text).unwrap();
    assert_eq!(back, plan);
    let r = verify(&c, &w, Plan::Reactive(&back));
    print!("{r}");
    assert!(
        r.failures.iter().all(|f| f.contains("timer never starts")),
        "{:?}",
        r.failures
    );
    assert!(r.bounces.iter().all(|b| b.window.is_some()));

    // a window reaching back over the bounce jump: a glide-off at 32 can't be answered
    let mut bad = back.clone();
    bad.reacts[0].t0 = LANDING + 2;
    let r = verify(&c, &w, Plan::Reactive(&bad));
    assert!(
        r.failures
            .iter()
            .any(|f| f.contains("tick 32 in its window") && f.contains("no edge")),
        "{:?}",
        r.failures
    );
    // a window that starts after the natural delays
    let mut late = back.clone();
    late.reacts[0].t0 = LANDING + 5;
    late.reacts[0].t1 = LANDING + 8;
    let r = verify(&c, &w, Plan::Reactive(&late));
    assert!(
        r.failures
            .iter()
            .any(|f| f.contains("misses a glide-off 3 ticks")),
        "{:?}",
        r.failures
    );
}
