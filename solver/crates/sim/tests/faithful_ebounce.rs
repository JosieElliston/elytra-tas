//! The faithful kernel's perfect-ebounce mode (`Faithful::step_glide`) against port:
//! glides that land on floors of many shapes (full blocks, slabs, stairs, snow,
//! fences, soul sand), slide, step up, and jump off with and without sprint, with
//! random rotations and keys. Every tick is exact on the glide state, or port raises
//! an error and the faithful kernel the matching event.

mod common;

use std::cell::RefCell;

use common::{from_port, keys_to_port, to_port};
use port::{TickInput, Unsupported};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use sim::faithful::{Event, Faithful, GlideTick};
use sim::state::{FlightState, GlideState, Input, Keys};
use world::{BlockPos, World};

/// Floors and things on them.
const FLOOR: &[&str] = &[
    "minecraft:stone",
    "minecraft:stone",
    "minecraft:oak_slab[type=bottom]",
    "minecraft:oak_slab[type=top]",
    "minecraft:oak_stairs[facing=east,half=bottom,shape=straight]",
    "minecraft:oak_stairs[facing=north,half=bottom,shape=inner_left]",
    "minecraft:oak_stairs[facing=south,half=top,shape=straight]",
    "minecraft:snow[layers=3]",
    "minecraft:snow[layers=7]",
    "minecraft:soul_sand",
    "minecraft:oak_fence[east=true,north=true]",
    "minecraft:cobblestone_wall[east=low,north=tall,up=true]",
    "minecraft:oak_fence_gate[facing=north,open=false]",
    "minecraft:bamboo[age=1]",
    "minecraft:hopper",
    "minecraft:oak_trapdoor[facing=south,half=bottom,open=false]",
    "minecraft:honey_block",
];

/// Blocks that end the glide or the tick, put on the floor now and then.
const EVENT: &[&str] = &[
    "minecraft:slime_block",
    "minecraft:red_bed[facing=north,part=foot]",
    "minecraft:cobweb",
    "minecraft:water",
    "minecraft:ladder[facing=north]",
    "minecraft:vine[north=true]",
];

/// The world's center; the known region is 16 blocks around it.
const C: [i32; 3] = [0, 64, 0];

#[derive(Debug, Clone)]
struct Case {
    floor: &'static str,
    /// The floor's height below the start, in blocks.
    depth: i32,
    /// Blocks on the floor (relative x, z, height above the floor), and of what.
    bumps: Vec<([i32; 3], &'static str)>,
    /// A ceiling this high above the floor, if any.
    ceiling: Option<i32>,
    pos: [f64; 3],
    vel: [f64; 3],
    sprinting: bool,
    no_jump_delay: i32,
    prev: Keys,
    ticks: Vec<(Input<f32>, Keys)>,
}

/// Keys with forward and sprint mostly held, and jump held with probability `jump`.
fn keys(jump: f64) -> impl Strategy<Value = Keys> {
    (
        prop::bool::weighted(0.95),
        prop::bool::weighted(0.03),
        prop::bool::weighted(0.1),
        prop::bool::weighted(0.1),
        prop::bool::weighted(jump),
        prop::bool::weighted(0.9),
    )
        .prop_map(|(forward, backward, left, right, jump, sprint)| Keys {
            forward,
            backward,
            left,
            right,
            jump,
            sneak: false,
            sprint,
        })
}

fn pitch() -> impl Strategy<Value = f32> {
    prop_oneof![
        6 => -30.0..=60.0f32,
        2 => -90.0..=90.0f32,
        1 => prop::sample::select(&[-90.0f32, 90.0, 0.0, -0.0][..]),
    ]
}

fn case() -> impl Strategy<Value = Case> {
    let bump = (
        [-3..=3i32, -3..=3i32, 0..=1i32],
        prop_oneof![6 => prop::sample::select(FLOOR), 1 => prop::sample::select(EVENT)],
    )
        .prop_map(|(p, b)| (p, b));
    (
        prop::sample::select(FLOOR),
        1..=3i32,
        prop::collection::vec(bump, 0..6),
        prop_oneof![3 => Just(None), 1 => (2..=4i32).prop_map(Some)],
        ([-1.0..1.0f64, 0.0..1.0f64, -1.0..1.0f64]),
        (
            prop::sample::select(&[0.05f64, 0.3, 0.8, 1.5][..]),
            -1.0..1.0f64,
            -1.0..1.0f64,
            -0.6..0.2f64,
        ),
        (
            any::<bool>(),
            prop_oneof![3 => Just(0), 1 => 0..=10i32],
            keys(0.5),
        ),
        (-1000.0..1000.0f32, prop::bool::weighted(0.5)),
        prop::sample::select(&[0.1, 0.5, 0.95][..])
            .prop_flat_map(|j| prop::collection::vec((-8.0..8.0f32, pitch(), keys(j)), 1..=40)),
    )
        .prop_map(
            |(
                floor,
                depth,
                bumps,
                ceiling,
                p,
                (h, vx, vz, vy),
                (sprinting, no_jump_delay, prev),
                (yaw0, smooth),
                ticks,
            )| {
                let mut yaw = yaw0;
                let ticks = ticks
                    .into_iter()
                    .map(|(dy, pitch, k)| {
                        // a smooth turn, or a random yaw each tick
                        yaw = if smooth { yaw + dy } else { yaw0 + dy * 100.0 };
                        (Input { yaw, pitch }, k)
                    })
                    .collect();
                Case {
                    floor,
                    depth,
                    bumps,
                    ceiling,
                    pos: [
                        C[0] as f64 + p[0],
                        C[1] as f64 + p[1] * depth as f64,
                        C[2] as f64 + p[2],
                    ],
                    vel: [vx * h, vy, vz * h],
                    sprinting,
                    no_jump_delay,
                    prev,
                    ticks,
                }
            },
        )
}

fn build(c: &Case) -> World {
    let r = 16;
    let mut w = World::air(
        BlockPos {
            x: C[0] - r,
            y: C[1] - r,
            z: C[2] - r,
        },
        BlockPos {
            x: C[0] + r,
            y: C[1] + r,
            z: C[2] + r,
        },
    );
    let fy = C[1] - c.depth;
    for x in -6..=6 {
        for z in -6..=6 {
            w.set(
                BlockPos {
                    x: C[0] + x,
                    y: fy,
                    z: C[2] + z,
                },
                c.floor,
            );
            if let Some(h) = c.ceiling {
                w.set(
                    BlockPos {
                        x: C[0] + x,
                        y: fy + 1 + h,
                        z: C[2] + z,
                    },
                    "minecraft:stone",
                );
            }
        }
    }
    for (p, b) in &c.bumps {
        w.set(
            BlockPos {
                x: C[0] + p[0],
                y: fy + 1 + p[2],
                z: C[2] + p[1],
            },
            b,
        );
    }
    w
}

#[derive(Default, Debug)]
struct Stats {
    cases: usize,
    skipped_inside: usize,
    ticks: usize,
    ground_ticks: usize,
    landings: usize,
    /// landings whose move was raised by a step-up
    step_up_landings: usize,
    /// ticks on the ground that stepped up
    ground_step_ups: usize,
    jumps: usize,
    sprint_jumps: usize,
    /// jumps held back by the jump delay
    delayed: usize,
    sprint_stops: usize,
    contacts: usize,
    minor: usize,
    /// `InsideBlock` refusals of a box outside every collision box
    refused_outside: usize,
    /// airborne ticks where `step` on the projected flight state agreed
    projected: usize,
    events: std::collections::BTreeMap<String, usize>,
}

/// The fields of a glide state port's player has, and whether it still glides.
fn glide_of(p: &port::LocalPlayer) -> Option<GlideState> {
    from_port(p).glide()
}

fn check_event(e: &Event, u: &Unsupported, what: &str) {
    let ok = match (e, u) {
        (Event::UnknownSpace(_), Unsupported::UnknownSpace(_)) => true,
        (Event::Climbable(a), Unsupported::Climbable(b))
        | (Event::Fluid(a), Unsupported::Fluid(b))
        | (Event::Stuck(a), Unsupported::Stuck(b))
        | (Event::Bounce(a), Unsupported::Bounce(b)) => a == b,
        _ => false,
    };
    assert!(ok, "{what}: faithful {e:?}, port {u:?}");
}

fn run(c: &Case, stats: &RefCell<Stats>) {
    let w = build(c);
    let mut st = stats.borrow_mut();
    st.cases += 1;
    let mut s = FlightState {
        pos: c.pos,
        vel: c.vel,
    }
    .lift_glide();
    s.sprinting = c.sprinting;
    s.no_jump_delay = c.no_jump_delay;
    s.keys = c.prev;
    let k0 = Faithful.look(&c.ticks[0].0);
    if let Err(Event::InsideBlock(_)) = Faithful.step_glide(&w, &s, &k0, c.ticks[0].1) {
        st.skipped_inside += 1;
        return;
    }
    let mut p = to_port(&s.lift());
    for (i, (u, keys)) in c.ticks.iter().enumerate() {
        let what = format!("tick {i} of {c:?}");
        let f = Faithful.step_glide(&w, &s, &Faithful.look(u), *keys);
        if let Err(Event::InsideBlock(_)) = f {
            // the box sank into a block through the 1e-7 margins, or started there
            let (hw, h) = ((0.6f32 / 2.0) as f64, 0.6f32 as f64);
            let bb = world::Aabb {
                min: [s.pos[0] - hw, s.pos[1], s.pos[2] - hw],
                max: [s.pos[0] + hw, s.pos[1] + h, s.pos[2] + hw],
            };
            let inside = w.collision_boxes(bb).unwrap().iter().any(|b| {
                (0..3).all(|k| b.max[k].min(bb.max[k]) - b.min[k].max(bb.min[k]) > 0.9e-7)
            });
            st.refused_outside += !inside as usize;
            *st.events.entry("InsideBlock".into()).or_default() += 1;
            return;
        }
        let r = port::tick(
            &mut p,
            &TickInput {
                y_rot: u.yaw,
                x_rot: u.pitch,
                keys: keys_to_port(*keys),
            },
            &w,
        );
        st.ticks += 1;
        // projection: off the ground, the glide state's flight state flies normal
        // flight the same, and the landing is `step`'s event
        if let Some(fl) = s.flight() {
            match (Faithful.step(&w, &fl, &Faithful.look(u)), &f) {
                (Ok(n), Ok(t)) if !t.landed => {
                    assert_eq!(
                        (n.state.pos, n.state.vel),
                        (t.state.pos, t.state.vel),
                        "{what}"
                    );
                    st.projected += 1;
                }
                (Err(Event::Landing), Ok(t)) => assert!(t.landed, "{what}"),
                // `step` stops at a landing's clip; the glide goes on to the checks
                // after the move (`Event::Landing`'s doc)
                (
                    Err(Event::Landing),
                    Err(
                        Event::Bounce(_)
                        | Event::Fluid(_)
                        | Event::Stuck(_)
                        | Event::UnknownSpace(_),
                    ),
                ) => {}
                (Err(a), Err(b)) => assert_eq!(a, *b, "{what}"),
                (n, t) => panic!("{what}: step {n:?}, step_glide {t:?}"),
            }
        }
        match (&f, &r) {
            (Ok(t), Ok(_)) => {
                let g =
                    glide_of(&p).unwrap_or_else(|| panic!("{what}: port stopped gliding: {p:?}"));
                let t: &GlideTick = t;
                assert_eq!(
                    (common::bits(t.state.pos), common::bits(t.state.vel)),
                    (common::bits(g.pos), common::bits(g.vel)),
                    "{what}: faithful {:?}, port {g:?}",
                    t.state
                );
                assert_eq!(t.state, g, "{what}");
                assert_eq!(
                    t.contact.is_some_and(|c| c.horizontal_collision),
                    p.horizontal_collision,
                    "{what}"
                );
                if t.landed {
                    st.landings += 1;
                    let dy = t.state.pos[1] - s.pos[1];
                    st.step_up_landings += (dy > 0.0) as usize;
                }
                if s.on_ground && !t.jumped && t.state.pos[1] > s.pos[1] {
                    st.ground_step_ups += 1;
                }
                st.ground_ticks += t.state.on_ground as usize;
                st.jumps += t.jumped as usize;
                st.sprint_jumps += (t.jumped && t.state.sprinting) as usize;
                st.delayed += (s.on_ground && keys.jump && !t.jumped) as usize;
                st.sprint_stops += (s.sprinting && !t.state.sprinting) as usize;
                st.contacts += t.contact.is_some() as usize;
                st.minor += t.state.minor_horizontal_collision as usize;
                s = t.state;
            }
            (Err(e), Err(u)) => {
                check_event(e, u, &what);
                let name = format!("{e:?}");
                *st.events
                    .entry(name.split('(').next().unwrap().to_string())
                    .or_default() += 1;
                return;
            }
            (Err(e), Ok(_)) => panic!("{what}: faithful raised {e:?}, port flew on: {p:?}"),
            (Ok(t), Err(u)) => panic!("{what}: faithful flew to {t:?}, port raised {u:?}"),
        }
        if (0..3).any(|k| (s.pos[k] - C[k] as f64).abs() > 9.0) {
            return;
        }
    }
}

#[test]
fn bounces_match_port() {
    let stats = RefCell::new(Stats::default());
    let mut runner = TestRunner::new(Config {
        cases: 20_000,
        failure_persistence: None,
        ..Config::default()
    });
    runner
        .run(&case(), |c| {
            run(&c, &stats);
            Ok(())
        })
        .unwrap();
    let st = stats.into_inner();
    println!("{st:?}");
    assert!(st.landings > 5_000, "{st:?}");
    assert!(st.jumps > 5_000 && st.sprint_jumps > 1_000, "{st:?}");
    assert!(st.delayed > 500, "{st:?}");
    for kind in ["Bounce", "Stuck", "Fluid", "Climbable"] {
        assert!(
            st.events.get(kind).copied().unwrap_or(0) > 5,
            "few {kind}: {st:?}"
        );
    }
    assert!(
        st.step_up_landings > 50 && st.ground_step_ups > 50,
        "{st:?}"
    );
    assert!(st.sprint_stops > 500 && st.minor > 10, "{st:?}");
    assert!(st.projected > 200_000, "{st:?}");
    assert_eq!(st.refused_outside, 0, "{st:?}");
}

/// A glide onto flat stone: the landing tick ends on the ground, still gliding; with
/// jump held the next tick jumps (0.42 up, the sprint boost along the yaw), and the
/// state after it is normal flight again.
#[test]
fn land_and_jump() {
    let c = Case {
        floor: "minecraft:stone",
        depth: 1,
        bumps: vec![],
        ceiling: None,
        pos: [0.5, 64.3, 0.5],
        vel: [0.4, -0.5, 0.1],
        sprinting: true,
        no_jump_delay: 0,
        prev: Keys::default(),
        ticks: vec![],
    };
    let w = build(&c);
    let mut s = FlightState {
        pos: c.pos,
        vel: c.vel,
    }
    .lift_glide();
    s.sprinting = true;
    let u = Input {
        yaw: -90.0,
        pitch: 20.0,
    };
    let fwd = Keys {
        forward: true,
        sprint: true,
        ..Keys::default()
    };
    let t = Faithful
        .step_glide(&w, &s, &Faithful.look(&u), fwd)
        .unwrap();
    assert!(t.landed && t.state.on_ground && t.state.flight().is_none());
    assert_eq!(t.state.pos[1], 64.0);
    assert_eq!(
        t.state.supporting_block,
        Some(BlockPos { x: 0, y: 63, z: 0 })
    );
    // plain `step` raises the landing
    assert_eq!(
        Faithful.step(
            &w,
            &FlightState {
                pos: c.pos,
                vel: c.vel
            },
            &Faithful.look(&u)
        ),
        Err(Event::Landing)
    );
    let jump = Keys { jump: true, ..fwd };
    let j = Faithful
        .step_glide(&w, &t.state, &Faithful.look(&u), jump)
        .unwrap();
    assert!(j.jumped && !j.state.on_ground && j.state.sprinting);
    assert_eq!(j.state.no_jump_delay, 10);
    let f = j.state.flight().expect("back in normal flight");
    // yaw -90 faces +x: the boost is +0.2 in x before the elytra update
    assert!(f.vel[0] > t.state.vel[0] + 0.15, "{f:?}");
    assert!(f.vel[1] > 0.3, "{f:?}");
    // port agrees
    let mut p = to_port(&t.state.lift());
    port::tick(
        &mut p,
        &TickInput {
            y_rot: u.yaw,
            x_rot: u.pitch,
            keys: keys_to_port(jump),
        },
        &w,
    )
    .unwrap();
    assert_eq!(glide_of(&p), Some(j.state));
}

/// Jump held from before the landing: the delay from the last jump holds the jump
/// back until it runs out, and the glide slides on the ground meanwhile.
#[test]
fn held_jump_waits_for_the_delay() {
    let c = Case {
        floor: "minecraft:stone",
        depth: 1,
        bumps: vec![],
        ceiling: None,
        pos: [0.5, 64.0, 0.5],
        vel: [0.6, 0.0, 0.0],
        sprinting: false,
        no_jump_delay: 0,
        prev: Keys::default(),
        ticks: vec![],
    };
    let w = build(&c);
    let mut s = FlightState {
        pos: c.pos,
        vel: c.vel,
    }
    .lift_glide();
    s.on_ground = true;
    s.no_jump_delay = 4;
    s.keys.jump = true;
    let u = Input {
        yaw: -90.0,
        pitch: 0.0,
    };
    let jump = Keys {
        jump: true,
        ..Keys::default()
    };
    let mut p = to_port(&s.lift());
    let mut jumped_at = None;
    for i in 1..=6 {
        let t = Faithful
            .step_glide(&w, &s, &Faithful.look(&u), jump)
            .unwrap();
        port::tick(
            &mut p,
            &TickInput {
                y_rot: u.yaw,
                x_rot: u.pitch,
                keys: keys_to_port(jump),
            },
            &w,
        )
        .unwrap();
        assert_eq!(glide_of(&p), Some(t.state), "tick {i}");
        if t.jumped {
            jumped_at.get_or_insert(i);
        }
        s = t.state;
    }
    assert_eq!(jumped_at, Some(4));
}
