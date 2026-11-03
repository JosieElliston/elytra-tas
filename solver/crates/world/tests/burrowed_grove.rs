//! The Burrowed Grove world (tests/data/burrowed_grove.world.gz, from
//! `extract_world.py 2026-10-19T23_03_50.zip --tick 24788 --bounds -90 -64 -66 45 190 100`,
//! tick 24788 being the tape's reset tick) under the tape's ground poses.

use std::path::Path;

use world::{Aabb, World};

fn hex_f64(h: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(h, 16).unwrap())
}

#[test]
fn ground_poses_stand_on_a_box_top() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let w = World::load(&dir.join("tests/data/burrowed_grove.world.gz")).unwrap();
    let h = w.header().unwrap();
    assert_eq!(h.source, "2026-10-19T23_03_50.zip");
    assert_eq!(h.dimension.as_deref(), Some("minecraft:eburrowedgrove"));
    assert_eq!(h.min_y_method, "registry");
    assert_eq!(w.build_height(), Some((-64, 384)));

    let tape = std::fs::read_to_string(dir.join("../../../tas/burrowed_grove.tas")).unwrap();
    assert!(tape.contains("source=2026-10-19T23_03_50.zip\nreset_tick=24788\n"));
    let mut n = 0;
    for line in tape.lines().filter(|l| {
        l.split('#')
            .nth(1)
            .is_some_and(|c| c.trim_end().ends_with(" ground"))
    }) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (x, y, z) = (hex_f64(f[4]), hex_f64(f[5]), hex_f64(f[6]));
        // the player's box (0.6 x 1.8), reaching 0.01 below its feet
        let area = Aabb {
            min: [x - 0.3, y - 0.01, z - 0.3],
            max: [x + 0.3, y + 1.8, z + 0.3],
        };
        let boxes = w.collision_boxes(area).unwrap();
        let under = |b: &&Aabb| {
            b.max[1] == y
                && b.min[0] < x + 0.3
                && b.max[0] > x - 0.3
                && b.min[2] < z + 0.3
                && b.max[2] > z - 0.3
        };
        assert!(
            boxes.iter().any(|b| under(&b)),
            "tick {}: no box top at y {y} under {x} {z}: {boxes:?}",
            f[0]
        );
        let inside = |b: &&Aabb| {
            b.max[1] > y
                && b.min[1] < y + 1.8
                && b.min[0] < x + 0.3
                && b.max[0] > x - 0.3
                && b.min[2] < z + 0.3
                && b.max[2] > z - 0.3
        };
        assert!(
            !boxes.iter().any(|b| inside(&b)),
            "tick {}: a box inside the player at {x} {y} {z}",
            f[0]
        );
        n += 1;
    }
    assert!(n >= 15, "{n} ground poses");
}
