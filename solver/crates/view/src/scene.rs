//! The world as the tracer sees it, and the tracer: a ray walks the cell grid with
//! Amanatides-Woo, and tests the baked quads of each non-empty cell.

use std::path::Path;

use world::{BlockPos, Cell, SectionView, World};

use crate::assets::{Baked, Baker, Fluid, NO_DIR, Quad, Texture, find_jar};
use crate::style::{Geometry, WorldStyle};

const UNKNOWN: u16 = u16::MAX;

enum Sec {
    Uniform(u16),
    Cells(Box<[u16; 4096]>),
}

struct Grid {
    min: [i32; 3],
    dim: [i32; 3],
    /// Section slot by grid position; slot 0 is unknown.
    index: Vec<u32>,
    secs: Vec<Sec>,
}

impl Grid {
    #[inline]
    fn slot(&self, s: [i32; 3]) -> usize {
        let (x, y, z) = (s[0] - self.min[0], s[1] - self.min[1], s[2] - self.min[2]);
        if x < 0 || y < 0 || z < 0 || x >= self.dim[0] || y >= self.dim[1] || z >= self.dim[2] {
            return 0;
        }
        self.index[((y * self.dim[2] + z) * self.dim[0] + x) as usize] as usize
    }

    #[inline]
    fn in_slot(&self, slot: usize, x: i32, y: i32, z: i32) -> u16 {
        match &self.secs[slot] {
            Sec::Uniform(v) => *v,
            Sec::Cells(c) => c[(((y & 15) << 8) | ((z & 15) << 4) | (x & 15)) as usize],
        }
    }

    #[inline]
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        self.in_slot(self.slot([x >> 4, y >> 4, z >> 4]), x, y, z)
    }
}

/// A world prepared for rendering. Reusable across frames.
pub struct Scene<'w> {
    pub(crate) world: &'w World,
    grid: Grid,
    baked: Vec<Baked>,
    /// Per state: has something to draw, by geometry mode.
    drawn_visual: Vec<bool>,
    drawn_collision: Vec<bool>,
    any_tall: bool,
    textures: Vec<Texture>,
    water: Option<u32>,
    lava: Option<u32>,
    /// World-space bounds of the stored sections.
    pub(crate) bounds: ([f64; 3], [f64; 3]),
    warnings: Vec<String>,
}

impl<'w> Scene<'w> {
    /// Finds the client jar for textures (`$ETAS_MC_JAR`, then the known paths). With no
    /// jar it warns on stderr and draws flat-colored collision boxes.
    pub fn new(world: &'w World) -> Result<Scene<'w>, String> {
        let jar = find_jar();
        if jar.is_none() {
            eprintln!(
                "view: no client jar found (set $ETAS_MC_JAR); drawing flat-colored collision boxes"
            );
        }
        Self::with_jar(world, jar.as_deref())
    }

    pub fn with_jar(world: &'w World, jar: Option<&Path>) -> Result<Scene<'w>, String> {
        let mut baker = Baker::open(jar)?;
        let n_states = world::block_table_text()
            .lines()
            .filter(|l| !l.starts_with('#'))
            .count();
        // the grid
        let (mut lo, mut hi) = ([i32::MAX; 3], [i32::MIN; 3]);
        let mut any = false;
        for (k, v) in world.sections() {
            if matches!(v, SectionView::Unknown) {
                continue;
            }
            any = true;
            for i in 0..3 {
                lo[i] = lo[i].min(k[i]);
                hi[i] = hi[i].max(k[i]);
            }
        }
        if !any {
            lo = [0; 3];
            hi = [0; 3];
        }
        let dim = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];
        let mut grid = Grid {
            min: lo,
            dim,
            index: vec![0; (dim[0] * dim[1] * dim[2]) as usize],
            secs: vec![Sec::Uniform(UNKNOWN)],
        };
        let mut used = vec![false; n_states];
        for (k, v) in world.sections() {
            let s = match v {
                SectionView::Unknown => continue,
                SectionView::Uniform(id) => {
                    used[id as usize] = true;
                    Sec::Uniform(id as u16)
                }
                SectionView::Cells(c) => {
                    for &id in c.iter() {
                        if id != UNKNOWN {
                            used[id as usize] = true;
                        }
                    }
                    Sec::Cells(Box::new(*c))
                }
            };
            let (x, y, z) = (k[0] - lo[0], k[1] - lo[1], k[2] - lo[2]);
            grid.index[((y * dim[2] + z) * dim[0] + x) as usize] = grid.secs.len() as u32;
            grid.secs.push(s);
        }
        // the states it uses
        let mut baked = vec![Baked::default(); n_states];
        let mut drawn_visual = vec![false; n_states];
        let mut drawn_collision = vec![false; n_states];
        let mut any_tall = false;
        for id in 0..n_states {
            if !used[id] {
                continue;
            }
            let b = baker.bake(world, id as u32);
            drawn_visual[id] = !b.quads.is_empty() || b.fluid != Fluid::None || b.barrier;
            drawn_collision[id] = !b.boxes.is_empty() || b.fluid != Fluid::None;
            any_tall |= b.tall;
            baked[id] = b;
        }
        let mut warnings = Vec::new();
        if !baker.failed.is_empty() {
            let n = baker.failed.len();
            let list = baker.failed.iter().take(5).cloned().collect::<Vec<_>>();
            warnings.push(format!(
                "{n} block states with no readable model, drawn as collision boxes: {}{}",
                list.join(" "),
                if n > 5 { " ..." } else { "" }
            ));
        }
        let bounds = (
            [
                lo[0] as f64 * 16.0,
                lo[1] as f64 * 16.0,
                lo[2] as f64 * 16.0,
            ],
            [
                (hi[0] + 1) as f64 * 16.0,
                (hi[1] + 1) as f64 * 16.0,
                (hi[2] + 1) as f64 * 16.0,
            ],
        );
        Ok(Scene {
            world,
            grid,
            baked,
            drawn_visual,
            drawn_collision,
            any_tall,
            water: baker.water,
            lava: baker.lava,
            textures: std::mem::take(&mut baker.textures),
            bounds,
            warnings,
        })
    }

    /// What baking couldn't do; empty if all went well.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// World-space bounds of the stored sections: (min, max).
    pub fn bounds(&self) -> ([f64; 3], [f64; 3]) {
        self.bounds
    }

    /// The first opaque thing along a ray in a style's world, for finding where to put
    /// an annotation or whether a point can be seen.
    pub fn raycast(
        &self,
        style: &WorldStyle,
        origin: [f64; 3],
        dir: [f64; 3],
        max: f64,
    ) -> Option<[f64; 3]> {
        let tr = Tracer::new(self, style, 0.0, 0.0);
        let l = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        let d = [dir[0] / l, dir[1] / l, dir[2] / l];
        tr.first_hit(origin, d, max)
            .map(|t| [origin[0] + d[0] * t, origin[1] + d[1] * t, origin[2] + d[2] * t])
    }

    /// The cell at `p`, as the world file has it.
    pub fn cell(&self, p: BlockPos) -> Cell {
        self.world.cell(p)
    }
}

/// Sun direction, toward the sun. Slightly off the axes so no face is exactly edge-on.
const SUN: [f64; 3] = [0.42, 0.82, 0.38];
const SHADOW: f32 = 0.68;
/// Collision edges stop being drawn when a sample covers this many blocks, or far blocks
/// would be all edge.
const EDGE_FOOT: f32 = 0.08;

#[inline]
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

#[inline]
fn dotf(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) struct Tracer<'a> {
    s: &'a Scene<'a>,
    style: &'a WorldStyle<'a>,
    collision: bool,
    drawn: &'a [bool],
    /// Sample footprint in blocks: `foot_z * distance + foot_0`.
    foot_z: f32,
    foot_0: f32,
    sky: [f32; 3],
    below_tall: bool,
}

/// A local hit from `hit_quads`.
struct Hit {
    t: f32,
    rgba: [f32; 4],
    q: usize,
    a: f32,
    b: f32,
}

impl<'a> Tracer<'a> {
    pub fn new(
        s: &'a Scene<'a>,
        style: &'a WorldStyle<'a>,
        foot_z: f32,
        foot_0: f32,
    ) -> Tracer<'a> {
        let collision = style.geometry == Geometry::Collision;
        let bg = style.background;
        Tracer {
            s,
            style,
            collision,
            drawn: if collision {
                &s.drawn_collision
            } else {
                &s.drawn_visual
            },
            foot_z,
            foot_0,
            sky: [bg.r, bg.g, bg.b],
            below_tall: collision && s.any_tall,
        }
    }

    #[inline]
    fn kept(&self, x: i32, y: i32, z: i32, st: u16) -> bool {
        if self.style.barriers.is_none() && self.s.baked[st as usize].barrier {
            return false;
        }
        match &self.style.keep {
            Some(k) => k(BlockPos { x, y, z }, self.s.world.info(st as u32)),
            None => true,
        }
    }

    /// The ray's interval inside the grid and the clips. The bool: t0 was set by a clip.
    #[inline]
    fn interval(&self, o: [f64; 3], d: [f64; 3], tmax: f64) -> Option<(f64, f64, bool, usize)> {
        let (lo, hi) = self.s.bounds;
        let (mut t0, mut t1) = (0.0f64, tmax);
        let mut axis = 3usize;
        for k in 0..3 {
            if d[k].abs() < 1e-12 {
                if o[k] < lo[k] || o[k] > hi[k] {
                    return None;
                }
                continue;
            }
            let (a, b) = ((lo[k] - o[k]) / d[k], (hi[k] - o[k]) / d[k]);
            let (a, b) = if a < b { (a, b) } else { (b, a) };
            if a > t0 {
                t0 = a;
                axis = k;
            }
            t1 = t1.min(b);
        }
        let mut clipped = false;
        for (n, c) in &self.style.clips {
            let dn = n[0] * d[0] + n[1] * d[1] + n[2] * d[2];
            let on = n[0] * o[0] + n[1] * o[1] + n[2] * o[2];
            if dn.abs() < 1e-12 {
                if on > *c {
                    return None;
                }
                continue;
            }
            let t = (c - on) / dn;
            if dn < 0.0 {
                // entering the kept side
                if t > t0 {
                    t0 = t;
                    clipped = true;
                }
            } else {
                t1 = t1.min(t);
            }
        }
        (t0 < t1).then_some((t0, t1, clipped, axis))
    }

    /// The nearest face a ray meets inside a cell, from local `t0` to `t1`.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    fn hit_quads(
        &self,
        quads: &[Quad],
        lo: [f32; 3],
        dl: [f32; 3],
        t0: f32,
        t1: f32,
        lod: usize,
        cell: [i32; 3],
        st: u16,
    ) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        let mut bt = t1 + 1e-4;
        for (i, q) in quads.iter().enumerate() {
            let denom = dotf(dl, q.n);
            if denom > -1e-6 {
                continue;
            }
            let rel = [q.p[0] - lo[0], q.p[1] - lo[1], q.p[2] - lo[2]];
            let t = dotf(rel, q.n) / denom;
            if t < t0 - 1e-5 || t > bt {
                continue;
            }
            let qr = [
                lo[0] + dl[0] * t - q.p[0],
                lo[1] + dl[1] * t - q.p[1],
                lo[2] + dl[2] * t - q.p[2],
            ];
            let a = dotf(qr, q.ga);
            let b = dotf(qr, q.gb);
            if !(-1e-4..=1.0001).contains(&a) || !(-1e-4..=1.0001).contains(&b) {
                continue;
            }
            let rgba = if q.tex == u32::MAX {
                [q.color[0], q.color[1], q.color[2], 1.0]
            } else {
                let tex = &self.s.textures[q.tex as usize];
                let u = q.uv0[0] + q.du[0] * a + q.dv[0] * b;
                let v = q.uv0[1] + q.du[1] * a + q.dv[1] * b;
                let c = tex.sample(u, v, if tex.transparent { lod.min(1) } else { lod });
                if tex.transparent {
                    // a transparent face against a same-state neighbor is hidden (glass)
                    if q.cull < 6 {
                        let (dk, ds) = (
                            (q.cull / 2) as usize,
                            if q.cull.is_multiple_of(2) { -1 } else { 1 },
                        );
                        let mut nc = cell;
                        nc[dk] += ds;
                        if self.s.grid.get(nc[0], nc[1], nc[2]) == st {
                            continue;
                        }
                    }
                    let cut = if tex.translucent { 0.02 } else { 0.5 };
                    if c[3] < cut {
                        continue;
                    }
                }
                let a2 = if tex.transparent && !tex.translucent {
                    // cutout: opaque where it passes
                    1.0 / c[3].max(1e-3)
                } else {
                    1.0
                };
                [
                    c[0] * a2 * q.color[0],
                    c[1] * a2 * q.color[1],
                    c[2] * a2 * q.color[2],
                    if a2 != 1.0 { 1.0 } else { c[3] },
                ]
            };
            bt = t + 1e-4;
            best = Some(Hit { t, rgba, q: i, a, b });
        }
        best
    }

    #[inline]
    fn occludes(&self, x: i32, y: i32, z: i32) -> bool {
        let st = self.s.grid.get(x, y, z);
        st != UNKNOWN && self.s.baked[st as usize].full_opaque && self.kept(x, y, z, st)
    }

    /// Vanilla-style smooth ambient occlusion at a face point.
    fn ambient(&self, p: [f64; 3], dir: u8) -> f32 {
        let k = (dir / 2) as usize;
        let s = if dir.is_multiple_of(2) { -1.0 } else { 1.0 };
        let mut c = p;
        c[k] += s * 1e-3;
        let o = [c[0].floor() as i32, c[1].floor() as i32, c[2].floor() as i32];
        let (i, j) = ((k + 1) % 3, (k + 2) % 3);
        let at = |di: i32, dj: i32| {
            let mut q = o;
            q[i] += di;
            q[j] += dj;
            self.occludes(q[0], q[1], q[2])
        };
        let light = |b: bool| if b { 0.2f32 } else { 1.0 };
        let corner = |si: i32, sj: i32| {
            let (a, b) = (at(si, 0), at(0, sj));
            let c = (a && b) || at(si, sj);
            (1.0 + light(a) + light(b) + light(c)) * 0.25
        };
        let (fi, fj) = (
            (c[i] - o[i] as f64) as f32,
            (c[j] - o[j] as f64) as f32,
        );
        let (c00, c10, c01, c11) = (
            corner(-1, -1),
            corner(1, -1),
            corner(-1, 1),
            corner(1, 1),
        );
        let a = c00 + (c10 - c00) * fi;
        let b = c01 + (c11 - c01) * fi;
        a + (b - a) * fj
    }

    /// Whether anything opaque lies toward the sun from `p`.
    fn shadowed(&self, p: [f64; 3]) -> bool {
        self.first_hit(p, SUN, 96.0).is_some()
    }

    /// The first opaque or cutout thing a ray meets, in this style's world.
    pub fn first_hit(&self, o: [f64; 3], d: [f64; 3], tmax: f64) -> Option<f64> {
        let (t0, t1, _, _) = self.interval(o, d, tmax)?;
        let mut c = [0i32; 3];
        let mut next = [f64::INFINITY; 3];
        let mut delta = [f64::INFINITY; 3];
        let mut step = [0i32; 3];
        for k in 0..3 {
            let p = o[k] + d[k] * (t0 + 1e-9);
            c[k] = p.floor() as i32;
            if d[k] > 1e-12 {
                step[k] = 1;
                delta[k] = 1.0 / d[k];
                next[k] = (c[k] as f64 + 1.0 - o[k]) / d[k];
            } else if d[k] < -1e-12 {
                step[k] = -1;
                delta[k] = -1.0 / d[k];
                next[k] = (c[k] as f64 - o[k]) / d[k];
            }
        }
        let mut t = t0;
        let df = d.map(|v| v as f32);
        loop {
            let tn = next[0].min(next[1]).min(next[2]).min(t1);
            let st = self.s.grid.get(c[0], c[1], c[2]);
            if st != UNKNOWN && self.drawn[st as usize] {
                let b = &self.s.baked[st as usize];
                if b.full_opaque && !self.collision {
                    if self.kept(c[0], c[1], c[2], st) {
                        return Some(t);
                    }
                } else if self.kept(c[0], c[1], c[2], st) {
                    let quads = if self.collision { &b.boxes } else { &b.quads };
                    if !quads.is_empty() {
                        let lo = self.local(o, d, t, c, st, b);
                        if let Some(h) =
                            self.hit_quads(quads, lo, df, 0.0, (tn - t) as f32, 0, c, st)
                            && h.rgba[3] > 0.9
                        {
                            return Some(t + h.t as f64);
                        }
                    }
                }
            }
            if self.below_tall {
                let stb = self.s.grid.get(c[0], c[1] - 1, c[2]);
                if stb != UNKNOWN
                    && self.s.baked[stb as usize].tall
                    && self.kept(c[0], c[1] - 1, c[2], stb)
                {
                    let b = &self.s.baked[stb as usize];
                    let below = [c[0], c[1] - 1, c[2]];
                    let lo = self.local(o, d, t, below, stb, b);
                    if let Some(h) =
                        self.hit_quads(&b.boxes, lo, df, 0.0, (tn - t) as f32, 0, below, stb)
                    {
                        return Some(t + h.t as f64);
                    }
                }
            }
            if tn >= t1 {
                return None;
            }
            let k = if next[0] <= next[1] && next[0] <= next[2] {
                0
            } else if next[1] <= next[2] {
                1
            } else {
                2
            };
            t = next[k];
            next[k] += delta[k];
            c[k] += step[k];
        }
    }

    #[inline]
    fn local(&self, o: [f64; 3], d: [f64; 3], t: f64, c: [i32; 3], st: u16, b: &Baked) -> [f32; 3] {
        let mut lo = [
            (o[0] + d[0] * t - c[0] as f64) as f32,
            (o[1] + d[1] * t - c[1] as f64) as f32,
            (o[2] + d[2] * t - c[2] as f64) as f32,
        ];
        if b.offset {
            let off = self.s.world.offset(
                st as u32,
                BlockPos {
                    x: c[0],
                    y: c[1],
                    z: c[2],
                },
            );
            for k in 0..3 {
                lo[k] -= off[k] as f32;
            }
        }
        lo
    }

    fn finish_color(&self, c: [f32; 3], cell: [i32; 3], st: u16) -> [f32; 3] {
        let mut c = c;
        if let Some(f) = &self.style.tint {
            let p = BlockPos {
                x: cell[0],
                y: cell[1],
                z: cell[2],
            };
            if let Some(t) = f(p, self.s.world.info(st as u32)) {
                c = mix3(c, [t.r, t.g, t.b], t.a);
            }
        }
        c
    }

    /// Lights a face hit: its directional shade, ambient occlusion and sun shadow.
    fn light(&self, q: &Quad, p: [f64; 3]) -> f32 {
        let mut l = q.shade;
        if q.ao && q.dir != NO_DIR {
            l *= self.ambient(p, q.dir);
        }
        if self.style.shadows {
            let n = [q.n[0] as f64, q.n[1] as f64, q.n[2] as f64];
            let lit = n[0] * SUN[0] + n[1] * SUN[1] + n[2] * SUN[2];
            if lit > 0.0 {
                let from = [
                    p[0] + n[0] * 2e-3,
                    p[1] + n[1] * 2e-3,
                    p[2] + n[2] * 2e-3,
                ];
                if self.shadowed(from) {
                    l *= SHADOW;
                }
            } else {
                l *= SHADOW + (1.0 - SHADOW) * 0.4;
            }
        }
        l
    }

    /// Traces one ray: (color, distance to the first opaque thing or infinity).
    pub fn trace(&self, o: [f64; 3], d: [f64; 3]) -> ([f32; 3], f32) {
        let style = self.style;
        let Some((t0, t1, clipped, entry_axis)) = self.interval(o, d, 1e7) else {
            return (self.sky, f32::INFINITY);
        };
        let mut acc = [0.0f32; 3];
        let mut alpha = 0.0f32;
        let mut depth = f32::INFINITY;
        let mut c = [0i32; 3];
        let mut next = [f64::INFINITY; 3];
        let mut delta = [f64::INFINITY; 3];
        let mut step = [0i32; 3];
        for k in 0..3 {
            let p = o[k] + d[k] * (t0 + 1e-9);
            c[k] = p.floor() as i32;
            if d[k] > 1e-12 {
                step[k] = 1;
                delta[k] = 1.0 / d[k];
                next[k] = (c[k] as f64 + 1.0 - o[k]) / d[k];
            } else if d[k] < -1e-12 {
                step[k] = -1;
                delta[k] = -1.0 / d[k];
                next[k] = (c[k] as f64 - o[k]) / d[k];
            }
        }
        let df = d.map(|v| v as f32);
        let mut t = t0;
        // the axis the ray entered the current cell by, 3 if it started inside
        let mut axis = entry_axis;
        let mut first = true;
        let mut in_fluid = false;
        let mut prev_st = 0u16;
        let mut sec = [i32::MAX; 3];
        let mut slot = 0usize;
        let mut done = false;
        while !done {
            let tn = next[0].min(next[1]).min(next[2]).min(t1);
            let s = [c[0] >> 4, c[1] >> 4, c[2] >> 4];
            if s != sec {
                sec = s;
                slot = self.s.grid.slot(s);
            }
            let st = self.s.grid.in_slot(slot, c[0], c[1], c[2]);
            if st == UNKNOWN {
                in_fluid = false;
                if let Some(u) = style.unknown {
                    // per-block opacity, whatever length of the cell the ray crosses
                    let a = u.a * (1.0 - alpha);
                    acc[0] += u.r * a;
                    acc[1] += u.g * a;
                    acc[2] += u.b * a;
                    alpha += a;
                }
            } else {
                let b = &self.s.baked[st as usize];
                let drawn = self.drawn[st as usize] && self.kept(c[0], c[1], c[2], st);
                if !drawn {
                    in_fluid = false;
                }
                if drawn {
                    // a clip plane cutting a solid block shows a hatched face
                    if first && clipped {
                        let p = [o[0] + d[0] * t0, o[1] + d[1] * t0, o[2] + d[2] * t0];
                        let inside = self.s.world.boxes_at(st as u32, BlockPos {
                            x: c[0],
                            y: c[1],
                            z: c[2],
                        });
                        if inside.iter().any(|bx| (0..3).all(|k| bx.min[k] - 1e-9 <= p[k] && p[k] <= bx.max[k] + 1e-9)) {
                            let band = ((p[0] + p[1] + p[2]) * 2.5).rem_euclid(1.0) < 0.22;
                            let k = if band { 0.42 } else { 0.72 };
                            let col = b.avg.map(|v| v * k);
                            let col = self.finish_color(col, c, st);
                            let a = 1.0 - alpha;
                            for i in 0..3 {
                                acc[i] += col[i] * a;
                            }
                            alpha = 1.0;
                            depth = t0 as f32;
                            break;
                        }
                    }
                    let foot = self.foot_z * t as f32 + self.foot_0;
                    let lod = (foot * 16.0).max(1.0).log2() as usize;
                    // fluid volume
                    if b.fluid != Fluid::None {
                        let h = if self.is_fluid(c[0], c[1] + 1, c[2]) {
                            1.0
                        } else {
                            fluid_height(&self.s.world.info(st as u32).state)
                        };
                        let lo = [
                            (o[0] + d[0] * t - c[0] as f64) as f32,
                            (o[1] + d[1] * t - c[1] as f64) as f32,
                            (o[2] + d[2] * t - c[2] as f64) as f32,
                        ];
                        let tc = (tn - t) as f32;
                        // entering the box [0,h] in y
                        let (te, tx) = if df[1].abs() < 1e-6 {
                            if lo[1] < h { (0.0, tc) } else { (tc + 1.0, tc) }
                        } else {
                            let (a, b) = ((0.0 - lo[1]) / df[1], (h - lo[1]) / df[1]);
                            let (a, b) = if a < b { (a, b) } else { (b, a) };
                            (a.max(0.0), b.min(tc))
                        };
                        if te <= tx {
                            if !in_fluid {
                                let (col, a) = self.fluid_color(b.fluid, lo, df, te, h, lod);
                                let a2 = a * (1.0 - alpha);
                                for i in 0..3 {
                                    acc[i] += col[i] * a2;
                                }
                                alpha += a2;
                                if a >= 0.99 && depth.is_infinite() {
                                    depth = (t + te as f64) as f32;
                                }
                            }
                            in_fluid = tx >= tc - 1e-4;
                        } else {
                            in_fluid = false;
                        }
                    } else {
                        in_fluid = false;
                    }
                    if alpha < 0.995 {
                        // a barrier: its entry face, unless the ray came through a barrier
                        if b.barrier && !self.collision {
                            if let Some(col) = style.barriers
                                && prev_st != st
                                && axis < 3
                            {
                                let dir = axis * 2 + (step[axis] < 0) as usize;
                                let sh = [0.6, 0.6, 0.5, 1.0, 0.8, 0.8][dir];
                                let a = col.a * (1.0 - alpha);
                                acc[0] += col.r * sh * a;
                                acc[1] += col.g * sh * a;
                                acc[2] += col.b * sh * a;
                                alpha += a;
                            }
                        } else if !self.collision
                            && b.cube.is_some()
                            && axis < 3
                            && !first
                        {
                            // plain opaque cube: the entry face is the hit
                            let dir = axis * 2 + (step[axis] < 0) as usize;
                            let q = &b.quads[b.cube.unwrap()[dir] as usize];
                            let p = [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t];
                            let lo = [
                                (p[0] - c[0] as f64) as f32 - q.p[0],
                                (p[1] - c[1] as f64) as f32 - q.p[1],
                                (p[2] - c[2] as f64) as f32 - q.p[2],
                            ];
                            let (a, bb) = (dotf(lo, q.ga), dotf(lo, q.gb));
                            let tx = if q.tex == u32::MAX {
                                [q.color[0], q.color[1], q.color[2], 1.0]
                            } else {
                                let tex = &self.s.textures[q.tex as usize];
                                let u = q.uv0[0] + q.du[0] * a + q.dv[0] * bb;
                                let v = q.uv0[1] + q.du[1] * a + q.dv[1] * bb;
                                let s = tex.sample(u, v, lod);
                                [s[0] * q.color[0], s[1] * q.color[1], s[2] * q.color[2], 1.0]
                            };
                            let l = self.light(q, p);
                            let col = self.finish_color([tx[0] * l, tx[1] * l, tx[2] * l], c, st);
                            let m = 1.0 - alpha;
                            for i in 0..3 {
                                acc[i] += col[i] * m;
                            }
                            alpha = 1.0;
                            if depth.is_infinite() {
                                depth = t as f32;
                            }
                            done = true;
                        } else {
                            // general: nearest face in this cell, repeated past translucent ones
                            let quads: &[Quad] = if self.collision { &b.boxes } else { &b.quads };
                            let lo = self.local(o, d, t, c, st, b);
                            let tc = (tn - t) as f32;
                            let mut from = 0.0f32;
                            for _ in 0..6 {
                                let Some(h) = self.hit_quads(quads, lo, df, from, tc, lod, c, st)
                                else {
                                    break;
                                };
                                let q = &quads[h.q];
                                let th = t + h.t as f64;
                                let p = [o[0] + d[0] * th, o[1] + d[1] * th, o[2] + d[2] * th];
                                let mut l = self.light(q, p);
                                if self.collision {
                                    let e = h.a.min(1.0 - h.a).min(h.b).min(1.0 - h.b);
                                    if foot < EDGE_FOOT && e < 0.012 + foot * 0.6 {
                                        l *= 0.45;
                                    }
                                }
                                let rgb = self.finish_color(
                                    [h.rgba[0] * l, h.rgba[1] * l, h.rgba[2] * l],
                                    c,
                                    st,
                                );
                                let m = 1.0 - alpha;
                                for i in 0..3 {
                                    acc[i] += rgb[i] * m;
                                }
                                alpha += h.rgba[3] * m;
                                if h.rgba[3] > 0.6 && depth.is_infinite() {
                                    depth = th as f32;
                                }
                                if alpha >= 0.995 {
                                    done = true;
                                    break;
                                }
                                from = h.t + 1e-3;
                            }
                        }
                        // a tall collision box reaching up from the cell below
                        if self.below_tall && !done {
                            let stb = self.s.grid.get(c[0], c[1] - 1, c[2]);
                            if stb != UNKNOWN
                                && self.s.baked[stb as usize].tall
                                && self.kept(c[0], c[1] - 1, c[2], stb)
                            {
                                done = self.tall(o, d, t, tn, c, stb, &mut acc, &mut alpha, &mut depth);
                            }
                        }
                    } else {
                        done = true;
                    }
                } else if self.below_tall {
                    let stb = self.s.grid.get(c[0], c[1] - 1, c[2]);
                    if stb != UNKNOWN
                        && self.s.baked[stb as usize].tall
                        && self.kept(c[0], c[1] - 1, c[2], stb)
                    {
                        done = self.tall(o, d, t, tn, c, stb, &mut acc, &mut alpha, &mut depth);
                    }
                }
                prev_st = st;
            }
            if alpha >= 0.995 {
                break;
            }
            if tn >= t1 {
                break;
            }
            let k = if next[0] <= next[1] && next[0] <= next[2] {
                0
            } else if next[1] <= next[2] {
                1
            } else {
                2
            };
            t = next[k];
            next[k] += delta[k];
            c[k] += step[k];
            axis = k;
            first = false;
        }
        // what's left shows the background
        let m = 1.0 - alpha;
        let mut col = [
            acc[0] + self.sky[0] * m,
            acc[1] + self.sky[1] * m,
            acc[2] + self.sky[2] * m,
        ];
        if style.desaturate > 0.0 && alpha > 0.0 {
            let y = col[0] * 0.3 + col[1] * 0.59 + col[2] * 0.11;
            col = mix3(col, [y, y, y], style.desaturate * alpha);
        }
        if let Some(f) = style.fog
            && depth.is_finite()
        {
            let x = depth as f64 / f;
            col = mix3(col, self.sky, 1.0 - (-x * x).exp() as f32);
        }
        (col, depth)
    }

    /// A tall collision box's faces in the cell above it. Returns true if the ray ended.
    #[allow(clippy::too_many_arguments)]
    fn tall(
        &self,
        o: [f64; 3],
        d: [f64; 3],
        t: f64,
        tn: f64,
        c: [i32; 3],
        stb: u16,
        acc: &mut [f32; 3],
        alpha: &mut f32,
        depth: &mut f32,
    ) -> bool {
        let b = &self.s.baked[stb as usize];
        let below = [c[0], c[1] - 1, c[2]];
        let lo = self.local(o, d, t, below, stb, b);
        let df = d.map(|v| v as f32);
        let Some(h) = self.hit_quads(&b.boxes, lo, df, 0.0, (tn - t) as f32, 0, below, stb) else {
            return false;
        };
        let th = t + h.t as f64;
        let p = [o[0] + d[0] * th, o[1] + d[1] * th, o[2] + d[2] * th];
        let q = &b.boxes[h.q];
        let mut l = self.light(q, p);
        let e = h.a.min(1.0 - h.a).min(h.b).min(1.0 - h.b);
        let foot = self.foot_z * th as f32 + self.foot_0;
        if foot < EDGE_FOOT && e < 0.012 + foot * 0.6 {
            l *= 0.45;
        }
        let m = 1.0 - *alpha;
        for i in 0..3 {
            acc[i] += h.rgba[i] * l * m;
        }
        *alpha = 1.0;
        if depth.is_infinite() {
            *depth = th as f32;
        }
        true
    }

    fn is_fluid(&self, x: i32, y: i32, z: i32) -> bool {
        let st = self.s.grid.get(x, y, z);
        st != UNKNOWN && self.s.baked[st as usize].fluid != Fluid::None
    }

    fn fluid_color(
        &self,
        f: Fluid,
        lo: [f32; 3],
        d: [f32; 3],
        te: f32,
        h: f32,
        lod: usize,
    ) -> ([f32; 3], f32) {
        let p = [lo[0] + d[0] * te, lo[1] + d[1] * te, lo[2] + d[2] * te];
        let top = (p[1] - h).abs() < 1e-3;
        let (tid, tint) = match f {
            Fluid::Water => (self.s.water, [0.247, 0.463, 0.894]),
            _ => (self.s.lava, [1.0, 1.0, 1.0]),
        };
        let shade = if top {
            1.0
        } else if p[2] < 1e-3 || p[2] > 0.999 {
            0.8
        } else {
            0.6
        };
        let alpha_default = if f == Fluid::Water { 0.65 } else { 1.0 };
        let Some(tid) = tid else {
            return (tint.map(|v| v * shade), alpha_default);
        };
        let tex = &self.s.textures[tid as usize];
        let (u, v) = if top {
            (p[0], p[2])
        } else if p[0] < 1e-3 || p[0] > 0.999 {
            (p[2], p[1])
        } else {
            (p[0], p[1])
        };
        let c = tex.sample(u, v, lod);
        let a = if f == Fluid::Lava { 1.0 } else { c[3].max(0.05) };
        let un = 1.0 / c[3].max(1e-3);
        (
            [
                c[0] * un * tint[0] * shade,
                c[1] * un * tint[1] * shade,
                c[2] * un * tint[2] * shade,
            ],
            a,
        )
    }
}

/// Fluid surface height in a cell, from the state's `level` (vanilla `FluidState.getHeight`).
fn fluid_height(state: &str) -> f32 {
    let lvl = state
        .split_once("level=")
        .and_then(|(_, r)| r.split([',', ']']).next())
        .and_then(|s| s.parse::<u32>().ok());
    match lvl {
        Some(l) if (1..8).contains(&l) => (8 - l) as f32 / 9.0,
        Some(l) if l >= 8 => 1.0,
        _ => 8.0 / 9.0,
    }
}
