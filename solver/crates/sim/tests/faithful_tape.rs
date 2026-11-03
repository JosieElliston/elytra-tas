//! The faithful kernel against port on the Burrowed Grove tape, in the course's world
//! (the world crate's fixture). Port replays the whole tape bit-exactly (port's
//! README); at every tick its state is in normal flight, the faithful kernel steps
//! the projected state and must land on port's next state bit for bit.

mod common;

use std::time::Instant;

use common::{Agree, agree, bits, burrowed_grove, from_port, tape, tick_input, to_port};
use sim::faithful::{Event, Faithful};
use sim::state::FlightState;

#[test]
fn tape_normal_flight_ticks() {
    let (tape, world) = (tape(), burrowed_grove());
    let mut p = tape.standing_start();
    let (mut flown, mut contacts, mut left) = (0, 0, Vec::new());
    for t in 1..=tape.ticks.len() {
        let full = from_port(&p);
        let flight = full.flight().filter(|_| !tape.input(t).keys.shift);
        let mut lifted = None;
        let f = flight.map(|s| {
            // a port tick from the lifted state: the lift keeps all the move reads
            let mut q = to_port(&s.lift());
            let r = port::tick(&mut q, &tick_input(&tape.flight_input(t)), &world);
            lifted = Some((q, r));
            Faithful.step_input(&world, &s, &tape.flight_input(t))
        });
        let r = port::tick(&mut p, &tape.input(t), &world).map(|_| ());
        let Some(f) = f else { continue };
        let (q, qr) = lifted.unwrap();
        assert_eq!(qr.is_ok(), r.is_ok(), "tick {t}: lifted port tick");
        if r.is_ok() {
            assert_eq!(
                (
                    bits([q.position.x, q.position.y, q.position.z]),
                    q.on_ground
                ),
                (
                    bits([p.position.x, p.position.y, p.position.z]),
                    p.on_ground
                ),
                "tick {t}: port from the lifted state"
            );
        }
        match agree(&f, &p, &r, &format!("tick {t}")) {
            Agree::Flight { contact } => {
                flown += 1;
                contacts += contact as usize;
            }
            Agree::Left => left.push((t, f.unwrap_err())),
        }
    }
    println!("{flown} normal-flight ticks exact ({contacts} with contact); left: {left:?}");
    assert!(flown > 680, "only {flown} normal-flight ticks");
    assert_eq!(left.first().map(|l| l.1), Some(Event::Landing));
}

/// The faithful kernel flying on its own from the first normal-flight state, against
/// port's continuous replay.
#[test]
fn tape_continuous_faithful() {
    let (tape, world) = (tape(), burrowed_grove());
    let mut p = tape.standing_start();
    let mut s: Option<FlightState<f64>> = None;
    let mut start = None;
    let mut states = Vec::new();
    let mut n = 0;
    for t in 1..=tape.ticks.len() {
        if s.is_none() {
            s = from_port(&p).flight();
            start = s.map(|s| (s, t));
        }
        let r = port::tick(&mut p, &tape.input(t), &world);
        let Some(cur) = s else { continue };
        match Faithful.step_input(&world, &cur, &tape.flight_input(t)) {
            Ok(tick) => {
                r.unwrap();
                assert_eq!(
                    (bits(tick.state.pos), bits(tick.state.vel)),
                    (bits(from_port(&p).pos), bits(from_port(&p).vel)),
                    "tick {t}"
                );
                s = Some(tick.state);
                states.push(tick.state);
                n += 1;
            }
            Err(e) => {
                assert_eq!(e, Event::Landing, "tick {t}");
                assert!(p.on_ground, "tick {t}: port didn't land");
                println!("{n} ticks flown, landing at tick {t}");
                // `fly` is the same loop
                let (s0, t0) = start.unwrap();
                let inputs: Vec<_> = (t0..=tape.ticks.len())
                    .map(|t| tape.flight_input(t))
                    .collect();
                let flown = Faithful.fly(&world, &s0, &inputs);
                assert_eq!(
                    flown.ticks.iter().map(|k| k.state).collect::<Vec<_>>(),
                    states
                );
                assert_eq!(flown.event, Some(Event::Landing));
                return;
            }
        }
    }
    panic!("no landing");
}

/// Ticks per second of `Faithful::step` and `port::tick` on the tape's normal-flight
/// ticks, each tick from port's state. `cargo test --release ... -- --ignored
/// --nocapture`.
#[test]
#[ignore]
fn bench_tape() {
    let (tape, world) = (tape(), burrowed_grove());
    let mut p = tape.standing_start();
    let mut cases = Vec::new();
    for t in 1..=tape.ticks.len() {
        if let Some(s) = from_port(&p).flight() {
            cases.push((s, p.clone(), t));
        }
        port::tick(&mut p, &tape.input(t), &world).ok();
    }
    let reps = 200;
    let t0 = Instant::now();
    let mut sink = 0.0;
    for _ in 0..reps {
        for (s, _, t) in &cases {
            if let Ok(k) = Faithful.step_input(&world, s, &tape.flight_input(*t)) {
                sink += k.state.pos[0];
            }
        }
    }
    let faithful = (reps * cases.len()) as f64 / t0.elapsed().as_secs_f64();
    let looks: Vec<_> = cases
        .iter()
        .map(|(_, _, t)| Faithful.look(&tape.flight_input(*t)))
        .collect();
    let t0 = Instant::now();
    for _ in 0..reps {
        for ((s, _, _), k) in cases.iter().zip(&looks) {
            if let Ok(k) = Faithful.step(&world, s, k) {
                sink += k.state.pos[0];
            }
        }
    }
    let faithful_look = (reps * cases.len()) as f64 / t0.elapsed().as_secs_f64();
    let reps = 20;
    let t0 = Instant::now();
    for _ in 0..reps {
        for (_, q, t) in &cases {
            let mut q = q.clone();
            port::tick(&mut q, &tape.input(*t), &world).ok();
            sink += q.position.x;
        }
    }
    let port = (reps * cases.len()) as f64 / t0.elapsed().as_secs_f64();
    println!(
        "{} ticks: faithful {faithful:.3e}/s ({faithful_look:.3e}/s with looks precomputed), port {port:.3e}/s ({sink:e})",
        cases.len()
    );
}
