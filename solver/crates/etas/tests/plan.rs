//! Plan files (`PLAN-FORMAT.md`): version-1 round trips, a plain tape as a plan, and
//! the lines a reader must refuse; version-2 segments and branches, their checks and
//! path enumeration; `etas plan` end to end.

mod common;

use etas::plan::{
    Action, Branch, DIVERGE_TOL, Event, FORMAT, FORMAT_1, Ignore, Leaf, Links, Plan, React, Segment,
};
use etas::server::{Arrival, ServerEvent};
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
        ("format".to_string(), FORMAT_1.to_string()),
        ("account".to_string(), "deftenvy".to_string()),
        ("diverge_tol".to_string(), "2e-4".to_string()),
    ];
    h.append(&mut t.header);
    t.header = h;
    t
}

#[test]
fn round_trip() {
    let mut plan = Plan::from_tape(with_header(&small_tape()));
    plan.reacts = vec![
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
    ];
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
    let good = Plan::from_tape(with_header(&small_tape())).write();
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
            good.replace("format=etas-plan 1", "format=etas-plan 3"),
            "reader knows",
        ),
        // version 1 has @react only
        (at_first_tick("@branch 2 glide_off x"), "unknown directive"),
        (good.replace("ticks=40", "ticks=41"), "ticks=41"),
        (at_first_tick("@server 1 glide_off"), "unknown directive"),
        (
            at_first_tick("@react 17 17 glide_off press_jump"),
            "holds J",
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

fn vel(x: f64, y: f64, z: f64) -> ServerEvent {
    ServerEvent::SetVelocity { v: [x, y, z] }
}

fn seg(name: &str, tape: &Tape, from: usize, to: usize, links: Links) -> Segment {
    Segment {
        name: name.into(),
        start: from,
        rows: tape.rows[from - 1..to].to_vec(),
        reacts: vec![],
        links,
    }
}

fn branch(tick: usize, event: ServerEvent, child: &str) -> Branch {
    Branch {
        tick,
        event,
        child: child.into(),
    }
}

/// Root 1..20 branching at 15 on two velocities and at 16 on one; A and B rejoin in J
/// at 26; C waits for a glide-off at 19, which D takes; J has a reaction and an ignore.
/// The rows are the recorded tape's: these tests are about structure, not physics.
fn branched() -> Plan {
    let tape = with_header(&small_tape());
    let mut plan = Plan::from_tape(Tape {
        rows: tape.rows[..20].to_vec(),
        ..tape.clone()
    });
    plan.tape.set("velocity_tol", "1e-6".into());
    plan.reacts = vec![React {
        t0: 5,
        t1: 9,
        event: Event::GlideOff,
        action: Action::PressJump,
    }];
    plan.links.branches = vec![
        branch(15, vel(0.0, 0.6, 0.0), "A"),
        branch(15, vel(0.0, 0.4, 0.0), "B"),
        branch(16, vel(0.0, 0.6, 0.0), "C"),
    ];
    let goto_j = || Links {
        goto: Some("J".into()),
        ..Links::default()
    };
    let mut j = seg("J", &tape, 26, 40, Links::default());
    j.reacts.push(React {
        t0: 30,
        t1: 33,
        event: Event::GlideOff,
        action: Action::PressJump,
    });
    j.links.ignores.push(Ignore {
        t0: 27,
        t1: 41,
        event: Event::SetVelocity,
    });
    plan.segments = vec![
        seg("A", &tape, 15, 25, goto_j()),
        seg("B", &tape, 15, 25, goto_j()),
        j,
        seg(
            "C",
            &tape,
            16,
            18,
            Links {
                branches: vec![branch(19, ServerEvent::GlideOff, "D")],
                ..Links::default()
            },
        ),
        seg("D", &tape, 19, 40, Links::default()),
    ];
    plan
}

#[test]
fn branched_round_trip() {
    let plan = branched();
    let text = plan.write();
    println!(
        "{}",
        text.lines()
            .filter(|l| l.starts_with('@') || l.contains('='))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(text.starts_with(&format!("format={FORMAT}\n")), "{text}");
    // every tick line counts
    assert!(text.contains(&format!("\nticks={}\n", 20 + 11 + 11 + 15 + 3 + 22)));
    let back = Plan::parse(&text).unwrap();
    let mut want = plan.clone();
    want.tape.set("format", FORMAT.into());
    want.tape.set("ticks", want.tick_lines().to_string());
    assert_eq!(back, want);
    assert_eq!(back.write(), text);
    assert_eq!(back.velocity_tol(), Some(1e-6));
    // the branch is the line before the tick line it names
    let lines: Vec<&str> = text.lines().collect();
    let i = lines
        .iter()
        .position(|l| l.starts_with("@branch 16 set_velocity"))
        .unwrap();
    assert!(lines[i + 1].starts_with("16 "), "{}", lines[i + 1]);
    // a branchless plan is still written as version 1
    let flat = Plan::from_tape(with_header(&small_tape()));
    assert!(flat.write().starts_with(&format!("format={FORMAT_1}\n")));
}

#[test]
fn paths_through_the_dag() {
    let plan = branched();
    let paths = plan.paths();
    assert_eq!(plan.path_count(), paths.len() as u128);
    let names: Vec<String> = paths.iter().map(|p| p.segments.join(">")).collect();
    assert_eq!(
        names,
        ["root>A>J", "root>B>J", "root>C>D", "root>C", "root"]
    );
    let leaves: Vec<Leaf> = paths.iter().map(|p| p.leaf).collect();
    assert_eq!(
        leaves,
        [
            Leaf::End,
            Leaf::End,
            Leaf::End,
            Leaf::Missing { tick: 19 },
            Leaf::End
        ]
    );
    let tape = with_header(&small_tape());
    for p in &paths {
        // ticks 1.. without a gap, each line from the segment the path was in
        assert!(p.tape.rows.iter().enumerate().all(|(i, r)| r.t == i + 1));
        assert_eq!(
            p.tape.get("ticks"),
            Some(p.tape.rows.len().to_string().as_str())
        );
        assert!(p.tape.rows.iter().all(|r| *r == tape.rows[r.t - 1]));
    }
    assert_eq!(paths[0].tape.rows.len(), 40);
    assert_eq!(
        paths[0].arrivals,
        [Arrival {
            tick: 15,
            event: vel(0.0, 0.6, 0.0)
        }]
    );
    assert_eq!(paths[1].arrivals[0].event, vel(0.0, 0.4, 0.0));
    assert_eq!(
        paths[2].arrivals,
        [
            Arrival {
                tick: 16,
                event: vel(0.0, 0.6, 0.0)
            },
            Arrival {
                tick: 19,
                event: ServerEvent::GlideOff
            }
        ]
    );
    assert_eq!(paths[3].tape.rows.len(), 18);
    assert_eq!(paths[4].tape.rows.len(), 20);
    assert!(paths[4].arrivals.is_empty());
    // the root's reaction on every path, J's only where J is flown
    assert_eq!(paths[0].reacts.len(), 2);
    assert_eq!(paths[2].reacts.len(), 1);
    // as a branchless plan for verify
    let flat = paths[0].plan();
    assert!(!flat.branches());
    Plan::parse(&flat.write()).unwrap();
}

/// A reaction window is cut where a path leaves its segment.
#[test]
fn reaction_cut_at_the_branch() {
    let mut plan = branched();
    plan.reacts[0] = React {
        t0: 10,
        t1: 18,
        event: Event::GlideOff,
        action: Action::PressJump,
    };
    // 16 holds J, so 17 can't be in a window: make the root's 16 a plain W
    plan.tape.rows[15].keys = "WR".into();
    let text = plan.write();
    let p = Plan::parse(&text).unwrap();
    let paths = p.paths();
    assert_eq!((paths[0].reacts[0].t0, paths[0].reacts[0].t1), (10, 14));
    assert_eq!((paths[2].reacts[0].t0, paths[2].reacts[0].t1), (10, 15));
    assert_eq!((paths[4].reacts[0].t0, paths[4].reacts[0].t1), (10, 18));
}

#[test]
fn branched_refusals() {
    let good = branched().write();
    let p = Plan::parse(&good).unwrap();
    assert_eq!(p.segments.len(), 5);
    let swap = |from: &str, to: &str| {
        assert!(good.contains(from), "{from}");
        good.replacen(from, to, 1)
    };
    let cases = [
        (swap("@segment D 19", "@segment D 20"), "tick 19, want 20"),
        (swap("@goto J\n", "@goto Q\n"), "no such segment"),
        (swap("glide_off D", "glide_off Q"), "no segment Q"),
        (swap("@segment J 26", "@segment J 27"), "tick 26, want 27"),
        (swap("velocity_tol=1e-6\n", ""), "no velocity_tol"),
        (
            swap("velocity_tol=1e-6", "velocity_tol=-1"),
            "not a tolerance",
        ),
        (swap("@segment A 15", "@segment root 15"), "not root"),
        (swap("@segment B 15", "@segment A 15"), "defined twice"),
        (
            swap("@segment D 19\n", "@segment D 19\nx=1\n"),
            "header line inside",
        ),
        (
            swap("@segment D 19\n", "@segment D 19\n@server 19 glide_off\n"),
            "unknown directive @server",
        ),
        (
            swap("@segment D 19\n", "@segment D 19\n@frob\n"),
            "unknown directive @frob",
        ),
        (
            swap("@goto J\n", "@goto J\n@goto J\n"),
            "more than one @goto",
        ),
        (
            swap(
                "@segment D 19\n",
                "@segment D 19\n@react 19 20 glide_off press_jump\n",
            ),
            "window not within",
        ),
        (
            swap("@segment D 19\n", "@segment D 19\n@ignore 3 4 glide_off\n"),
            "not within",
        ),
        (
            swap(
                "@segment D 19\n",
                "@segment D 19\n@react 24 27 glide_off press_jump\n@branch 26 glide_off J\n",
            ),
            "inside @react",
        ),
        (good.clone() + "@segment E 30\n", "not reachable"),
    ];
    for (text, want) in cases {
        let e = Plan::parse(&text).unwrap_err();
        assert!(e.contains(want), "{want}: {e}");
    }
    // ambiguous payloads: within 2 * velocity_tol of each other
    let mut amb = branched();
    amb.links.branches[1].event = vel(0.0, 0.6 + 1.5e-6, 0.0);
    let e = Plan::parse(&amb.write()).unwrap_err();
    assert!(e.contains("would match both"), "{e}");
    amb.links.branches[1].event = vel(0.0, 0.6 + 3e-6, 0.0);
    Plan::parse(&amb.write()).unwrap();
    // a same-tick cycle, and one observation taken twice along a same-tick chain
    let mut cyc = branched();
    cyc.segments.push(seg(
        "E",
        &with_header(&small_tape()),
        19,
        18,
        Links::default(),
    ));
    cyc.segments[3].links.goto = Some("E".into());
    cyc.segments[3].links.branches.clear();
    let n = cyc.segments.len();
    cyc.segments[n - 1].links.goto = Some("F".into());
    cyc.segments.push(Segment {
        name: "F".into(),
        start: 19,
        rows: vec![],
        reacts: vec![],
        links: Links {
            goto: Some("E".into()),
            ..Links::default()
        },
    });
    let e = Plan::parse(&cyc.write()).unwrap_err();
    assert!(e.contains("reachable from itself"), "{e}");
    let mut twice = branched();
    // C at 16 -> X at 16 (empty) -> branch on set_velocity at 16 again
    twice.links.branches[2].child = "X".into();
    twice.segments.push(Segment {
        name: "X".into(),
        start: 16,
        rows: vec![],
        reacts: vec![],
        links: Links {
            branches: vec![branch(16, vel(1.0, 0.0, 0.0), "C")],
            ..Links::default()
        },
    });
    let e = Plan::parse(&twice.write()).unwrap_err();
    assert!(e.contains("can never match"), "{e}");
    // a different event at the same tick is fine: two observations before one line
    twice.segments.last_mut().unwrap().links.branches[0] = branch(16, ServerEvent::GlideOff, "C");
    let p = Plan::parse(&twice.write()).unwrap();
    let paths = p.paths();
    assert!(
        paths
            .iter()
            .any(|q| q.arrivals.len() == 3 && q.arrivals[0].tick == 16 && q.arrivals[1].tick == 16)
    );
}

/// The rows port flies from `tape`'s start with its inputs, under `arrivals`, for
/// ticks `from..=to`.
fn fly(
    tape: &Tape,
    w: &world::World,
    arrivals: &[Arrival],
    from: usize,
    to: usize,
) -> Vec<etas::tape::TapeRow> {
    let mut p = etas::course::rest_state(tape.start_pos, tape.start_yaw, tape.start_pitch);
    let mut out = Vec::new();
    for r in &tape.rows[..to] {
        etas::server::apply_at(&mut p, arrivals, r.t, w);
        let input = port::TickInput {
            y_rot: r.yaw,
            x_rot: r.pitch,
            keys: etas::state::keys_of(&r.keys).unwrap(),
        };
        port::tick(&mut p, &input, w).unwrap();
        if r.t >= from {
            out.push(etas::tape::TapeRow {
                pos: [p.position.x, p.position.y, p.position.z],
                ground: p.on_ground,
                ..r.clone()
            });
        }
    }
    out
}

fn velocity_before(tape: &Tape, w: &world::World, t: usize) -> [f64; 3] {
    let mut p = etas::course::rest_state(tape.start_pos, tape.start_yaw, tape.start_pitch);
    for r in &tape.rows[..t - 1] {
        let input = port::TickInput {
            y_rot: r.yaw,
            x_rot: r.pitch,
            keys: etas::state::keys_of(&r.keys).unwrap(),
        };
        port::tick(&mut p, &input, w).unwrap();
    }
    let v = p.delta_movement;
    [v.x, v.y, v.z]
}

/// The recorded Burrowed Grove run as the root, with two children flown by port under
/// a velocity 1e-4 larger than port's own at tick 300 or 301: every path's recorded
/// states reproduce under its arrivals, and the plan survives a write and read.
#[test]
fn branched_plan_verifies() {
    let (c, w, tape) = common::load();
    let mut plan = Plan::from_tape(with_header(&tape));
    plan.tape.set("velocity_tol", "1e-9".into());
    let n = tape.rows.len();
    for (name, t) in [("A", 300), ("B", 301)] {
        let v = velocity_before(&tape, &w, t).map(|x| x * (1.0 + 1e-4));
        let a = [Arrival {
            tick: t,
            event: vel(v[0], v[1], v[2]),
        }];
        plan.links.branches.push(branch(t, a[0].event, name));
        plan.segments.push(Segment {
            name: name.into(),
            start: t,
            rows: fly(&tape, &w, &a, t, n),
            reacts: vec![],
            links: Links::default(),
        });
    }
    let plan = Plan::parse(&plan.write()).unwrap();
    let results = etas::plan::verify_paths(&c, &w, &plan);
    assert_eq!(results.len(), 3);
    for (p, r) in &results {
        println!(
            "{}: {} arrivals, ok {}, finish {:?}, failures {:?}",
            p.segments.join(">"),
            p.arrivals.len(),
            r.ok(),
            r.finish_tick(),
            r.failures
        );
        assert!(
            r.mismatch.is_none() && r.stopped.is_none(),
            "{:?} {:?}",
            r.mismatch,
            r.stopped
        );
        assert_eq!(r.arrivals, p.arrivals);
    }
    // the same child under the wrong arrival tick doesn't reproduce
    let mut wrong = plan.paths().remove(0);
    wrong.arrivals[0].tick += 1;
    let r = etas::verify::verify_path(
        &c,
        &w,
        etas::verify::Plan::Reactive(&wrong.plan()),
        &wrong.arrivals,
    );
    assert!(r.mismatch.is_some());
}

/// The plans the playback gametest wrote (`oracle/README.md`, "Playback tests"), if it
/// has run: the Java writer's version-2 files read here, with the paths the test flies.
#[test]
fn gametest_plans_parse() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../oracle/build/playback");
    for (name, want) in [
        (
            "branches.tas",
            &["root>A>J", "root>B>J", "root>C>C2>J", "root>C", "root"][..],
        ),
        ("glide-branches.tas", &["root>G", "root>G2", "root"][..]),
    ] {
        let Ok(p) = Plan::read(&dir.join(name)) else {
            println!("{name}: not there (the gametest hasn't run)");
            continue;
        };
        let names: Vec<String> = p.paths().iter().map(|q| q.segments.join(">")).collect();
        assert_eq!(names, want, "{name}");
        // and it writes back as a plan that reads the same
        assert_eq!(Plan::parse(&p.write()).unwrap().paths(), p.paths());
    }
}
