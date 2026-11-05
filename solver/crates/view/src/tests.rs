use world::{BlockPos, World};

use crate::color::{RED, WHITE};
use crate::{Camera, CellMask, Scene, Stroke, WorldStyle, find_jar};

fn p(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos { x, y, z }
}

fn small() -> World {
    World::air(p(-8, 0, -8), p(8, 8, 8))
}

fn down(s: &Scene, style: &WorldStyle, x: f64, z: f64) -> Option<f64> {
    s.raycast(style, [x, 20.0, z], [0.0, -1.0, 0.0], 40.0).map(|h| h[1])
}

#[test]
fn top_view_is_a_map() {
    let w = small();
    let s = Scene::with_jar(&w, None).unwrap();
    let f = s.frame(&Camera::top(&[[-4.0, 0.0, -4.0], [4.0, 0.0, 4.0]]), 200, 200);
    let o = f.project([0.0, 0.0, 0.0]).unwrap();
    let east = f.project([1.0, 0.0, 0.0]).unwrap();
    let south = f.project([0.0, 0.0, 1.0]).unwrap();
    assert!(east[0] > o[0], "+x is right");
    assert!(south[1] > o[1], "+z is down");
}

#[test]
fn side_view_has_y_up() {
    let w = small();
    let s = Scene::with_jar(&w, None).unwrap();
    let f = s.frame(&Camera::side(&[[-4.0, 0.0, 0.0], [4.0, 4.0, 0.0]], 180.0), 200, 200);
    let o = f.project([0.0, 0.0, 0.0]).unwrap();
    assert!(f.project([0.0, 1.0, 0.0]).unwrap()[1] < o[1]);
    assert!(f.project([1.0, 0.0, 0.0]).unwrap()[0] > o[0], "looking north, +x is right");
}

#[test]
fn rays_and_projection_agree() {
    let w = small();
    let s = Scene::with_jar(&w, None).unwrap();
    for cam in [
        Camera::fit(&[[-4.0, 0.0, -4.0], [4.0, 4.0, 4.0]]),
        Camera::top(&[[-4.0, 0.0, -4.0], [4.0, 4.0, 4.0]]),
    ] {
        let f = s.frame(&cam, 300, 200);
        let q = [1.5, 2.5, -0.5];
        let px = f.project(q).unwrap();
        let (o, d) = f.view().ray(px[0], px[1]);
        let to = [q[0] - o[0], q[1] - o[1], q[2] - o[2]];
        let t = to[0] * d[0] + to[1] * d[1] + to[2] * d[2];
        for k in 0..3 {
            assert!((o[k] + d[k] * t - q[k]).abs() < 1e-9);
        }
    }
}

#[test]
fn fit_keeps_points_inside_the_margins() {
    let w = small();
    let s = Scene::with_jar(&w, None).unwrap();
    let pts = [[-40.0, 0.0, 3.0], [40.0, 30.0, -7.0], [5.0, 60.0, 20.0]];
    for cam in [Camera::fit(&pts).from(120.0, 30.0), Camera::top(&pts), Camera::side(&pts, 90.0)] {
        let f = s.frame(&cam.margin(20.0), 400, 300);
        for q in pts {
            let px = f.project(q).unwrap();
            assert!(px[0] >= 19.9 && px[0] <= 380.1, "{px:?}");
            assert!(px[1] >= 19.9 && px[1] <= 280.1, "{px:?}");
        }
    }
}

#[test]
fn collision_geometry_is_what_the_physics_sees() {
    let mut w = small();
    w.set(p(0, 0, 0), "minecraft:oak_fence");
    w.set(p(3, 0, 0), "minecraft:oak_slab[type=top]");
    let s = Scene::with_jar(&w, None).unwrap();
    let col = WorldStyle::new().collision();
    // 1.5 high, reached in the cell above the fence
    let top = down(&s, &col, 0.5, 0.5).unwrap();
    assert!((top - 1.5).abs() < 1e-6, "{top}");
    // from the side too, at a height above the fence's own cell
    let side = s.raycast(&col, [-3.0, 1.4, 0.5], [1.0, 0.0, 0.0], 10.0).unwrap();
    assert!((side[0] - 0.375).abs() < 1e-6, "{side:?}");
    assert!(s.raycast(&col, [-3.0, 1.6, 0.5], [1.0, 0.0, 0.0], 10.0).is_none());
    // from below, a top slab's underside
    let under = s.raycast(&col, [3.5, -3.0, 0.5], [0.0, 1.0, 0.0], 10.0).unwrap();
    assert!((under[1] - 0.5).abs() < 1e-6, "{under:?}");
}

#[test]
fn keep_and_clip_remove_blocks() {
    let mut w = small();
    w.set(p(0, 2, 0), "minecraft:stone");
    w.set(p(0, 5, 0), "minecraft:stone");
    let s = Scene::with_jar(&w, None).unwrap();
    assert!((down(&s, &WorldStyle::new(), 0.5, 0.5).unwrap() - 6.0).abs() < 1e-6);
    let keep = WorldStyle::new().keep(|q, _| q.y < 4);
    assert!((down(&s, &keep, 0.5, 0.5).unwrap() - 3.0).abs() < 1e-6);
    let clip = WorldStyle::new().below_y(4.0);
    assert!((down(&s, &clip, 0.5, 0.5).unwrap() - 3.0).abs() < 1e-6);
}

#[test]
fn trench_cuts_down_to_the_lowest_pass() {
    let m = CellMask::trench(&[[0.5, 50.0, 0.5], [1.5, 10.0, 0.5]], 3.0, 2.0);
    assert!(m.not_above(p(0, 12, 0)));
    assert!(!m.not_above(p(0, 13, 0)), "above the lower pass");
    assert!(m.not_above(p(40, 200, 0)), "away from the path, untouched");
}

/// An annotation behind a wall is drawn at `occluded` opacity.
#[test]
fn strokes_respect_depth() {
    let mut w = small();
    for x in -4..=4 {
        for y in 0..=8 {
            w.set(p(x, y, 0), "minecraft:stone");
        }
    }
    let s = Scene::with_jar(&w, None).unwrap();
    let cam = Camera::look_at([0.5, 4.5, 8.0], [0.5, 4.5, 0.0]).fov(50.0);
    let draw = |at_z: f64, occluded: f32| {
        let mut f = s.frame(&cam, 101, 101);
        f.samples(1);
        f.line(
            [-1.0, 4.5, at_z],
            [2.0, 4.5, at_z],
            Stroke::new(RED, 5.0).occluded(occluded).halo(0.0),
        );
        f.finish()
    };
    let center = |img: &crate::Image| {
        let i = (50 * 101 + 50) * 4;
        [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
    };
    let mut plain = s.frame(&cam, 101, 101);
    plain.samples(1);
    let base = center(&plain.finish());
    assert_eq!(center(&draw(-2.0, 0.0)), base, "hidden");
    let red = [(RED.r * 255.0) as u8, (RED.g * 255.0) as u8, (RED.b * 255.0) as u8];
    let xray = center(&draw(-2.0, 1.0));
    assert!((xray[0] as i32 - red[0] as i32).abs() <= 1, "{xray:?}");
    let half = center(&draw(-2.0, 0.5));
    assert!(half != base && half != xray, "{half:?}");
    let front = center(&draw(3.0, 0.0));
    assert!((front[0] as i32 - red[0] as i32).abs() <= 1, "in front is unaffected");
}

/// Variant rotations against what each model must do, from the client jar. Skipped if
/// there's none.
#[test]
fn block_models_rotate() {
    let Some(jar) = find_jar() else {
        eprintln!("no client jar; skipped");
        return;
    };
    let mut w = small();
    w.set(p(0, 0, 0), "minecraft:oak_stairs[facing=east]");
    w.set(p(2, 0, 0), "minecraft:oak_stairs[facing=south]");
    w.set(p(4, 0, 0), "minecraft:oak_stairs[facing=west,half=top]");
    w.set(p(-2, 0, 0), "minecraft:ladder[facing=south]");
    let s = Scene::with_jar(&w, Some(&jar)).unwrap();
    assert!(s.warnings().is_empty(), "{:?}", s.warnings());
    let st = WorldStyle::new();
    let y = |x, z| down(&s, &st, x, z).unwrap();
    let close = |a: f64, b: f64| (a - b).abs() < 1e-5;
    // a stair's high side is the side it faces
    assert!(close(y(0.75, 0.5), 1.0) && close(y(0.25, 0.5), 0.5));
    assert!(close(y(2.5, 0.75), 1.0) && close(y(2.5, 0.25), 0.5));
    // a top stair is solid at its top everywhere
    assert!(close(y(4.25, 0.5), 1.0) && close(y(4.75, 0.5), 1.0));
    // a ladder facing south hangs on its cell's north side
    let hit = s
        .raycast(&st, [-1.83, 0.53, 5.0], [0.0, 0.0, -1.0], 10.0)
        .unwrap();
    assert!(hit[2] < 0.25, "{hit:?}");
}

#[test]
fn renders_without_a_jar() {
    let mut w = small();
    w.set(p(0, 0, 0), "minecraft:stone");
    let s = Scene::with_jar(&w, None).unwrap();
    let mut f = s.frame(&Camera::look_at([0.5, 3.0, 0.5], [0.5, 0.0, 0.5001]), 32, 32);
    f.text([1.0, 1.0], "x", crate::TextStyle::default().color(WHITE));
    let img = f.finish();
    assert_eq!(img.rgba.len(), 32 * 32 * 4);
}
