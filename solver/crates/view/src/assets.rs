//! Reads block models and textures out of the client jar, and bakes the block states a
//! world uses into quads. The jar's assets are Mojang's, so they're read at runtime and
//! never committed.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;
use world::{Effect, World};

/// `$ETAS_MC_JAR`, then the loom cache `tools/block_dump.sh` uses, then a Modrinth copy.
pub fn find_jar() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ETAS_MC_JAR").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [
        ".gradle/caches/fabric-loom/26.2/minecraft-merged.jar",
        "Library/Application Support/ModrinthApp/meta/versions/26.2-0.19.5/26.2-0.19.5.jar",
    ]
    .iter()
    .map(|p| home.join(p))
    .find(|p| p.is_file())
}

/// One texture's first animation frame, with a box-filtered mip chain.
pub(crate) struct Texture {
    pub size: u32,
    /// Level 0 is `size` square; premultiplied RGBA, 0 to 1.
    pub mips: Vec<Vec<[f32; 4]>>,
    /// Holds alpha other than 1: must be tested, or blended.
    pub transparent: bool,
    /// Holds alpha strictly between 0 and 1, so it blends.
    pub translucent: bool,
    pub avg: [f32; 3],
}

impl Texture {
    fn new(size: u32, level0: Vec<[f32; 4]>) -> Texture {
        let transparent = level0.iter().any(|p| p[3] < 0.999);
        let translucent = level0.iter().any(|p| p[3] > 0.02 && p[3] < 0.98);
        let mut mips = vec![level0];
        let mut s = size as usize;
        while s > 1 {
            let prev = mips.last().unwrap();
            let n = s / 2;
            let mut next = vec![[0.0f32; 4]; n * n];
            for y in 0..n {
                for x in 0..n {
                    let mut c = [0.0f32; 4];
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let p = prev[(y * 2 + dy) * s + x * 2 + dx];
                        for k in 0..4 {
                            c[k] += p[k] * 0.25;
                        }
                    }
                    next[y * n + x] = c;
                }
            }
            mips.push(next);
            s = n;
        }
        let top = mips.last().unwrap()[0];
        let a = top[3].max(0.001);
        Texture {
            size,
            transparent,
            translucent,
            avg: [top[0] / a, top[1] / a, top[2] / a],
            mips,
        }
    }

    /// Nearest texel at mip `lod`, premultiplied.
    #[inline]
    pub fn sample(&self, u: f32, v: f32, lod: usize) -> [f32; 4] {
        let lod = lod.min(self.mips.len() - 1);
        let s = (self.size >> lod) as i32;
        let x = ((u * s as f32).floor() as i32).rem_euclid(s);
        let y = ((v * s as f32).floor() as i32).rem_euclid(s);
        self.mips[lod][(y * s + x) as usize]
    }
}

/// `Quad` faces carry this as their direction when they aren't axis-aligned.
pub(crate) const NO_DIR: u8 = 6;

/// One baked face, block-local (a cell is the unit cube).
#[derive(Clone)]
pub(crate) struct Quad {
    pub p: [f32; 3],
    pub n: [f32; 3],
    /// Reciprocal basis: `a = (q - p) . ga` and `b = (q - p) . gb`, both in 0 to 1 on the quad.
    pub ga: [f32; 3],
    pub gb: [f32; 3],
    pub uv0: [f32; 2],
    pub du: [f32; 2],
    pub dv: [f32; 2],
    /// Texture index, or `u32::MAX` for the flat `color`.
    pub tex: u32,
    /// Multiplied into the texture, or the flat color itself.
    pub color: [f32; 3],
    pub shade: f32,
    pub ao: bool,
    /// Face direction (0 -x, 1 +x, 2 -y, 3 +y, 4 -z, 5 +z), or [`NO_DIR`].
    pub dir: u8,
    /// Set if a same-state neighbor on this side hides the face (glass walls).
    pub cull: u8,
}

/// Everything the tracer needs about one block state.
#[derive(Clone, Default)]
pub(crate) struct Baked {
    pub quads: Vec<Quad>,
    /// Collision-box quads, for [`crate::Geometry::Collision`].
    pub boxes: Vec<Quad>,
    /// Fills the cell with opaque faces, so a hit on it ends the ray at the cell's entry
    /// and it occludes ambient light.
    pub full_opaque: bool,
    /// Index into `quads` per face direction, if the state is a plain opaque cube.
    pub cube: Option<[u16; 6]>,
    pub fluid: Fluid,
    pub barrier: bool,
    pub offset: bool,
    /// Average color, for cut faces.
    pub avg: [f32; 3],
    /// A collision box leaves the cell upward (fences, walls).
    pub tall: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Fluid {
    #[default]
    None,
    Water,
    Lava,
}

pub(crate) struct Baker {
    jar: Option<zip::ZipArchive<File>>,
    models: HashMap<String, Option<Value>>,
    states: HashMap<String, Option<Value>>,
    tex_ids: HashMap<String, u32>,
    pub textures: Vec<Texture>,
    pub water: Option<u32>,
    pub lava: Option<u32>,
    /// Block names with a model that couldn't be read.
    pub failed: Vec<String>,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-9);
    [a[0] / l, a[1] / l, a[2] / l]
}

/// Vanilla's per-face brightness, weighted by the normal for rotated elements.
fn face_shade(n: [f32; 3]) -> f32 {
    let (x, y, z) = (n[0], n[1], n[2]);
    x * x * 0.6 + z * z * 0.8 + y * y * if y > 0.0 { 1.0 } else { 0.5 }
}

const DIRS: [&str; 6] = ["west", "east", "down", "up", "north", "south"];

fn dir_of(name: &str) -> Option<usize> {
    let name = if name == "bottom" { "down" } else { name };
    DIRS.iter().position(|d| *d == name)
}

/// Vanilla `FaceInfo`: the corners of face `dir` on box `(a, b)`, in vertex order.
fn face_corners(dir: usize, a: [f32; 3], b: [f32; 3]) -> [[f32; 3]; 4] {
    match dir {
        2 => [
            [a[0], a[1], b[2]],
            [a[0], a[1], a[2]],
            [b[0], a[1], a[2]],
            [b[0], a[1], b[2]],
        ],
        3 => [
            [a[0], b[1], a[2]],
            [a[0], b[1], b[2]],
            [b[0], b[1], b[2]],
            [b[0], b[1], a[2]],
        ],
        4 => [
            [b[0], b[1], a[2]],
            [b[0], a[1], a[2]],
            [a[0], a[1], a[2]],
            [a[0], b[1], a[2]],
        ],
        5 => [
            [a[0], b[1], b[2]],
            [a[0], a[1], b[2]],
            [b[0], a[1], b[2]],
            [b[0], b[1], b[2]],
        ],
        0 => [
            [a[0], b[1], a[2]],
            [a[0], a[1], a[2]],
            [a[0], a[1], b[2]],
            [a[0], b[1], b[2]],
        ],
        _ => [
            [b[0], b[1], b[2]],
            [b[0], a[1], b[2]],
            [b[0], a[1], a[2]],
            [b[0], b[1], a[2]],
        ],
    }
}

/// Vanilla `FaceBakery.defaultFaceUV`, on a 0-to-1 box: (u0, v0, u1, v1).
fn default_uv(dir: usize, a: [f32; 3], b: [f32; 3]) -> [f32; 4] {
    match dir {
        2 => [a[0], 1.0 - b[2], b[0], 1.0 - a[2]],
        3 => [a[0], a[2], b[0], b[2]],
        4 => [1.0 - b[0], 1.0 - b[1], 1.0 - a[0], 1.0 - a[1]],
        5 => [a[0], 1.0 - b[1], b[0], 1.0 - a[1]],
        0 => [a[2], 1.0 - b[1], b[2], 1.0 - a[1]],
        _ => [1.0 - b[2], 1.0 - b[1], 1.0 - a[2], 1.0 - a[1]],
    }
}

/// A constant for a tinted face. There's no biome data in a world file, so these are
/// plains' colors.
fn tint_of(block: &str) -> [f32; 3] {
    let n = block.trim_start_matches("minecraft:");
    let rgb = |v: u32| {
        [
            ((v >> 16) & 255) as f32 / 255.0,
            ((v >> 8) & 255) as f32 / 255.0,
            (v & 255) as f32 / 255.0,
        ]
    };
    if n == "birch_leaves" {
        rgb(0x80A755)
    } else if n == "spruce_leaves" {
        rgb(0x619961)
    } else if n == "lily_pad" {
        rgb(0x208030)
    } else if n == "redstone_wire" {
        rgb(0xB00000)
    } else if n.ends_with("_leaves") || n == "vine" {
        rgb(0x77AB2F)
    } else {
        rgb(0x91BD59)
    }
}

/// A rough color for a block with no texture source (no jar).
fn flat_color(block: &str) -> [f32; 3] {
    let n = block.trim_start_matches("minecraft:");
    let table: [(&str, u32); 12] = [
        ("grass", 0x7BA84F),
        ("leaves", 0x4F8A32),
        ("dirt", 0x866043),
        ("sand", 0xDBCFA3),
        ("log", 0x6B5335),
        ("planks", 0xA2834F),
        ("stone", 0x808080),
        ("deepslate", 0x505055),
        ("snow", 0xF0F4F8),
        ("ice", 0x9CC4F4),
        ("glass", 0xC8DCE4),
        ("wool", 0xC8C8C8),
    ];
    for (k, v) in table {
        if n.contains(k) {
            return [
                ((v >> 16) & 255) as f32 / 255.0,
                ((v >> 8) & 255) as f32 / 255.0,
                (v & 255) as f32 / 255.0,
            ];
        }
    }
    let h = n.bytes().fold(2166136261u32, |h, b| {
        (h ^ b as u32).wrapping_mul(16777619)
    });
    let c = |s: u32| 0.42 + ((h >> s) & 63) as f32 / 63.0 * 0.28;
    [c(0), c(6), c(12)]
}

#[allow(clippy::too_many_arguments)]
fn make_quad(
    c: [[f32; 3]; 4],
    uvs: [[f32; 2]; 4],
    tex: u32,
    color: [f32; 3],
    shade: Option<f32>,
    ao: bool,
    dir: u8,
    cull: u8,
) -> Option<Quad> {
    // parameter a runs v0 -> v3, b runs v0 -> v1
    let (ea, eb) = (sub(c[3], c[0]), sub(c[1], c[0]));
    let raw = cross(eb, ea);
    if dot(raw, raw) < 1e-12 {
        return None;
    }
    let n = normalize(raw);
    let (ca, cb) = (cross(eb, n), cross(n, ea));
    let (da, db) = (dot(ea, ca), dot(eb, cb));
    let ga = [ca[0] / da, ca[1] / da, ca[2] / da];
    let gb = [cb[0] / db, cb[1] / db, cb[2] / db];
    Some(Quad {
        p: c[0],
        n,
        ga,
        gb,
        uv0: uvs[0],
        du: [uvs[3][0] - uvs[0][0], uvs[3][1] - uvs[0][1]],
        dv: [uvs[1][0] - uvs[0][0], uvs[1][1] - uvs[0][1]],
        tex,
        color,
        shade: shade.unwrap_or_else(|| face_shade(n)),
        ao,
        dir,
        cull,
    })
}

/// The six faces of a box, textured with `tex`'s default projection or flat `color`.
fn box_quads(min: [f32; 3], max: [f32; 3], tex: u32, color: [f32; 3], out: &mut Vec<Quad>) {
    for dir in 0..6 {
        let c = face_corners(dir, min, max);
        let uv = default_uv(dir, min, max);
        let uvs = [
            [uv[0], uv[1]],
            [uv[0], uv[3]],
            [uv[2], uv[3]],
            [uv[2], uv[1]],
        ];
        if let Some(q) = make_quad(c, uvs, tex, color, None, true, dir as u8, 7) {
            out.push(q);
        }
    }
}

fn rot_y(p: [f32; 3], n: i32) -> [f32; 3] {
    // y=90 takes +x to +z, around the block's center
    let (mut x, mut z) = (p[0] - 0.5, p[2] - 0.5);
    for _ in 0..n {
        (x, z) = (-z, x);
    }
    [x + 0.5, p[1], z + 0.5]
}

fn rot_x(p: [f32; 3], n: i32) -> [f32; 3] {
    // x=90 takes -z to -y, around the block's center
    let (mut y, mut z) = (p[1] - 0.5, p[2] - 0.5);
    for _ in 0..n {
        (y, z) = (z, -y);
    }
    [p[0], y + 0.5, z + 0.5]
}

impl Baker {
    pub fn open(jar: Option<&Path>) -> Result<Baker, String> {
        let jar = match jar {
            Some(p) => {
                let f = File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
                Some(zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", p.display()))?)
            }
            None => None,
        };
        let mut b = Baker {
            jar,
            models: HashMap::new(),
            states: HashMap::new(),
            tex_ids: HashMap::new(),
            textures: Vec::new(),
            water: None,
            lava: None,
            failed: Vec::new(),
        };
        b.water = b.texture("block/water_still");
        b.lava = b.texture("block/lava_still");
        Ok(b)
    }

    pub fn has_jar(&self) -> bool {
        self.jar.is_some()
    }

    fn read(&mut self, path: &str) -> Option<Vec<u8>> {
        let mut f = self.jar.as_mut()?.by_name(path).ok()?;
        let mut v = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut v).ok()?;
        Some(v)
    }

    fn model(&mut self, name: &str) -> Option<Value> {
        let key = name.trim_start_matches("minecraft:").to_string();
        if let Some(v) = self.models.get(&key) {
            return v.clone();
        }
        let v = self
            .read(&format!("assets/minecraft/models/{key}.json"))
            .and_then(|b| serde_json::from_slice(&b).ok());
        self.models.insert(key, v.clone());
        v
    }

    fn blockstate(&mut self, block: &str) -> Option<Value> {
        if let Some(v) = self.states.get(block) {
            return v.clone();
        }
        let n = block.trim_start_matches("minecraft:");
        let v = self
            .read(&format!("assets/minecraft/blockstates/{n}.json"))
            .and_then(|b| serde_json::from_slice(&b).ok());
        self.states.insert(block.to_string(), v.clone());
        v
    }

    fn texture(&mut self, name: &str) -> Option<u32> {
        let key = name.trim_start_matches("minecraft:").to_string();
        if let Some(&i) = self.tex_ids.get(&key) {
            return Some(i);
        }
        let bytes = self.read(&format!("assets/minecraft/textures/{key}.png"))?;
        let t = decode_png(&bytes)?;
        let i = self.textures.len() as u32;
        self.textures.push(t);
        self.tex_ids.insert(key, i);
        Some(i)
    }

    /// The model's parent chain merged: textures child-first, elements from the nearest.
    fn resolve(&mut self, name: &str) -> Option<(HashMap<String, String>, Vec<Value>, bool)> {
        let mut tex: HashMap<String, String> = HashMap::new();
        let mut elements = None;
        let mut ao = None;
        let mut cur = name.to_string();
        for _ in 0..32 {
            let m = self.model(&cur)?;
            if let Some(t) = m.get("textures").and_then(Value::as_object) {
                for (k, v) in t {
                    // a texture is a name, or {"sprite": name, "force_translucent": true}
                    let s = v
                        .as_str()
                        .or_else(|| v.get("sprite").and_then(Value::as_str));
                    if let Some(s) = s {
                        tex.entry(k.clone()).or_insert_with(|| s.to_string());
                    }
                }
            }
            if elements.is_none() {
                elements = m.get("elements").and_then(Value::as_array).cloned();
            }
            if ao.is_none() {
                ao = m.get("ambientocclusion").and_then(Value::as_bool);
            }
            match m.get("parent").and_then(Value::as_str) {
                Some(p) => cur = p.to_string(),
                None => break,
            }
        }
        Some((tex, elements.unwrap_or_default(), ao.unwrap_or(true)))
    }

    fn texture_ref(&mut self, tex: &HashMap<String, String>, r: &str) -> Option<u32> {
        let mut cur = r.to_string();
        for _ in 0..16 {
            match cur.strip_prefix('#') {
                Some(k) => cur = tex.get(k)?.clone(),
                None => return self.texture(&cur),
            }
        }
        None
    }

    /// One `{model, x, y, uvlock}` application into quads.
    fn apply(&mut self, v: &Value, block: &str, out: &mut Vec<Quad>) -> bool {
        // a weighted list: its first entry, so a render doesn't depend on a seed
        let v = v.as_array().and_then(|a| a.first()).unwrap_or(v);
        let Some(name) = v.get("model").and_then(Value::as_str) else {
            return false;
        };
        let Some((tex, elements, model_ao)) = self.resolve(name) else {
            return false;
        };
        let xr = (v.get("x").and_then(Value::as_i64).unwrap_or(0) / 90).rem_euclid(4) as i32;
        let yr = (v.get("y").and_then(Value::as_i64).unwrap_or(0) / 90).rem_euclid(4) as i32;
        let uvlock = v.get("uvlock").and_then(Value::as_bool).unwrap_or(false);
        let tint = tint_of(block);
        for e in &elements {
            let vec3 = |k: &str| -> Option<[f32; 3]> {
                let a = e.get(k)?.as_array()?;
                Some([
                    a[0].as_f64()? as f32 / 16.0,
                    a[1].as_f64()? as f32 / 16.0,
                    a[2].as_f64()? as f32 / 16.0,
                ])
            };
            let (Some(from), Some(to)) = (vec3("from"), vec3("to")) else {
                continue;
            };
            let shaded = e.get("shade").and_then(Value::as_bool).unwrap_or(true);
            let rot = e.get("rotation");
            let Some(faces) = e.get("faces").and_then(Value::as_object) else {
                continue;
            };
            for (fname, f) in faces {
                let Some(dir) = dir_of(fname) else { continue };
                let Some(tid) = f
                    .get("texture")
                    .and_then(Value::as_str)
                    .and_then(|r| self.texture_ref(&tex, r))
                else {
                    continue;
                };
                let corners = face_corners(dir, from, to);
                let mut world = corners;
                let rotated = rot.is_some();
                if let Some(r) = rot {
                    for c in &mut world {
                        *c = element_rotation(*c, r);
                    }
                }
                let mut d = dir as u8;
                if xr != 0 || yr != 0 {
                    for c in &mut world {
                        *c = rot_y(rot_x(*c, xr), yr);
                    }
                    d = rotate_dir(d, xr, yr);
                }
                let cullface = f
                    .get("cullface")
                    .and_then(Value::as_str)
                    .and_then(dir_of)
                    .map_or(7, |c| rotate_dir(c as u8, xr, yr));
                let explicit = f.get("uv").and_then(Value::as_array).and_then(|a| {
                    Some([
                        a.first()?.as_f64()? as f32 / 16.0,
                        a.get(1)?.as_f64()? as f32 / 16.0,
                        a.get(2)?.as_f64()? as f32 / 16.0,
                        a.get(3)?.as_f64()? as f32 / 16.0,
                    ])
                });
                // uvlock: the texture stays fixed in world space, which is the default
                // projection of the rotated face
                let (uv, frot) = if uvlock && !rotated && (xr != 0 || yr != 0) {
                    let (mut a, mut b) = ([1.0f32; 3], [0.0f32; 3]);
                    for c in &world {
                        for k in 0..3 {
                            a[k] = a[k].min(c[k]);
                            b[k] = b[k].max(c[k]);
                        }
                    }
                    (default_uv(d as usize, a, b), 0)
                } else {
                    (
                        explicit.unwrap_or_else(|| default_uv(dir, from, to)),
                        f.get("rotation").and_then(Value::as_i64).unwrap_or(0) as i32 / 90,
                    )
                };
                // uvlock's projection names corners in the rotated face's own vertex
                // order, so reorder them to match
                let world = if uvlock && !rotated && (xr != 0 || yr != 0) {
                    let (mut a, mut b) = ([1.0f32; 3], [0.0f32; 3]);
                    for c in &world {
                        for k in 0..3 {
                            a[k] = a[k].min(c[k]);
                            b[k] = b[k].max(c[k]);
                        }
                    }
                    face_corners(d as usize, a, b)
                } else {
                    world
                };
                let table = [
                    [uv[0], uv[1]],
                    [uv[0], uv[3]],
                    [uv[2], uv[3]],
                    [uv[2], uv[1]],
                ];
                let uvs: [[f32; 2]; 4] =
                    std::array::from_fn(|i| table[(i + frot.rem_euclid(4) as usize) % 4]);
                let color = if f.get("tintindex").is_some() {
                    tint
                } else {
                    [1.0; 3]
                };
                let axis = !rotated;
                let shade = if shaded { None } else { Some(1.0) };
                if let Some(q) = make_quad(
                    world,
                    uvs,
                    tid,
                    color,
                    shade,
                    model_ao && axis && shaded,
                    if axis { d } else { NO_DIR },
                    cullface,
                ) {
                    out.push(q);
                }
            }
        }
        true
    }

    /// Bake one state.
    pub fn bake(&mut self, world: &World, id: u32) -> Baked {
        let info = world.info(id);
        let detail = world.detail(id);
        let (block, props) = match info.state.split_once('[') {
            Some((b, p)) => (b, p.trim_end_matches(']')),
            None => (info.state.as_str(), ""),
        };
        let props: Vec<(&str, &str)> = props
            .split(',')
            .filter_map(|kv| kv.split_once('='))
            .collect();
        let mut b = Baked {
            fluid: match detail.fluid.as_str() {
                "water" => Fluid::Water,
                "lava" => Fluid::Lava,
                _ => Fluid::None,
            },
            barrier: block == "minecraft:barrier",
            offset: info.offset != world::OffsetType::None,
            ..Baked::default()
        };
        if matches!(info.effect, Effect::Water | Effect::Lava) && b.fluid == Fluid::None {
            b.fluid = if info.effect == Effect::Water {
                Fluid::Water
            } else {
                Fluid::Lava
            };
        }
        let air = matches!(
            block,
            "minecraft:air"
                | "minecraft:cave_air"
                | "minecraft:void_air"
                | "minecraft:light"
                | "minecraft:structure_void"
        );
        let mut particle = None;
        if !air && !b.barrier && self.has_jar() {
            if let Some(bs) = self.blockstate(block) {
                let mut applied = false;
                if let Some(vs) = bs.get("variants").and_then(Value::as_object) {
                    for (k, v) in vs {
                        if variant_matches(k, &props) {
                            applied |= self.apply(v, block, &mut b.quads);
                            if particle.is_none() {
                                particle = self.particle_of(v);
                            }
                            break;
                        }
                    }
                }
                if let Some(parts) = bs.get("multipart").and_then(Value::as_array) {
                    for p in parts {
                        let hit = p.get("when").is_none_or(|w| when_matches(w, &props));
                        if hit && let Some(a) = p.get("apply") {
                            applied |= self.apply(a, block, &mut b.quads);
                            if particle.is_none() {
                                particle = self.particle_of(a);
                            }
                        }
                    }
                }
                if !applied {
                    self.failed.push(info.state.clone());
                }
            } else {
                self.failed.push(info.state.clone());
            }
        }
        let flat = flat_color(block);
        // a fluid's own model has no geometry; the tracer draws its volume
        let fluid_only = b.fluid != Fluid::None && b.quads.is_empty();
        if b.quads.is_empty() && !air && !b.barrier && !fluid_only {
            // block entities and no-jar: collision boxes, in the particle texture
            let (tex, color) = match particle {
                Some(t) => (t, [1.0; 3]),
                None => (u32::MAX, flat),
            };
            for bx in &info.boxes {
                box_quads(
                    bx.min.map(|v| v as f32),
                    bx.max.map(|v| v as f32),
                    tex,
                    color,
                    &mut b.quads,
                );
            }
        }
        // average color and transparency
        let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
        let mut any_transparent = false;
        for q in &b.quads {
            let c = if q.tex == u32::MAX {
                q.color
            } else {
                let t = &self.textures[q.tex as usize];
                any_transparent |= t.transparent;
                [
                    t.avg[0] * q.color[0],
                    t.avg[1] * q.color[1],
                    t.avg[2] * q.color[2],
                ]
            };
            for k in 0..3 {
                sum[k] += c[k];
            }
            n += 1.0;
        }
        b.avg = if n > 0.0 {
            [sum[0] / n, sum[1] / n, sum[2] / n]
        } else if b.fluid != Fluid::None {
            let t = if b.fluid == Fluid::Water {
                self.water
            } else {
                self.lava
            };
            t.map_or(flat, |t| self.textures[t as usize].avg)
        } else {
            flat
        };
        let full_box = info.boxes.len() == 1
            && info.boxes[0].min == [0.0; 3]
            && info.boxes[0].max == [1.0; 3];
        b.full_opaque = full_box && !any_transparent && !b.quads.is_empty() && !b.barrier;
        if b.full_opaque {
            let mut idx = [u16::MAX; 6];
            let mut plain = b.quads.len() == 6;
            for (i, q) in b.quads.iter().enumerate() {
                if q.dir == NO_DIR || idx[q.dir as usize] != u16::MAX {
                    plain = false;
                    break;
                }
                idx[q.dir as usize] = i as u16;
            }
            if plain && idx.iter().all(|&i| i != u16::MAX) {
                b.cube = Some(idx);
            }
        }
        for bx in &info.boxes {
            box_quads(
                bx.min.map(|v| v as f32),
                bx.max.map(|v| v as f32),
                u32::MAX,
                b.avg,
                &mut b.boxes,
            );
            b.tall |= bx.max[1] > 1.0;
        }
        b
    }

    fn particle_of(&mut self, v: &Value) -> Option<u32> {
        let v = v.as_array().and_then(|a| a.first()).unwrap_or(v);
        let (tex, _, _) = self.resolve(v.get("model")?.as_str()?)?;
        self.texture_ref(&tex, "#particle")
    }
}

fn rotate_dir(d: u8, xr: i32, yr: i32) -> u8 {
    if d >= 6 {
        return d;
    }
    let mut v: [f32; 3] = [0.5; 3];
    v[(d / 2) as usize] = if d.is_multiple_of(2) { 0.0 } else { 1.0 };
    let r = rot_y(rot_x(v, xr), yr);
    for k in 0..3 {
        if (r[k] - 0.5).abs() > 0.25 {
            return (k as u8) * 2 + (r[k] > 0.5) as u8;
        }
    }
    d
}

/// A model element's own `rotation` applied to a point (block-local, 0 to 1).
fn element_rotation(p: [f32; 3], r: &Value) -> [f32; 3] {
    let origin: [f32; 3] = r
        .get("origin")
        .and_then(Value::as_array)
        .map(|a| std::array::from_fn(|i| a[i].as_f64().unwrap_or(8.0) as f32 / 16.0))
        .unwrap_or([0.5; 3]);
    let (axis, angle) = if let Some(a) = r.get("axis").and_then(Value::as_str) {
        (
            a.chars().next().unwrap_or('y'),
            r.get("angle").and_then(Value::as_f64).unwrap_or(0.0),
        )
    } else {
        // {"x": deg, "y": deg, "z": deg}, for an element turned about one axis
        let pick = |k: &str| r.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        match (pick("x"), pick("y"), pick("z")) {
            (x, _, _) if x != 0.0 => ('x', x),
            (_, y, _) if y != 0.0 => ('y', y),
            (_, _, z) => ('z', z),
        }
    };
    let rescale = r.get("rescale").and_then(Value::as_bool).unwrap_or(false);
    let t = (angle as f32).to_radians();
    let (s, c) = t.sin_cos();
    let d = sub(p, origin);
    // right-handed rotation about +axis
    let mut q = match axis {
        'x' => [d[0], c * d[1] - s * d[2], s * d[1] + c * d[2]],
        'y' => [c * d[0] + s * d[2], d[1], -s * d[0] + c * d[2]],
        _ => [c * d[0] - s * d[1], s * d[0] + c * d[1], d[2]],
    };
    if rescale {
        let k = 1.0 / c.abs().max(s.abs()).max(1e-3);
        match axis {
            'x' => {
                q[1] *= k;
                q[2] *= k;
            }
            'y' => {
                q[0] *= k;
                q[2] *= k;
            }
            _ => {
                q[0] *= k;
                q[1] *= k;
            }
        }
    }
    [q[0] + origin[0], q[1] + origin[1], q[2] + origin[2]]
}

fn variant_matches(key: &str, props: &[(&str, &str)]) -> bool {
    key.split(',')
        .filter(|s| !s.is_empty())
        .all(|kv| match kv.split_once('=') {
            Some((k, v)) => props.iter().any(|(pk, pv)| *pk == k && *pv == v),
            None => false,
        })
}

fn when_matches(w: &Value, props: &[(&str, &str)]) -> bool {
    if let Some(a) = w.get("OR").and_then(Value::as_array) {
        return a.iter().any(|x| when_matches(x, props));
    }
    if let Some(a) = w.get("AND").and_then(Value::as_array) {
        return a.iter().all(|x| when_matches(x, props));
    }
    let Some(o) = w.as_object() else { return false };
    o.iter().all(|(k, v)| {
        let want = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
        let have = props.iter().find(|(pk, _)| pk == k).map(|(_, pv)| *pv);
        have.is_some_and(|h| want.split('|').any(|w| w == h))
    })
}

fn decode_png(bytes: &[u8]) -> Option<Texture> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0u8; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = info.color_type.samples();
    // animations stack frames vertically; the first frame is the top square
    let s = w.min(h);
    if !s.is_power_of_two() {
        return None;
    }
    let mut px = Vec::with_capacity(s * s);
    for y in 0..s {
        for x in 0..s {
            let o = (y * w + x) * ch;
            let p = &buf[o..o + ch];
            let (r, g, b, a) = match ch {
                1 => (p[0], p[0], p[0], 255),
                2 => (p[0], p[0], p[0], p[1]),
                3 => (p[0], p[1], p[2], 255),
                _ => (p[0], p[1], p[2], p[3]),
            };
            let a = a as f32 / 255.0;
            px.push([
                r as f32 / 255.0 * a,
                g as f32 / 255.0 * a,
                b as f32 / 255.0 * a,
                a,
            ]);
        }
    }
    Some(Texture::new(s as u32, px))
}
