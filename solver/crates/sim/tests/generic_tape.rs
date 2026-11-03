//! The generic kernel against the Burrowed Grove tape (`tas/burrowed_grove.tas`): the
//! regression set ARCHITECTURE.md bounds its gap from the game on.
//!
//! The tape has positions only, so a tick's starting velocity is the previous tick's
//! displacement (exact up to the subtraction's rounding, ~1e-14). Tick t flies with
//! tick t's rotation (README, "Tick order"). Run with `--nocapture` for the gap report.

use sim::generic::{Generic, Scalar};
use sim::state::{FlightState, Input};

struct Row {
    yaw: f32,
    pitch: f32,
    pos: [f64; 3],
    ground: bool,
}

struct Tape {
    glide_tick: usize,
    rows: Vec<Row>, // rows[t] is tick t; tick 0 is the start pose
}

fn load() -> Tape {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tas/burrowed_grove.tas"
    );
    let text = std::fs::read_to_string(path).expect("read the tape");
    let mut glide_tick = None;
    let mut rows = Vec::new();
    for line in text.lines() {
        let (data, comment) = line.split_once('#').unwrap_or((line, ""));
        let data = data.trim();
        if let Some((k, v)) = data.split_once('=') {
            match k {
                "glide_tick" => glide_tick = Some(v.parse().unwrap()),
                "start" => {
                    let f: Vec<&str> = v.split_whitespace().collect();
                    let d = |s: &str| f64::from_bits(u64::from_str_radix(s, 16).unwrap());
                    let h = |s: &str| f32::from_bits(u32::from_str_radix(s, 16).unwrap());
                    rows.push(Row {
                        pos: [d(f[0]), d(f[1]), d(f[2])],
                        yaw: h(f[3]),
                        pitch: h(f[4]),
                        ground: true,
                    });
                }
                _ => {}
            }
            continue;
        }
        if data.is_empty() {
            continue;
        }
        let f: Vec<&str> = data.split_whitespace().collect();
        assert_eq!(
            f[0].parse::<usize>().unwrap(),
            rows.len(),
            "ticks are consecutive from 1"
        );
        let d = |s: &str| f64::from_bits(u64::from_str_radix(s, 16).unwrap());
        let h = |s: &str| f32::from_bits(u32::from_str_radix(s, 16).unwrap());
        rows.push(Row {
            yaw: h(f[1]),
            pitch: h(f[2]),
            pos: [d(f[4]), d(f[5]), d(f[6])],
            ground: comment.split_whitespace().any(|w| w == "ground"),
        });
    }
    Tape {
        glide_tick: glide_tick.expect("glide_tick in header"),
        rows,
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn max_abs(a: [f64; 3]) -> f64 {
    a.iter().fold(0.0, |m, x| m.max(x.abs()))
}

/// Ticks t whose start state (t-1, with velocity from t-2) and end are both in the
/// air while gliding. The glide flag is on from `glide_tick`; its first tick and the
/// next start from a pre-glide velocity, as in the extractor.
fn glide_ticks(tape: &Tape) -> Vec<usize> {
    (tape.glide_tick + 2..tape.rows.len())
        .filter(|&t| !(tape.rows[t].ground || tape.rows[t - 1].ground || tape.rows[t - 2].ground))
        .collect()
}

/// Per tick: (tick, pose gap, velocity gap), max-abs over axes, for kernel `k` on `T`.
fn gaps<T: Scalar>(
    k: &Generic,
    tape: &Tape,
    ticks: &[usize],
    to: impl Fn(f64) -> T,
) -> Vec<(usize, f64, f64)> {
    ticks
        .iter()
        .map(|&t| {
            let r = &tape.rows;
            let vel = sub(r[t - 1].pos, r[t - 2].pos);
            let s = FlightState {
                pos: r[t - 1].pos.map(&to),
                vel: vel.map(&to),
            };
            let u = Input {
                yaw: to(r[t].yaw as f64),
                pitch: to(r[t].pitch as f64),
            };
            let next = k.step(&s, &u);
            let pose = max_abs(sub(next.pos.map(|x| x.re()), r[t].pos));
            let dv = max_abs(sub(next.vel.map(|x| x.re()), sub(r[t].pos, r[t - 1].pos)));
            (t, pose, dv)
        })
        .collect()
}

fn summary(
    g: &[(usize, f64, f64)],
    which: impl Fn(&(usize, f64, f64)) -> f64,
) -> (f64, usize, f64) {
    let mut v: Vec<f64> = g.iter().map(&which).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let worst = g
        .iter()
        .max_by(|a, b| which(a).partial_cmp(&which(b)).unwrap())
        .unwrap();
    (*v.last().unwrap(), worst.0, v[v.len() / 2])
}

/// Ticks where `LivingEntity.aiStep` zeroes part of the starting velocity.
fn zeroing_ticks(tape: &Tape, ticks: &[usize]) -> Vec<usize> {
    ticks
        .iter()
        .copied()
        .filter(|&t| {
            let v = sub(tape.rows[t - 1].pos, tape.rows[t - 2].pos);
            v[1].abs() < sim::generic::ZERO_Y
                || v[0] * v[0] + v[2] * v[2] < sim::generic::ZERO_XZ_SQR
        })
        .collect()
}

#[test]
fn tape_gap() {
    let tape = load();
    let ticks = glide_ticks(&tape);
    let k = Generic::default();
    let zeroing = zeroing_ticks(&tape, &ticks);
    // The tape's header lists 555-559 as the extractor's mismatches: these.
    assert_eq!(zeroing, vec![555, 556, 557, 558, 559]);

    let g64 = gaps(&k, &tape, &ticks, |x| x);
    let g32 = gaps(&k, &tape, &ticks, |x| x as f32);
    let (m64, at64, med64) = summary(&g64, |g| g.1);
    let (m32, at32, med32) = summary(&g32, |g| g.1);
    let (mv32, atv32, medv32) = summary(&g32, |g| g.2);
    println!(
        "{} contact-free gliding ticks ({}..={}), {} of them zeroing",
        ticks.len(),
        ticks[0],
        ticks.last().unwrap(),
        zeroing.len()
    );
    println!("f64 pose gap: max {m64:.3e} (tick {at64}), median {med64:.3e}");
    println!("f32 pose gap: max {m32:.3e} (tick {at32}), median {med32:.3e}");
    println!("f32 velocity gap: max {mv32:.3e} (tick {atv32}), median {medv32:.3e}");
    let z: Vec<String> = g64
        .iter()
        .filter(|g| zeroing.contains(&g.0))
        .map(|g| format!("{}: {:.1e}", g.0, g.1))
        .collect();
    println!("f64 at the zeroing ticks: {}", z.join(", "));

    // Bounds: ~5x the max observed as of 2026-11-02 21:51 EST (f64 pose 1.94e-5,
    // f32 pose 1.96e-5, f32 velocity 1.94e-5, all at tick 326; medians 4.5e-6, 5.5e-6,
    // 4.5e-6). The gap is mostly the game's sine table in the look vector, so this
    // catches a sign or term error, not a wrong constant in the 8th digit.
    assert!(m64 < 1e-4, "f64 pose gap {m64:e} at tick {at64}");
    assert!(m32 < 1e-4, "f32 pose gap {m32:e} at tick {at32}");
    assert!(mv32 < 1e-4, "f32 velocity gap {mv32:e} at tick {atv32}");

    // Without the zeroing, the zeroing ticks are off by much more.
    let off = Generic {
        zero_small_velocity: false,
        ..k
    };
    let g_off = gaps(&off, &tape, &zeroing, |x| x);
    let min_off = g_off.iter().fold(f64::INFINITY, |m, g| m.min(g.1));
    println!("f64 at the zeroing ticks, zeroing off: min {min_off:.1e}");
    assert!(
        min_off > 10.0 * m64,
        "zeroing off should miss ticks 555-559"
    );
}

#[test]
fn rollout_matches_steps() {
    // `fly` over a stretch is the same as stepping by hand, and stays near the tape.
    let tape = load();
    let k = Generic::default();
    let (a, b) = (100, 200);
    let r = &tape.rows;
    let s0 = FlightState {
        pos: r[a].pos,
        vel: sub(r[a].pos, r[a - 1].pos),
    };
    let inputs: Vec<Input<f64>> = (a + 1..=b)
        .map(|t| Input {
            yaw: r[t].yaw as f64,
            pitch: r[t].pitch as f64,
        })
        .collect();
    let states = k.fly(&s0, &inputs);
    assert_eq!(states.len(), inputs.len() + 1);
    let mut s = s0;
    for (i, u) in inputs.iter().enumerate() {
        s = k.step(&s, u);
        assert_eq!(states[i + 1], s);
    }
    let drift = max_abs(sub(states.last().unwrap().pos, r[b].pos));
    println!("open-loop drift over ticks {a}..{b}: {drift:.3e}");
    assert!(drift < 0.05, "open-loop drift {drift}");
}
