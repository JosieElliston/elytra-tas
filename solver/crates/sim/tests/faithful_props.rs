//! The faithful kernel against port in random box worlds: walls, ceilings, floors and
//! corners (axis planes of one block type), scattered blocks, random gliding states
//! near them and random inputs. Every tick is exact, or port leaves normal flight and
//! the faithful kernel raises the matching event (`common::agree` says what matches).
//! Starts inside a block are skipped: they are not normal flight.

mod common;

use std::cell::RefCell;
use std::collections::BTreeMap;

use common::{Agree, agree, from_port, tick_input, to_port};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use sim::faithful::{Event, Faithful};
use sim::state::{FlightState, Input};
use world::{Aabb, BlockPos, World};

/// Blocks with collision boxes of many shapes: full, half, stairs, large (fences,
/// walls, gates), thin, offset (bamboo, dripstone), many boxes (cauldron, hopper).
const SOLID: &[&str] = &[
    "minecraft:stone",
    "minecraft:stone",
    "minecraft:oak_slab[type=bottom]",
    "minecraft:oak_slab[type=top]",
    "minecraft:oak_stairs[facing=east,half=bottom,shape=straight]",
    "minecraft:oak_stairs[facing=north,half=top,shape=outer_left]",
    "minecraft:oak_fence[east=true,north=true]",
    "minecraft:oak_fence",
    "minecraft:cobblestone_wall[east=low,north=tall,up=true]",
    "minecraft:oak_fence_gate[facing=north,open=false]",
    "minecraft:glass_pane[east=true,west=true]",
    "minecraft:iron_bars[north=true,south=true]",
    "minecraft:snow[layers=3]",
    "minecraft:oak_trapdoor[facing=south,open=true]",
    "minecraft:end_rod[facing=up]",
    "minecraft:iron_chain[axis=x]",
    "minecraft:bamboo[age=1]",
    "minecraft:pointed_dripstone",
    "minecraft:cauldron",
    "minecraft:hopper",
    "minecraft:anvil",
    "minecraft:soul_sand",
    "minecraft:lantern[hanging=true]",
    "minecraft:vine[north=true]",
];

/// Blocks that end normal flight (or, for the waterlogged slab, collide and are fluid).
const EVENT: &[&str] = &[
    "minecraft:ladder[facing=north]",
    "minecraft:scaffolding",
    "minecraft:water",
    "minecraft:lava",
    "minecraft:oak_slab[type=bottom,waterlogged=true]",
    "minecraft:cobweb",
    "minecraft:sweet_berry_bush[age=2]",
    "minecraft:powder_snow",
    "minecraft:slime_block",
    "minecraft:red_bed[facing=north,part=foot]",
];

/// The world's center; the known region is 16 blocks around it.
const C: [i32; 3] = [0, 64, 0];

#[derive(Debug, Clone)]
enum Prim {
    /// A plane `axis = at` over `[lo, hi]` in the other two axes (cyclic order).
    Plane {
        axis: usize,
        at: i32,
        lo: [i32; 2],
        hi: [i32; 2],
        block: &'static str,
    },
    Block {
        pos: [i32; 3],
        block: &'static str,
    },
}

#[derive(Debug, Clone)]
struct Case {
    prims: Vec<Prim>,
    pos: [f64; 3],
    vel: [f64; 3],
    inputs: Vec<Input<f32>>,
}

fn block(events: bool) -> BoxedStrategy<&'static str> {
    if events {
        prop_oneof![6 => prop::sample::select(SOLID), 1 => prop::sample::select(EVENT)].boxed()
    } else {
        prop::sample::select(SOLID).boxed()
    }
}

fn prim(events: bool) -> impl Strategy<Value = Prim> {
    prop_oneof![
        3 => (0..3usize, -3..=3i32, [-5..=0i32, -5..=0i32], [0..=5i32, 0..=5i32], block(events))
            .prop_map(|(axis, at, lo, hi, block)| Prim::Plane { axis, at, lo, hi, block }),
        2 => ([-4..=4i32, -4..=4i32, -4..=4i32], block(events)).prop_map(|(pos, block)| Prim::Block { pos, block }),
    ]
}

fn pitch() -> impl Strategy<Value = f32> {
    prop_oneof![
        8 => -90.0..=90.0f32,
        1 => prop::sample::select(&[-90.0f32, 90.0, 0.0, -0.0, -45.0, 30.0][..]),
    ]
}

fn input() -> impl Strategy<Value = Input<f32>> {
    (-1000.0..1000.0f32, pitch()).prop_map(|(yaw, pitch)| Input { yaw, pitch })
}

/// Per axis, maybe put a face of the box on a block-grid line (eighths), off by
/// nothing or a hair: where the collision's 1e-7 margins decide.
fn snap() -> impl Strategy<Value = Option<(bool, i32, f64)>> {
    prop_oneof![
        2 => Just(None),
        1 => (any::<bool>(), -24..=24i32, prop::sample::select(&[0.0, 1e-9, -1e-9, 5e-8, -5e-8, 1e-7, -1e-7, 2e-7, -2e-7, 1e-6][..]))
            .prop_map(Some),
    ]
}

fn case(events: bool, ticks: usize) -> impl Strategy<Value = Case> {
    (
        prop::collection::vec(prim(events), 1..6),
        [-3.0..3.0f64, -3.0..3.0f64, -3.0..3.0f64],
        [snap(), snap(), snap()],
        prop::sample::select(&[0.002f64, 0.05, 0.5, 1.5, 3.0][..]),
        [-1.0..1.0f64, -1.0..1.0f64, -1.0..1.0f64],
        prop::collection::vec(input(), 1..=ticks),
    )
        .prop_map(|(prims, mut p, snaps, scale, v, inputs)| {
            let (hw, h) = ((0.6f32 / 2.0) as f64, 0.6f32 as f64);
            for (i, sn) in snaps.iter().enumerate() {
                if let Some((upper, eighths, off)) = *sn {
                    let line = eighths as f64 / 8.0 + off;
                    // the box's lower or upper face on the line
                    let (lo, hi) = if i == 1 { (0.0, h) } else { (-hw, hw) };
                    p[i] = line - if upper { hi } else { lo };
                }
            }
            Case {
                prims,
                pos: [p[0] + C[0] as f64, p[1] + C[1] as f64, p[2] + C[2] as f64],
                vel: v.map(|x| x * scale),
                inputs,
            }
        })
}

fn build(prims: &[Prim]) -> World {
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
    let mut set = |p: [i32; 3], b: &str| {
        w.set(
            BlockPos {
                x: p[0] + C[0],
                y: p[1] + C[1],
                z: p[2] + C[2],
            },
            b,
        );
    };
    for p in prims {
        match p {
            Prim::Plane {
                axis,
                at,
                lo,
                hi,
                block,
            } => {
                let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
                for i in lo[0]..=hi[0] {
                    for j in lo[1]..=hi[1] {
                        let mut q = [0; 3];
                        q[*axis] = *at;
                        q[b] = i;
                        q[c] = j;
                        set(q, block);
                    }
                }
            }
            Prim::Block { pos, block } => set(*pos, block),
        }
    }
    w
}

/// The gliding box overlaps a collision box by about 1e-7 or more on every axis.
fn inside(w: &World, pos: [f64; 3]) -> bool {
    let (hw, h) = ((0.6f32 / 2.0) as f64, 0.6f32 as f64);
    let bb = Aabb {
        min: [pos[0] - hw, pos[1], pos[2] - hw],
        max: [pos[0] + hw, pos[1] + h, pos[2] + hw],
    };
    w.collision_boxes(bb)
        .unwrap()
        .iter()
        .any(|b| (0..3).all(|i| b.max[i].min(bb.max[i]) - b.min[i].max(bb.min[i]) > 0.9e-7))
}

#[derive(Default, Debug)]
struct Stats {
    cases: usize,
    skipped_inside: usize,
    /// starts inside a box that flew (no inner grid line in the way)
    started_inside: usize,
    /// flights refused later, inside a box: from a start inside, or from outside
    /// (through the 1e-7 margins' rounding)
    refused_later: usize,
    sank_from_outside: usize,
    ticks: usize,
    contacts: usize,
    /// contacts on consecutive ticks: sliding along a wall or ceiling
    slides: usize,
    damage: usize,
    events: BTreeMap<String, usize>,
}

/// Fly the case with both, port continuously on its own state, until one leaves
/// normal flight or the flight leaves the blocks.
fn run(c: &Case, stats: &RefCell<Stats>) {
    let w = build(&c.prims);
    let mut st = stats.borrow_mut();
    st.cases += 1;
    let mut s = FlightState {
        pos: c.pos,
        vel: c.vel,
    };
    // a start inside a block is not normal flight
    if let Err(Event::InsideBlock(_)) = Faithful.step_input(&w, &s, &c.inputs[0]) {
        st.skipped_inside += 1;
        return;
    }
    st.started_inside += inside(&w, c.pos) as usize;
    let mut p = to_port(&s.lift());
    let mut last_contact = false;
    for (i, u) in c.inputs.iter().enumerate() {
        if i > 0
            && let Err(Event::InsideBlock(_)) = Faithful.step_input(&w, &s, u)
        {
            assert!(
                inside(&w, s.pos),
                "tick {i}: refused a state outside every box: {c:?}"
            );
            st.refused_later += 1;
            st.sank_from_outside += !inside(&w, c.pos) as usize;
            return;
        }
        // past the blocks there is nothing to hit, and the known region ends
        if (0..3).any(|k| (s.pos[k] - C[k] as f64).abs() > 9.0) {
            return;
        }
        let f = Faithful.step_input(&w, &s, u);
        let r = port::tick(&mut p, &tick_input(u), &w).map(|_| ());
        st.ticks += 1;
        match agree(&f, &p, &r, &format!("tick {i} of {c:?}")) {
            Agree::Flight { contact } => {
                let t = f.unwrap();
                st.contacts += contact as usize;
                st.slides += (contact && last_contact) as usize;
                last_contact = contact;
                st.damage += t.contact.is_some_and(|c| c.damage.is_some()) as usize;
                s = t.state;
                assert_eq!(from_port(&p).flight().map(|f| f.pos), Some(s.pos));
            }
            Agree::Left => {
                let e = f.unwrap_err();
                let name = format!("{e:?}");
                *st.events
                    .entry(name.split('(').next().unwrap().to_string())
                    .or_default() += 1;
                return;
            }
        }
    }
}

fn check(events: bool, ticks: usize, cases: u32) -> Stats {
    let stats = RefCell::new(Stats::default());
    let mut runner = TestRunner::new(Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    });
    runner
        .run(&case(events, ticks), |c| {
            run(&c, &stats);
            Ok(())
        })
        .unwrap();
    let st = stats.into_inner();
    println!("{st:?}");
    st
}

#[test]
fn single_ticks() {
    let st = check(true, 1, 50_000);
    assert!(st.contacts > 1500, "{st:?}");
    for kind in ["Landing", "Fluid", "Climbable", "Stuck"] {
        assert!(
            st.events.get(kind).copied().unwrap_or(0) > 10,
            "few {kind}: {st:?}"
        );
    }
}

#[test]
fn rollouts() {
    let st = check(false, 40, 10_000);
    assert!(st.contacts > 1500 && st.slides > 300, "{st:?}");
    // possible only through the 1e-7 margins' rounding, with an inner grid line
    assert_eq!(st.sank_from_outside, 0, "{st:?}");
}

#[test]
fn rollouts_with_events() {
    let st = check(true, 40, 10_000);
    assert!(st.events.get("Bounce").copied().unwrap_or(0) > 0, "{st:?}");
}

/// A start inside a block is refused.
#[test]
fn inside_block_is_refused() {
    let mut w = World::air(
        BlockPos {
            x: -8,
            y: 56,
            z: -8,
        },
        BlockPos { x: 8, y: 72, z: 8 },
    );
    w.set(BlockPos { x: 0, y: 64, z: 0 }, "minecraft:stone");
    let s = FlightState {
        pos: [0.5, 64.2, 0.5],
        vel: [0.3, -0.1, 0.0],
    };
    let e = Faithful.step_input(
        &w,
        &s,
        &Input {
            yaw: 0.0,
            pitch: 10.0,
        },
    );
    assert_eq!(e, Err(Event::InsideBlock(BlockPos { x: 0, y: 64, z: 0 })));
}
