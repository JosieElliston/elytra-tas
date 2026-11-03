//! A hand-made world written by Python (`make_small_world.py`, through
//! `extract_world.write_world`) and read back here.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use world::{Aabb, BlockPos, Cell, Effect, World, state_id};

fn small_world(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let out = dir.join(name);
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/make_small_world.py");
    let st = Command::new("python3")
        .arg(&script)
        .arg(&out)
        .status()
        .expect("python3");
    assert!(st.success(), "{} failed", script.display());
    out
}

fn p(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos { x, y, z }
}

fn known(w: &World, pos: BlockPos) -> &str {
    match w.cell(pos) {
        Cell::Known(id) => &w.info(id).state,
        Cell::Unknown => panic!("{pos:?} unknown"),
    }
}

#[test]
fn round_trip() {
    let w = World::load(&small_world("small.world.gz")).unwrap();
    let h = w.header().unwrap();
    assert_eq!(h.source, "hand-made (make_small_world.py)");
    assert_eq!(h.tick, None);
    assert_eq!(h.dimension.as_deref(), Some("test:small"));
    assert_eq!(h.min_y_method, "given");
    assert_eq!(h.bounds, Some([[-16, -16, 0], [31, 47, 31]]));
    assert_eq!(w.build_height(), Some((-16, 48)));

    assert_eq!(known(&w, p(5, 0, 5)), "minecraft:stone");
    assert_eq!(known(&w, p(-16, 0, 15)), "minecraft:stone");
    assert_eq!(known(&w, p(5, 1, 5)), "minecraft:air");
    assert_eq!(
        known(&w, p(3, 1, 3)),
        "minecraft:oak_slab[type=bottom,waterlogged=false]"
    );
    assert_eq!(known(&w, p(9, 31, 9)), "minecraft:glass");
    // chunk z 1 is in the bounds but was never loaded: explicit unknown
    assert_eq!(w.cell(p(5, 0, 20)), Cell::Unknown);
    // chunk x 2 is outside the bounds: not in the file
    assert_eq!(w.cell(p(40, 0, 5)), Cell::Unknown);
    // outside the build height it is air, loaded or not
    assert_eq!(known(&w, p(5, -17, 5)), "minecraft:air");
    assert_eq!(known(&w, p(40, 32, 40)), "minecraft:air");

    // the file's tags: vine climbable and not glide-through, scaffolding not climbable
    let eff = |pos| match w.cell(pos) {
        Cell::Known(id) => (w.info(id).effect, w.detail(id).climbable),
        Cell::Unknown => panic!(),
    };
    assert_eq!(eff(p(-5, 1, 2)), (Effect::Climbable, true));
    assert_eq!(eff(p(-6, 1, 2)), (Effect::Climbable, true));
    assert_eq!(eff(p(-7, 1, 2)), (Effect::None, false));

    // a slab on the floor
    let area = Aabb {
        min: [3.2, 0.9, 3.2],
        max: [3.8, 2.0, 3.8],
    };
    let b = w.collision_boxes(area).unwrap();
    assert_eq!(
        b,
        vec![
            Aabb {
                min: [3.0, 0.0, 3.0],
                max: [4.0, 1.0, 4.0]
            },
            Aabb {
                min: [3.0, 1.0, 3.0],
                max: [4.0, 1.5, 4.0]
            }
        ]
    );
    // bamboo, offset by its position
    let bamboo = state_id("minecraft:bamboo").unwrap();
    let want = w.boxes_at(bamboo, p(20, 1, 5));
    let o = w.offset(bamboo, p(20, 1, 5));
    assert_ne!(o, [0.0; 3]);
    assert_eq!(want[0].min[0], (0.40625 + o[0]) + 20.0);
    let area = Aabb {
        min: [20.0, 1.2, 5.0],
        max: [21.0, 1.8, 6.0],
    };
    assert_eq!(w.collision_boxes(area).unwrap(), want);
    // the shell under a box at z 15..16 reaches into the unknown chunk z 1
    let area = Aabb {
        min: [5.2, 1.0, 15.2],
        max: [5.8, 2.8, 15.8],
    };
    assert_eq!(w.collision_boxes(area).unwrap_err().0.z, 16);
}

#[test]
fn corrupt_content_is_rejected() {
    let path = small_world("corrupt.world.gz");
    let mut data = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(&path).unwrap())
        .read_to_end(&mut data)
        .unwrap();
    let last = data.len() - 1;
    data[last] ^= 1;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&data).unwrap();
    std::fs::write(&path, gz.finish().unwrap()).unwrap();
    let e = World::load(&path).unwrap_err().to_string();
    assert!(e.contains("content sha256"), "{e}");
}
