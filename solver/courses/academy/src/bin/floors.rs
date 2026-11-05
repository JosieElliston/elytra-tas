//! Phase `floors`, for `tools/run_classes.py`: the floors of the start area as connected
//! components, one JSON line per cell: `{"x","z","y","head","c"}`. A cell is the top of a
//! collision box with room for a gliding player (0.6 blocks) over its whole column, `head` the
//! room above it up to 8 blocks, `c` its component. Cells join when they share an edge and are
//! within a step (0.6 blocks) of each other in height. why a step? you walk across one without
//! leaving the ground, and a higher ledge is another place to land.
//!
//! `cargo run --release -p academy --bin floors -- X0 Y0 Z0 X1 Y1 Z1 > floors.jsonl`
use etas::course::Course;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use world::Aabb;

const STEP: f64 = 0.6;
const ROOM: f64 = 0.6;
const MAX_HEAD: f64 = 8.0;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (_c, w) = Course::load(dir).unwrap();
    let v: Vec<i32> = std::env::args()
        .skip(1)
        .map(|s| s.parse().unwrap())
        .collect();
    let [x0, y0, z0, x1, y1, z1] = v[..] else {
        panic!("want X0 Y0 Z0 X1 Y1 Z1");
    };
    let mut cells: Vec<(i32, i32, f64, f64)> = Vec::new();
    for x in x0..x1 {
        for z in z0..z1 {
            let column = w
                .collision_boxes(Aabb {
                    min: [x as f64, y0 as f64, z as f64],
                    max: [x as f64 + 1.0, y1 as f64 + MAX_HEAD, z as f64 + 1.0],
                })
                .unwrap_or_default();
            let mut tops: Vec<f64> = column.iter().map(|b| b.max[1]).collect();
            tops.sort_by(f64::total_cmp);
            tops.dedup();
            for t in tops {
                if t < y0 as f64 || t >= y1 as f64 {
                    continue;
                }
                let head = column
                    .iter()
                    .filter(|b| b.max[1] > t && b.min[1] >= t)
                    .map(|b| b.min[1] - t)
                    .fold(MAX_HEAD, f64::min);
                let inside = column.iter().any(|b| b.min[1] < t && b.max[1] > t);
                if head >= ROOM && !inside {
                    cells.push((x, z, t, head));
                }
            }
        }
    }
    let mut at: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, c) in cells.iter().enumerate() {
        at.entry((c.0, c.1)).or_default().push(i);
    }
    let mut comp = vec![usize::MAX; cells.len()];
    let mut n = 0;
    for i in 0..cells.len() {
        if comp[i] != usize::MAX {
            continue;
        }
        comp[i] = n;
        let mut q = VecDeque::from([i]);
        while let Some(a) = q.pop_front() {
            let (x, z, t, _) = cells[a];
            for d in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                for &b in at.get(&(x + d.0, z + d.1)).into_iter().flatten() {
                    if comp[b] == usize::MAX && (cells[b].2 - t).abs() <= STEP {
                        comp[b] = n;
                        q.push_back(b);
                    }
                }
            }
        }
        n += 1;
    }
    for (i, c) in cells.iter().enumerate() {
        println!(
            "{{\"x\":{},\"z\":{},\"y\":{},\"head\":{},\"c\":{}}}",
            c.0, c.1, c.2, c.3, comp[i]
        );
    }
}
