use world::{BlockPos, World};

use crate::color::{RED, WHITE};
use crate::{Camera, NearMask, Scene, Stroke, TubeCut, WorldStyle, find_jar, tools};

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
fn barriers_are_hidden_unless_asked_for() {
    let mut w = small();
    w.set(p(0, 2, 0), "minecraft:stone");
    w.set(p(0, 5, 0), "minecraft:barrier");
    let s = Scene::with_jar(&w, None).unwrap();
    let height = |style: &WorldStyle| down(&s, style, 0.5, 0.5).unwrap();
    assert!((height(&WorldStyle::new().collision()) - 3.0).abs() < 1e-6);
    assert!((height(&WorldStyle::new().collision().show_barriers()) - 6.0).abs() < 1e-6);
    // translucent when shown, so a visual ray goes on to the stone either way
    assert!((height(&WorldStyle::new()) - 3.0).abs() < 1e-6);
    assert!((height(&WorldStyle::new().show_barriers()) - 3.0).abs() < 1e-6);
    // a clip through a hidden barrier doesn't hatch it
    let top = |style: WorldStyle| {
        let mut f = s.frame(&Camera::top(&[[-4.0, 0.0, -4.0], [4.0, 0.0, 4.0]]), 64, 64);
        f.style(style.below_y(5.5));
        f.samples(1);
        f.finish()
    };
    assert!(top(WorldStyle::new()).rgba == top(WorldStyle::new().keep(|q, _| q.y != 5)).rgba);
}

#[test]
fn near_mask_measures_to_the_nearest_part_of_the_cell() {
    let m = NearMask::new(&[[0.5, 0.5, 0.5]], 10.0);
    assert!(m.contains(p(0, 0, 0)));
    assert!(m.contains(p(10, 0, 0)), "its near face is 9.5 away");
    assert!(!m.contains(p(11, 0, 0)), "10.5 away");
    assert!(!m.contains(p(8, 8, 8)), "13 away on the diagonal");
    assert!(!NearMask::new(&[], 10.0).contains(p(0, 0, 0)));
}

#[test]
fn solid_barriers_block_only_where_kept() {
    let mut w = small();
    w.set(p(0, 2, 0), "minecraft:stone");
    // a thick stack, as in a hull, whose upper cells are filtered out
    w.set(p(0, 7, 0), "minecraft:barrier");
    w.set(p(0, 6, 0), "minecraft:barrier");
    w.set(p(0, 5, 0), "minecraft:barrier");
    let s = Scene::with_jar(&w, None).unwrap();
    let height = |style: &WorldStyle| down(&s, style, 0.5, 0.5).unwrap();
    let solid = || WorldStyle::new().solid_barriers();
    assert!((height(&solid()) - 8.0).abs() < 1e-6);
    assert!((height(&solid().barriers_where(|q, _| q.y < 6)) - 6.0).abs() < 1e-6);
    assert!((height(&solid().barriers_where(|_, _| false)) - 3.0).abs() < 1e-6);
    assert!((height(&solid().collision().barriers_where(|q, _| q.y < 6)) - 6.0).abs() < 1e-6);
    // the trace draws the kept cell's face, though a filtered barrier came before it
    let top = |style: WorldStyle| {
        let mut f = s.frame(&Camera::top(&[[-4.0, 0.0, -4.0], [4.0, 0.0, 4.0]]), 64, 64);
        f.style(style.desaturate(0.0));
        f.samples(1);
        f.finish()
    };
    let px = |i: &crate::Image| {
        let at = ((32 * i.w + 32) * 4) as usize;
        [i.rgba[at], i.rgba[at + 1], i.rgba[at + 2]]
    };
    let shown = px(&top(solid().barriers_where(|q, _| q.y < 6)));
    assert!(shown[0] > 200 && shown[1] < 60, "opaque red, got {shown:?}");
}

const BOUNDS: ([f64; 3], [f64; 3]) = ([-200.0, -200.0, -200.0], [200.0, 200.0, 200.0]);

/// The block holding a position.
fn at(v: [f64; 3]) -> BlockPos {
    p(v[0].floor() as i32, v[1].floor() as i32, v[2].floor() as i32)
}

fn along(from: [f64; 3], dir: [f64; 3], k: f64) -> [f64; 3] {
    [from[0] + dir[0] * k, from[1] + dir[1] * k, from[2] + dir[2] * k]
}

#[test]
fn tube_cut_from_above_digs_to_the_lowest_pass() {
    let pts = [[0.5, 50.0, 0.5], [1.5, 10.0, 0.5], [-30.5, 10.0, 0.5], [30.5, 10.0, 0.5]];
    let view = Camera::top(&pts).resolve(400, 400, BOUNDS);
    let m = TubeCut::new(&view, &pts, 3.0, 2.0);
    assert!(m.keeps(p(0, 11, 0)), "within the headroom of the lower pass");
    assert!(!m.keeps(p(0, 13, 0)), "above the lower pass");
    assert!(m.keeps(p(15, 200, 0)), "away from the path, untouched");
}

#[test]
fn tube_cut_follows_a_tilted_camera() {
    let at_p = [0.5, 50.5, 0.5];
    let pts = [at_p, [20.5, 50.5, 0.5], [-20.5, 50.5, 0.5]];
    let view = Camera::fit(&pts).from(30.0, 40.0).ortho().resolve(800, 600, BOUNDS);
    let m = TubeCut::new(&view, &pts, 3.0, 2.0);
    let (fwd, right) = (view.fwd, view.right);
    assert!(!m.keeps(at(along(at_p, fwd, -8.0))), "between the camera and the path");
    assert!(m.keeps(at(along(at_p, fwd, 8.0))), "behind the path");
    assert!(m.keeps(at(along(along(at_p, fwd, -8.0), view.up, 8.0))), "in front, off to the side");
    assert!(m.keeps(at(along(at_p, right, 2.0))), "beside the path, not in front of it");
}

#[test]
fn tube_cut_narrows_toward_a_perspective_eye() {
    let pt = [0.5, 0.5, 40.5];
    let view = Camera::look_at([0.5, 0.5, 0.5], pt).resolve(800, 600, BOUNDS);
    let m = TubeCut::new(&view, &[pt], 8.0, 2.0);
    // halfway to the eye, the sight lines to the tube span half its radius
    assert!(!m.keeps(p(2, 0, 20)), "in the cone");
    assert!(m.keeps(p(8, 0, 20)), "outside the cone, though within the radius of the path's axis");
}

#[test]
fn tube_cut_spares_a_floor_beside_the_path() {
    // flying a block above a floor, seen from 55 degrees: a floor block toward the camera
    // is nearer than the path, but in front of nothing
    let pts: Vec<[f64; 3]> = (-20..=20).map(|x| [x as f64 + 0.5, 51.5, 0.5]).collect();
    let view = Camera::fit(&pts).from(0.0, 55.0).ortho().resolve(800, 600, BOUNDS);
    let m = TubeCut::new(&view, &pts, 10.0, 1.0);
    for z in -8..=8 {
        assert!(m.keeps(p(0, 49, z)), "floor at z = {z}");
    }
    assert!(!m.keeps(at(along(pts[20], view.fwd, -8.0))), "over the path");
}

#[test]
fn tube_cut_clears_blocks_around_a_buried_eye() {
    let pt = [0.5, 0.5, 40.5];
    let view = Camera::look_at([0.5, 0.5, 0.5], pt).resolve(800, 600, BOUNDS);
    let m = TubeCut::new(&view, &[pt], 2.0, 1.0);
    assert!(!m.keeps(p(0, 0, 0)), "the block holding the eye");
    assert!(!m.keeps(p(1, 1, 1)), "off the sight line at its center, but it covers it");
    assert!(m.keeps(p(0, 0, -5)), "behind the eye");
    assert!(m.keeps(p(0, 0, 45)), "behind the path");
}

#[test]
fn readability_avoids_flying_at_the_camera() {
    let path: Vec<[f64; 3]> = (0..100).map(|i| [i as f64, 0.0, 0.0]).collect();
    let paths = [path.clone()];
    let across = Camera::fit(&path).from(0.0, 30.0).resolve(800, 600, BOUNDS);
    let along = Camera::fit(&path).from(-90.0, 30.0).resolve(800, 600, BOUNDS);
    let (a, b) = (tools::readability(&across, &paths), tools::readability(&along, &paths));
    assert!(a.worst_stretch > 0.8, "{a:?}");
    assert!(b.worst_stretch < 0.7, "{b:?}");
    assert!(a.score() > b.score());
}

#[test]
fn readability_counts_a_path_over_itself_but_not_other_paths() {
    let out: Vec<[f64; 3]> = (0..100).map(|i| [i as f64, 0.0, 0.0]).collect();
    let back: Vec<[f64; 3]> = out.iter().rev().cloned().collect();
    let there_and_back = [out.iter().chain(&back).cloned().collect::<Vec<_>>()];
    let view = Camera::fit(&out).from(0.0, 30.0).resolve(800, 600, BOUNDS);
    assert!(tools::readability(&view, &there_and_back).clear_of_itself < 0.2);
    assert_eq!(tools::readability(&view, &[out, back]).clear_of_itself, 1.0);
}

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
