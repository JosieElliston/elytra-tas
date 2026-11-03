//! Readers for the real client data the tests replay: `.tas` tapes and logger logs.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use port::{Input, Vec3};

/// The repo root (three levels above this crate).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// One tape line: the input of tick `t` and the pose at its end.
#[derive(Debug, Clone)]
pub struct TapeTick {
    pub t: usize,
    pub yaw: f32,
    pub pitch: f32,
    pub keys: Input,
    pub pos: Vec3,
    /// The extractor's `ground` marker: onGround at the end of the tick.
    pub ground: bool,
}

#[derive(Debug, Clone)]
pub struct Tape {
    pub header: HashMap<String, String>,
    pub start_pos: Vec3,
    pub start_yaw: f32,
    pub start_pitch: f32,
    /// `ticks[0]` is tick 1.
    pub ticks: Vec<TapeTick>,
}

impl Tape {
    pub fn glide_tick(&self) -> usize {
        self.header["glide_tick"].parse().unwrap()
    }

    pub fn glide_mismatch(&self) -> Vec<usize> {
        self.header
            .get("glide_mismatch")
            .map(|s| s.split_whitespace().map(|t| t.parse().unwrap()).collect())
            .unwrap_or_default()
    }

    /// The pose at the end of tick `t` (0 is the start pose).
    pub fn pos(&self, t: usize) -> Vec3 {
        if t == 0 {
            self.start_pos
        } else {
            self.ticks[t - 1].pos
        }
    }

    pub fn tick(&self, t: usize) -> &TapeTick {
        &self.ticks[t - 1]
    }

    /// onGround at the end of tick `t`; the start pose is standing.
    pub fn ground(&self, t: usize) -> bool {
        t == 0 || self.ticks[t - 1].ground
    }

    pub fn keys(&self, t: usize) -> Input {
        if t == 0 {
            Input::EMPTY
        } else {
            self.ticks[t - 1].keys
        }
    }
}

fn f32_hex(s: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(s, 16).unwrap())
}

fn f64_hex(s: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(s, 16).unwrap())
}

pub fn read_tape(path: &Path) -> Tape {
    let text = std::fs::read_to_string(path).unwrap();
    let mut header = HashMap::new();
    let mut ticks = Vec::new();
    for line in text.lines() {
        let (data, comment) = match line.split_once('#') {
            Some((d, c)) => (d.trim(), c),
            None => (line.trim(), ""),
        };
        if data.is_empty() {
            continue;
        }
        if let Some((k, v)) = data.split_once('=') {
            header.insert(k.to_string(), v.trim().to_string());
            continue;
        }
        let f: Vec<&str> = data.split_whitespace().collect();
        ticks.push(TapeTick {
            t: f[0].parse().unwrap(),
            yaw: f32_hex(f[1]),
            pitch: f32_hex(f[2]),
            keys: Input::from_tape(f[3]),
            pos: Vec3::new(f64_hex(f[4]), f64_hex(f[5]), f64_hex(f[6])),
            ground: comment.split_whitespace().any(|w| w == "ground"),
        });
    }
    let s: Vec<&str> = header["start"].split_whitespace().collect();
    let tape = Tape {
        start_pos: Vec3::new(f64_hex(s[0]), f64_hex(s[1]), f64_hex(s[2])),
        start_yaw: f32_hex(s[3]),
        start_pitch: f32_hex(s[4]),
        header,
        ticks,
    };
    for (i, t) in tape.ticks.iter().enumerate() {
        assert_eq!(t.t, i + 1, "tape ticks must be consecutive from 1");
    }
    tape
}

/// A logger `T` record: the local player at START (`s`) or END (`e`) of a client tick.
#[derive(Debug, Clone, Copy)]
pub struct TRecord {
    pub tick: u64,
    pub pos: Vec3,
    pub y_rot: f32,
    pub x_rot: f32,
    pub vel: Vec3,
    pub on_ground: bool,
    pub h_coll: bool,
    pub fall_flying: bool,
    pub sprinting: bool,
    pub input: u32,
}

/// A tick's START and END records, in file order.
pub struct TickPair {
    pub s: TRecord,
    pub e: TRecord,
}

/// The logger's log directory; `ETAS_LOG_DIR` overrides it.
pub fn log_dir() -> PathBuf {
    match std::env::var_os("ETAS_LOG_DIR") {
        Some(d) => PathBuf::from(d),
        None => {
            let home = std::env::var_os("HOME").unwrap_or_default();
            Path::new(&home)
                .join("Library/Application Support/ModrinthApp/profiles/main/elytra-tas/logs")
        }
    }
}

/// Every tick with both an `s` and an `e` record for the same tick number.
pub fn read_tick_pairs(path: &Path) -> Vec<TickPair> {
    let f = std::fs::File::open(path).unwrap();
    let r = BufReader::new(flate2::read::GzDecoder::new(f));
    let mut out = Vec::new();
    let mut pending: Option<TRecord> = None;
    for line in r.lines() {
        let line = line.unwrap();
        if !line.starts_with("T\t") {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let b = |i: usize| f[i] == "1";
        let d = |i: usize| f[i].parse::<f64>().unwrap();
        let rec = TRecord {
            tick: f[2].parse().unwrap(),
            pos: Vec3::new(d(4), d(5), d(6)),
            // floats are logged widened to double: the cast back is exact
            y_rot: d(7) as f32,
            x_rot: d(8) as f32,
            vel: Vec3::new(d(9), d(10), d(11)),
            on_ground: b(12),
            h_coll: b(13),
            fall_flying: b(14),
            sprinting: b(15),
            input: f[16].parse().unwrap(),
        };
        match f[3] {
            "s" => pending = Some(rec),
            "e" => {
                if let Some(s) = pending.take()
                    && s.tick == rec.tick
                {
                    out.push(TickPair { s, e: rec });
                }
            }
            _ => {}
        }
    }
    out
}
