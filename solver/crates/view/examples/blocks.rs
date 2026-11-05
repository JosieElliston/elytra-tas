//! `cargo run --release -p view --example blocks -- OUT.png`: a test bed of block shapes
//! in every orientation, to compare with the game by eye.
use view::{Camera, Image, Scene, WorldStyle, tools};
use world::{BlockPos, World};

fn main() -> Result<(), String> {
    let out = std::env::args().nth(1).unwrap();
    let p = |x, y, z| BlockPos { x, y, z };
    let mut w = World::air(p(-2, 0, -2), p(30, 12, 22));
    for x in -2..=30 {
        for z in -2..=22 {
            w.set(p(x, 0, z), "minecraft:grass_block");
        }
    }
    let facings = ["north", "east", "south", "west"];
    let mut x = 0;
    for (i, f) in facings.iter().enumerate() {
        w.set(p(x + i as i32 * 2, 1, 0), &format!("minecraft:oak_stairs[facing={f}]"));
        w.set(p(x + i as i32 * 2, 1, 2), &format!("minecraft:stone_brick_stairs[facing={f},half=top]"));
        w.set(p(x + i as i32 * 2, 1, 4), &format!("minecraft:furnace[facing={f}]"));
        w.set(p(x + i as i32 * 2, 1, 6), &format!("minecraft:oak_trapdoor[facing={f},open=true]"));
        w.set(p(x + i as i32 * 2, 1, 8), &format!("minecraft:ladder[facing={f}]"));
        w.set(p(x + i as i32 * 2, 1, 10), &format!("minecraft:oak_door[facing={f}]"));
        w.set(p(x + i as i32 * 2, 2, 10), &format!("minecraft:oak_door[facing={f},half=upper]"));
    }
    x = 10;
    for (i, a) in ["x", "y", "z"].iter().enumerate() {
        w.set(p(x + i as i32 * 2, 1, 0), &format!("minecraft:oak_log[axis={a}]"));
    }
    w.set(p(x, 1, 2), "minecraft:oak_slab");
    w.set(p(x + 2, 1, 2), "minecraft:oak_slab[type=top]");
    w.set(p(x, 1, 4), "minecraft:oak_fence[north=true,east=true]");
    w.set(p(x + 1, 1, 4), "minecraft:oak_fence[west=true,east=true]");
    w.set(p(x + 2, 1, 4), "minecraft:oak_fence[west=true]");
    w.set(p(x, 1, 6), "minecraft:cobblestone_wall[east=tall,up=true]");
    w.set(p(x + 1, 1, 6), "minecraft:cobblestone_wall[west=tall,up=false,east=tall]");
    w.set(p(x + 2, 1, 6), "minecraft:cobblestone_wall[west=low]");
    w.set(p(x, 1, 8), "minecraft:short_grass");
    w.set(p(x + 1, 1, 8), "minecraft:poppy");
    w.set(p(x + 2, 1, 8), "minecraft:bamboo");
    w.set(p(x + 3, 1, 8), "minecraft:oak_leaves");
    w.set(p(x, 1, 10), "minecraft:chest");
    w.set(p(x + 2, 1, 10), "minecraft:oak_sign");
    x = 20;
    for z in 0..3 {
        for k in 0..3 {
            w.set(p(x + k, 1 + z, 0), "minecraft:glass");
            w.set(p(x + k, 1 + z, 3), "minecraft:red_stained_glass");
            w.set(p(x + k, 1 + z, 6), "minecraft:barrier");
        }
    }
    w.set(p(x, 1, 9), "minecraft:glass_pane[east=true,west=false]");
    w.set(p(x + 1, 1, 9), "minecraft:glass_pane[west=true,east=true]");
    for xx in x..x + 5 {
        for zz in 12..16 {
            w.set(p(xx, 0, zz), "minecraft:water");
            w.set(p(xx, 1, zz + 4), "minecraft:water");
        }
    }
    w.set(p(x + 6, 1, 13), "minecraft:lava");
    w.set(p(x + 8, 1, 2), "minecraft:vine[north=true]");
    w.set(p(x + 8, 1, 1), "minecraft:stone");
    w.set(p(x + 8, 1, 6), "minecraft:bell");
    w.set(p(x + 8, 1, 9), "minecraft:lantern");
    w.set(p(x + 8, 1, 11), "minecraft:torch");
    w.set(p(x + 8, 1, 14), "minecraft:wall_torch[facing=north]");
    w.set(p(x + 8, 1, 13), "minecraft:stone");
    let scene = Scene::new(&w)?;
    println!("{:?}", scene.warnings());
    let pts = [[-2.0, 0.0, -2.0], [31.0, 5.0, 23.0]];
    let orient = [[-0.5, 1.0, -0.5], [8.0, 3.0, 11.5]];
    let shapes = [[9.5, 1.0, -0.5], [15.0, 3.0, 11.5]];
    let clear = [[19.5, 1.0, -0.5], [30.0, 4.0, 15.0]];
    let mut out_imgs = Vec::new();
    for (name, cam, style) in [
        ("orientations: stairs, top stairs, furnace, trapdoor, ladder, door; N E S W, x grows right. from the south", Camera::fit(&orient).from(180.0, 35.0).fov(35.0).margin(10.0), WorldStyle::new().desaturate(0.0)),
        ("shapes: logs xyz, slabs, fences, walls, plants, chest, sign", Camera::fit(&shapes).from(180.0, 35.0).fov(35.0).margin(10.0), WorldStyle::new().desaturate(0.0)),
        ("glass, stained glass, barrier, pane, vine, bell, lantern, torches, fluids", Camera::fit(&clear).from(205.0, 35.0).fov(35.0).margin(10.0), WorldStyle::new().show_barriers().desaturate(0.0)),
        ("collision", Camera::fit(&pts).from(150.0, 40.0).fov(35.0), WorldStyle::new().collision().show_barriers().desaturate(0.0)),
        ("cut at y=1.5 with a clip", Camera::fit(&orient).from(180.0, 55.0).fov(35.0).margin(10.0), WorldStyle::new().below_y(1.5).desaturate(0.0)),
    ] {
        let mut f = scene.frame(&cam, 1200, 760);
        f.style(style);
        tools::title(&mut f, name);
        out_imgs.push(f.finish());
    }
    Image::grid(&out_imgs, 1).save(out)
}
