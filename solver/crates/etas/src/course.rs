//! Course files: EMC's rules for one map, fitted by `fit_course.py` (whose docstring is
//! the format's spec), and the world they refer to.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use port::{LocalPlayer, TickInput, Vec3};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use world::World;

use crate::booster::Booster;
use crate::state::keys_of;

const FORMAT: &str = "etas-course";
/// Version 1 is version 2 with no boosters.
const VERSION: u32 = 2;

/// A box over the observed position, `min <= p <= max`; `None` is unbounded.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Bounds {
    pub min: [Option<f64>; 3],
    pub max: [Option<f64>; 3],
}

impl Bounds {
    pub fn contains(&self, p: [f64; 3]) -> bool {
        (0..3)
            .all(|k| self.min[k].is_none_or(|m| m <= p[k]) && self.max[k].is_none_or(|m| p[k] <= m))
    }

    /// The first parameter `s` in `[0, 1]` at which the segment `a + s (b - a)` is
    /// inside (`enter`) or outside (`!enter`), or None. Slab method, so for leaving it is
    /// where the segment crosses the box's surface last before being outside.
    pub fn crossing(&self, a: [f64; 3], b: [f64; 3], enter: bool) -> Option<f64> {
        let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
        for k in 0..3 {
            let d = b[k] - a[k];
            let (mn, mx) = (
                self.min[k].unwrap_or(f64::NEG_INFINITY),
                self.max[k].unwrap_or(f64::INFINITY),
            );
            if d == 0.0 {
                if a[k] < mn || a[k] > mx {
                    (lo, hi) = (1.0, 0.0);
                }
                continue;
            }
            let (t0, t1) = ((mn - a[k]) / d, (mx - a[k]) / d);
            lo = lo.max(t0.min(t1));
            hi = hi.min(t0.max(t1));
        }
        if enter {
            (lo <= hi && hi >= 0.0 && lo <= 1.0).then(|| lo.max(0.0))
        } else if self.contains(a) && lo <= hi {
            (hi <= 1.0).then(|| hi.max(0.0))
        } else {
            Some(0.0)
        }
    }
}

/// Which box of a region's bracket a rule is evaluated with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Working,
    Inner,
    Outer,
}

impl Variant {
    pub const ALL: [Variant; 3] = [Variant::Working, Variant::Inner, Variant::Outer];

    pub fn name(self) -> &'static str {
        match self {
            Variant::Working => "working",
            Variant::Inner => "inner",
            Variant::Outer => "outer",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Region {
    pub working: Bounds,
    pub inner: Bounds,
    pub outer: Bounds,
    pub faces: BTreeMap<String, String>,
    pub status: String,
    #[serde(default)]
    pub note: String,
}

impl Region {
    pub fn bounds(&self, v: Variant) -> &Bounds {
        match v {
            Variant::Working => &self.working,
            Variant::Inner => &self.inner,
            Variant::Outer => &self.outer,
        }
    }
}

/// A predicate over a server-observed state.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pred {
    In(String),
    Not(Box<Pred>),
    And(Vec<Pred>),
    Or(Vec<Pred>),
    Field(String),
}

/// What the server sees of one client tick: the end pose its move packet carries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observed {
    pub pos: [f64; 3],
    pub on_ground: bool,
    pub fall_flying: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub name: Option<String>,
    /// `first` or `any`.
    pub event: String,
    pub when: Pred,
    pub after: String,
    #[serde(default)]
    pub until: Option<String>,
    pub status: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rules {
    pub timer_start: Rule,
    pub finish: Rule,
    pub kill: Vec<Rule>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorldRef {
    pub path: PathBuf,
    pub content_sha256: String,
    #[serde(default)]
    pub extract: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Observation {
    /// [lo, hi] ticks from the judged pose to a tick-end chat, tape numbering.
    pub tick_chat_lag: [usize; 2],
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceRef {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Start {
    pub source: SourceRef,
    /// x y z as double bits, yaw pitch as float bits.
    pub pose: [String; 5],
    /// [yaw bits, pitch bits, keys] for ticks 1..=glide_tick.
    pub inputs: Vec<[String; 3]>,
    pub glide_tick: usize,
    pub timer_tick: usize,
    pub finish_tick: usize,
    pub finish_text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Course {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub dimension: String,
    pub world: WorldRef,
    pub fitted_from: serde_json::Value,
    pub observation: Observation,
    pub start: Start,
    pub regions: BTreeMap<String, Region>,
    pub rules: Rules,
    pub ground_allowed: Vec<String>,
    #[serde(default)]
    pub boosters: Vec<Booster>,
    /// The directory the course file is in (not in the file).
    #[serde(skip)]
    pub dir: PathBuf,
    /// sha256 (hex) of the course file's bytes (not in the file).
    #[serde(skip)]
    pub sha256: String,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn bits32(s: &str) -> Result<f32, String> {
    u32::from_str_radix(s, 16)
        .map(f32::from_bits)
        .map_err(|e| format!("float bits {s:?}: {e}"))
}

fn bits64(s: &str) -> Result<f64, String> {
    u64::from_str_radix(s, 16)
        .map(f64::from_bits)
        .map_err(|e| format!("double bits {s:?}: {e}"))
}

impl Course {
    /// Load `DIR/course.json` (or a course file path) and its world, checking the
    /// world's hash. why check? the rules were fitted against those blocks; a
    /// re-extracted world with other content needs a refit, or at least a look.
    pub fn load(path: &Path) -> Result<(Course, World), String> {
        let file = if path.is_dir() {
            path.join("course.json")
        } else {
            path.to_path_buf()
        };
        let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let mut c: Course =
            serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", file.display()))?;
        if c.format != FORMAT || !(1..=VERSION).contains(&c.version) {
            return Err(format!(
                "{}: format {} version {}, reader knows {FORMAT} 1..={VERSION}",
                file.display(),
                c.format,
                c.version
            ));
        }
        c.dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        c.sha256 = hex(&Sha256::digest(&bytes));
        for r in [&c.rules.timer_start, &c.rules.finish]
            .into_iter()
            .chain(&c.rules.kill)
        {
            c.check_pred(&r.when)?;
        }
        for g in &c.ground_allowed {
            c.region(g)?;
        }
        let wpath = c.dir.join(&c.world.path);
        let w = World::load(&wpath).map_err(|e| e.to_string())?;
        let got = w
            .header()
            .map(|h| h.content_sha256.clone())
            .unwrap_or_default();
        if got != c.world.content_sha256 {
            return Err(format!(
                "{}: content {got}, the course was fitted against {}",
                wpath.display(),
                c.world.content_sha256
            ));
        }
        Ok((c, w))
    }

    pub fn region(&self, name: &str) -> Result<&Region, String> {
        self.regions
            .get(name)
            .ok_or_else(|| format!("no region {name}"))
    }

    fn check_pred(&self, p: &Pred) -> Result<(), String> {
        match p {
            Pred::In(r) => self.region(r).map(|_| ()),
            Pred::Not(q) => self.check_pred(q),
            Pred::And(v) | Pred::Or(v) => v.iter().try_for_each(|q| self.check_pred(q)),
            Pred::Field(f) if f == "on_ground" || f == "fall_flying" => Ok(()),
            Pred::Field(f) => Err(format!("unknown field {f}")),
        }
    }

    pub fn eval(&self, p: &Pred, o: &Observed, v: Variant) -> bool {
        match p {
            Pred::In(r) => self.regions[r].bounds(v).contains(o.pos),
            Pred::Not(q) => !self.eval(q, o, v),
            Pred::And(qs) => qs.iter().all(|q| self.eval(q, o, v)),
            Pred::Or(qs) => qs.iter().any(|q| self.eval(q, o, v)),
            Pred::Field(f) => match f.as_str() {
                "on_ground" => o.on_ground,
                _ => o.fall_flying,
            },
        }
    }

    pub fn ground_allowed(&self, pos: [f64; 3], v: Variant) -> bool {
        self.ground_allowed
            .iter()
            .any(|g| self.regions[g].bounds(v).contains(pos))
    }

    /// The pose after the reset teleport, at rest: what a tape's tick 0 is.
    pub fn start_state(&self) -> Result<LocalPlayer, String> {
        let p = &self.start.pose;
        let pos = [bits64(&p[0])?, bits64(&p[1])?, bits64(&p[2])?];
        Ok(rest_state(pos, bits32(&p[3])?, bits32(&p[4])?))
    }

    /// The recorded prefix's inputs, ticks 1..=glide_tick.
    pub fn start_inputs(&self) -> Result<Vec<(TickInput, String)>, String> {
        self.start
            .inputs
            .iter()
            .map(|[y, p, k]| {
                Ok((
                    TickInput {
                        y_rot: bits32(y)?,
                        x_rot: bits32(p)?,
                        keys: keys_of(k)?,
                    },
                    k.clone(),
                ))
            })
            .collect()
    }

    /// The repo root, for `start.source.path`: the course file is in `solver/courses/<name>/`.
    pub fn repo_root(&self) -> PathBuf {
        self.dir.join("../../..")
    }
}

/// A player standing at rest at `pos` after a teleport: `LocalPlayer::new` with the
/// vertical velocity a standing player settles at, `(0 - 0.08) * 0.98F` (gravity, then
/// drag; the move clips it, the velocity keeps it). port's tape test starts the same way.
pub fn rest_state(pos: [f64; 3], yaw: f32, pitch: f32) -> LocalPlayer {
    let mut p = LocalPlayer::new(Vec3::new(pos[0], pos[1], pos[2]), yaw, pitch);
    p.delta_movement = Vec3::new(0.0, (0.0 - 0.08) * 0.98f32 as f64, 0.0);
    p
}
