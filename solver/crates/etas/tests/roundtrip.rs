//! Both artifact formats survive a write and a read, and the state survives the trip
//! through `port::LocalPlayer`.

mod common;

use etas::state::FullState;
use etas::tape::Tape;
use etas::trajectory::{Trajectory, from_tape};

#[test]
fn trajectory_roundtrip() {
    let (c, w, tape) = common::load();
    let (traj, err) = from_tape(&tape, &w, Some(&c), "test");
    assert!(err.is_none(), "{err:?}");
    assert_eq!(traj.rows.len(), tape.rows.len());
    let dir = std::env::temp_dir().join(format!("etas-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rec.traj.jsonl");
    traj.write(&path).unwrap();
    let back = Trajectory::read(&path).unwrap();
    assert_eq!(back, traj);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Every replayed state, to a `LocalPlayer` and back, unchanged; and the player built
/// from it equals port's own. why the second? a field port adds later would be left at
/// its default by `to_player` and silently missing from artifacts.
#[test]
fn full_state_is_all_of_local_player() {
    let (_, w, tape) = common::load();
    let mut p = etas::course::rest_state(tape.start_pos, tape.start_yaw, tape.start_pitch);
    for r in &tape.rows {
        let input = port::TickInput {
            y_rot: r.yaw,
            x_rot: r.pitch,
            keys: etas::state::keys_of(&r.keys).unwrap(),
        };
        port::tick(&mut p, &input, &w).unwrap();
        let s = FullState::from(&p);
        let q = s.to_player().unwrap();
        assert_eq!(q, p, "tick {}", r.t);
        assert_eq!(FullState::from(&q), s, "tick {}", r.t);
    }
}

/// tape -> trajectory -> tape gives back the file byte for byte.
#[test]
fn tape_roundtrip() {
    let (c, w, tape) = common::load();
    let text = std::fs::read_to_string(common::tape_path()).unwrap();
    assert_eq!(Tape::parse(&tape.write()).unwrap(), tape);
    let (traj, _) = from_tape(&tape, &w, Some(&c), "test");
    let back = traj.to_tape();
    assert_eq!(back, tape);
    assert_eq!(back.write(), text);
}
