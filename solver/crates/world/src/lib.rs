//! Block data of a course: what we know exactly about the world, as opposed to the
//! course's fitted rules. Written by Python (`extract_world.py`, whose docstring is
//! the file format's spec), read here.
//!
//! A chunk the replay never loaded is unknown, not air, and a move whose box enters
//! unknown space fails `verify`. why? otherwise a route through an unloaded wall would
//! verify.
//!
//! The items below the contract marker are what `port` and `sim` build against;
//! change a signature only together with its callers.
//!
//! The block table (`data/blocks-26.2.tsv.gz`, `tools/block_dump.sh`) is embedded in
//! the binary. why? [`World::air`], [`World::set`] and [`World::info`] need it with
//! no file argument, and a binary then doesn't depend on its working directory. A
//! world file names the table it was written against by sha256, and [`World::load`]
//! rejects a file whose table differs, since a cell's boxes, offset and effect come
//! from the table.

use std::collections::HashMap;
use std::error::Error;
use std::io::Read;
use std::sync::{Arc, OnceLock};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// A block position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// An axis-aligned box, in whatever frame its context says (block-local in
/// [`BlockInfo::boxes`], world coordinates from [`World::collision_boxes`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

/// What a block does to movement beyond its collision boxes. Kernels either model a
/// class or treat entering it as an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Ladders, scaffolding, ...: entering one ends the glide
    /// (`LivingEntity.travelFallFlying`). Blocks also in the world's
    /// `can_glide_through` tag (vines in vanilla) are not this class but have
    /// [`BlockDetail::climbable`] set: they climb only on foot. Takes precedence over
    /// the fluid of a waterlogged state. Not covered: an open trapdoor above a ladder
    /// (`LivingEntity.trapdoorUsableAsLadder`), which depends on its neighbor.
    Climbable,
    Water,
    Lava,
    /// Slime, beds: vertical restitution.
    Bounce,
    /// Cobweb, sweet berry bush, powder snow: a stuck-speed multiplier.
    Stuck,
}

/// How a block's shape is offset by its position (vanilla `BlockBehaviour.OffsetType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffsetType {
    None,
    Xz,
    Xyz,
}

/// One block state, from `data/blocks-26.2.tsv.gz`.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockInfo {
    /// e.g. `minecraft:oak_slab[type=bottom,waterlogged=false]`
    pub state: String,
    pub friction: f32,
    pub speed_factor: f32,
    pub jump_factor: f32,
    pub effect: Effect,
    pub offset: OffsetType,
    /// Collision boxes, block-local, without any position offset.
    /// `CollisionContext.empty()` shapes: scaffolding and powder snow, whose shape
    /// depends on the entity, have their empty-context shape here.
    pub boxes: Vec<Aabb>,
}

/// A block state id (26.2 network id).
pub type StateId = u32;

/// A cell of the world: a known block state, or space the source never covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    Known(StateId),
    Unknown,
}

/// A query touched space the world file doesn't cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownSpace(pub BlockPos);

impl std::fmt::Display for UnknownSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let p = self.0;
        write!(f, "unknown space at {} {} {}", p.x, p.y, p.z)
    }
}

impl Error for UnknownSpace {}

/// One stored section, for code that walks a whole world (the renderer). Cell indices
/// are `(y << 8) | (z << 4) | x` within the section. `u16::MAX` in [`SectionView::Cells`]
/// is an unknown cell.
#[derive(Debug, Clone, Copy)]
pub enum SectionView<'a> {
    Unknown,
    Uniform(StateId),
    Cells(&'a [u16; 4096]),
}

/// The rest of a state's table row, which [`BlockInfo`] doesn't carry.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockDetail {
    /// Block class, e.g. `SlimeBlock`.
    pub class: String,
    /// `-`, `water` or `lava`.
    pub fluid: String,
    /// The class apart from climbing ([`Effect::Climbable`] comes from tags).
    pub own_effect: Effect,
    /// In this world's `climbable` block tag (the server's, if the world file has them).
    pub climbable: bool,
    /// In this world's `can_glide_through` block tag.
    pub glide_through: bool,
    /// `BlockStateBase.hasLargeCollisionShape`: true when the shape leaves the unit
    /// cube, and for every dynamic-shape state (no shape cache).
    pub large_shape: bool,
    /// `getMaxHorizontalOffset` / `getMaxVerticalOffset`, for [`World::offset`].
    pub max_horizontal_offset: f32,
    pub max_vertical_offset: f32,
    /// Block registry id.
    pub block_id: u32,
    /// The block's default state.
    pub is_default: bool,
    /// `BlockState.isSuffocating(EmptyBlockGetter.INSTANCE, BlockPos.ZERO)`: what
    /// `LocalPlayer.moveTowardsClosestSpace` tests. World-dependent cases (shulker
    /// boxes, closed without their block entity) aren't covered.
    pub suffocating: bool,
}

/// Provenance of a loaded world file; see `extract_world.py` for the fields.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Header {
    pub format: String,
    pub version: u32,
    pub source: String,
    pub tick: Option<i64>,
    pub dimension: Option<String>,
    pub dimension_type: Option<String>,
    /// `registry`, `vanilla dimension type`, `vote` (a guess) or `given`.
    pub min_y_method: String,
    pub bounds: Option<[[i32; 3]; 2]>,
    pub content_sha256: String,
    #[serde(default)]
    pub coverage: serde_json::Value,
}

/// Stored per cell; `u16::MAX` is an unknown cell. why u16? 26.2 has 32366 states
/// (`GLOBAL_BITS` 15), and a full section is then 8 KiB.
const UNKNOWN_CELL: u16 = u16::MAX;
const FORMAT: &str = "etas-world";
const VERSION: u32 = 1;

#[derive(Debug, Clone)]
enum Section {
    Unknown,
    Uniform(u16),
    Cells(Box<[u16; 4096]>),
}

fn cell_index(x: i32, y: i32, z: i32) -> usize {
    (((y & 15) << 8) | ((z & 15) << 4) | (x & 15)) as usize
}

/// Block infos and details for one set of climbing tags.
#[derive(Debug)]
struct Blocks {
    infos: Vec<BlockInfo>,
    details: Vec<BlockDetail>,
}

/// The embedded table, parsed once.
struct Table {
    vanilla: Arc<Blocks>,
    by_state: HashMap<String, StateId>,
    default_of: HashMap<String, StateId>,
    sha256: String,
    air: StateId,
}

static TABLE_GZ: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../data/blocks-26.2.tsv.gz"
));
static TABLE: OnceLock<Table> = OnceLock::new();
static TABLE_TEXT: OnceLock<String> = OnceLock::new();

/// The embedded block table, decompressed (comment lines included).
pub fn block_table_text() -> &'static str {
    TABLE_TEXT.get_or_init(|| {
        let mut s = String::new();
        flate2::read::GzDecoder::new(TABLE_GZ)
            .read_to_string(&mut s)
            .expect("embedded block table");
        s
    })
}

/// sha256 (hex) of the decompressed block table, as world files name it.
pub fn block_table_sha256() -> &'static str {
    &table().sha256
}

fn table() -> &'static Table {
    TABLE.get_or_init(|| {
        let text = block_table_text();
        let mut infos = Vec::new();
        let mut details = Vec::new();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = line.split('\t').collect();
            assert_eq!(f.len(), 18, "block table row: {line}");
            assert_eq!(
                f[0].parse::<usize>().unwrap(),
                infos.len(),
                "block table ids are dense"
            );
            let boxes = if f[7] == "-" {
                Vec::new()
            } else {
                f[7].split(';')
                    .map(|b| {
                        let v: Vec<f64> = b.split(',').map(|x| x.parse().unwrap()).collect();
                        Aabb {
                            min: [v[0], v[1], v[2]],
                            max: [v[3], v[4], v[5]],
                        }
                    })
                    .collect()
            };
            let own_effect = match f[8] {
                "none" => Effect::None,
                "water" => Effect::Water,
                "lava" => Effect::Lava,
                "bounce" => Effect::Bounce,
                "stuck" => Effect::Stuck,
                e => panic!("block table effect {e}"),
            };
            let offset = match f[9] {
                "none" => OffsetType::None,
                "xz" => OffsetType::Xz,
                "xyz" => OffsetType::Xyz,
                o => panic!("block table offset {o}"),
            };
            let detail = BlockDetail {
                class: f[2].to_string(),
                fluid: f[6].to_string(),
                own_effect,
                climbable: f[15] == "1",
                glide_through: f[16] == "1",
                large_shape: f[12] == "1",
                max_horizontal_offset: f[10].parse().unwrap(),
                max_vertical_offset: f[11].parse().unwrap(),
                block_id: f[13].parse().unwrap(),
                is_default: f[14] == "1",
                suffocating: f[17] == "1",
            };
            infos.push(BlockInfo {
                state: f[1].to_string(),
                // Java's Float.toString round-trips through an f32 parse
                friction: f[3].parse().unwrap(),
                speed_factor: f[4].parse().unwrap(),
                jump_factor: f[5].parse().unwrap(),
                effect: effect_of(&detail),
                offset,
                boxes,
            });
            details.push(detail);
        }
        assert!(infos.len() < UNKNOWN_CELL as usize);
        let by_state: HashMap<String, StateId> = infos
            .iter()
            .enumerate()
            .map(|(i, b)| (b.state.clone(), i as StateId))
            .collect();
        let default_of = infos
            .iter()
            .zip(&details)
            .enumerate()
            .filter(|(_, (_, d))| d.is_default)
            .map(|(i, (b, _))| (block_name(&b.state).to_string(), i as StateId))
            .collect();
        let sha256 = hex(&Sha256::digest(text.as_bytes()));
        let air = by_state["minecraft:air"];
        Table {
            vanilla: Arc::new(Blocks { infos, details }),
            by_state,
            default_of,
            sha256,
            air,
        }
    })
}

fn effect_of(d: &BlockDetail) -> Effect {
    if d.climbable && !d.glide_through {
        Effect::Climbable
    } else {
        d.own_effect
    }
}

fn block_name(state: &str) -> &str {
    state.split('[').next().unwrap_or(state)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The state id of a state string: a full state as in the table, a bare block name
/// (its default state), or a name with some properties (the rest from the default
/// state), e.g. `minecraft:bamboo[age=1]`.
pub fn state_id(state: &str) -> Option<StateId> {
    let t = table();
    if let Some(&id) = t.by_state.get(state) {
        return Some(id);
    }
    let (name, props) = match state.split_once('[') {
        Some((n, p)) => (n, p.strip_suffix(']')?),
        None => (state, ""),
    };
    let default = &t.vanilla.infos[*t.default_of.get(name)? as usize].state;
    let Some((_, dprops)) = default.split_once('[') else {
        return (props.is_empty()).then(|| t.by_state[default]);
    };
    let mut values: Vec<(String, String)> = dprops
        .trim_end_matches(']')
        .split(',')
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap();
            (k.to_string(), v.to_string())
        })
        .collect();
    for kv in props.split(',').filter(|s| !s.is_empty()) {
        let (k, v) = kv.split_once('=')?;
        values.iter_mut().find(|(dk, _)| dk == k)?.1 = v.to_string();
    }
    let full = format!(
        "{name}[{}]",
        values
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    t.by_state.get(&full).copied()
}

/// `Mth.getSeed(x, 0, z)`. `x * 3129871` is an int product in the Java.
fn offset_seed(x: i32, z: i32) -> i64 {
    let mut seed = (x.wrapping_mul(3129871) as i64) ^ (z as i64).wrapping_mul(116129781);
    seed = seed
        .wrapping_mul(seed)
        .wrapping_mul(42317861)
        .wrapping_add(seed.wrapping_mul(11));
    seed >> 16
}

/// `Mth.clamp(double, double, double)`.
fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo { lo } else { v.min(hi) }
}

fn offset_of(offset: OffsetType, d: &BlockDetail, pos: BlockPos) -> [f64; 3] {
    if offset == OffsetType::None {
        return [0.0; 3];
    }
    // BlockBehaviour.Properties.offsetType's functions
    let seed = offset_seed(pos.x, pos.z);
    let mh = d.max_horizontal_offset as f64;
    let x = clamp((((seed & 15) as f32 / 15.0f32) as f64 - 0.5) * 0.5, -mh, mh);
    let z = clamp(
        (((seed >> 8 & 15) as f32 / 15.0f32) as f64 - 0.5) * 0.5,
        -mh,
        mh,
    );
    let y = if offset == OffsetType::Xyz {
        (((seed >> 4 & 15) as f32 / 15.0f32) as f64 - 1.0) * d.max_vertical_offset as f64
    } else {
        0.0
    };
    [x, y, z]
}

fn overlaps(b: &Aabb, a: &Aabb) -> bool {
    (0..3).all(|i| b.min[i] < a.max[i] && b.max[i] > a.min[i])
}

fn err(msg: String) -> Box<dyn Error> {
    msg.into()
}

#[derive(Deserialize)]
struct ContentHead {
    min_y: i32,
    height: Option<i32>,
    block_table: TableRef,
    block_tags: Option<HashMap<String, Vec<String>>>,
    palette: Vec<String>,
}

#[derive(Deserialize)]
struct TableRef {
    sha256: String,
}

// ---- contract ----------------------------------------------------------------------

#[derive(Clone)]
pub struct World {
    blocks: Arc<Blocks>,
    sections: HashMap<[i32; 3], Section>,
    /// (min_y, height): cells outside are air everywhere (`LevelChunk.getBlockState`).
    build: Option<(i32, i32)>,
    header: Option<Header>,
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World")
            .field("sections", &self.sections.len())
            .field("build", &self.build)
            .field("header", &self.header)
            .finish()
    }
}

impl World {
    /// Load a world file.
    pub fn load(path: &std::path::Path) -> Result<World, Box<dyn std::error::Error>> {
        let mut data = Vec::new();
        flate2::read::GzDecoder::new(std::fs::File::open(path)?)
            .read_to_end(&mut data)
            .map_err(|e| err(format!("{}: {e}", path.display())))?;
        Self::parse(&data).map_err(|e| err(format!("{}: {e}", path.display())))
    }

    fn parse(data: &[u8]) -> Result<World, Box<dyn Error>> {
        let nl = data
            .iter()
            .position(|&b| b == b'\n')
            .ok_or("no header line")?;
        let header: Header = serde_json::from_slice(&data[..nl])?;
        if header.format != FORMAT || header.version != VERSION {
            return Err(err(format!(
                "format {} version {}, reader knows {FORMAT} {VERSION}",
                header.format, header.version
            )));
        }
        let content = &data[nl + 1..];
        let sha = hex(&Sha256::digest(content));
        if sha != header.content_sha256 {
            return Err(err(format!(
                "content sha256 {sha}, header says {}",
                header.content_sha256
            )));
        }
        let nl = content
            .iter()
            .position(|&b| b == b'\n')
            .ok_or("no content head")?;
        let head: ContentHead = serde_json::from_slice(&content[..nl])?;
        let t = table();
        if head.block_table.sha256 != t.sha256 {
            return Err(err(format!(
                "written against block table {}, this build embeds {}; re-extract the world",
                head.block_table.sha256, t.sha256
            )));
        }
        let palette: Vec<u16> = head
            .palette
            .iter()
            .map(|s| {
                t.by_state
                    .get(s)
                    .map(|&id| id as u16)
                    .ok_or_else(|| err(format!("state {s} not in the block table")))
            })
            .collect::<Result<_, _>>()?;
        let blocks = match &head.block_tags {
            None => t.vanilla.clone(),
            Some(tags) => Arc::new(with_tags(&t.vanilla, tags)),
        };
        let mut r = Reader {
            b: &content[nl + 1..],
            i: 0,
        };
        let n = r.u32()?;
        let mut sections = HashMap::with_capacity(n as usize);
        let pal = |i: u16| -> Result<u16, Box<dyn Error>> {
            palette
                .get(i as usize)
                .copied()
                .ok_or_else(|| err(format!("palette index {i} of {}", palette.len())))
        };
        for _ in 0..n {
            let key = [r.i32()?, r.i32()?, r.i32()?];
            let s = match r.u8()? {
                0 => Section::Unknown,
                1 => Section::Uniform(pal(r.u16()?)?),
                2 => {
                    let mut cells = Box::new([0u16; 4096]);
                    for c in cells.iter_mut() {
                        let i = r.u16()?;
                        *c = if i == UNKNOWN_CELL {
                            UNKNOWN_CELL
                        } else {
                            pal(i)?
                        };
                    }
                    Section::Cells(cells)
                }
                k => return Err(err(format!("section kind {k}"))),
            };
            if sections.insert(key, s).is_some() {
                return Err(err(format!("section {key:?} twice")));
            }
        }
        if r.i != r.b.len() {
            return Err(err(format!(
                "{} bytes after the last section",
                r.b.len() - r.i
            )));
        }
        Ok(World {
            blocks,
            sections,
            build: head.height.map(|h| (head.min_y, h)),
            header: Some(header),
        })
    }

    /// A world that is air everywhere inside `[min, max]` (block coordinates,
    /// inclusive) and unknown outside, with blocks set by `set`. For tests.
    pub fn air(min: BlockPos, max: BlockPos) -> World {
        let air = table().air as u16;
        let mut w = World {
            blocks: table().vanilla.clone(),
            sections: HashMap::new(),
            build: None,
            header: None,
        };
        let (lo, hi) = ([min.x, min.y, min.z], [max.x, max.y, max.z]);
        for sx in lo[0] >> 4..=hi[0] >> 4 {
            for sy in lo[1] >> 4..=hi[1] >> 4 {
                for sz in lo[2] >> 4..=hi[2] >> 4 {
                    let base = [sx * 16, sy * 16, sz * 16];
                    let whole = (0..3).all(|i| lo[i] <= base[i] && base[i] + 15 <= hi[i]);
                    let s = if whole {
                        Section::Uniform(air)
                    } else {
                        let mut cells = Box::new([UNKNOWN_CELL; 4096]);
                        for y in base[1].max(lo[1])..=(base[1] + 15).min(hi[1]) {
                            for z in base[2].max(lo[2])..=(base[2] + 15).min(hi[2]) {
                                for x in base[0].max(lo[0])..=(base[0] + 15).min(hi[0]) {
                                    cells[cell_index(x, y, z)] = air;
                                }
                            }
                        }
                        Section::Cells(cells)
                    };
                    w.sections.insert([sx, sy, sz], s);
                }
            }
        }
        w
    }

    /// Set a block by state string (e.g. `minecraft:stone`; see [`state_id`] for the
    /// forms accepted). For tests. Panics on an unknown state, or outside the build
    /// height of a loaded world.
    pub fn set(&mut self, pos: BlockPos, state: &str) {
        let id = state_id(state).unwrap_or_else(|| panic!("no block state {state}")) as u16;
        if let Some((min_y, h)) = self.build {
            assert!(
                (min_y..min_y + h).contains(&pos.y),
                "{pos:?} is outside the build height"
            );
        }
        let s = self
            .sections
            .entry([pos.x >> 4, pos.y >> 4, pos.z >> 4])
            .or_insert(Section::Unknown);
        let cells = match s {
            Section::Cells(c) => c,
            Section::Uniform(v) if *v == id => return,
            Section::Uniform(v) => {
                *s = Section::Cells(Box::new([*v; 4096]));
                let Section::Cells(c) = s else { unreachable!() };
                c
            }
            Section::Unknown => {
                *s = Section::Cells(Box::new([UNKNOWN_CELL; 4096]));
                let Section::Cells(c) = s else { unreachable!() };
                c
            }
        };
        cells[cell_index(pos.x, pos.y, pos.z)] = id;
    }

    pub fn cell(&self, pos: BlockPos) -> Cell {
        if let Some((min_y, h)) = self.build
            && (pos.y < min_y || pos.y >= min_y + h)
        {
            return Cell::Known(table().air);
        }
        match self.sections.get(&[pos.x >> 4, pos.y >> 4, pos.z >> 4]) {
            None | Some(Section::Unknown) => Cell::Unknown,
            Some(Section::Uniform(v)) => Cell::Known(*v as StateId),
            Some(Section::Cells(c)) => match c[cell_index(pos.x, pos.y, pos.z)] {
                UNKNOWN_CELL => Cell::Unknown,
                v => Cell::Known(v as StateId),
            },
        }
    }

    pub fn info(&self, id: StateId) -> &BlockInfo {
        &self.blocks.infos[id as usize]
    }

    /// The collision boxes of the blocks vanilla's `BlockCollisions` returns for
    /// `area`, in world coordinates, position offsets applied, in its cursor order
    /// (x fastest, then y, then z). That is: cells overlapping `area` (with its
    /// 1e-7 margin), plus the face-adjacent shell for blocks with a large shape
    /// (fences, walls), each block's boxes kept whole if any of them overlaps `area`
    /// with positive volume. Vanilla's 1e-7 merge tolerance in that overlap test is
    /// not reproduced, so a box overlapping by less may be returned; `Shapes.collide`
    /// ignores such boxes anyway. Coordinates are `(local + offset) + pos`, vanilla's
    /// order of additions. Fails if `area` touches unknown space, i.e. any cell
    /// vanilla would read (the shell's edge cells too, which it reads for moving
    /// pistons; the table has no shape for those, as for any block entity shape).
    pub fn collision_boxes(&self, area: Aabb) -> Result<Vec<Aabb>, UnknownSpace> {
        let lo: [i32; 3] = std::array::from_fn(|i| (area.min[i] - 1.0E-7).floor() as i32 - 1);
        let hi: [i32; 3] = std::array::from_fn(|i| (area.max[i] + 1.0E-7).floor() as i32 + 1);
        let mut out = Vec::new();
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    let face = [(x, 0), (y, 1), (z, 2)]
                        .iter()
                        .filter(|(v, i)| *v == lo[*i] || *v == hi[*i])
                        .count();
                    if face == 3 {
                        continue;
                    }
                    let pos = BlockPos { x, y, z };
                    let Cell::Known(id) = self.cell(pos) else {
                        return Err(UnknownSpace(pos));
                    };
                    let (info, detail) = (
                        &self.blocks.infos[id as usize],
                        &self.blocks.details[id as usize],
                    );
                    if face == 2 || (face == 1 && !detail.large_shape) || info.boxes.is_empty() {
                        continue;
                    }
                    let start = out.len();
                    out.extend(world_boxes(info, detail, pos));
                    if !out[start..].iter().any(|b| overlaps(b, &area)) {
                        out.truncate(start);
                    }
                }
            }
        }
        Ok(out)
    }
}

// ---- end of contract ---------------------------------------------------------------

/// A block's boxes at `pos`, in world coordinates.
fn world_boxes<'a>(
    info: &'a BlockInfo,
    detail: &BlockDetail,
    pos: BlockPos,
) -> impl Iterator<Item = Aabb> + 'a {
    let o = offset_of(info.offset, detail, pos);
    let p = [pos.x as f64, pos.y as f64, pos.z as f64];
    info.boxes.iter().map(move |b| Aabb {
        min: std::array::from_fn(|i| (b.min[i] + o[i]) + p[i]),
        max: std::array::from_fn(|i| (b.max[i] + o[i]) + p[i]),
    })
}

fn with_tags(base: &Blocks, tags: &HashMap<String, Vec<String>>) -> Blocks {
    let names = |t: &str| -> std::collections::HashSet<&str> {
        tags.get(t)
            .map(|v| v.iter().map(String::as_str).collect())
            .unwrap_or_default()
    };
    let (climbable, glide) = (
        names("minecraft:climbable"),
        names("minecraft:can_glide_through"),
    );
    let mut infos = base.infos.clone();
    let mut details = base.details.clone();
    for (info, d) in infos.iter_mut().zip(details.iter_mut()) {
        let name = block_name(&info.state);
        d.climbable = climbable.contains(name);
        d.glide_through = glide.contains(name);
        info.effect = effect_of(d);
    }
    Blocks { infos, details }
}

impl World {
    /// The rest of a state's table row, with this world's climbing tags.
    pub fn detail(&self, id: StateId) -> &BlockDetail {
        &self.blocks.details[id as usize]
    }

    /// `BlockState.getOffset(pos)`: the shape offset of state `id` at `pos`.
    pub fn offset(&self, id: StateId, pos: BlockPos) -> [f64; 3] {
        offset_of(self.info(id).offset, self.detail(id), pos)
    }

    /// State `id`'s collision boxes as if it stood at `pos`, in world coordinates,
    /// offset applied, whatever the cell holds.
    pub fn boxes_at(&self, id: StateId, pos: BlockPos) -> Vec<Aabb> {
        world_boxes(self.info(id), self.detail(id), pos).collect()
    }

    /// The file's header; None for a world made by [`World::air`].
    pub fn header(&self) -> Option<&Header> {
        self.header.as_ref()
    }

    /// Every stored section by section coordinate (block coordinate `>> 4`). A section
    /// not listed is unknown, except outside the build height, where cells are air.
    pub fn sections(&self) -> impl Iterator<Item = ([i32; 3], SectionView<'_>)> {
        self.sections.iter().map(|(k, s)| {
            let v = match s {
                Section::Unknown => SectionView::Unknown,
                Section::Uniform(v) => SectionView::Uniform(*v as StateId),
                Section::Cells(c) => SectionView::Cells(c),
            };
            (*k, v)
        })
    }

    /// (min_y, height) of the build height, if the world has one.
    pub fn build_height(&self) -> Option<(i32, i32)> {
        self.build
    }
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], Box<dyn Error>> {
        let s = self.b.get(self.i..self.i + N).ok_or("sections truncated")?;
        self.i += N;
        Ok(s.try_into().unwrap())
    }
    fn u8(&mut self) -> Result<u8, Box<dyn Error>> {
        Ok(self.take::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, Box<dyn Error>> {
        Ok(u16::from_le_bytes(self.take()?))
    }
    fn u32(&mut self) -> Result<u32, Box<dyn Error>> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn i32(&mut self) -> Result<i32, Box<dyn Error>> {
        Ok(i32::from_le_bytes(self.take()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(h: &str) -> f64 {
        f64::from_bits(u64::from_str_radix(h, 16).unwrap())
    }

    /// `#offset_check` / `#shape_check` lines: vanilla's getOffset and moved collision
    /// shape at sample positions, from tools/BlockDump.java.
    fn checks(kind: &str) -> Vec<Vec<&'static str>> {
        block_table_text()
            .lines()
            .filter_map(|l| l.strip_prefix(kind))
            .map(|l| l.trim_start_matches('\t').split('\t').collect())
            .collect()
    }

    #[test]
    fn offsets_match_vanilla() {
        let w = World::air(BlockPos { x: 0, y: 0, z: 0 }, BlockPos { x: 0, y: 0, z: 0 });
        let c = checks("#offset_check");
        assert!(c.len() >= 30, "{}", c.len());
        let mut bamboo = 0;
        for f in c {
            let id = state_id(f[0]).unwrap();
            let pos = BlockPos {
                x: f[1].parse().unwrap(),
                y: 0,
                z: f[2].parse().unwrap(),
            };
            let want = [bits(f[3]), bits(f[4]), bits(f[5])];
            let got = w.offset(id, pos);
            assert_eq!(
                got.map(f64::to_bits),
                want.map(f64::to_bits),
                "{} at {pos:?}",
                f[0]
            );
            bamboo += f[0].starts_with("minecraft:bamboo[") as usize;
        }
        assert!(bamboo >= 10);
    }

    #[test]
    fn bamboo_boxes_match_vanilla() {
        // collision_boxes over the bamboo cell, against vanilla's
        // getCollisionShape(pos).move(pos).toAabbs()
        let mut n = 0;
        for f in checks("#shape_check") {
            let pos = BlockPos {
                x: f[1].parse().unwrap(),
                y: f[2].parse().unwrap(),
                z: f[3].parse().unwrap(),
            };
            let mut w = World::air(
                BlockPos {
                    x: pos.x - 2,
                    y: pos.y - 2,
                    z: pos.z - 2,
                },
                BlockPos {
                    x: pos.x + 2,
                    y: pos.y + 2,
                    z: pos.z + 2,
                },
            );
            w.set(pos, f[0]);
            let c = [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5];
            let area = Aabb {
                min: [c[0] - 0.49, c[1] - 0.49, c[2] - 0.49],
                max: [c[0] + 0.49, c[1] + 0.49, c[2] + 0.49],
            };
            let got = w.collision_boxes(area).unwrap();
            let want: Vec<[f64; 6]> = f[4]
                .split(';')
                .map(|b| <[f64; 6]>::try_from(b.split(',').map(bits).collect::<Vec<_>>()).unwrap())
                .collect();
            assert_eq!(got.len(), want.len(), "{} at {pos:?}", f[0]);
            for (g, w) in got.iter().zip(&want) {
                let g6 = [g.min[0], g.min[1], g.min[2], g.max[0], g.max[1], g.max[2]];
                assert_eq!(
                    g6.map(f64::to_bits),
                    w.map(f64::to_bits),
                    "{} at {pos:?}",
                    f[0]
                );
            }
            n += 1;
        }
        assert!(n >= 30);
    }

    #[test]
    fn bamboo_offset_by_hand() {
        // Mth.getSeed(1, 0, 0) = (3129871^2 * 42317861 + 3129871 * 11) >> 16 in Java's
        // wrapping long arithmetic (done here in i128, truncated); then
        // x = ((seed & 15) / 15f - 0.5) * 0.5.
        let s: i128 = 3129871;
        let seed = ((s * s * 42317861 + s * 11) as i64) >> 16;
        assert_eq!(offset_seed(1, 0), seed);
        let id = state_id("minecraft:bamboo").unwrap();
        let w = World::air(BlockPos { x: 0, y: 0, z: 0 }, BlockPos { x: 3, y: 3, z: 3 });
        let o = w.offset(id, BlockPos { x: 1, y: 7, z: 0 });
        let x = (((seed & 15) as f32 / 15.0) as f64 - 0.5) * 0.5;
        assert_eq!(o[0], x.clamp(-0.25, 0.25));
        assert_eq!(o[1], 0.0);
        // the table's box is the un-offset Block.column(3, 0, 16)
        assert_eq!(
            w.info(id).boxes,
            vec![Aabb {
                min: [0.40625, 0.0, 0.40625],
                max: [0.59375, 1.0, 0.59375]
            }]
        );
    }

    #[test]
    fn state_strings() {
        assert_eq!(state_id("minecraft:stone"), Some(1));
        let slab = state_id("minecraft:oak_slab").unwrap();
        assert_eq!(
            table().vanilla.infos[slab as usize].state,
            "minecraft:oak_slab[type=bottom,waterlogged=false]"
        );
        let top = state_id("minecraft:oak_slab[type=top]").unwrap();
        assert_eq!(
            table().vanilla.infos[top as usize].state,
            "minecraft:oak_slab[type=top,waterlogged=false]"
        );
        assert_eq!(state_id("minecraft:oak_slab[shape=top]"), None);
        assert_eq!(state_id("minecraft:no_such_block"), None);
    }

    #[test]
    fn effects() {
        let w = World::air(BlockPos { x: 0, y: 0, z: 0 }, BlockPos { x: 0, y: 0, z: 0 });
        let e = |s: &str| w.info(state_id(s).unwrap()).effect;
        assert_eq!(e("minecraft:ladder"), Effect::Climbable);
        assert_eq!(e("minecraft:ladder[waterlogged=true]"), Effect::Climbable);
        assert_eq!(e("minecraft:scaffolding"), Effect::Climbable);
        assert_eq!(
            e("minecraft:vine"),
            Effect::None,
            "vanilla: can_glide_through"
        );
        assert!(w.detail(state_id("minecraft:vine").unwrap()).climbable);
        assert_eq!(e("minecraft:water"), Effect::Water);
        assert_eq!(e("minecraft:lava"), Effect::Lava);
        assert_eq!(e("minecraft:oak_slab[waterlogged=true]"), Effect::Water);
        assert_eq!(e("minecraft:slime_block"), Effect::Bounce);
        assert_eq!(e("minecraft:red_bed"), Effect::Bounce);
        assert_eq!(e("minecraft:cobweb"), Effect::Stuck);
        assert_eq!(e("minecraft:sweet_berry_bush"), Effect::Stuck);
        assert_eq!(e("minecraft:powder_snow"), Effect::Stuck);
        assert_eq!(e("minecraft:stone"), Effect::None);
    }

    #[test]
    fn collision_shell_and_unknown() {
        let lo = BlockPos { x: 0, y: 0, z: 0 };
        let mut w = World::air(lo, BlockPos { x: 9, y: 9, z: 9 });
        w.set(BlockPos { x: 4, y: 3, z: 4 }, "minecraft:oak_fence");
        w.set(BlockPos { x: 6, y: 3, z: 4 }, "minecraft:stone");
        // a box above the fence's cell, inside its 1.5-high shape
        let area = Aabb {
            min: [4.2, 4.1, 4.2],
            max: [4.8, 4.4, 4.8],
        };
        let b = w.collision_boxes(area).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].max[1], 4.5);
        // a box resting on stone touches it, which is no overlap
        let area = Aabb {
            min: [6.2, 4.0, 4.2],
            max: [6.8, 5.0, 4.8],
        };
        assert!(w.collision_boxes(area).unwrap().is_empty());
        // a box reaching into it overlaps
        let area = Aabb {
            min: [6.2, 3.9, 4.2],
            max: [6.8, 5.0, 4.8],
        };
        assert_eq!(
            w.collision_boxes(area).unwrap(),
            vec![Aabb {
                min: [6.0, 3.0, 4.0],
                max: [7.0, 4.0, 5.0]
            }]
        );
        // the shell reaches unknown space at x = -1
        let area = Aabb {
            min: [0.2, 4.0, 4.2],
            max: [0.8, 5.0, 4.8],
        };
        assert_eq!(
            w.collision_boxes(area),
            Err(UnknownSpace(BlockPos { x: -1, y: 3, z: 3 }))
        );
        assert_eq!(w.cell(BlockPos { x: 10, y: 0, z: 0 }), Cell::Unknown);
        assert_eq!(
            w.cell(BlockPos { x: 9, y: 9, z: 9 }),
            Cell::Known(table().air)
        );
    }
}
