//! Hand-built cases for each way a faithful tick ends normal flight, and for contact,
//! each checked against port's tick of the same lifted state.

mod common;

use common::{Agree, compare_tick, port_step};
use sim::faithful::{Event, Faithful};
use sim::state::{FlightState, Input};
use world::{BlockPos, World};

fn bp(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos { x, y, z }
}

/// Air over `[-8, 8] x [56, 72] x [-8, 8]`, unknown outside, with `blocks` set.
fn world(blocks: &[(BlockPos, &str)]) -> World {
    let mut w = World::air(bp(-8, 56, -8), bp(8, 72, 8));
    for (p, b) in blocks {
        w.set(*p, b);
    }
    w
}

const LEVEL: Input<f32> = Input {
    yaw: 0.0,
    pitch: 0.0,
};

fn fs(pos: [f64; 3], vel: [f64; 3]) -> FlightState<f64> {
    FlightState { pos, vel }
}

/// A faithful tick ending in `want`, matched by port.
fn ends(w: &World, s: FlightState<f64>, u: Input<f32>, want: Event) {
    let (f, a) = compare_tick(w, &s, &u, &format!("{want:?}"));
    assert_eq!(f, Err(want));
    assert_eq!(a, Agree::Left);
}

#[test]
fn landing() {
    let w = world(&[(bp(0, 63, 0), "minecraft:stone")]);
    ends(
        &w,
        fs([0.5, 64.2, 0.5], [0.0, -0.5, 0.3]),
        LEVEL,
        Event::Landing,
    );
    // a top slab is landed on at its top face too
    let w = world(&[(bp(0, 63, 0), "minecraft:oak_slab[type=bottom]")]);
    ends(
        &w,
        fs([0.5, 63.7, 0.5], [0.0, -0.5, 0.0]),
        LEVEL,
        Event::Landing,
    );
}

#[test]
fn climbable_at_the_start_of_a_tick() {
    let w = world(&[(bp(0, 64, 0), "minecraft:ladder[facing=north]")]);
    // the tick that moves into the ladder's cell is still normal flight; the next
    // starts in it
    let s = fs([0.5, 64.5, -0.6], [0.0, 0.0, 0.7]);
    let (f, a) = compare_tick(&w, &s, &LEVEL, "into the ladder");
    assert!(matches!(a, Agree::Flight { .. }));
    let s = f.unwrap().state;
    assert!((0.0..1.0).contains(&s.pos[2]), "{s:?}");
    ends(&w, s, LEVEL, Event::Climbable(bp(0, 64, 0)));
}

/// Vines are in vanilla's `can_glide_through` (World::air uses vanilla's tags), so
/// a glide flies through them. EMC's tags differ: see docs/things-that-bite.md.
#[test]
fn vines_are_flown_through() {
    let w = world(&[(bp(0, 64, 0), "minecraft:vine[north=true]")]);
    let (f, a) = compare_tick(&w, &fs([0.5, 64.5, 0.5], [0.0, 0.0, 0.7]), &LEVEL, "vine");
    assert_eq!(a, Agree::Flight { contact: false });
    assert!(f.is_ok());
}

#[test]
fn fluid_at_start_and_after_the_move() {
    let w = world(&[(bp(0, 64, 0), "minecraft:water")]);
    ends(
        &w,
        fs([0.5, 64.2, 0.5], [0.0, 0.0, 0.5]),
        LEVEL,
        Event::Fluid(bp(0, 64, 0)),
    );
    ends(
        &w,
        fs([0.5, 64.2, -1.0], [0.0, 0.0, 0.8]),
        LEVEL,
        Event::Fluid(bp(0, 64, 0)),
    );
    // a waterlogged slab collides and is a fluid
    let w = world(&[(
        bp(0, 64, 0),
        "minecraft:oak_slab[type=top,waterlogged=true]",
    )]);
    ends(
        &w,
        fs([0.5, 63.5, 0.5], [0.0, 0.5, 0.0]),
        LEVEL,
        Event::Fluid(bp(0, 64, 0)),
    );
}

#[test]
fn stuck() {
    let w = world(&[(bp(0, 64, 0), "minecraft:cobweb")]);
    ends(
        &w,
        fs([0.5, 64.2, -1.0], [0.0, 0.0, 0.8]),
        LEVEL,
        Event::Stuck(bp(0, 64, 0)),
    );
    let w = world(&[(bp(0, 64, 0), "minecraft:powder_snow")]);
    ends(
        &w,
        fs([0.5, 64.2, -1.0], [0.0, 0.0, 0.8]),
        LEVEL,
        Event::Stuck(bp(0, 64, 0)),
    );
}

#[test]
fn bounce() {
    // falling faster than gravity: the restitution bounces
    let w = world(&[(bp(0, 63, 0), "minecraft:slime_block")]);
    ends(
        &w,
        fs([0.5, 64.2, 0.5], [0.0, -0.5, 0.3]),
        LEVEL,
        Event::Bounce(bp(0, 63, 0)),
    );
    // the bed's top is 0.5625; the foot is the supporting block
    let w = world(&[(bp(0, 63, 0), "minecraft:red_bed[facing=north,part=foot]")]);
    ends(
        &w,
        fs([0.5, 63.7, 0.5], [0.0, -0.5, 0.0]),
        LEVEL,
        Event::Bounce(bp(0, 63, 0)),
    );
}

#[test]
fn unknown_space() {
    let w = world(&[]);
    // the known region ends at x = 8; the move's cursor reaches the next cell
    let (f, _) = compare_tick(&w, &fs([7.0, 64.0, 0.5], [1.5, 0.0, 0.0]), &LEVEL, "edge");
    assert!(
        matches!(f, Err(Event::UnknownSpace(p)) if p.x == 9),
        "{f:?}"
    );
}

#[test]
fn wall_contact_with_damage() {
    let w = world(&[
        (bp(1, 64, 0), "minecraft:stone"),
        (bp(1, 65, 0), "minecraft:stone"),
    ]);
    let s = fs([0.2, 64.2, 0.5], [1.5, 0.0, 0.2]);
    let u = Input {
        yaw: -90.0,
        pitch: 10.0,
    }; // facing +x
    let (f, a) = compare_tick(&w, &s, &u, "wall");
    assert_eq!(a, Agree::Flight { contact: true });
    let t = f.unwrap();
    let c = t.contact.unwrap();
    assert_eq!(c.clipped, [true, false, false]);
    assert!(c.horizontal_collision);
    assert_eq!(t.state.vel[0].to_bits(), (-0.0f64).to_bits());
    // against the formula on port's own numbers
    let (after, _) = port_step(&w, &s, &u);
    let (v, h_before) = Faithful.velocity(s.vel, &Faithful.look(&u));
    let h_after = after.delta_movement.horizontal_distance();
    assert!(v[0] > 1.0);
    assert_eq!(c.damage, Some(((h_before - h_after) * 10.0 - 3.0) as f32));
    // the box face rests on the wall
    assert!((t.state.pos[0] + 0.3 - 1.0).abs() < 1e-7);
}

#[test]
fn graze_and_ceiling_without_damage() {
    let w = world(&[
        (bp(1, 64, 0), "minecraft:stone"),
        (bp(0, 65, 0), "minecraft:stone"),
    ]);
    // slow into the wall: a contact, no damage
    let (f, _) = compare_tick(
        &w,
        &fs([0.5, 64.1, 0.5], [0.3, 0.0, 0.1]),
        &Input {
            yaw: -90.0,
            pitch: 0.0,
        },
        "graze",
    );
    let c = f.unwrap().contact.unwrap();
    assert!(c.horizontal_collision && c.damage.is_none());
    // up into the ceiling
    let (f, a) = compare_tick(
        &w,
        &fs([0.5, 64.2, 0.5], [0.0, 0.6, 0.1]),
        &Input {
            yaw: 0.0,
            pitch: -30.0,
        },
        "ceiling",
    );
    assert_eq!(a, Agree::Flight { contact: true });
    let t = f.unwrap();
    let c = t.contact.unwrap();
    assert_eq!(c.clipped, [false, true, false]);
    assert!(!c.horizontal_collision && c.damage.is_none());
    assert_eq!(t.state.vel[1].to_bits(), (-0.0f64).to_bits());
}

/// The lift's conditions on the state alone.
#[test]
fn flight_projection() {
    let s = fs([0.5, 64.0, 0.5], [0.1, 0.0, 0.0]);
    let full = s.lift();
    assert_eq!(full.flight(), Some(s));
    for f in [
        |x: &mut sim::state::FullState| x.on_ground = true,
        |x: &mut sim::state::FullState| x.fall_flying = false,
        |x: &mut sim::state::FullState| x.pose = sim::state::Pose::Standing,
    ] {
        let mut x = full;
        f(&mut x);
        assert_eq!(x.flight(), None);
    }
    // history fields don't matter
    let mut x = full;
    x.sprinting = true;
    x.fall_distance = 3.0;
    assert_eq!(x.flight(), Some(s));
}
