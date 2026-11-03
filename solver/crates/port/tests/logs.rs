//! Logger logs (`mod/`, `T` records): exact client state at START (`s`) and END (`e`)
//! of each tick. Each airborne tick without collision flags is replayed from its `s`
//! record in an empty world and compared with its `e` record.
//!
//! The logs live outside the repo (`common::log_dir`, override with `ETAS_LOG_DIR`);
//! a missing log is skipped with a message, not a failure.

mod common;

use common::{TRecord, log_dir, read_tick_pairs};
use port::{BlockMap, Input, LocalPlayer, Pose, TickInput, Vec3, tick};

/// Logs with long glides: two singleplayer, two EMC. `clean`: every replayed tick must
/// match. The other two have ticks the empty world can't explain (measured
/// 2026-11-02 22:15 EST): 01-35-06 has creative flight (no glide start on a jump
/// edge, hovering at v_y = 0); 17-17-51 is `undeploy_test` under the EMC hub's roof
/// (a ceiling hit with no collision flag, crouching where standing doesn't fit) and a
/// firework boost (the rocket sets the velocity after the player's move, so the
/// position matches and the velocity doesn't).
const LOGS: [(&str, bool); 4] = [
    ("2026-10-24_14-49-59.tsv.gz", true),  // singleplayer
    ("2026-10-20_01-35-06.tsv.gz", false), // singleplayer
    ("2026-10-20_01-41-33.tsv.gz", true),  // EMC
    ("2026-10-29_17-17-51.tsv.gz", false), // EMC
];

fn err(a: Vec3, b: Vec3) -> f64 {
    (a.x - b.x)
        .abs()
        .max((a.y - b.y).abs())
        .max((a.z - b.z).abs())
}

/// The player at START of a tick, from its `s` record. The pose isn't logged; it is
/// taken from the glide flag and shift. why? in vanilla the server's glide-off brings
/// its pose with the flag (entity data). The undeploy mod can drop that pose
/// (`pose_dropped`), leaving `FALL_FLYING` (crawl speed) for a tick, which this misses.
fn state_from(s: &TRecord) -> LocalPlayer {
    let mut p = LocalPlayer::new(s.pos, s.y_rot, s.x_rot);
    p.delta_movement = s.vel;
    p.on_ground = s.on_ground;
    p.horizontal_collision = s.h_coll;
    p.fall_flying = s.fall_flying;
    p.sprinting = s.sprinting;
    p.sprint_speed_modifier = s.sprinting;
    // the keys of the last `KeyboardInput.tick()`
    let prev_keys = Input::from_bits(s.input);
    p.input.tick(prev_keys);
    p.pose = if s.fall_flying {
        Pose::FallFlying
    } else if prev_keys.shift {
        Pose::Crouching
    } else {
        Pose::Standing
    };
    p
}

#[derive(Default, Debug)]
struct Stats {
    /// Airborne at start and end, no horizontal collision at either.
    candidates: usize,
    /// Position and velocity identical: no player tick ran (singleplayer paused, or
    /// `connection.hasClientLoaded()` false while a world loads). Not replayed.
    frozen: usize,
    gliding: usize,
    exact: usize,
    /// Within 1e-12 but not identical.
    close: usize,
    worst_close: f64,
    mismatches: Vec<String>,
}

fn replay_log(name: &str) -> Option<Stats> {
    let path = log_dir().join(name);
    if !path.exists() {
        println!("{name}: not found in {}, skipped", log_dir().display());
        return None;
    }
    let level = BlockMap::default();
    let mut st = Stats::default();
    for pair in read_tick_pairs(&path) {
        let (s, e) = (&pair.s, &pair.e);
        if s.on_ground || e.on_ground || s.h_coll || e.h_coll {
            continue;
        }
        st.candidates += 1;
        if s.pos == e.pos && s.vel == e.vel {
            st.frozen += 1;
            continue;
        }
        st.gliding += s.fall_flying as usize;
        let mut p = state_from(s);
        // the rotation physics saw: the mouse turns the player between ticks, and a
        // tick handler (tas_play) sets it after the START record
        let input = TickInput {
            y_rot: e.y_rot,
            x_rot: e.x_rot,
            keys: Input::from_bits(e.input),
        };
        let r = tick(&mut p, &input, &level);
        let pe = err(p.position, e.pos);
        let ve = err(p.delta_movement, e.vel);
        let flags_ok = p.fall_flying == e.fall_flying
            && p.sprinting == e.sprinting
            && !p.on_ground
            && !p.horizontal_collision;
        if r.is_ok() && flags_ok && pe.max(ve) == 0.0 {
            st.exact += 1;
        } else if r.is_ok() && flags_ok && pe.max(ve) < 1e-12 {
            st.close += 1;
            st.worst_close = st.worst_close.max(pe.max(ve));
        } else {
            st.mismatches.push(format!(
                "tick {}: glide {}->{} (port {}), sprint {}->{} (port {}), keys {}->{}: pos {pe:.2e} vel {ve:.2e} {:?}",
                s.tick,
                s.fall_flying as u8,
                e.fall_flying as u8,
                p.fall_flying as u8,
                s.sprinting as u8,
                e.sprinting as u8,
                p.sprinting as u8,
                s.input,
                e.input,
                r.err()
            ));
        }
    }
    Some(st)
}

#[test]
fn logger_airborne_ticks_reproduce() {
    let mut any = false;
    for (name, clean) in LOGS {
        let Some(st) = replay_log(name) else { continue };
        any = true;
        let replayed = st.candidates - st.frozen;
        println!(
            "{name}: {replayed} airborne ticks without collision flags replayed ({} gliding; {} frozen skipped): \
             {} exact, {} within 1e-12 (worst {:.1e}), {} mismatched ({:.2}% matched)",
            st.gliding,
            st.frozen,
            st.exact,
            st.close,
            st.worst_close,
            st.mismatches.len(),
            100.0 * (st.exact + st.close) as f64 / replayed as f64
        );
        for m in st.mismatches.iter().take(8) {
            println!("  {m}");
        }
        assert!(replayed > 1000, "{name}: only {replayed} ticks replayed");
        if clean {
            assert!(
                st.mismatches.is_empty(),
                "{name}: {} mismatched",
                st.mismatches.len()
            );
        } else {
            assert!(
                st.mismatches.len() * 20 < replayed,
                "{name}: over 5% mismatched"
            );
        }
    }
    if !any {
        println!("no logs found in {}; nothing checked", log_dir().display());
    }
}
