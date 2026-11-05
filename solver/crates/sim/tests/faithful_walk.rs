//! The faithful kernel's full mode (`Faithful::step_full`) against port: the two
//! recorded runs with server events (`etas`'s `tests/data/ebounce_events` and
//! `tests/data/alpine`: glide-offs, lost glides, walking starts, sneak, glide starts,
//! booster velocities), tick by tick from port's state and flown on its own; and
//! random walking, jumping, sneaking and sprinting over box worlds.

mod common;

use common::{from_port, keys_from_port, keys_to_port, to_port};
use port::{LocalPlayer, TickInput, Unsupported, Vec3};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use sim::faithful::{Event, Faithful, FullTick};
use sim::state::{FullState, Input, Keys, Pose};
use world::{BlockPos, World};

/// A recorded tape with its `@server` lines (`extract_run.py`'s docstring).
struct Record {
    start: LocalPlayer,
    /// `rows[0]` is tick 1: rotation, keys, and the arrivals handled before it.
    rows: Vec<(f32, f32, port::Input, Vec<Arrival>)>,
}

#[derive(Clone, Copy, Debug)]
enum Arrival {
    GlideOff,
    Velocity([f64; 3]),
}

fn hex32(s: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(s, 16).unwrap())
}

fn hex64(s: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(s, 16).unwrap())
}

fn record(name: &str, file: &str) -> (Record, World) {
    let dir = common::repo_root()
        .join("solver/crates/etas/tests/data")
        .join(name);
    let text = std::fs::read_to_string(dir.join(file)).unwrap();
    let world = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.to_string_lossy().ends_with(".world.gz"))
        .unwrap();
    let world = World::load(&world).unwrap();
    let (mut start, mut flags) = (None, 0u8);
    let mut rows = Vec::new();
    let mut pending = Vec::new();
    for line in text.lines() {
        let data = line.split('#').next().unwrap().trim();
        if data.is_empty() {
            continue;
        }
        if let Some(rest) = data.strip_prefix("@server ") {
            let f: Vec<&str> = rest.split_whitespace().collect();
            assert_eq!(f[0].parse::<usize>().unwrap(), rows.len() + 1);
            pending.push(match f[1] {
                "glide_off" => Arrival::GlideOff,
                _ => Arrival::Velocity([hex64(f[2]), hex64(f[3]), hex64(f[4])]),
            });
            continue;
        }
        if let Some(v) = data.strip_prefix("start=") {
            let s: Vec<&str> = v.split_whitespace().collect();
            start = Some((
                Vec3::new(hex64(s[0]), hex64(s[1]), hex64(s[2])),
                hex32(s[3]),
                hex32(s[4]),
            ));
            continue;
        }
        if let Some(v) = data.strip_prefix("start_flags=") {
            flags = u8::from_str_radix(v, 16).unwrap();
            continue;
        }
        if data.contains('=') {
            continue;
        }
        let f: Vec<&str> = data.split_whitespace().collect();
        let mut keys = port::Input::from_tape(f[3]);
        keys.shift = f[3].contains('C');
        rows.push((hex32(f[1]), hex32(f[2]), keys, std::mem::take(&mut pending)));
    }
    let (pos, yaw, pitch) = start.unwrap();
    // `etas::course::rest_state` with `start_flags=` (`etas::tape::Tape::start_player`)
    let mut p = LocalPlayer::new(pos, yaw, pitch);
    p.delta_movement = Vec3::new(0.0, (0.0 - 0.08) * 0.98f32 as f64, 0.0);
    if flags & 0x80 != 0 {
        p.fall_flying = true;
        p.pose = port::Pose::FallFlying;
    }
    if flags & 0x08 != 0 {
        p.set_sprinting(true);
    }
    (Record { start: p, rows }, world)
}

fn port_pose(p: Pose) -> port::Pose {
    to_port(&FullState {
        pose: p,
        ..from_port(&LocalPlayer::new(Vec3::ZERO, 0.0, 0.0))
    })
    .pose
}

/// Apply a tick's arrivals to both: the same glide-off pose (the kernel's
/// `server_pose`) and velocity.
fn apply(world: &World, p: &mut LocalPlayer, f: &mut FullState, arrivals: &[Arrival]) {
    for a in arrivals {
        match *a {
            Arrival::GlideOff => {
                let pose = Faithful.server_pose(world, f).unwrap();
                f.apply_glide_off(pose);
                p.handle_glide_off(port_pose(pose));
            }
            Arrival::Velocity(v) => {
                f.apply_velocity(v);
                p.handle_set_entity_motion(Vec3::new(v[0], v[1], v[2]));
            }
        }
    }
}

#[derive(Default, Debug)]
struct Counts {
    ticks: usize,
    gliding: usize,
    without_glide_after_deploy: usize,
    on_ground: usize,
    glide_starts: usize,
    glide_offs: usize,
    velocities: usize,
}

fn fly_record(name: &str, file: &str) -> Counts {
    let (rec, world) = record(name, file);
    let mut p = rec.start.clone();
    let mut alone = from_port(&p);
    let mut n = Counts::default();
    let mut deployed = false;
    for (i, (yaw, pitch, keys, arrivals)) in rec.rows.iter().enumerate() {
        let t = i + 1;
        let mut f = from_port(&p);
        apply(&world, &mut p, &mut f, arrivals);
        // the kernel flown on its own gets the same arrivals
        for a in arrivals {
            match *a {
                Arrival::GlideOff => {
                    let pose = Faithful.server_pose(&world, &alone).unwrap();
                    alone.apply_glide_off(pose);
                    n.glide_offs += 1;
                }
                Arrival::Velocity(v) => {
                    alone.apply_velocity(v);
                    n.velocities += 1;
                }
            }
        }
        let u = Input {
            yaw: *yaw,
            pitch: *pitch,
        };
        let tick: FullTick = Faithful
            .step_full(&world, &f, &u, keys_from_port(*keys))
            .unwrap_or_else(|e| panic!("{name} tick {t}: {e:?}"));
        port::tick(
            &mut p,
            &TickInput {
                y_rot: *yaw,
                x_rot: *pitch,
                keys: *keys,
            },
            &world,
        )
        .unwrap();
        let want = from_port(&p);
        assert_eq!(tick.state, want, "{name} tick {t}: faithful vs port");
        let a = Faithful
            .step_full(&world, &alone, &u, keys_from_port(*keys))
            .unwrap();
        assert_eq!(a.state, want, "{name} tick {t}: faithful flown alone");
        alone = a.state;
        n.ticks += 1;
        deployed |= want.fall_flying;
        n.gliding += want.fall_flying as usize;
        n.without_glide_after_deploy += (deployed && !want.fall_flying) as usize;
        n.on_ground += want.on_ground as usize;
        n.glide_starts += tick.glide_started as usize;
    }
    n
}

/// Every tick of both records, from port's state and flown alone, is port's tick
/// exactly, full state included.
#[test]
fn records_fly_exactly() {
    let e = fly_record("ebounce_events", "ebounce_run1.tas");
    println!("ebounce run 1: {e:?}");
    assert_eq!(e.ticks, 295);
    assert_eq!(e.glide_offs, 4);
    assert_eq!(e.glide_starts, 5);
    assert_eq!(e.without_glide_after_deploy, 6, "{e:?}");
    let a = fly_record("alpine", "alpine_run0.tas");
    println!("alpine run 0: {a:?}");
    assert_eq!(a.velocities, 8);
    assert!(a.on_ground > 1000, "{a:?}");
}

/// What a random tick holds.
fn keys_strategy() -> impl Strategy<Value = Keys> {
    (
        prop::bool::weighted(0.6),
        prop::bool::weighted(0.1),
        prop::bool::weighted(0.2),
        prop::bool::weighted(0.2),
        prop::bool::weighted(0.3),
        prop::bool::weighted(0.15),
        prop::bool::weighted(0.5),
    )
        .prop_map(|(f, b, l, r, j, s, sp)| Keys {
            forward: f,
            backward: b,
            left: l,
            right: r,
            jump: j,
            sneak: s,
            sprint: sp,
        })
}

const BLOCKS: &[&str] = &[
    "minecraft:stone",
    "minecraft:oak_slab[type=bottom]",
    "minecraft:oak_stairs[facing=east,half=bottom,shape=straight]",
    "minecraft:snow[layers=3]",
    "minecraft:soul_sand",
    "minecraft:ice",
    "minecraft:honey_block",
    "minecraft:oak_fence[east=true,north=true]",
];

/// A floor of mixed blocks at y 0 and -1 with holes (ledges), walls and low
/// ceilings here and there, in a 24-block square; air above, unknown outside.
fn box_world(seed: u64) -> World {
    let mut rng = seed;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let mut b = World::air(
        BlockPos {
            x: -12,
            y: -4,
            z: -12,
        },
        BlockPos { x: 12, y: 8, z: 12 },
    );
    for x in -12..12 {
        for z in -12..12 {
            let r = next() % 100;
            if r < 8 {
                continue; // a hole
            }
            let name = BLOCKS[(next() % BLOCKS.len() as u64) as usize];
            let y = if r < 20 { 0 } else { -1 };
            b.set(
                BlockPos { x, y, z },
                if r < 20 { name } else { "minecraft:stone" },
            );
            if r > 88 {
                // walls, and low ceilings to crouch under
                b.set(BlockPos { x, y: 0, z }, "minecraft:stone");
                b.set(BlockPos { x, y: 1, z }, "minecraft:stone");
            } else if r > 84 {
                b.set(BlockPos { x, y: 2, z }, "minecraft:stone");
            }
        }
    }
    b
}

/// Ticks matched: all, on the ground, sneaking on the ground, with a horizontal
/// collision, gliding, jumping off the ground.
type Seen = [usize; 6];

fn sample(world: &World, seed: u64, inputs: &[(f32, f32, Keys)], seen: &mut Seen) {
    let start = Vec3::new(0.5 + (seed % 7) as f64 * 0.01, 1.0, 0.5);
    let mut p = LocalPlayer::new(start, 0.0, 0.0);
    p.delta_movement = Vec3::new(0.0, -0.0784000015258789, 0.0);
    // without a glider most of the time: a jump edge in the air would start a glide
    p.glider_equipped = seed.is_multiple_of(3);
    for (ticks, (yaw, pitch, keys)) in inputs.iter().enumerate() {
        let was_ground = p.on_ground;
        let s = from_port(&p);
        let u = Input {
            yaw: *yaw,
            pitch: *pitch,
        };
        let f = Faithful.step_full(world, &s, &u, *keys);
        let r = port::tick(
            &mut p,
            &TickInput {
                y_rot: *yaw,
                x_rot: *pitch,
                keys: keys_to_port(*keys),
            },
            world,
        );
        match (f, r) {
            (Ok(t), Ok(_)) => {
                assert_eq!(t.state, from_port(&p), "tick {ticks} from {s:?}");
                let q = &t.state;
                for (i, c) in [
                    true,
                    q.on_ground,
                    q.on_ground && keys.sneak,
                    q.horizontal_collision,
                    q.fall_flying,
                    was_ground && q.vel[1] > 0.0,
                ]
                .into_iter()
                .enumerate()
                {
                    seen[i] += c as usize;
                }
            }
            (Err(e), Err(u)) => {
                let same = matches!(
                    (e, u),
                    (Event::UnknownSpace(_), Unsupported::UnknownSpace(_))
                        | (Event::Climbable(_), Unsupported::Climbable(_))
                        | (Event::Fluid(_), Unsupported::Fluid(_))
                        | (Event::Stuck(_), Unsupported::Stuck(_))
                        | (Event::Bounce(_), Unsupported::Bounce(_))
                        | (Event::InsideBlock(_), _)
                );
                assert!(same, "tick {ticks}: faithful {e:?}, port {u:?}");
                break;
            }
            (Err(Event::InsideBlock(_)), Ok(_)) => break,
            (f, r) => panic!("tick {ticks} from {s:?}: faithful {f:?}, port {r:?}"),
        }
    }
}

/// Random walks over box worlds: faithful and port agree on every tick's full state.
#[test]
fn random_walks_match_port() {
    let mut runner = TestRunner::new(Config {
        cases: 300,
        ..Config::default()
    });
    let seen = std::cell::RefCell::new([0usize; 6]);
    runner
        .run(
            &(
                any::<u64>(),
                prop::collection::vec(
                    (-400.0f32..400.0, -90.0f32..90.0, keys_strategy()),
                    100..300,
                ),
            ),
            |(seed, inputs)| {
                let w = box_world(seed | 1);
                sample(&w, seed, &inputs, &mut seen.borrow_mut());
                Ok(())
            },
        )
        .unwrap();
    let s = seen.into_inner();
    println!(
        "{} ticks matched: {} on the ground ({} sneaking), {} with a horizontal collision, {} gliding, {} jumps",
        s[0], s[1], s[2], s[3], s[4], s[5]
    );
    assert!(s[0] > 20_000 && s.iter().all(|&n| n > 100), "{s:?}");
}
