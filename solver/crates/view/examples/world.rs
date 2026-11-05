//! `cargo run --release -p view --example world -- WORLD.gz OUT.png`: the whole world
//! from three directions, for looking at the renderer itself.
use std::time::Instant;
use view::{Camera, Image, Scene, tools};

fn main() -> Result<(), String> {
    let a: Vec<String> = std::env::args().collect();
    let t = Instant::now();
    let w = world::World::load(a[1].as_ref()).map_err(|e| e.to_string())?;
    println!("load {:?}", t.elapsed());
    let t = Instant::now();
    let scene = Scene::new(&w)?;
    println!("scene {:?} {:?}", t.elapsed(), scene.warnings());
    let (lo, hi) = w.header().and_then(|h| h.bounds).map_or(scene.bounds(), |b| {
        (b[0].map(|v| v as f64), b[1].map(|v| v as f64))
    });
    let lo = [lo[0], lo[1].max(30.0), lo[2]];
    let pts = [lo, hi];
    let mut out = Vec::new();
    for (name, cam) in [
        ("top", Camera::top(&pts)),
        ("perspective", Camera::fit(&pts).from(145.0, 35.0)),
    ] {
        let t = Instant::now();
        let mut f = scene.frame(&cam, 1400, 900);
        tools::rulers(&mut f);
        tools::compass(&mut f, None);
        tools::title(&mut f, name);
        let img = f.finish();
        println!("{name} {:?}", t.elapsed());
        out.push(img);
    }
    Image::grid(&out, 1).save(&a[2])
}
