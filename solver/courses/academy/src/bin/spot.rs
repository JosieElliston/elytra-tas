//! Phase `spot`: `optimize --chain` plans against the blocks in a box, with a dot and a tick number every `EVERY` ticks,
//! so two lines at the same tick show who's ahead.
//!
//! `cargo run --release -p academy --bin spot -- NAME X0 Y0 Z0 X1 Y1 Z1 OUT PLAN...`

use std::path::Path;

use etas::course::Course;
use view::color::{GREEN, RED, WHITE, YELLOW};
use view::{Camera, LabelStyle, Marker, Scene, Stroke, WorldStyle, tools};

const EVERY: usize = 10;

fn run() -> Result<(), String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 9 {
        return Err("NAME X0 Y0 Z0 X1 Y1 Z1 OUT PLAN...".into());
    }
    let v: Vec<f64> = a[1..7].iter().map(|x| x.parse().map_err(|_| "a number")).collect::<Result<_, _>>()?;
    let (lo, hi) = ([v[0], v[1], v[2]], [v[3], v[4], v[5]]);
    let (_, world) = Course::load(dir)?;
    let colors = [WHITE, RED, GREEN, YELLOW];
    let mut plans = Vec::new();
    for (i, p) in a[8..].iter().enumerate() {
        let f = dir.join(format!("runs/secant/chain/{}_chain_{p}.tsv", a[0]));
        let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        let pts: Vec<(usize, [f64; 3])> = text
            .lines()
            .enumerate()
            .filter_map(|(t, l)| {
                let c: Vec<f64> = l.split('\t').skip(2).filter_map(|x| x.parse().ok()).collect();
                let q = [c[0], c[1], c[2]];
                (0..3).all(|k| q[k] >= lo[k] && q[k] <= hi[k]).then_some((t, q))
            })
            .collect();
        plans.push((p.clone(), colors[i % colors.len()], pts));
    }
    let scene = Scene::new(&world)?;
    let bounds = vec![lo, hi];
    let draw = |f: &mut view::Frame, labels: bool| {
        for (_, c, pts) in plans.iter().rev() {
            let line: Vec<[f64; 3]> = pts.iter().map(|x| x.1).collect();
            f.polyline(&line, Stroke::new(*c, 2.5).occluded(0.6));
            for (t, q) in pts.iter().filter(|x| x.0 % EVERY == 0) {
                f.point(*q, Marker::circle(*c, 7.0).occluded(0.6));
                if labels && t % (2 * EVERY) == 0 {
                    f.label(*q, format!("{t}"), LabelStyle::default().size(12.0).accent(*c));
                }
            }
        }
    };
    let keys: Vec<(&str, _)> = plans.iter().map(|(n, c, _)| (n.as_str(), *c)).collect();
    let views = [
        ("top", Camera::top(&bounds), WorldStyle::new().inside(lo, hi).below_y(hi[1])),
        ("east", Camera::side(&bounds, 90.0), WorldStyle::new().inside(lo, hi)),
        ("south", Camera::side(&bounds, 0.0), WorldStyle::new().inside(lo, hi)),
    ];
    for (name, cam, style) in views {
        let mut f = scene.frame(&cam, 1500, 1100);
        f.style(style);
        draw(&mut f, true);
        tools::rulers(&mut f);
        tools::compass(&mut f, None);
        tools::legend(&mut f, &keys);
        tools::title(&mut f, &format!("{} from {name}", a[0]));
        f.finish().save(dir.join(format!("runs/secant/chain/{}_{name}.png", a[7])))?;
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("spot: {e}");
        std::process::exit(1);
    }
}
