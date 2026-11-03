//! The oracle's cases (`solver/oracle`, a Fabric client gametest): Minecraft's own
//! trajectories in random worlds, replayed through port. Each case directory holds the
//! logger's log of the case (`log.tsv.gz`, `T` records) and `oracle.tsv`: the world
//! (`A` air box, `B` blocks) and the `P`/`Q` records with the state the log lacks, at
//! the head and return of `LocalPlayer.tick`. The format is in solver/oracle/README.md.
//!
//! Two replays per case. Continuous: from the first tick's logged state, each tick
//! from port's own previous result, applying only the flags, pose and sprint speed
//! modifier the server changed between ticks (the glide-off after a landing, its view
//! of sprinting); it stops at the first mismatch or any other change between ticks. Per tick: each
//! tick from the logged state at its start. Every field is compared exactly.
//!
//! Cases live in `solver/oracle/cases/` (`ORACLE_CASES_DIR` overrides). None there
//! is a skip with a message, not a failure.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use common::{TRecord, TickPair, read_tick_pairs};
use port::{BlockPos, Input, LocalPlayer, Pose, TickInput, Vec2, Vec3, tick};
use world::World;

fn cases_dir() -> PathBuf {
    match std::env::var_os("ORACLE_CASES_DIR") {
        Some(d) => PathBuf::from(d),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../oracle/cases"),
    }
}

/// A `P` or `Q` record's state fields.
#[derive(Debug, Clone, PartialEq)]
struct Hidden {
    pose: Pose,
    on_ground: bool,
    h_coll: bool,
    v_coll: bool,
    v_coll_below: bool,
    minor_h_coll: bool,
    support: Option<BlockPos>,
    on_ground_no_blocks: bool,
    fall_distance: f64,
    fall_flying: bool,
    sprinting: bool,
    sprint_mod: bool,
    no_jump_delay: i32,
    jumping: bool,
    xxa: f32,
    yya: f32,
    zza: f32,
    keys: u32,
    move_vector: Vec2,
    sprint_trigger_time: i32,
    auto_jump_time: i32,
    crouching: bool,
}

/// A `P` record: the input the driver set for the tick, and the state physics starts from.
#[derive(Debug, Clone)]
struct Pre {
    yaw: f32,
    pitch: f32,
    /// The game JVM's `Math.cos` of the tick's lean angle (the lift).
    cos: f64,
    h: Hidden,
}

struct Case {
    name: String,
    kind: String,
    meta: HashMap<String, String>,
    world: World,
    pre: BTreeMap<u64, Pre>,
    post: BTreeMap<u64, Hidden>,
    pairs: Vec<TickPair>,
}

fn pose(s: &str) -> Pose {
    match s {
        "STANDING" => Pose::Standing,
        "FALL_FLYING" => Pose::FallFlying,
        "SWIMMING" => Pose::Swimming,
        "SPIN_ATTACK" => Pose::SpinAttack,
        "CROUCHING" => Pose::Crouching,
        p => panic!("pose {p} is not one port has"),
    }
}

fn hidden(f: &[&str]) -> Hidden {
    let b = |i: usize| f[i] == "1";
    // floats are written widened to double, so the cast back is exact
    let fl = |i: usize| f[i].parse::<f64>().unwrap() as f32;
    let int = |i: usize| f[i].parse::<i32>().unwrap();
    Hidden {
        pose: pose(f[0]),
        on_ground: b(1),
        h_coll: b(2),
        v_coll: b(3),
        v_coll_below: b(4),
        minor_h_coll: b(5),
        support: (f[6] != "-").then(|| {
            let c: Vec<i32> = f[6].split(',').map(|v| v.parse().unwrap()).collect();
            BlockPos {
                x: c[0],
                y: c[1],
                z: c[2],
            }
        }),
        on_ground_no_blocks: b(7),
        fall_distance: f[8].parse().unwrap(),
        fall_flying: b(9),
        sprinting: b(10),
        sprint_mod: b(11),
        no_jump_delay: int(12),
        jumping: b(13),
        xxa: fl(14),
        yya: fl(15),
        zza: fl(16),
        keys: f[17].parse().unwrap(),
        move_vector: Vec2::new(fl(18), fl(19)),
        sprint_trigger_time: int(20),
        auto_jump_time: int(21),
        crouching: b(22),
    }
}

fn read_case(dir: &Path) -> Case {
    let text = std::fs::read_to_string(dir.join("oracle.tsv")).unwrap();
    let mut meta = HashMap::new();
    let mut world = None;
    let mut pre = BTreeMap::new();
    let mut post = BTreeMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        match f[0] {
            "M" => {
                for kv in &f[1..] {
                    let (k, v) = kv.split_once('=').unwrap();
                    meta.insert(k.to_string(), v.to_string());
                }
            }
            "A" => {
                let c: Vec<i32> = f[1..7].iter().map(|v| v.parse().unwrap()).collect();
                world = Some(World::air(
                    BlockPos {
                        x: c[0],
                        y: c[1],
                        z: c[2],
                    },
                    BlockPos {
                        x: c[3],
                        y: c[4],
                        z: c[5],
                    },
                ));
            }
            "B" => {
                let w = world.as_mut().expect("B record before A");
                let pos = BlockPos {
                    x: f[1].parse().unwrap(),
                    y: f[2].parse().unwrap(),
                    z: f[3].parse().unwrap(),
                };
                w.set(pos, f[4]);
            }
            "P" => {
                pre.insert(
                    f[1].parse().unwrap(),
                    Pre {
                        yaw: f[2].parse::<f64>().unwrap() as f32,
                        pitch: f[3].parse::<f64>().unwrap() as f32,
                        cos: f[5].parse().unwrap(),
                        h: hidden(&f[6..]),
                    },
                );
            }
            "Q" => {
                post.insert(f[1].parse().unwrap(), hidden(&f[2..]));
            }
            _ => {}
        }
    }
    Case {
        name: dir.file_name().unwrap().to_string_lossy().into_owned(),
        kind: meta.get("kind").cloned().unwrap_or_default(),
        meta,
        world: world.expect("no A record"),
        pre,
        post,
        pairs: read_tick_pairs(&dir.join("log.tsv.gz")),
    }
}

/// The player at the head of `LocalPlayer.tick`: the logger's START record and the `P`
/// record, which also holds the keys and `autoJumpTime` the driver set for this tick.
fn state_from(s: &TRecord, h: &Hidden) -> LocalPlayer {
    let mut p = LocalPlayer::new(s.pos, s.y_rot, s.x_rot);
    p.delta_movement = s.vel;
    p.on_ground = s.on_ground;
    p.horizontal_collision = s.h_coll;
    p.vertical_collision = h.v_coll;
    p.vertical_collision_below = h.v_coll_below;
    p.minor_horizontal_collision = h.minor_h_coll;
    p.main_supporting_block_pos = h.support;
    p.on_ground_no_blocks = h.on_ground_no_blocks;
    p.fall_distance = h.fall_distance;
    p.pose = h.pose;
    p.fall_flying = s.fall_flying;
    p.sprinting = s.sprinting;
    p.no_jump_delay = h.no_jump_delay;
    p.jumping = h.jumping;
    p.xxa = h.xxa;
    p.yya = h.yya;
    p.zza = h.zza;
    p.sprint_speed_modifier = h.sprint_mod;
    set_input(&mut p, h);
    p.sprint_trigger_time = h.sprint_trigger_time;
    p.crouching = h.crouching;
    p
}

/// What the driver sets before each tick: a plain `ClientInput`'s keys and move vector,
/// and `autoJumpTime` (1 on a jump press). Its `tick()` is empty, so port's
/// `input.tick(keys)` with the same keys leaves them as they are.
fn set_input(p: &mut LocalPlayer, h: &Hidden) {
    p.input.key_presses = Input::from_bits(h.keys);
    p.input.move_vector = h.move_vector;
    p.auto_jump_time = h.auto_jump_time;
}

fn input_bits(i: Input) -> u32 {
    i.forward as u32
        | (i.backward as u32) << 1
        | (i.left as u32) << 2
        | (i.right as u32) << 3
        | (i.jump as u32) << 4
        | (i.shift as u32) << 5
        | (i.sprint as u32) << 6
}

/// The first field where port's result differs from the game's END state, as
/// `field: port X, game Y`.
fn compare(p: &LocalPlayer, e: &TRecord, q: &Hidden) -> Option<String> {
    macro_rules! check {
        ($name:expr, $port:expr, $game:expr) => {
            if $port != $game {
                return Some(format!("{}: port {:?}, game {:?}", $name, $port, $game));
            }
        };
    }
    check!("pos", p.position, e.pos);
    check!("vel", p.delta_movement, e.vel);
    check!("y_rot", p.y_rot, e.y_rot);
    check!("x_rot", p.x_rot, e.x_rot);
    check!("on_ground", p.on_ground, e.on_ground);
    check!("horizontal_collision", p.horizontal_collision, e.h_coll);
    check!("fall_flying", p.fall_flying, e.fall_flying);
    check!("sprinting", p.sprinting, e.sprinting);
    check!("key_presses", input_bits(p.input.key_presses), e.input);
    check!("pose", p.pose, q.pose);
    check!("vertical_collision", p.vertical_collision, q.v_coll);
    check!(
        "vertical_collision_below",
        p.vertical_collision_below,
        q.v_coll_below
    );
    check!(
        "minor_horizontal_collision",
        p.minor_horizontal_collision,
        q.minor_h_coll
    );
    check!(
        "main_supporting_block_pos",
        p.main_supporting_block_pos,
        q.support
    );
    check!(
        "on_ground_no_blocks",
        p.on_ground_no_blocks,
        q.on_ground_no_blocks
    );
    check!("fall_distance", p.fall_distance, q.fall_distance);
    check!(
        "sprint_speed_modifier",
        p.sprint_speed_modifier,
        q.sprint_mod
    );
    check!("no_jump_delay", p.no_jump_delay, q.no_jump_delay);
    check!("jumping", p.jumping, q.jumping);
    check!("xxa", p.xxa, q.xxa);
    check!("yya", p.yya, q.yya);
    check!("zza", p.zza, q.zza);
    check!("move_vector", p.input.move_vector, q.move_vector);
    check!(
        "sprint_trigger_time",
        p.sprint_trigger_time,
        q.sprint_trigger_time
    );
    check!("auto_jump_time", p.auto_jump_time, q.auto_jump_time);
    check!("crouching", p.crouching, q.crouching);
    None
}

/// The tick glided and the game JVM's `Math.cos` of the lean angle differs from fdlibm,
/// which port uses: a last-place difference in the lift, noise by ARCHITECTURE.md's
/// standard ("Testing the physics"), not a port bug.
fn jvm_cos_differs(s: &TRecord, pre: &Pre, p: &LocalPlayer) -> bool {
    let lean = p.x_rot * (std::f64::consts::PI / 180.0) as f32;
    s.fall_flying && port::mth::java_cos(lean as f64) != pre.cos
}

fn err(a: Vec3, b: Vec3) -> f64 {
    (a.x - b.x)
        .abs()
        .max((a.y - b.y).abs())
        .max((a.z - b.z).abs())
}

/// What changed between one tick's END and the next tick's START: the server's doing,
/// since nothing else touches the player between ticks in a gametest.
fn between_ticks(e: &TRecord, q: &Hidden, s: &TRecord, h: &Hidden) -> Vec<&'static str> {
    let mut v = Vec::new();
    if e.pos != s.pos || e.vel != s.vel {
        v.push("pos/vel");
    }
    if e.fall_flying != s.fall_flying {
        v.push("fall_flying");
    }
    if e.sprinting != s.sprinting {
        v.push("sprinting");
    }
    if e.on_ground != s.on_ground || e.h_coll != s.h_coll {
        v.push("collision flags");
    }
    if q.pose != h.pose {
        v.push("pose");
    }
    if q.sprint_mod != h.sprint_mod {
        // update_attributes: the server's sprinting modifier on movement speed
        v.push("sprint_speed_modifier");
    }
    v
}

#[derive(Default)]
struct Result {
    ticks: usize,
    /// Continuous replay: ticks bit-exact before the first mismatch, and that mismatch.
    continuous: usize,
    continuous_end: Option<String>,
    /// Continuous replay: ticks off only by the JVM's cos, after which it resumed from
    /// the logged state.
    continuous_noise: usize,
    per_tick_exact: usize,
    /// Per tick: mismatched ticks explained by the JVM's cos.
    per_tick_noise: usize,
    per_tick_mismatches: Vec<String>,
    glide_offs: usize,
    /// Ticks the server changed the flags, pose or sprint modifier before (glide-offs
    /// included).
    flag_updates: usize,
}

fn replay(case: &Case) -> Result {
    let mut r = Result::default();
    let ticks: Vec<&TickPair> = case
        .pairs
        .iter()
        .filter(|t| case.pre.contains_key(&t.s.tick) && case.post.contains_key(&t.s.tick))
        .collect();
    let unrecorded = case.pairs.len() - ticks.len();
    assert_eq!(
        unrecorded, 0,
        "{}: {unrecorded} logged ticks have no P/Q record",
        case.name
    );
    r.ticks = ticks.len();

    // continuous
    let first = ticks[0];
    let mut p = state_from(&first.s, &case.pre[&first.s.tick].h);
    for (i, t) in ticks.iter().enumerate() {
        let pre = &case.pre[&t.s.tick];
        let q = &case.post[&t.s.tick];
        if i > 0 {
            let prev = ticks[i - 1];
            if prev.s.tick + 1 != t.s.tick {
                r.continuous_end = Some(format!("tick {}: ticks missing from the log", t.s.tick));
                break;
            }
            let changed = between_ticks(&prev.e, &case.post[&prev.s.tick], &t.s, &pre.h);
            if changed.iter().any(|c| {
                !matches!(
                    *c,
                    "fall_flying" | "sprinting" | "pose" | "sprint_speed_modifier"
                )
            }) {
                r.continuous_end = Some(format!(
                    "tick {}: the server changed {changed:?} between ticks",
                    t.s.tick
                ));
                break;
            }
            if changed.contains(&"fall_flying") {
                r.glide_offs += 1;
            }
            if !changed.is_empty() {
                // the server's shared flags (glide-off, its view of sprinting) and pose,
                // from set_entity_data, and its sprinting speed modifier, from
                // update_attributes; both arrive a tick behind the client
                r.flag_updates += 1;
                p.fall_flying = t.s.fall_flying;
                p.sprinting = t.s.sprinting;
                p.pose = pre.h.pose;
                p.sprint_speed_modifier = pre.h.sprint_mod;
            }
        }
        set_input(&mut p, &pre.h);
        // what the server didn't change must be port's own result
        let mut logged = state_from(&t.s, &pre.h);
        logged.y_rot = p.y_rot;
        logged.x_rot = p.x_rot;
        if logged != p {
            let (a, b) = (format!("{p:#?}"), format!("{logged:#?}"));
            let (x, y) = a.lines().zip(b.lines()).find(|(x, y)| x != y).unwrap();
            r.continuous_end = Some(format!(
                "start of tick {}: port {}, game {}",
                t.s.tick,
                x.trim(),
                y.trim()
            ));
            break;
        }
        let input = TickInput {
            y_rot: pre.yaw,
            x_rot: pre.pitch,
            keys: Input::from_bits(pre.h.keys),
        };
        let mismatch = match tick(&mut p, &input, &case.world) {
            Err(u) => Some(format!("port: {u:?}")),
            Ok(_) => compare(&p, &t.e, q),
        };
        if let Some(m) = mismatch {
            if jvm_cos_differs(&t.s, pre, &p) {
                r.continuous_noise += 1;
                if let Some(next) = ticks.get(i + 1) {
                    p = state_from(&next.s, &case.pre[&next.s.tick].h);
                    // the glide-off check above compares the log with itself from here
                }
                continue;
            }
            r.continuous_end = Some(format!("tick {}: {m}", t.s.tick));
            break;
        }
        r.continuous += 1;
    }

    // per tick
    for t in &ticks {
        let pre = &case.pre[&t.s.tick];
        let q = &case.post[&t.s.tick];
        let mut p = state_from(&t.s, &pre.h);
        let input = TickInput {
            y_rot: pre.yaw,
            x_rot: pre.pitch,
            keys: Input::from_bits(pre.h.keys),
        };
        let mismatch = match tick(&mut p, &input, &case.world) {
            Err(u) => Some(format!("port: {u:?}")),
            Ok(_) => compare(&p, &t.e, q).map(|m| {
                format!(
                    "{m} (pos off {:.1e}, vel off {:.1e}; start: ground {} glide {} pose {:?} keys {})",
                    err(p.position, t.e.pos),
                    err(p.delta_movement, t.e.vel),
                    t.s.on_ground as u8,
                    t.s.fall_flying as u8,
                    pre.h.pose,
                    pre.h.keys
                )
            }),
        };
        match mismatch {
            None => r.per_tick_exact += 1,
            Some(_) if jvm_cos_differs(&t.s, pre, &p) => r.per_tick_noise += 1,
            Some(m) => r
                .per_tick_mismatches
                .push(format!("tick {}: {m}", t.s.tick)),
        }
    }
    r
}

#[test]
fn oracle_cases_replay() {
    let dir = cases_dir();
    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join("oracle.tsv").exists() && p.join("log.tsv.gz").exists())
            .collect(),
        Err(_) => Vec::new(),
    };
    if dirs.is_empty() {
        println!(
            "no oracle cases in {}; nothing checked (solver/oracle/README.md makes them)",
            dir.display()
        );
        return;
    }
    dirs.sort();
    // cases, continuous-exact cases, ticks, ticks exact per tick, ticks off by the JVM's cos
    let mut by_kind: BTreeMap<String, [usize; 5]> = BTreeMap::new();
    let mut report = String::new();
    let mut failed = Vec::new();
    for d in &dirs {
        let case = read_case(d);
        let r = replay(&case);
        let k = by_kind.entry(case.kind.clone()).or_default();
        k[0] += 1;
        k[1] += (r.continuous_end.is_none()) as usize;
        k[2] += r.ticks;
        k[3] += r.per_tick_exact;
        k[4] += r.per_tick_noise;
        let _ = writeln!(
            report,
            "{} ({}): {} ticks, continuous exact {}/{} (+{} JVM cos){}, per tick exact {}/{} (+{} JVM cos), glide-offs {} (server flag updates {})",
            case.name,
            case.meta.get("structure").map_or("-", |s| s.as_str()),
            r.ticks,
            r.continuous,
            r.ticks,
            r.continuous_noise,
            r.continuous_end
                .as_ref()
                .map_or(String::new(), |e| format!(" (stopped: {e})")),
            r.per_tick_exact,
            r.ticks,
            r.per_tick_noise,
            r.glide_offs,
            r.flag_updates
        );
        for m in r.per_tick_mismatches.iter().take(5) {
            let _ = writeln!(report, "    {m}");
        }
        if r.per_tick_mismatches.len() > 5 {
            let _ = writeln!(report, "    ... {} more", r.per_tick_mismatches.len() - 5);
        }
        if r.continuous_end.is_some() || !r.per_tick_mismatches.is_empty() {
            failed.push(case.name.clone());
        }
    }
    println!("{report}");
    for (kind, [n, cont, ticks, exact, noise]) in &by_kind {
        println!(
            "{kind}: {cont}/{n} cases continuous-exact, {exact}/{ticks} ticks exact per tick ({:.2}%), {noise} more off only by the JVM's cos",
            100.0 * *exact as f64 / *ticks as f64
        );
    }
    assert!(
        failed.is_empty(),
        "{} cases mismatched: {failed:?}",
        failed.len()
    );
}
