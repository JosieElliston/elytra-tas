//! Phase `solve`: the course's seed run (`ebounce_run4.tas`, run 4 of the 2026-10-19
//! ebounce replay, whose bounces lost the glide) re-solved into a plan with perfect
//! ebounces and a branch to a lost-glide flight wherever a bounce's glide-off can come
//! at a tick no jump press rescues. Writes `runs/main.traj.jsonl` (the main line),
//! `runs/lost_*.traj.jsonl` (each lost-glide path) and `runs/downtown.tas` (the plan),
//! then verifies every path of the plan.
//!
//! 1. Perfect bounces. For each glide-off the recording got, the stretch from before it
//!    to before the next landing is re-solved (`solve::bounces::resolve_stretch`:
//!    reach the recorded state there in the fewest ticks, DDP on the generic kernel,
//!    repaired under the faithful kernel) without the glide-off, so the bounce keeps
//!    its glide and the rest of the recording flies on from the same state.
//! 2. The timed flight. The stretch from after the last bounce to just before the
//!    finish, re-solved the same way: the whole of the time saved.
//! 3. The plan. Each bounce gets the `@react … glide_off press_jump` window verify
//!    finds; the window must cover the natural delays (`verify::GLIDE_OFF_NATURAL`).
//!    Every delay in `GLIDE_OFF_MIN..=GLIDE_OFF_MAX` (up to the next landing) outside
//!    it gets `@branch <t> glide_off lost_<landing>_d<delay>`: the lost glide, the
//!    redeploy, and a re-solved stretch to the main line's state before its next
//!    landing (`solve::bounces::lost_glide`), then the main line's inputs. Where the
//!    lost glide can't win back the main line's state there, the path bounces on its own
//!    and rejoins after the main line's next bounce, or at the end of the timed stretch
//!    (which has no slack before it, so only more ticks get there). A lost path's
//!    own bounces get windows, not branches: a glide-off there outside its window
//!    aborts the attempt.
//!
//! `cargo run --release -p super_classic_downtown --bin solve`

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use etas::course::Course;
use etas::plan::{
    Action, Child, Event, Ignore, Plan, React, plan_from, verify_paths, with_children,
};
use etas::server::ServerEvent;
use etas::solve::bounces::{Lost, fly, lost_glide, normal_flight, resolve_stretch, state_at};
use etas::solve::ddp::Opts;
use etas::tape::Tape;
use etas::trajectory::{Header, InputRow, Trajectory, from_tape};
use etas::verify::{self, GLIDE_OFF_MAX, GLIDE_OFF_MIN, GLIDE_OFF_NATURAL, Report};
use world::World;

const OBJECTIVE: &str = "reach_recorded_state_min_ticks";
/// Ticks past the judged finish the plan flies, past the course's latest chat lag.
/// why? playback ends the attempt at the plan's last line, a success only if "Course
/// complete!" came by then; and the recording's tail lands and hits a wall, whose
/// glide-off and velocity echo the plan doesn't fly, so a longer tail diverges.
const TAIL_PAST_LAG: usize = 2;

fn opts() -> Opts {
    Opts {
        tol: 1e-6,
        ..Opts::default()
    }
}

fn landings(traj: &Trajectory) -> Vec<usize> {
    (1..=traj.rows.len())
        .filter(|&t| state_at(traj, t).on_ground && !state_at(traj, t - 1).on_ground)
        .collect()
}

/// Make the bounce whose glide-off came at `g` perfect: re-solve a stretch from a
/// tick before `g` (back to the tick after the bounce jump) to one before the next
/// landing, preferring the latest start and the latest end that solve.
fn perfect(
    world: &World,
    course: &Course,
    rec: &Trajectory,
    g: usize,
) -> Result<Trajectory, String> {
    let jump = (1..g)
        .rev()
        .find(|&t| state_at(rec, t - 1).on_ground)
        .ok_or("no bounce before the glide-off")?;
    let land = (g..=rec.rows.len())
        .find(|&t| state_at(rec, t).on_ground)
        .ok_or("no landing after the glide-off")?;
    let mut errors = Vec::new();
    for a in (jump..g).rev() {
        for b in (a + 6..=land.saturating_sub(3)).rev().take(5) {
            if !normal_flight(state_at(rec, a)) || !normal_flight(state_at(rec, b)) {
                continue;
            }
            let n = b - a;
            let t0 = Instant::now();
            let header = Header::new(Some(course), OBJECTIVE, "");
            match resolve_stretch(
                world,
                rec,
                a,
                b,
                (n * 6 / 10).max(3)..=n + 4,
                &opts(),
                header,
            ) {
                Ok(s) if s.stopped.is_none() => {
                    println!(
                        "  perfect bounce (glide-off {g}): stretch {a}..{b} in {} ticks ({:+}), end off by {:.1e} blocks, {:.1e} b/t, {:.2} s",
                        s.reached.inputs.len(),
                        -s.saved,
                        s.reached.pos_err,
                        s.reached.vel_err,
                        t0.elapsed().as_secs_f64()
                    );
                    return Ok(s.traj);
                }
                Ok(s) => errors.push(format!("{a}..{b}: port stopped {:?}", s.stopped)),
                Err(e) => errors.push(e),
            }
        }
    }
    Err(format!("glide-off {g}: {}", errors.join("; ")))
}

fn header_for(course: &Course, objective: &str, source: String) -> Header {
    let mut h = Header::new(Some(course), objective, &source);
    h.tas_header = Some(vec![("source".to_string(), source)]);
    h
}

fn summary(what: &str, r: &Report) -> String {
    format!(
        "{what}: {} ({} ticks, timer {:?} finish {:?}{})",
        if r.ok() { "PASS" } else { "FAIL" },
        r.ticks,
        r.timer_tick(),
        r.finish_tick(),
        match (r.timer_tick(), r.finish_tick()) {
            (Some(s), Some(f)) => format!(", timed {} ticks", f - s),
            _ => String::new(),
        }
    )
}

/// The `@ignore`s for what the server sends after the finish (the wall-hit echo of a
/// landing, its glide-off), from tick `from` to the segment's end.
fn tail_ignores(from: usize, end: usize) -> Vec<Ignore> {
    [Event::GlideOff, Event::SetVelocity]
        .into_iter()
        .map(|event| Ignore {
            t0: from,
            t1: end,
            event,
        })
        .collect()
}

fn reacts_of(r: &Report, after: usize) -> Vec<React> {
    r.bounces
        .iter()
        .filter(|b| b.landing >= after && b.lost.is_none())
        .filter_map(|b| b.window_to_plan())
        .map(|(t0, t1)| React {
            t0,
            t1,
            event: Event::GlideOff,
            action: Action::PressJump,
        })
        .collect()
}

fn run() -> Result<bool, String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (course, world) = Course::load(dir)?;
    let tape_path = course.repo_root().join(&course.start.source.path);
    let tape = Tape::read(&tape_path)?;
    let (rec, err) = from_tape(&tape, &world, Some(&course), &course.start.source.path);
    if let Some((t, u)) = err {
        return Err(format!("seed: port stopped at {t}: {u:?}"));
    }
    let seed = verify::verify(&course, &world, verify::Plan::Trajectory(&rec));
    println!(
        "{}",
        summary("seed (the record, its glide-offs applied)", &seed)
    );
    let seed_timed = match (seed.timer_tick(), seed.finish_tick()) {
        (Some(s), Some(f)) if seed.ok() => f - s,
        _ => return Err(format!("the seed doesn't verify: {:?}", seed.failures)),
    };

    // 1. perfect bounces, in order (each splice shifts the later ticks)
    let mut main = rec.clone();
    while let Some(g) = main
        .arrivals
        .iter()
        .find(|a| a.event == ServerEvent::GlideOff)
        .map(|a| a.tick)
    {
        main = perfect(&world, &course, &main, g)?;
    }

    // 2. the timed flight: after the last bounce to just before the finish
    let r = verify::verify(&course, &world, verify::Plan::Trajectory(&main));
    let finish = r.finish_tick().ok_or("main line: no finish")?;
    let last = *landings(&main)
        .iter()
        .rfind(|&&l| l < finish)
        .ok_or("no bounce")?;
    let a = (last + 2..finish)
        .find(|&t| normal_flight(state_at(&main, t)))
        .ok_or("no flight after the last bounce")?;
    let b = (a..finish - 2)
        .rev()
        .find(|&t| normal_flight(state_at(&main, t)))
        .ok_or("no flight before the finish")?;
    let n = b - a;
    let t0 = Instant::now();
    let s = resolve_stretch(
        &world,
        &main,
        a,
        b,
        n * 6 / 10..=n,
        &opts(),
        Header::new(Some(&course), OBJECTIVE, ""),
    )?;
    if let Some((t, u)) = s.stopped {
        return Err(format!("timed stretch: port stopped at {t}: {u:?}"));
    }
    println!(
        "  timed flight: stretch {a}..{b} in {} ticks ({:+}), end off by {:.1e} blocks, {:.1e} b/t, {:.2} s",
        s.reached.inputs.len(),
        -s.saved,
        s.reached.pos_err,
        s.reached.vel_err,
        t0.elapsed().as_secs_f64()
    );
    main = s.traj;
    // where the re-solved timed flight ends: a state a lost glide can always reach
    // given more ticks (the timed stretch has no slack before it)
    let timed_end = a + s.reached.inputs.len();

    // the main line as the plan flies it: no server events (its bounces are perfect;
    // the recording's wall-hit echo after the finish is left out), cut short after the
    // finish
    let r = verify::verify(&course, &world, verify::Plan::Trajectory(&main));
    let finish = r.finish_tick().ok_or("main line: no finish")?;
    let tail = course.observation.tick_chat_lag[1] + TAIL_PAST_LAG;
    let inputs: Vec<InputRow> = main.rows[..(finish + tail).min(main.rows.len())]
        .iter()
        .map(|r| r.input.clone())
        .collect();
    let (flown, stopped) = fly(&main.initial, &inputs, &[], &world, main.header.clone());
    if let Some((t, u)) = stopped {
        return Err(format!(
            "main line without arrivals: port stopped at {t}: {u:?}"
        ));
    }
    main = flown;
    let source = format!(
        "solve: {} with perfect ebounces (each stretch from before a recorded glide-off to \
         before the next landing re-solved to reach the recorded state) and the timed flight \
         re-solved (reach the recorded state before the finish in the fewest ticks); DDP on the \
         generic kernel through ghost walls, repaired under the faithful kernel, flown with port",
        course.start.source.path
    );
    main.header = header_for(&course, OBJECTIVE, source);
    main.write(&dir.join("runs/main.traj.jsonl"))?;
    let (mut plan, mr) = plan_from(&course, &world, &main);
    println!("{}", summary("main line", &mr));
    if !mr.ok() {
        return Err(format!("main line fails: {:?}", mr.failures));
    }
    let main_finish = mr.finish_tick().unwrap();
    let main_timed = main_finish - mr.timer_tick().unwrap();
    plan.links.ignores = tail_ignores(main_finish + 1, plan.tape.rows.len() + 1);

    // 3. per bounce: the window, and a lost-glide child for every other delay
    // bounces before the finish (the tail lands after it)
    let lands: Vec<usize> = landings(&main)
        .into_iter()
        .filter(|&l| l < main_finish)
        .collect();
    let mut children = Vec::new();
    let mut lost_paths: Vec<(String, Lost, Report)> = Vec::new();
    let mut uncovered: Vec<String> = Vec::new();
    for bounce in &mr.bounces {
        let l = bounce.landing;
        let window = bounce.window_to_plan();
        let covered = |t: usize| window.is_some_and(|(t0, t1)| (t0..=t1).contains(&t));
        let natural: Vec<usize> = GLIDE_OFF_NATURAL
            .iter()
            .copied()
            .filter(|&d| !covered(l + d))
            .collect();
        let next = lands.iter().copied().find(|&x| x > l);
        println!(
            "bounce {l}: rescued delays {:?}, window {:?}{}",
            bounce.rescuable.iter().map(|t| t - l).collect::<Vec<_>>(),
            window.map(|(a, b)| (a - l, b - l)),
            if natural.is_empty() {
                String::new()
            } else {
                format!(", MISSES natural delays {natural:?}")
            }
        );
        // where the lost path rejoins: the main line's state before its next landing,
        // or 20 ticks on after the last bounce
        let rejoin_by = next.map_or(l + 20, |x| x - 3);
        let last_t = (l + GLIDE_OFF_MAX)
            .min(next.unwrap_or(usize::MAX))
            .min(main.rows.len());
        for t in (l + GLIDE_OFF_MIN..=last_t).filter(|&t| !covered(t)) {
            let d = t - l;
            let name = format!("lost_{l}_d{d}");
            let t0 = Instant::now();
            // the latest rejoin tick that solves, of the last few before the next
            // landing; failing that, through a bounce of its own, rejoining after the
            // main line's next one
            let before_next: Vec<(usize, bool, bool)> = (t + 4..=rejoin_by.max(t + 6))
                .rev()
                .filter(|&u| normal_flight(state_at(&main, u)))
                .take(6)
                .map(|u| (u, false, false))
                .collect();
            let after_next: Vec<(usize, bool, bool)> = match next {
                Some(x) => (x + 6..=x + 16)
                    .filter(|&u| u <= main.rows.len() && normal_flight(state_at(&main, u)))
                    .map(|u| (u, true, false))
                    .collect(),
                None => Vec::new(),
            };
            // last, the position at the end of the timed stretch, at any velocity
            let late: Vec<(usize, bool, bool)> = (timed_end - 10..=timed_end)
                .rev()
                .step_by(5)
                .filter(|&u| normal_flight(state_at(&main, u)))
                .map(|u| (u, next.is_some(), true))
                .collect();
            let mut errors = Vec::new();
            let mut found = None;
            for (rejoin, through, free) in before_next.into_iter().chain(after_next).chain(late) {
                let header = header_for(
                    &course,
                    OBJECTIVE,
                    format!(
                        "solve: the main line with the glide lost at {t} (bounce {l}, delay {d}), redeployed, {}re-solved to the main line's {} at {rejoin}",
                        if through {
                            "through a bounce of its own, "
                        } else {
                            ""
                        },
                        if free { "position" } else { "state" }
                    ),
                );
                match lost_glide(
                    &world,
                    &main,
                    t,
                    rejoin,
                    through,
                    free,
                    -2..=8,
                    &opts(),
                    header,
                ) {
                    Ok(x) => {
                        found = Some(x);
                        break;
                    }
                    Err(e) => errors.push(e),
                }
            }
            let Some(lost) = found else {
                println!(
                    "  {name}: NOT COVERED (a glide-off at {t} aborts the attempt): {} rejoin ticks tried, none solves; the last: {}",
                    errors.len(),
                    errors
                        .last()
                        .map_or("", |e| e.as_str())
                        .chars()
                        .take(300)
                        .collect::<String>()
                );
                uncovered.push(name);
                continue;
            };
            if let Some((u, e)) = lost.stopped {
                return Err(format!("{name}: port stopped at {u}: {e:?}"));
            }
            let lr = verify::verify(&course, &world, verify::Plan::Trajectory(&lost.traj));
            println!(
                "  {name}: press {}, re-solved from {}, rejoins main {}{} at {} ({:+} ticks), {:.2} s; {}",
                lost.press,
                lost.from,
                if lost.free_velocity {
                    "'s position"
                } else {
                    ""
                },
                lost.rejoin,
                lost.rejoined_at,
                lost.rejoined_at as isize - lost.rejoin as isize,
                t0.elapsed().as_secs_f64(),
                summary("alone", &lr)
            );
            lost.traj
                .write(&dir.join(format!("runs/{name}.traj.jsonl")))?;
            let rows: Vec<_> = lost.traj.to_tape().rows[t - 1..].to_vec();
            let end = t + rows.len();
            let ignores = match lr.finish_tick() {
                Some(f) if f + 1 >= t => tail_ignores(f + 1, end),
                _ => Vec::new(),
            };
            children.push(Child {
                name: name.clone(),
                tick: t,
                event: ServerEvent::GlideOff,
                rows,
                reacts: reacts_of(&lr, t),
                ignores,
            });
            lost_paths.push((name, lost, lr));
        }
    }
    let plan: Plan = with_children(plan, children)?;
    let out = dir.join("runs/downtown.tas");
    std::fs::write(&out, plan.write()).map_err(|e| format!("{}: {e}", out.display()))?;
    println!(
        "wrote {}: {} paths, {} tick lines",
        out.display(),
        plan.path_count(),
        plan.tick_lines()
    );

    // 4. every path, as `etas verify` checks it
    let plan = Plan::read(&out)?;
    let mut ok = true;
    for (p, r) in verify_paths(&course, &world, &plan) {
        let name = p.segments.join(" > ");
        println!("{}", summary(&format!("path {name}"), &r));
        for b in &r.bounces {
            if let Some(t) = b.lost {
                println!("    bounce {}: glide lost at {t}", b.landing);
            }
        }
        for f in &r.failures {
            println!("    FAIL: {f}");
        }
        ok &= r.ok();
    }
    println!(
        "seed {seed_timed} timed ticks; main line {main_timed} ({:+})",
        main_timed as isize - seed_timed as isize
    );
    if !uncovered.is_empty() {
        println!("not covered (abort): {}", uncovered.join(", "));
    }
    for (name, _, lr) in &lost_paths {
        if let (Some(s), Some(f)) = (lr.timer_tick(), lr.finish_tick()) {
            println!(
                "  {name}: {} timed ticks ({:+} on the main line)",
                f - s,
                (f - s) as isize - main_timed as isize
            );
        }
    }
    Ok(ok)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("solve: {e}");
            ExitCode::from(2)
        }
    }
}
