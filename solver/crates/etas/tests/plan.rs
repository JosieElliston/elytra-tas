//! Plan files (`PLAN-FORMAT.md`, version 1): round trips, a plain tape as a plan, and
//! the lines a reader must refuse; `etas plan` end to end.

mod common;

use etas::plan::{Action, DIVERGE_TOL, Event, FORMAT, Plan, React};
use etas::tape::Tape;

fn small_tape() -> Tape {
    let (_, _, tape) = common::load();
    let mut t = tape.clone();
    t.rows.truncate(40);
    t.set("ticks", "40".into());
    t
}

fn with_header(tape: &Tape) -> Tape {
    let mut t = tape.clone();
    let mut h = vec![
        ("format".to_string(), FORMAT.to_string()),
        ("account".to_string(), "deftenvy".to_string()),
        ("diverge_tol".to_string(), "2e-4".to_string()),
    ];
    h.append(&mut t.header);
    t.header = h;
    t
}

#[test]
fn round_trip() {
    let plan = Plan {
        tape: with_header(&small_tape()),
        reacts: vec![
            React {
                t0: 5,
                t1: 9,
                event: Event::GlideOff,
                action: Action::PressJump,
            },
            React {
                t0: 20,
                t1: 20,
                event: Event::GlideOff,
                action: Action::PressJump,
            },
        ],
    };
    let text = plan.write();
    // each directive is the line before its window's first tick
    let lines: Vec<&str> = text.lines().collect();
    let i = lines
        .iter()
        .position(|l| *l == "@react 5 9 glide_off press_jump")
        .unwrap();
    assert!(lines[i + 1].starts_with("5 "), "{}", lines[i + 1]);
    assert!(
        text.lines()
            .next()
            .unwrap()
            .starts_with("format=etas-plan 1")
    );
    let back = Plan::parse(&text).unwrap();
    assert_eq!(back, plan);
    assert_eq!(back.diverge_tol(), 2e-4);
    // and again: writing is stable
    assert_eq!(back.write(), text);
}

/// A plain `.tas` tape reads as a plan with no reactions, and writes back as itself.
#[test]
fn plain_tape_is_a_plan() {
    let text = std::fs::read_to_string(common::tape_path()).unwrap();
    let plan = Plan::parse(&text).unwrap();
    assert!(plan.reacts.is_empty());
    assert_eq!(plan.tape, Tape::parse(&text).unwrap());
    assert_eq!(plan.diverge_tol(), DIVERGE_TOL);
    assert_eq!(plan.write(), plan.tape.write());
}

#[test]
fn refuses_what_it_does_not_know() {
    let good = Plan {
        tape: with_header(&small_tape()),
        reacts: vec![],
    }
    .write();
    let at_first_tick = |extra: &str| good.replacen("\n1 ", &format!("\n{extra}\n1 "), 1);
    let cases = [
        (at_first_tick("@segment 1 2"), "unknown directive"),
        (
            at_first_tick("@react 1 2 booster press_jump"),
            "unknown event",
        ),
        (
            at_first_tick("@react 1 2 glide_off press_use"),
            "unknown action",
        ),
        (
            at_first_tick("@react 3 2 glide_off press_jump"),
            "not within",
        ),
        (
            at_first_tick("@react 1 41 glide_off press_jump"),
            "not within",
        ),
        (at_first_tick("@react 1 2 glide_off"), "fields"),
        (
            good.replace("format=etas-plan 1", "format=etas-plan 2"),
            "reader knows",
        ),
        (
            at_first_tick("@react 1 2 glide_off press_jump").replace("format=etas-plan 1\n", ""),
            "without format",
        ),
        (
            good.replace("diverge_tol=2e-4", "diverge_tol=x"),
            "diverge_tol",
        ),
    ];
    for (text, want) in cases {
        let e = Plan::parse(&text).unwrap_err();
        assert!(e.contains(want), "{want}: {e}");
    }
    // a directive anywhere after the header is fine, comments too
    let moved = good.replacen(
        "\n30 ",
        "\n@react 2 4 glide_off press_jump    # late in the file\n30 ",
        1,
    );
    let p = Plan::parse(&moved).unwrap();
    assert_eq!(p.reacts.len(), 1);
    assert_eq!((p.reacts[0].t0, p.reacts[0].t1), (2, 4));
}

/// `etas plan` on a trajectory: writes the plan with its reactions and verifies it.
#[test]
fn etas_plan_cli() {
    let (c, w) = etas::course::Course::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ebounce"),
    )
    .unwrap();
    let tape = Tape::read(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/ebounce/ebounce_run1.tas"),
    )
    .unwrap();
    let (mut traj, _) = etas::trajectory::seed_perfect_bounces(&tape, &w, Some(&c), "test");
    traj.rows.truncate(150);
    let dir = std::env::temp_dir().join(format!("etas-plan-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tp = dir.join("seed.traj.jsonl");
    let out = dir.join("seed.tas");
    traj.write(&tp).unwrap();
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_etas"))
        .args(["plan", tp.to_str().unwrap(), "--course"])
        .arg(&c.dir)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&o.stdout);
    println!("{stdout}");
    // the timer never starts in these 150 ticks, so verify fails on that alone
    assert_eq!(
        o.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(stdout.contains("reactions"), "{stdout}");
    assert!(!stdout.contains("no @react"), "{stdout}");
    let p = Plan::read(&out).unwrap();
    assert!(!p.reacts.is_empty());
    assert_eq!(p.tape.get("account"), Some("deftenvy"));
    assert_eq!(p.tape.get("course"), Some(c.name.as_str()));
    assert_eq!(
        p.tape.get("world_sha256"),
        Some(c.world.content_sha256.as_str())
    );
    let _ = std::fs::remove_dir_all(&dir);
}
