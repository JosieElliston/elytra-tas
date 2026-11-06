//! What our artifacts look like: runs, course regions, boosters and `verify` reports as
//! annotations on a [`view::Frame`], and [`sheet`], the contact sheet `etas view` makes.
//! Everything here is built from `view`'s public primitives.

use std::path::Path;

use view::color::{GREEN, INK, RED, SERIES, WHITE, YELLOW, series};
use view::{Camera, Color, Frame, Image, LabelStyle, Marker, Ramp, Scene, Shape, Stroke, TextStyle, TubeCut, V3, WorldStyle, tools};
use world::World;

use crate::course::{Bounds, Course};
use crate::plan;
use crate::server::ServerEvent;
use crate::tape::Tape;
use crate::trajectory::{Trajectory, from_tape};
use crate::verify::Report;

/// A run's feet positions, tick 0 (the initial state) first, so index is tick.
pub fn points(t: &Trajectory) -> Vec<V3> {
    std::iter::once(t.initial.position)
        .chain(t.rows.iter().map(|r| r.state.position))
        .collect()
}

/// Speed in blocks per tick at each entry of [`points`].
pub fn speeds(t: &Trajectory) -> Vec<f64> {
    let len = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    std::iter::once(len(t.initial.delta_movement))
        .chain(t.rows.iter().map(|r| len(r.state.delta_movement)))
        .collect()
}

/// The path as the physics moves it: per tick, y first, then the larger horizontal axis
/// of the requested move, then the other (`Entity.collide`). Each entry is a point and
/// its fractional tick, and ticks `a..=b` end on their exact positions. Teleports are
/// straight lines. Off by default in [`run`]: a tick's L is a small zigzag, so a fast
/// run looks noisy, but it's what collision actually tests.
pub fn l_step_points(t: &Trajectory, a: usize, b: usize) -> Vec<(V3, f64)> {
    let pts = points(t);
    let mut out = vec![(pts[a], a as f64)];
    for tick in a + 1..=b {
        let (from, to) = (pts[tick - 1], pts[tick]);
        let teleported = t.arrivals.iter().any(|x| {
            x.tick == tick && matches!(x.event, ServerEvent::Teleport { .. })
        });
        if teleported {
            out.push((to, tick as f64));
            continue;
        }
        let total: f64 = (0..3).map(|k| (to[k] - from[k]).abs()).sum();
        let mut at = from;
        let mut walked = 0.0;
        let first = l_step_first_horizontal(t, tick);
        for axis in [1, first, 2 - first] {
            let d = to[axis] - at[axis];
            if d == 0.0 {
                continue;
            }
            walked += d.abs();
            at[axis] = to[axis];
            let frac = if total > 0.0 { walked / total } else { 1.0 };
            out.push((at, tick as f64 - 1.0 + frac));
        }
        // land on the tick's exact bits
        out.last_mut().unwrap().0 = to;
    }
    out
}

/// Which horizontal axis tick `tick` moved along first: 0 (x) or 2 (z), as
/// `Direction.axisStepOrder`, x unless |x| < |z| of the requested move.
fn l_step_first_horizontal(t: &Trajectory, tick: usize) -> usize {
    let row = &t.rows[tick - 1];
    let from = state_at(t, tick - 1).position;
    let to = row.state.position;
    // without a horizontal collision the requested move is what happened
    let mut d = [to[0] - from[0], to[2] - from[2]];
    if row.state.horizontal_collision
        && let Some(requested) = requested_move(t, tick)
    {
        d = requested;
    }
    if d[0].abs() < d[1].abs() { 2 } else { 0 }
}

/// The horizontal move tick `tick` asked for, which an artifact doesn't store and a wall
/// clips: the tick flown again in an empty world, where nothing clips it. Block friction
/// and speed factors are missing there, which scale x and z alike, so which is larger
/// survives. None if the state doesn't resume.
fn requested_move(t: &Trajectory, tick: usize) -> Option<[f64; 2]> {
    let start = state_at(t, tick - 1);
    let mut p = start.to_player().ok()?;
    let c = start.position.map(|v| v.floor() as i32);
    let corner = |s: i32| world::BlockPos {
        x: c[0] + s,
        y: c[1] + s,
        z: c[2] + s,
    };
    let empty = World::air(corner(-32), corner(32));
    crate::server::apply_at(&mut p, &t.arrivals, tick, &empty);
    let before = p.position;
    let input = t.rows[tick - 1].input.tick_input().ok()?;
    port::tick(&mut p, &input, &empty).ok()?;
    Some([p.position.x - before.x, p.position.z - before.z])
}

fn state_at(t: &Trajectory, tick: usize) -> &crate::state::FullState {
    if tick == 0 {
        &t.initial
    } else {
        &t.rows[(tick - 1).min(t.rows.len() - 1)].state
    }
}

/// How [`run`] draws a trajectory.
#[derive(Debug, Clone)]
pub struct RunStyle {
    pub color: Color,
    pub width: f32,
    /// Color by speed over `lo..hi`, instead of `color`.
    pub by_speed: Option<(f64, f64)>,
    /// A dot every this many ticks, and a tick label every fifth dot. 0 for none.
    pub tick_every: usize,
    pub tick_labels: bool,
    /// Mark ground touches, wall and ceiling contacts, and server events.
    pub events: bool,
    /// Draw the hitbox and look at both ends.
    pub ends: bool,
    /// Only these ticks of the run.
    pub ticks: Option<(usize, usize)>,
    /// Join ticks with the physics' L-shaped move ([`l_step_points`]) instead of straight
    /// lines.
    pub l_step: bool,
}

impl RunStyle {
    pub fn new(color: Color) -> RunStyle {
        RunStyle {
            color,
            width: 2.5,
            by_speed: None,
            tick_every: 10,
            tick_labels: true,
            events: true,
            ends: true,
            ticks: None,
            l_step: false,
        }
    }
}

/// A trajectory: its path, tick marks, events and end poses. The path is the feet
/// position, the coordinate every report and tape uses, which is the bottom of the
/// hitbox.
pub fn run(f: &mut Frame, t: &Trajectory, st: &RunStyle) {
    let pts = points(t);
    let (a, b) = st.ticks.unwrap_or((0, pts.len() - 1));
    let (a, b) = (a.min(pts.len() - 1), b.min(pts.len() - 1));
    let stroke = Stroke::new(st.color, st.width);
    let v = speeds(t);
    let (line, values): (Vec<V3>, Vec<f64>) = if st.l_step {
        l_step_points(t, a, b)
            .into_iter()
            .map(|(p, tick)| {
                let i = (tick.floor() as usize).min(v.len() - 1);
                let j = (i + 1).min(v.len() - 1);
                (p, v[i] + (v[j] - v[i]) * (tick - i as f64))
            })
            .unzip()
    } else {
        (pts[a..=b].to_vec(), v[a..=b].to_vec())
    };
    match st.by_speed {
        Some((lo, hi)) => {
            tools::gradient(f, &line, &values, lo, hi, &Ramp::magnitude(), stroke)
        }
        None => f.polyline(&line, stroke),
    }
    if st.tick_every > 0 {
        let dot = Marker::circle(WHITE, 4.0);
        let mut k = a.div_ceil(st.tick_every) * st.tick_every;
        while k <= b {
            f.point(pts[k], dot);
            if st.tick_labels && k.is_multiple_of(st.tick_every * 5) {
                f.label(pts[k], format!("{k}"), LabelStyle::default().size(11.0).accent(st.color));
            }
            k += st.tick_every;
        }
    }
    if st.events {
        events(f, t, a, b);
    }
    if st.ends {
        for tick in [a, b] {
            let s = state_at(t, tick);
            tools::hitbox(f, s.position, Stroke::new(st.color, 1.5).halo(1.0));
            tools::look(f, s.position, s.y_rot, s.x_rot, 2.0, Stroke::new(WHITE, 1.5).halo(1.0));
        }
    }
}

/// Markers for what happened on a run, by [`events_legend`]'s key.
pub fn events(f: &mut Frame, t: &Trajectory, a: usize, b: usize) {
    let mut was_ground = t.initial.on_ground;
    for r in &t.rows {
        let s = &r.state;
        if (a..=b).contains(&r.t) {
            if s.on_ground && !was_ground {
                f.point(s.position, Marker::new(Shape::Triangle, GREEN, 11.0));
            }
            if s.horizontal_collision {
                f.point(s.position, Marker::new(Shape::Cross, YELLOW, 11.0));
            }
            if s.vertical_collision && !s.vertical_collision_below {
                f.point(s.position, Marker::new(Shape::Square, YELLOW, 9.0));
            }
        }
        was_ground = s.on_ground;
    }
    // a booster re-sends for several ticks, so a burst of one kind is one label
    let mut i = 0;
    while i < t.arrivals.len() {
        let first = &t.arrivals[i];
        let kind = std::mem::discriminant(&first.event);
        let mut j = i;
        while j + 1 < t.arrivals.len()
            && std::mem::discriminant(&t.arrivals[j + 1].event) == kind
            && t.arrivals[j + 1].tick <= t.arrivals[j].tick + 5
        {
            j += 1;
        }
        let last_tick = t.arrivals[j].tick;
        i = j + 1;
        if !(a..=b).contains(&first.tick) {
            continue;
        }
        let at = state_at(t, first.tick.saturating_sub(1));
        let (shape, col, text) = match first.event {
            ServerEvent::GlideOff => (Shape::Square, RED, "glide off"),
            ServerEvent::SetVelocity { .. } => (Shape::Diamond, SERIES[1], "velocity"),
            ServerEvent::Teleport { .. } => (Shape::Circle, RED, "teleport"),
        };
        f.point(at.position, Marker::new(shape, col, 11.0));
        let ticks = if last_tick == first.tick {
            format!("@{}", first.tick)
        } else {
            format!("@{}-{last_tick}", first.tick)
        };
        f.label(
            at.position,
            format!("{text} {ticks}"),
            LabelStyle::default().size(11.0).accent(col),
        );
    }
}

/// The key to [`events`]' markers.
pub const EVENTS_LEGEND: &str = "triangle ground  X wall  square ceiling/glide off  diamond velocity  dots every 10 ticks";

/// A `Bounds` with its open faces clamped to `limit`.
fn clamp_bounds(b: &Bounds, limit: (V3, V3)) -> (V3, V3) {
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    for k in 0..3 {
        lo[k] = b.min[k].unwrap_or(limit.0[k]);
        hi[k] = b.max[k].unwrap_or(limit.1[k]);
    }
    (lo, hi)
}

/// A region's working box, with its inner and outer boxes if `bracket`.
pub fn region(f: &mut Frame, limit: (V3, V3), r: &crate::course::Region, name: &str, color: Color, bracket: bool) {
    let (lo, hi) = clamp_bounds(&r.working, limit);
    f.aabb(
        lo,
        hi,
        Stroke::new(color, 2.0).occluded(0.5),
        Some(color.alpha(0.05)),
    );
    let top = [(lo[0] + hi[0]) * 0.5, hi[1], (lo[2] + hi[2]) * 0.5];
    f.label(top, name, LabelStyle::default().accent(color));
    if bracket {
        for (b, a) in [(&r.inner, 0.9), (&r.outer, 0.4)] {
            let (l, h) = clamp_bounds(b, limit);
            f.aabb(l, h, Stroke::new(color.alpha(a), 1.0).halo(0.0).occluded(0.3), None);
        }
    }
}

/// The start area, finish and boosters.
pub fn course(f: &mut Frame, limit: (V3, V3), c: &Course) {
    for (name, r) in &c.regions {
        let color = match name.as_str() {
            "finish" => Color::hex(0xFFFFFF),
            "start_area" => GREEN,
            _ => Color::hex(0xC0C0C0),
        };
        region(f, limit, r, name, color, false);
    }
    for b in &c.boosters {
        let label = format!("{} {:.1} b/t", b.name, b.speed);
        region(f, limit, &b.region, &label, Color::hex(0xFF9D00), false);
    }
}

/// What `verify` found: [`report_failures`] and [`clearance`].
pub fn report(f: &mut Frame, t: &Trajectory, r: &Report) {
    report_failures(f, t, r);
    clearance(f, t, r);
}

/// The report's tightest clearance.
pub fn clearance(f: &mut Frame, t: &Trajectory, r: &Report) {
    if let Some(c) = &r.clearance {
        let p = state_at(t, c.tick).position;
        f.point(p, Marker::circle(YELLOW, 9.0).ring(INK));
        f.label(
            p,
            format!("min clearance {:.3} @{}", c.min, c.tick),
            LabelStyle::default().accent(YELLOW),
        );
    }
}

/// Kills, damaging contacts and a state that didn't reproduce.
pub fn report_failures(f: &mut Frame, t: &Trajectory, r: &Report) {
    for k in &r.kills {
        f.point(k.pos, Marker::new(Shape::Cross, RED, 16.0).ring(WHITE));
        f.label(k.pos, format!("KILL {} @{}", k.rule, k.tick), LabelStyle::default().accent(RED));
    }
    for c in &r.contacts {
        if c.damage > 0.0 && c.before_finish {
            let p = state_at(t, c.tick).position;
            f.point(p, Marker::new(Shape::Cross, RED, 13.0).ring(WHITE));
            f.label(
                p,
                format!("{:.1} dmg @{}", c.damage, c.tick),
                LabelStyle::default().accent(RED),
            );
        }
    }
    if let Some((tick, why)) = &r.mismatch {
        let p = state_at(t, *tick).position;
        f.point(p, Marker::new(Shape::Cross, RED, 16.0).ring(WHITE));
        f.label(p, format!("mismatch @{tick}: {why}"), LabelStyle::default().accent(RED));
    }
}

/// Spokes joining two runs at equal ticks, every `every` ticks: where they part, and
/// the lean of the spokes shows which one is ahead.
pub fn spokes(f: &mut Frame, a: &Trajectory, b: &Trajectory, every: usize, color: Color, ticks: Option<(usize, usize)>) {
    let (pa, pb) = (points(a), points(b));
    let n = pa.len().min(pb.len());
    let (lo, hi) = ticks.unwrap_or((0, n - 1));
    let st = Stroke::new(color, 1.0).halo(0.0).occluded(0.25);
    for t in (lo..=hi.min(n - 1)).step_by(every.max(1)) {
        f.line(pa[t], pb[t], st);
    }
}

/// One thing to draw in a [`sheet`].
pub struct SheetRun {
    pub name: String,
    pub traj: Trajectory,
    pub report: Option<Report>,
}

/// What a file holds, as runs: a trajectory, a tape, or a plan's paths.
pub fn load_runs(path: &Path, name: &str, c: &Course, w: &World) -> Result<Vec<SheetRun>, String> {
    let one = |traj: Trajectory| {
        vec![SheetRun {
            name: name.to_string(),
            traj,
            report: None,
        }]
    };
    if path.extension().is_none_or(|e| e != "tas") {
        return Ok(one(Trajectory::read(path)?));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if text.lines().any(|l| l.trim_start().starts_with("@server ")) {
        let tape = Tape::parse(&text)?;
        return Ok(one(fly(&tape, w, c, name)?));
    }
    let p = plan::Plan::parse(&text)?;
    let count = p.path_count();
    if count > SERIES.len() as u128 {
        eprintln!(
            "{name}: {count} paths; drawing its root segment only. Draw chosen paths from a script."
        );
        return Ok(one(fly(&p.tape, w, c, name)?));
    }
    let mut out = Vec::new();
    for path in p.paths() {
        let mut tape = path.tape.clone();
        tape.arrivals = path.arrivals.clone();
        let n = if path.segments.len() <= 1 {
            name.to_string()
        } else {
            path.segments.last().unwrap().clone()
        };
        out.push(SheetRun {
            traj: fly(&tape, w, c, &n)?,
            name: n,
            report: None,
        });
    }
    Ok(out)
}

fn fly(tape: &Tape, w: &World, c: &Course, name: &str) -> Result<Trajectory, String> {
    let (t, err) = from_tape(tape, w, Some(c), name);
    if let Some((tick, u)) = err {
        eprintln!("{name}: port stopped at tick {tick}: {u:?}; drawing up to there");
    }
    if t.rows.is_empty() {
        return Err(format!("{name}: no ticks flown"));
    }
    Ok(t)
}

/// Options of [`sheet`].
#[derive(Debug, Clone)]
pub struct SheetOptions {
    /// Frame this tick range.
    pub ticks: Option<(usize, usize)>,
    pub collision: bool,
    /// Draw every barrier, translucent. Otherwise only those within `barrier_radius` of a
    /// run are drawn, solid. They collide either way.
    pub barriers: bool,
    /// How far from a run barriers are drawn.
    pub barrier_radius: f64,
    /// Join ticks with the physics' L-shaped move.
    pub l_step: bool,
    /// Pixels per panel.
    pub width: u32,
    pub height: u32,
    /// The cut toward the camera: blocks more than this in front of the path are hidden.
    /// why so little? more leaves blocks over the path, which hide it.
    pub headroom: f64,
    /// How far around the path the cut toward the camera reaches.
    pub radius: f64,
    /// The side panel shows blocks up to this far behind the path.
    pub slice: f64,
}

impl Default for SheetOptions {
    fn default() -> Self {
        SheetOptions {
            ticks: None,
            collision: false,
            barriers: false,
            barrier_radius: 10.0,
            l_step: false,
            width: 1100,
            height: 720,
            headroom: 1.0,
            radius: 10.0,
            slice: 40.0,
        }
    }
}

fn summary(r: &SheetRun) -> String {
    let mut s = format!("{}: {} ticks", r.name, r.traj.rows.len());
    if let Some(rep) = &r.report {
        if let Some(t) = rep.finish_tick() {
            s += &format!(", finish tick {t}");
        }
        if let Some(t) = rep.timer_seconds() {
            s += &format!(", {t:.2}s");
        }
        let walls = rep.contacts.iter().filter(|c| c.before_finish).count();
        s += &format!(", {walls} contacts, {} kills", rep.kills.len());
        s += if rep.ok() { ", PASS" } else { ", FAIL" };
    }
    s
}

/// The yaw to see a path from the side: across its longer horizontal extent, along a
/// world axis so the rulers read coordinates.
fn side_yaw(pts: &[V3]) -> f64 {
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for p in pts {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    if hi[0] - lo[0] >= hi[2] - lo[2] { 180.0 } else { 90.0 }
}

/// For a side view along a world axis: only the blocks behind the path. An orthographic
/// ray keeps its position across the view and its height, so a block hides a path point
/// only if it matches both. Near any path point, blocks are cut back to the nearest pass
/// at that position and height, so nothing hides the path; elsewhere, back to the framed
/// stretch's nearest pass at that position, which gives the backdrop.
struct Backdrop {
    /// 0 for looking along x, 2 for z: the depth axis.
    axis: usize,
    /// +1 if depth grows with the coordinate.
    sign: f64,
    min: i32,
    near: Vec<f64>,
    /// Nearest pass by (across, y), within a small band of the path.
    near_at: std::collections::HashMap<(i32, i32), f64>,
    depth: f64,
}

impl Backdrop {
    /// `whole` is every drawn point, `framed` the stretch the view is fitted to.
    fn new(whole: &[V3], framed: &[V3], yaw: f64, depth: f64) -> Backdrop {
        let look = view::look_vector(yaw, 0.0);
        let axis = if look[0].abs() > look[2].abs() { 0 } else { 2 };
        let sign = look[axis].signum();
        let across = 2 - axis;
        let lo = framed.iter().map(|p| p[across]).fold(f64::MAX, f64::min).floor() as i32;
        let hi = framed.iter().map(|p| p[across]).fold(f64::MIN, f64::max).ceil() as i32;
        let n = (hi - lo + 1).max(1) as usize;
        let mut near = vec![f64::MAX; n];
        for p in framed {
            let i = (p[across].floor() as i32 - lo) as usize;
            near[i] = near[i].min(p[axis] * sign);
        }
        // across a stretch with no path, the nearest known depth
        let mut last = f64::MAX;
        for v in near.iter_mut() {
            if *v == f64::MAX {
                *v = last;
            } else {
                last = *v;
            }
        }
        let mut last = f64::MAX;
        for v in near.iter_mut().rev() {
            if *v == f64::MAX {
                *v = last;
            } else {
                last = *v;
            }
        }
        let mut near_at = std::collections::HashMap::new();
        for p in whole {
            let (u, y) = (p[across].floor() as i32, p[1].floor() as i32);
            let d = p[axis] * sign;
            for du in -2..=2 {
                for dy in -2..=2 {
                    let e = near_at.entry((u + du, y + dy)).or_insert(f64::MAX);
                    if d < *e {
                        *e = d;
                    }
                }
            }
        }
        Backdrop {
            axis,
            sign,
            min: lo,
            near,
            near_at,
            depth,
        }
    }

    fn keeps(&self, p: world::BlockPos) -> bool {
        let (d, u) = if self.axis == 0 { (p.x, p.z) } else { (p.z, p.x) };
        let i = (u - self.min).clamp(0, self.near.len() as i32 - 1) as usize;
        let at = (d as f64 + 0.5) * self.sign;
        let near = self.near_at.get(&(u, p.y)).copied().unwrap_or(self.near[i]);
        at >= near - 1.0 && at <= self.near[i].max(near) + self.depth
    }
}

/// Top, side and perspective views of runs in one image, a text line per run.
pub fn sheet(scene: &Scene, c: Option<&Course>, runs: &[SheetRun], o: &SheetOptions) -> Image {
    assert!(!runs.is_empty());
    assert!(
        runs.len() <= SERIES.len(),
        "{} runs in one sheet; use panels",
        runs.len()
    );
    // what to frame
    let mut framed_paths: Vec<Vec<V3>> = Vec::new();
    let mut whole: Vec<V3> = Vec::new();
    for r in runs {
        let pts = points(&r.traj);
        let (a, b) = o.ticks.unwrap_or((0, pts.len() - 1));
        let (a, b) = (a.min(pts.len() - 1), b.min(pts.len() - 1));
        framed_paths.push(pts[a..=b].to_vec());
        whole.extend_from_slice(&pts);
    }
    let framed = framed_paths.concat();
    let near = view::NearMask::new(&whole, o.barrier_radius);
    let yaw = side_yaw(&framed);
    let backdrop = Backdrop::new(&whole, &framed, yaw, o.slice);
    let single = runs.len() == 1;
    let speed = if single {
        let v = speeds(&runs[0].traj);
        let hi = v.iter().cloned().fold(0.0, f64::max);
        Some((0.0, hi.max(0.1)))
    } else {
        None
    };
    let base_style = || {
        let style = if o.collision { WorldStyle::new().collision() } else { WorldStyle::new() };
        if o.barriers {
            style.show_barriers()
        } else {
            style.solid_barriers().barriers_where(|p, _| near.contains(p))
        }
    };
    let panel = |cam: &Camera, name: &str, style: WorldStyle| -> Image {
        let mut f = scene.frame(cam, o.width, o.height);
        f.style(style);
        if let Some(c) = c {
            course(&mut f, scene.bounds(), c);
        }
        if runs.len() == 2 {
            spokes(&mut f, &runs[0].traj, &runs[1].traj, 20, WHITE, o.ticks);
        }
        for (i, r) in runs.iter().enumerate() {
            let mut st = RunStyle::new(if single { WHITE } else { series(i) });
            st.by_speed = speed;
            st.l_step = o.l_step;
            // paths of a plan share most of their ticks; one set of labels will do
            st.tick_labels = i == 0 || runs.len() <= 2;
            run(&mut f, &r.traj, &st);
            if let Some(rep) = &r.report {
                report_failures(&mut f, &r.traj, rep);
            }
        }
        // the tightest clearance over all the runs
        let tightest = runs
            .iter()
            .filter_map(|r| Some((r, r.report.as_ref()?, r.report.as_ref()?.clearance.as_ref()?.min)))
            .min_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((r, rep, _)) = tightest {
            clearance(&mut f, &r.traj, rep);
        }
        tools::rulers(&mut f);
        tools::compass(&mut f, None);
        tools::title(&mut f, name);
        if let Some((lo, hi)) = speed {
            tools::colorbar(&mut f, &Ramp::magnitude(), lo, hi, "b/tick");
        } else {
            let legend: Vec<(&str, Color)> = runs
                .iter()
                .enumerate()
                .map(|(i, r)| (r.name.as_str(), series(i)))
                .collect();
            tools::legend(&mut f, &legend);
        }
        f.finish()
    };
    let view_of = |cam: &Camera| scene.frame(cam, o.width, o.height).view().clone();
    let top = Camera::top(&framed);
    let top_cut = TubeCut::new(&view_of(&top), &whole, o.radius, o.headroom);
    let persp = tools::readable_view(scene, &framed_paths, o.width, o.height, 50.0);
    let persp_cut = TubeCut::new(&view_of(&persp), &whole, o.radius, o.headroom);
    // the text gets its own strip, so it can't cover the start of a run
    let lines: Vec<String> = runs.iter().map(summary).collect();
    let hidden = if o.barriers {
        "blocks in front of the path are hidden, but still collide".to_string()
    } else {
        format!(
            "blocks in front of the path and barriers over {} blocks from it are hidden, but still collide",
            o.barrier_radius
        )
    };
    let text = format!("{}\n{}\n{hidden}", lines.join("\n"), EVENTS_LEGEND);
    let header = {
        let height = text.lines().count() as u32 * 17 + 14;
        let mut f = scene.frame(&Camera::look_at([0.0; 3], [0.0, 0.0, 1.0]), o.width, height);
        f.style(WorldStyle::new().hidden().background(INK));
        f.samples(1);
        f.text([6.0, 6.0], text, TextStyle::default().size(13.0).background(None));
        f.finish()
    };
    let images = [
        header,
        panel(&top, "top", base_style().keep(|p, _| top_cut.keeps(p))),
        panel(
            &Camera::side(&framed, yaw),
            &format!("side, looking along yaw {yaw:.0}"),
            base_style().keep(|p, _| backdrop.keeps(p)),
        ),
        panel(&persp, "perspective", base_style().keep(|p, _| persp_cut.keeps(p))),
    ];
    Image::grid(&images, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_of_a_recorded_run_without_a_jar() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../courses/burrowed_grove");
        let (c, w) = Course::load(Path::new(dir)).unwrap();
        let run = Path::new(dir).join("runs/recorded.traj.jsonl");
        let mut runs = load_runs(&run, "recorded", &c, &w).unwrap();
        assert_eq!(runs.len(), 1);
        runs[0].report = Some(crate::verify::verify(
            &c,
            &w,
            crate::verify::Plan::Trajectory(&runs[0].traj),
        ));
        let scene = Scene::with_jar(&w, None).unwrap();
        let o = SheetOptions {
            width: 300,
            height: 200,
            ..Default::default()
        };
        let img = sheet(&scene, Some(&c), &runs, &o);
        assert_eq!(img.w, 300);
        // three panels and a header
        assert!(img.h > 600);
    }

    /// One airborne tick from `v` at the origin of a world with stone at `wall_z` (if any).
    fn one_tick(v: [f64; 3], wall_z: Option<i32>) -> Trajectory {
        let at = |x, y, z| world::BlockPos { x, y, z };
        let mut w = World::air(at(-16, 0, -16), at(16, 31, 16));
        if let Some(z) = wall_z {
            for x in -16..=16 {
                for y in 0..=31 {
                    w.set(at(x, y, z), "minecraft:stone");
                }
            }
        }
        let mut p = port::LocalPlayer::new(port::Vec3::new(0.5, 15.0, 0.5), 0.0, 0.0);
        p.set_delta_movement(port::Vec3::new(v[0], v[1], v[2]));
        let initial = crate::state::FullState::from(&p);
        let input = crate::trajectory::InputRow {
            yaw: 0.0,
            pitch: 0.0,
            keys: "-".into(),
        };
        port::tick(&mut p, &input.tick_input().unwrap(), &w).unwrap();
        Trajectory {
            header: crate::trajectory::Header::new(None, "test", "test"),
            initial,
            arrivals: Vec::new(),
            rows: vec![crate::trajectory::Row {
                t: 1,
                input,
                state: crate::state::FullState::from(&p),
            }],
        }
    }

    /// The axes each leg of the tick moved along, in order.
    fn legs(t: &Trajectory) -> Vec<usize> {
        let l = l_step_points(t, 0, 1);
        assert_eq!(l.last().unwrap().0, t.rows[0].state.position, "ends exactly");
        l.windows(2)
            .map(|w| (0..3).find(|&k| w[0].0[k] != w[1].0[k]).unwrap())
            .collect()
    }

    #[test]
    fn l_step_goes_y_then_the_larger_horizontal_axis() {
        assert_eq!(legs(&one_tick([0.8, -0.3, 0.4], None)), [1, 0, 2]);
        assert_eq!(legs(&one_tick([0.4, -0.3, 0.8], None)), [1, 2, 0]);
        // a tie goes to x, as `axis_step_order` has it
        assert_eq!(legs(&one_tick([0.5, -0.3, 0.5], None)), [1, 0, 2]);
    }

    /// z is the larger axis asked for, so it goes first, though the wall leaves x the
    /// larger axis actually moved. Compare port's `axis_step_order`.
    #[test]
    fn l_step_order_comes_from_the_requested_move_at_a_wall() {
        let t = one_tick([0.4, -0.3, 0.9], Some(1));
        let s = &t.rows[0].state;
        assert!(s.horizontal_collision);
        let (dx, dz) = (s.position[0] - 0.5, s.position[2] - 0.5);
        assert!(dx.abs() > dz.abs(), "the wall flips which looks larger: {dx} {dz}");
        assert_eq!(legs(&t), [1, 2, 0]);
    }

    #[test]
    fn backdrop_never_hides_a_pass_behind_a_nearer_one() {
        // two passes at the same x, at different heights and depths, looking along z
        let pts = vec![[0.5, 10.5, 0.5], [0.5, 50.5, 20.5]];
        let b = Backdrop::new(&pts, &pts, 180.0, 40.0);
        let at = |x, y, z| world::BlockPos { x, y, z };
        // yaw 180 looks toward -z, so larger z is nearer the camera
        assert!(!b.keeps(at(0, 10, 3)), "in front of the low pass");
        assert!(b.keeps(at(0, 10, -3)), "behind the low pass");
        assert!(!b.keeps(at(0, 50, 23)), "in front of the high pass");
        assert!(b.keeps(at(0, 50, 15)), "behind the high pass");
    }
}
