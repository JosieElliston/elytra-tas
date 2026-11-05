//! `plan::branched`: paths merged into a branching plan, structurally (the lines are
//! the Burrowed Grove tape's, the arrivals made up). Whether such a plan flies is
//! `verify`'s question; the alpine course crate's `solve` asks it of a real one.

mod common;

use etas::plan::{FORMAT, Leaf, PathTape, Plan, branched};
use etas::server::{Arrival, ServerEvent};
use etas::tape::{Tape, TapeRow};

fn vel(tick: usize, x: f64) -> Arrival {
    Arrival {
        tick,
        event: ServerEvent::SetVelocity { v: [x, 0.5, 0.25] },
    }
}

/// `rows` with every pose from tick `from` on moved by `dx`: a path that parted there.
fn moved(rows: &[TapeRow], from: usize, dx: f64) -> Vec<TapeRow> {
    rows.iter()
        .map(|r| {
            let mut r = r.clone();
            if r.t >= from {
                r.pos[0] += dx;
            }
            r
        })
        .collect()
}

fn header() -> (Tape, Vec<TapeRow>) {
    let (_, _, tape) = common::load();
    let rows = tape.rows[..60].to_vec();
    let mut h = tape.clone();
    h.rows.clear();
    let mut keys = vec![
        ("format".to_string(), FORMAT.to_string()),
        ("account".to_string(), "deftenvy".to_string()),
        ("velocity_tol".to_string(), "1e-6".to_string()),
    ];
    keys.append(&mut h.header);
    h.header = keys;
    (h, rows)
}

/// Three paths: none boosted; boosted at 30; boosted at 30 and 40 (that one shorter).
/// The plan's paths are exactly them, and it survives a write and a parse.
#[test]
fn paths_come_back() {
    let (h, rows) = header();
    let b = moved(&rows, 30, 1.0);
    let c = moved(&b, 40, 0.5);
    let input = vec![
        PathTape {
            rows: rows.clone(),
            arrivals: vec![],
        },
        PathTape {
            rows: b,
            arrivals: vec![vel(30, 1.0)],
        },
        PathTape {
            rows: c[..55].to_vec(),
            arrivals: vec![vel(30, 1.0), vel(40, 2.0)],
        },
    ];
    let plan = branched(&h, vec![], &input).unwrap();
    assert_eq!(plan.segments.len(), 2);
    let text = plan.write();
    let back = Plan::parse(&text).unwrap();
    // (the plan in memory keeps the tape's ticks=; the written one counts its lines)
    assert_eq!(back.write(), text);
    assert_eq!(back.segments, plan.segments);
    let paths = back.paths();
    assert_eq!(paths.len(), 3);
    for p in &paths {
        assert_eq!(p.leaf, Leaf::End);
        let want = input.iter().find(|i| i.arrivals == p.arrivals).unwrap();
        assert_eq!(p.tape.rows, want.rows);
    }
}

/// A path whose arrival is the only thing that tells it apart must still have it: two
/// paths that differ in their lines under the same arrivals can't be flown.
#[test]
fn parting_without_an_observation_is_refused() {
    let (h, rows) = header();
    let input = vec![
        PathTape {
            rows: rows.clone(),
            arrivals: vec![vel(30, 1.0)],
        },
        PathTape {
            rows: moved(&rows, 45, 1.0),
            arrivals: vec![vel(30, 1.0)],
        },
    ];
    let e = branched(&h, vec![], &input).unwrap_err();
    assert!(e.contains("tick 45"), "{e}");
}
