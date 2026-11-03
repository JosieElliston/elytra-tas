//! The Burrowed Grove tape (`tas/burrowed_grove.tas`): real client poses, end of tick.

mod common;

use common::{Tape, read_tape, repo_root};
use port::{BlockMap, Input, LocalPlayer, Pose, TickInput, Vec3, tick};

fn tape() -> Tape {
    read_tape(&repo_root().join("tas/burrowed_grove.tas"))
}

fn input(tape: &Tape, t: usize) -> TickInput {
    let k = tape.tick(t);
    TickInput {
        y_rot: k.yaw,
        x_rot: k.pitch,
        keys: k.keys,
    }
}

/// The velocity at the end of tick `t`, which the tape doesn't carry, from its poses
/// as the extractor reads them: gliding, the displacement (the elytra path moves by
/// the velocity and doesn't change it after); otherwise `travelInAir`'s drag applied
/// to the displacement, with the ground friction if the tick started on ground.
fn derive_velocity(tape: &Tape, t: usize) -> Vec3 {
    let d = Vec3::new(
        tape.pos(t).x - tape.pos(t - 1).x,
        tape.pos(t).y - tape.pos(t - 1).y,
        tape.pos(t).z - tape.pos(t - 1).z,
    );
    if t >= tape.glide_tick() {
        d
    } else {
        let friction = (if tape.ground(t - 1) { 0.6f32 } else { 1.0f32 }) * 0.91f32;
        Vec3::new(
            d.x * friction as f64,
            (d.y - 0.08) * 0.98f32 as f64,
            d.z * friction as f64,
        )
    }
}

/// The player at the end of tick `t`, rebuilt from the tape.
fn state_after(tape: &Tape, t: usize) -> LocalPlayer {
    let (yaw, pitch) = if t == 0 {
        (tape.start_yaw, tape.start_pitch)
    } else {
        (tape.tick(t).yaw, tape.tick(t).pitch)
    };
    let mut p = LocalPlayer::new(tape.pos(t), yaw, pitch);
    p.on_ground = tape.ground(t);
    p.fall_flying = t >= tape.glide_tick();
    p.pose = if p.fall_flying {
        Pose::FallFlying
    } else {
        Pose::Standing
    };
    let keys: Input = tape.keys(t);
    p.input.tick(keys);
    // the extractor writes R from the recorded sprint flag
    p.sprinting = keys.sprint;
    p.sprint_speed_modifier = keys.sprint;
    p.delta_movement = derive_velocity(tape, t);
    p
}

/// The start pose, at rest on the ground: the velocity a standing player settles at,
/// `(0 - 0.08) * 0.98F` (gravity, then drag; the move clips it, the velocity keeps it).
fn standing_start(tape: &Tape) -> LocalPlayer {
    let mut p = LocalPlayer::new(tape.start_pos, tape.start_yaw, tape.start_pitch);
    p.delta_movement = Vec3::new(0.0, (0.0 - 0.08) * 0.98f32 as f64, 0.0);
    p
}

fn err(a: Vec3, b: Vec3) -> f64 {
    (a.x - b.x)
        .abs()
        .max((a.y - b.y).abs())
        .max((a.z - b.z).abs())
}

/// Airborne ticks only (start and end of tick off the ground), each from the tape's
/// previous pose, in an empty world. Ground and contact ticks need the course's world:
/// see `tape_full_replay_with_world`.
#[test]
fn tape_airborne_ticks_empty_world() {
    let tape = tape();
    let level = BlockMap::default();
    let mismatch = tape.glide_mismatch();
    let mut worst: (f64, usize) = (0.0, 0);
    let mut n = 0;
    let mut bad = Vec::new();
    for t in 3..=tape.ticks.len() {
        if tape.ground(t - 1) || tape.ground(t) {
            continue;
        }
        let mut p = state_after(&tape, t - 1);
        tick(&mut p, &input(&tape, t), &level).unwrap();
        let e = err(p.position, tape.pos(t));
        n += 1;
        if e > worst.0 {
            worst = (e, t);
        }
        if e > 1e-12 {
            bad.push((t, e));
        }
        assert_eq!(
            p.fall_flying,
            t >= tape.glide_tick(),
            "tick {t}: glide flag"
        );
    }
    println!(
        "{n} airborne ticks; worst {:.3e} at tick {}",
        worst.0, worst.1
    );
    println!("over 1e-12: {bad:?}; the extractor's glide_mismatch: {mismatch:?}");
    assert!(n > 650, "only {n} airborne ticks");
    assert!(bad.is_empty(), "ticks off by more than 1e-12: {bad:?}");
}

/// The whole tape continuously from the start pose, with a flat stone floor at the
/// start standing in for the course (the ground ticks are on flat ground there), up
/// to the first ground contact after the glide.
#[test]
fn tape_continuous_replay_flat_floor() {
    let tape = tape();
    let mut level = BlockMap::default();
    // the ground ticks 1-16 stay over x 1.3..3.5, z 56.5..58.5; the glide drops below
    // y 166 at x < -1.2, past this floor
    level.floor(0, 5, 165, 54, 60);
    let mut p = standing_start(&tape);
    let mut worst: (f64, usize) = (0.0, 0);
    let mut last = 0;
    for t in 1..=tape.ticks.len() {
        if t > tape.glide_tick() && tape.ground(t) {
            break;
        }
        let ev = tick(&mut p, &input(&tape, t), &level).unwrap();
        assert_eq!(
            ev.start_fall_flying,
            t == tape.glide_tick(),
            "tick {t}: glide start"
        );
        assert_eq!(p.on_ground, tape.ground(t), "tick {t}: onGround");
        let e = err(p.position, tape.pos(t));
        if e > worst.0 {
            worst = (e, t);
        }
        last = t;
    }
    println!(
        "ticks 1..={last}: worst position error {:.3e} at tick {}",
        worst.0, worst.1
    );
    assert_eq!(worst.0, 0.0, "continuous replay is not bit-exact");
}

/// The full tape, ground and contact ticks included, against the course's world: the
/// world crate's fixture, extracted from the tape's source replay (`source=` in its
/// header).
#[test]
fn tape_full_replay_with_world() {
    let tape = tape();
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../world/tests/data/burrowed_grove.world.gz"
    );
    let world = world::World::load(std::path::Path::new(path)).unwrap();
    let mut p = standing_start(&tape);
    for t in 1..=tape.ticks.len() {
        tick(&mut p, &input(&tape, t), &world).unwrap_or_else(|u| panic!("tick {t}: {u:?}"));
        let e = err(p.position, tape.pos(t));
        assert_eq!(e, 0.0, "tick {t}: off by {e:e}");
        assert_eq!(p.on_ground, tape.ground(t), "tick {t}: onGround");
    }
}
