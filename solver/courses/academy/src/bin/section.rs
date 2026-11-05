//! Phase `section`, for `tools/route_hybrids.py`: at plane z = Z, rows of 0/1 per 0.1 block: is a gliding
//! player's 0.6-cube, centered in x and z with its feet at y, clear of collision boxes. A last
//! argument `x` cuts at plane x = Z instead, with the same rows of y and z in place of x.
//!
//! `cargo run --release -p academy --bin section -- Z X0 X1 Y0 Y1 [x]`
use etas::course::Course;
use std::path::Path;
use world::Aabb;
fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (_c, w) = Course::load(dir).unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let v: Vec<f64> = args[..5].iter().map(|s| s.parse().unwrap()).collect();
    let (z, x0, x1, y0, y1) = (v[0], v[1], v[2], v[3], v[4]);
    let across_x = args.get(5).is_some_and(|a| a == "x");
    // the cut's axis, then the other horizontal one, in [x, y, z] order
    let world = |a: f64, y: f64, c: f64| {
        if across_x { [c, y, a] } else { [a, y, c] }
    };
    let nx = ((x1 - x0) * 10.0) as usize;
    let ny = ((y1 - y0) * 10.0) as usize;
    let eps = 1e-3;
    let boxes = w
        .collision_boxes(Aabb {
            min: world(x0 - 1.0, y0 - 1.0, z - 1.0),
            max: world(x1 + 1.0, y1 + 1.0, z + 1.0),
        })
        .unwrap();
    for j in 0..ny {
        let y = y0 + j as f64 * 0.1;
        let mut l = String::with_capacity(nx);
        for i in 0..nx {
            let x = x0 + i as f64 * 0.1;
            let (a, b) = (
                world(x - 0.3 + eps, y + eps, z - 0.3 + eps),
                world(x + 0.3 - eps, y + 0.6 - eps, z + 0.3 - eps),
            );
            let hit = boxes
                .iter()
                .any(|q| (0..3).all(|k| q.min[k] < b[k] && q.max[k] > a[k]));
            l.push(if hit { '1' } else { '0' });
        }
        println!("{l}");
    }
}
