//! `Entity.move` against small hand-built box worlds.

use port::level::block_info;
use port::{BlockMap, BlockPos, Input, LocalPlayer, Pose, TickInput, Vec3, tick};
use world::Aabb;

fn full(map: &mut BlockMap, x: i32, y: i32, z: i32) {
    map.set(
        BlockPos { x, y, z },
        block_info(
            "minecraft:stone",
            &[Aabb {
                min: [0.0; 3],
                max: [1.0; 3],
            }],
        ),
    );
}

/// A player in the air at `pos`, standing pose, moving by `v` this tick: with no keys
/// and no glide, `travelInAir` moves by the velocity as it is (gravity comes after).
fn airborne(pos: Vec3, v: Vec3) -> LocalPlayer {
    let mut p = LocalPlayer::new(pos, 0.0, 0.0);
    p.on_ground = false;
    p.delta_movement = v;
    p
}

/// No keys, rotation 0 (the `airborne` players' rotation).
const STILL: TickInput = TickInput {
    y_rot: 0.0,
    x_rot: 0.0,
    keys: Input::EMPTY,
};

/// y first, then the larger horizontal axis: with |z| > |x| the move resolves z
/// before x, so a block that only the x step reaches (after z) clips x, not z.
#[test]
fn axis_order_y_then_larger_horizontal() {
    let mut map = BlockMap::default();
    full(&mut map, 1, 0, 1);
    full(&mut map, 1, 1, 1);
    let start = Vec3::new(0.0, 0.5, 0.0);
    let mut p = airborne(start, Vec3::new(0.9, 0.0, 1.0));
    tick(&mut p, &STILL, &map).unwrap();
    // z first to z in [0.7, 1.3], then x stops at the block's face x = 1 (box half
    // width 0.3F widened: 0.30000001192092896)
    let half = 0.3f32 as f64;
    assert_eq!(p.position.z, start.z + 1.0);
    assert_eq!(p.position.x, 1.0 - half);
    assert!(p.horizontal_collision);
    // the clipped axis' velocity is `-v * 0.0` (restitution 0), then air drag
    assert_eq!(p.delta_movement.x, 0.0);
    assert!(p.delta_movement.x.is_sign_negative());
    assert_eq!(p.delta_movement.z, 1.0 * 0.91f32 as f64);

    // the same block with |x| > |z|: x first passes under it (z still small), then z
    // is clipped
    let mut q = airborne(start, Vec3::new(1.0, 0.0, 0.9));
    tick(&mut q, &STILL, &map).unwrap();
    assert_eq!(q.position.x, start.x + 1.0);
    assert_eq!(q.position.z, 1.0 - half);
}

/// Diagonal past a block's corner: whether the L-shaped move clips the corner
/// depends on which horizontal axis goes first, not on the straight segment.
#[test]
fn corner_clipping() {
    let mut map = BlockMap::default();
    // a block in the +x, -z quadrant, beside the start
    full(&mut map, 1, 0, -1);
    full(&mut map, 1, 1, -1);
    let start = Vec3::new(0.0, 0.5, 0.0);
    let half = 0.3f32 as f64;

    // |z| > |x|: z first moves the box clear of the block, then x passes it
    let mut p = airborne(start, Vec3::new(1.0, 0.0, 1.1));
    tick(&mut p, &STILL, &map).unwrap();
    assert_eq!((p.position.x, p.position.z), (1.0, 1.1));
    assert!(!p.horizontal_collision);

    // |x| > |z|: x first runs into the block's -x face while the box still overlaps
    // it in z
    let mut q = airborne(start, Vec3::new(1.1, 0.0, 1.0));
    tick(&mut q, &STILL, &map).unwrap();
    assert_eq!((q.position.x, q.position.z), (1.0 - half, 1.0));
    assert!(q.horizontal_collision);
}

/// Sprinting on the ground into a bottom slab steps up onto it without losing the
/// horizontal move.
#[test]
fn step_up_onto_bottom_slab_while_sprinting() {
    let mut map = BlockMap::default();
    map.floor(-2, 6, -1, -2, 2);
    let slab = block_info(
        "minecraft:stone_slab[type=bottom,waterlogged=false]",
        &[Aabb {
            min: [0.0; 3],
            max: [1.0, 0.5, 1.0],
        }],
    );
    for z in -2..=2 {
        map.set(BlockPos { x: 2, y: 0, z }, slab.clone());
        map.set(BlockPos { x: 3, y: 0, z }, slab.clone());
    }
    // facing +x: `getInputVector` turns forward (0, 0, 1) by yaw -90 to (1, 0, 0)
    let mut p = LocalPlayer::new(Vec3::new(0.5, 0.0, 0.5), -90.0, 0.0);
    p.delta_movement = Vec3::new(0.0, (0.0 - 0.08) * 0.98f32 as f64, 0.0);
    let keys = Input {
        forward: true,
        sprint: true,
        ..Input::EMPTY
    };
    let mut stepped = None;
    for t in 0..20 {
        let x_before = p.position.x;
        tick(
            &mut p,
            &TickInput {
                y_rot: -90.0,
                x_rot: 0.0,
                keys,
            },
            &map,
        )
        .unwrap();
        assert!(p.on_ground, "tick {t}: left the ground");
        assert!(p.sprinting, "tick {t}: stopped sprinting");
        assert!(p.position.x > x_before, "tick {t}: stopped moving");
        if p.position.y > 0.0 && stepped.is_none() {
            stepped = Some(t);
            assert_eq!(p.position.y, 0.5, "steps exactly onto the slab top");
            assert!(
                !p.horizontal_collision,
                "the step-up move is not a horizontal collision"
            );
            assert_eq!(
                p.main_supporting_block_pos,
                Some(BlockPos { x: 2, y: 0, z: 0 })
            );
        }
        if p.position.x > 3.5 {
            break;
        }
    }
    assert!(stepped.is_some(), "never stepped up");
    assert_eq!(p.position.y, 0.5);
}

/// A full block is 1 tall, above the 0.6 step height: walking into it stops.
#[test]
fn no_step_up_onto_full_block() {
    let mut map = BlockMap::default();
    map.floor(-2, 6, -1, -2, 2);
    for z in -2..=2 {
        full(&mut map, 2, 0, z);
    }
    let mut p = LocalPlayer::new(Vec3::new(0.5, 0.0, 0.5), -90.0, 0.0);
    let keys = Input {
        forward: true,
        sprint: true,
        ..Input::EMPTY
    };
    for _ in 0..20 {
        tick(
            &mut p,
            &TickInput {
                y_rot: -90.0,
                x_rot: 0.0,
                keys,
            },
            &map,
        )
        .unwrap();
    }
    assert_eq!(p.position.y, 0.0);
    assert_eq!(p.position.x, 2.0 - 0.3f32 as f64);
    assert!(p.horizontal_collision);
    // a head-on wall is not a minor collision, so sprint stops
    assert!(!p.sprinting);
}

/// Falling onto the floor: lands exactly on the top face, `onGround`, the supporting
/// block found, and the vertical velocity zeroed by the clip and then refilled by one
/// tick of gravity and drag.
#[test]
fn landing() {
    let mut map = BlockMap::default();
    map.floor(-2, 2, -1, -2, 2);
    let mut p = airborne(Vec3::new(0.5, 0.3, 0.5), Vec3::new(0.0, -0.5, 0.0));
    tick(&mut p, &STILL, &map).unwrap();
    assert_eq!(p.position.y, 0.0);
    assert!(p.on_ground && p.vertical_collision && p.vertical_collision_below);
    assert_eq!(
        p.main_supporting_block_pos,
        Some(BlockPos { x: 0, y: -1, z: 0 })
    );
    assert_eq!(p.delta_movement.y, (0.0 - 0.08) * 0.98f32 as f64);
    assert_eq!(p.fall_distance, 0.0);

    // gliding into the ground: the client keeps gliding (only the server ends it),
    // and the elytra path doesn't add gravity after the clip
    let mut g = airborne(Vec3::new(0.5, 0.3, 0.5), Vec3::new(0.3, -0.5, 0.0));
    g.fall_flying = true;
    g.pose = Pose::FallFlying;
    tick(
        &mut g,
        &TickInput {
            y_rot: -90.0,
            x_rot: 10.0,
            keys: Input::EMPTY,
        },
        &map,
    )
    .unwrap();
    assert_eq!(g.position.y, 0.0);
    assert!(g.on_ground && g.fall_flying);
    assert_eq!(g.delta_movement.y, 0.0);
    assert!(g.position.x > 0.5);
}

/// Water in the box is not simulated: it is an error naming the block.
#[test]
fn fluid_is_unsupported() {
    let mut map = BlockMap::default();
    let mut water = block_info("minecraft:water[level=0]", &[]);
    water.effect = world::Effect::Water;
    map.set(BlockPos { x: 0, y: 0, z: 0 }, water);
    let mut p = airborne(Vec3::new(0.5, 0.5, 0.5), Vec3::ZERO);
    assert_eq!(
        tick(&mut p, &STILL, &map),
        Err(port::Unsupported::Fluid(BlockPos { x: 0, y: 0, z: 0 }))
    );
}

/// Unknown space anywhere a query reaches is an error, not air.
#[test]
fn unknown_space_is_an_error() {
    let mut map = BlockMap::default();
    map.known = Some((
        BlockPos {
            x: -5,
            y: -5,
            z: -5,
        },
        BlockPos { x: 5, y: 5, z: 5 },
    ));
    let mut p = airborne(Vec3::new(4.5, 0.0, 0.5), Vec3::new(1.5, 0.0, 0.0));
    assert!(matches!(
        tick(&mut p, &STILL, &map),
        Err(port::Unsupported::UnknownSpace(_))
    ));
}
