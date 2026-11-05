//! The booster model (`etas::booster`) against every booster velocity recorded at
//! alpine: `tests/data/boosters/alpine.json` (`python3 boosters.py fixture alpine`,
//! three logger logs and a replay) under the boosters fitted into the alpine test
//! course (`python3 fit_course.py alpine --boosters-into ...`).

use std::collections::BTreeMap;

use etas::booster::{Booster, SentMove, predict};
use etas::server::{Arrival, ServerEvent};
use serde::Deserialize;
use serde_json::Value;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

#[derive(Deserialize)]
struct Boosters {
    boosters: Vec<Booster>,
}

fn boosters() -> BTreeMap<String, Booster> {
    let s = std::fs::read_to_string(format!("{DATA}/alpine/course.json")).unwrap();
    let c: Boosters = serde_json::from_str(&s).unwrap();
    c.boosters
        .into_iter()
        .map(|b| (b.name.clone(), b))
        .collect()
}

fn bits(v: &Value) -> f32 {
    f32::from_bits(u32::from_str_radix(v.as_str().unwrap(), 16).unwrap())
}

#[derive(Default, Debug)]
struct Tally {
    arrivals: usize,
    lag: usize,
    doubled: usize,
    covered: usize,
    switched: usize,
    uncovered: Vec<String>,
    late_first: Vec<String>,
    sure: usize,
    sure_misses: Vec<String>,
    no_first: usize,
}

#[test]
fn recorded_boosts_are_in_the_cases() {
    let bs = boosters();
    let fx: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{DATA}/boosters/alpine.json")).unwrap(),
    )
    .unwrap();
    let mut per: BTreeMap<String, Tally> = BTreeMap::new();
    for w in fx["windows"].as_array().unwrap() {
        let b = &bs[w["booster"].as_str().unwrap()];
        let src = w["source"].as_str().unwrap();
        let t = per.entry(b.name.clone()).or_default();
        let moves: Vec<SentMove> = w["moves"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| SentMove {
                tick: m[0].as_u64().unwrap() as usize,
                pos: [
                    m[1].as_f64().unwrap(),
                    m[2].as_f64().unwrap(),
                    m[3].as_f64().unwrap(),
                ],
                y_rot: bits(&m[4]),
                x_rot: bits(&m[5]),
                on_ground: m[6].as_u64().unwrap() == 1,
            })
            .collect();
        // recorded arrivals. Out of the model, and set aside: a delay outside the
        // booster's cases (server lag), and two arrivals on one tick (two server ticks'
        // packets handled before one client tick: a client frame stall)
        let all: Vec<(Arrival, usize)> = w["arrivals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                let v = [
                    a[1].as_f64().unwrap(),
                    a[2].as_f64().unwrap(),
                    a[3].as_f64().unwrap(),
                ];
                let tick = a[0].as_u64().unwrap() as usize;
                (
                    Arrival {
                        tick,
                        event: ServerEvent::SetVelocity { v },
                    },
                    a[4].as_u64().unwrap() as usize,
                )
            })
            .collect();
        let (mut rec, mut aside) = (vec![], vec![]);
        for &(a, trigger) in &all {
            t.arrivals += 1;
            if !b.delays.contains(&(a.tick - trigger)) {
                t.lag += 1;
                aside.push(a.event);
            } else if all.iter().filter(|(o, _)| o.tick == a.tick).count() > 1 {
                t.doubled += 1;
                aside.push(a.event);
            } else {
                rec.push(a);
            }
        }
        let cases = predict(b, &moves);
        let mut used = vec![false; rec.len()];
        for j in 0..cases[0].visits.len() {
            // the delay case whose visit explains the most of the arrivals in its span
            let mut best: Option<(usize, Vec<usize>, Result<etas::booster::Checked, String>)> =
                None;
            for c in &cases {
                let v = &c.visits[j];
                let idx: Vec<usize> = (0..rec.len())
                    .filter(|&i| !used[i] && v.span().contains(&rec[i].tick))
                    .collect();
                let r: Vec<Arrival> = idx.iter().map(|&i| rec[i]).collect();
                let res = v.check(&r);
                let better = match (&best, &res) {
                    (None, _) => true,
                    (Some((_, _, Err(_))), Ok(_)) => true,
                    (Some((_, bi, Ok(_))), Ok(_)) => idx.len() > bi.len(),
                    _ => false,
                };
                if better {
                    best = Some((c.delay, idx, res));
                }
            }
            let (delay, idx, res) = best.unwrap();
            let v = &cases.iter().find(|c| c.delay == delay).unwrap().visits[j];
            let at = format!("{src}@{} (delay {delay})", v.candidates[0].tick - delay);
            match res {
                Ok(ch) => {
                    for &i in &idx {
                        used[i] = true;
                    }
                    t.covered += ch.matched;
                    if let Some(f) = idx
                        .first()
                        .and_then(|&i| v.candidates.iter().position(|c| *c == rec[i]))
                    {
                        t.sure += v.given_first(f).unwrap().0.len();
                    }
                    // a sure boost that came, but late or doubled, isn't a miss
                    t.sure_misses.extend(
                        ch.sure_misses
                            .iter()
                            .filter(|m| !aside.contains(&m.event))
                            .map(|m| format!("{at}: tick {}", m.tick)),
                    );
                    t.no_first += ch.no_first as usize;
                }
                Err(e) => t.late_first.push(format!("{at}: {e}")),
            }
        }
        // one the visit's chosen case misses but another delay case has: the delay
        // changed within the visit (moves near a server tick edge)
        for (i, a) in rec.iter().enumerate() {
            if used[i] {
                continue;
            }
            if cases.iter().any(|c| c.possible().contains(a)) {
                t.switched += 1;
            } else {
                t.uncovered.push(format!("{src}: arrival at {}", a.tick));
            }
        }
    }
    for (name, t) in &per {
        eprintln!(
            "{name}: {} arrivals: {} lag, {} doubled, {} covered, {} in another delay case; \
             uncovered {:?}; rejected visits {:?}; {} sure, misses {:?}; \
             {} visits without the sure first boost",
            t.arrivals,
            t.lag,
            t.doubled,
            t.covered,
            t.switched,
            t.uncovered,
            t.late_first,
            t.sure,
            t.sure_misses,
            t.no_first
        );
    }
    let total: usize = per.values().map(|t| t.arrivals).sum();
    assert!(total > 250, "fixture has {total} arrivals");
    for (name, t) in &per {
        assert!(
            t.uncovered.is_empty() && t.late_first.is_empty(),
            "{name}: {t:?}"
        );
        assert_eq!(
            t.covered + t.lag + t.doubled + t.switched,
            t.arrivals,
            "{name}"
        );
        assert!(t.switched * 20 <= t.arrivals, "{name}: {t:?}");
        // a sure arrival that didn't come: at most 2% of them
        assert!(t.sure_misses.len() * 50 <= t.sure.max(1), "{name}: {t:?}");
    }
}
