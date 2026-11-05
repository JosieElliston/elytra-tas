//! `.tas` tapes (format in `extract_run.py`'s docstring): the recorded runs, and the
//! branchless plan `tas_play` flies. A plan with reactions is `plan::Plan`, which
//! parses its `@` lines and hands the rest here.
//!
//! A recorded tape may carry `@server` lines: what the server did to the player
//! (`server::Arrival`). Such a tape is a record, not a plan: playback doesn't send
//! the server's packets, so `verify` replays it as a record (arrivals applied, no
//! reactions required), and plan readers refuse it.

use std::fmt::Write as _;
use std::path::Path;

use crate::server::Arrival;

/// One tape line: tick `t`'s input and the pose at its end, as float/double bits.
#[derive(Debug, Clone, PartialEq)]
pub struct TapeRow {
    pub t: usize,
    pub yaw: f32,
    pub pitch: f32,
    /// As written (`WRJ` and `WJR` are the same keys); see `state::keys_of`.
    pub keys: String,
    pub pos: [f64; 3],
    /// The `ground` marker in the comment: onGround at the end of the tick.
    pub ground: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tape {
    /// Header lines in file order, values without their comments.
    pub header: Vec<(String, String)>,
    /// `start=`: the pose at the end of tick 0.
    pub start_pos: [f64; 3],
    pub start_yaw: f32,
    pub start_pitch: f32,
    /// `rows[0]` is tick 1.
    pub rows: Vec<TapeRow>,
    /// The `@server` lines, sorted by tick (file order within a tick).
    pub arrivals: Vec<Arrival>,
}

fn f32_hex(s: &str) -> Result<f32, String> {
    u32::from_str_radix(s, 16)
        .map(f32::from_bits)
        .map_err(|e| format!("float bits {s:?}: {e}"))
}

fn f64_hex(s: &str) -> Result<f64, String> {
    u64::from_str_radix(s, 16)
        .map(f64::from_bits)
        .map_err(|e| format!("double bits {s:?}: {e}"))
}

impl Tape {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.header
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn set(&mut self, key: &str, value: String) {
        match self.header.iter_mut().find(|(k, _)| k == key) {
            Some(kv) => kv.1 = value,
            None => self.header.push((key.into(), value)),
        }
    }

    /// The `start=` value for this tape's start pose: x y z as double bits, yaw pitch
    /// as float bits.
    pub fn start_value(&self) -> String {
        let p = self.start_pos;
        format!(
            "{:016x} {:016x} {:016x} {:08x} {:08x}",
            p[0].to_bits(),
            p[1].to_bits(),
            p[2].to_bits(),
            self.start_yaw.to_bits(),
            self.start_pitch.to_bits()
        )
    }

    pub fn read(path: &Path) -> Result<Tape, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Tape::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Tape, String> {
        let mut header = Vec::new();
        let mut rows = Vec::new();
        let mut arrivals = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let (data, comment) = match line.split_once('#') {
                Some((d, c)) => (d.trim(), c),
                None => (line.trim(), ""),
            };
            if data.is_empty() {
                continue;
            }
            if let Some(rest) = data.strip_prefix("@server ") {
                let f: Vec<&str> = rest.split_whitespace().collect();
                let a: Arrival = Arrival::parse(&f).map_err(|e| format!("line {}: {e}", n + 1))?;
                if a.tick != rows.len() + 1 {
                    return Err(format!(
                        "line {}: @server for tick {} before tick line {}",
                        n + 1,
                        a.tick,
                        rows.len() + 1
                    ));
                }
                arrivals.push(a);
                continue;
            }
            if data.starts_with('@') {
                return Err(format!(
                    "line {}: a plan directive in a tape (read it as a plan)",
                    n + 1
                ));
            }
            if let Some((k, v)) = data.split_once('=') {
                header.push((k.to_string(), v.trim().to_string()));
                continue;
            }
            let f: Vec<&str> = data.split_whitespace().collect();
            if f.len() != 7 {
                return Err(format!("line {}: {} fields, want 7", n + 1, f.len()));
            }
            let t: usize = f[0].parse().map_err(|e| format!("line {}: {e}", n + 1))?;
            if t != rows.len() + 1 {
                return Err(format!("line {}: tick {t}, want {}", n + 1, rows.len() + 1));
            }
            rows.push(TapeRow {
                t,
                yaw: f32_hex(f[1])?,
                pitch: f32_hex(f[2])?,
                keys: f[3].to_string(),
                pos: [f64_hex(f[4])?, f64_hex(f[5])?, f64_hex(f[6])?],
                ground: comment.split_whitespace().any(|w| w == "ground"),
            });
        }
        if let Some(a) = arrivals.iter().find(|a| a.tick > rows.len()) {
            return Err(format!(
                "@server for tick {} after the last tick line",
                a.tick
            ));
        }
        let start = header
            .iter()
            .find(|(k, _)| k == "start")
            .ok_or("no start= line")?
            .1
            .clone();
        let s: Vec<&str> = start.split_whitespace().collect();
        if s.len() != 5 {
            return Err(format!("start= has {} fields, want 5", s.len()));
        }
        let tape = Tape {
            start_pos: [f64_hex(s[0])?, f64_hex(s[1])?, f64_hex(s[2])?],
            start_yaw: f32_hex(s[3])?,
            start_pitch: f32_hex(s[4])?,
            header,
            rows,
            arrivals,
        };
        // `start_flags=` is checked here, so a read tape always has a start player
        tape.start_player()?;
        Ok(tape)
    }

    /// The player at the end of tick 0: at rest at `start=` (`course::rest_state`),
    /// with `start_flags=` (a recorded run's shared flags at the reset, hex; absent is
    /// 0): 0x80 gliding (a reset mid-glide keeps the flag until the server's
    /// glide-off), 0x08 sprinting. `start_ground=0`: tick 0 ended in the air (a tape whose
    /// line 1 is a teleport, `extract_run.py`).
    pub fn start_player(&self) -> Result<port::LocalPlayer, String> {
        let mut p = crate::course::rest_state(self.start_pos, self.start_yaw, self.start_pitch);
        if let Some(v) = self.get("start_flags") {
            let f = u8::from_str_radix(v, 16).map_err(|e| format!("start_flags={v}: {e}"))?;
            if f & 0x80 != 0 {
                p.fall_flying = true;
                p.pose = port::Pose::FallFlying;
            }
            if f & 0x08 != 0 {
                p.set_sprinting(true);
            }
            if f & !0x88 != 0 {
                return Err(format!("start_flags={v}: only 0x80 and 0x08 are modeled"));
            }
        }
        if self.get("start_ground") == Some("0") {
            p.on_ground = false;
        }
        Ok(p)
    }

    /// Whether this tape records server events: a record, not a plan.
    pub fn is_record(&self) -> bool {
        !self.arrivals.is_empty()
    }

    /// The tape as `extract_run.py` writes it, human-readable comments included.
    pub fn write(&self) -> String {
        self.write_with(|_| String::new())
    }

    /// [`Tape::write`], with `before(t)`'s lines written before tick line `t` (a
    /// plan's directives), after the tape's own `@server` lines for `t`.
    pub fn write_with(&self, before: impl Fn(usize) -> String) -> String {
        let mut out = String::new();
        for (k, v) in &self.header {
            if k == "start" {
                let p = self.start_pos;
                let _ = writeln!(
                    out,
                    "start={}    # {:?} {:?} {:?} yaw {:?} pitch {:?}",
                    self.start_value(),
                    p[0],
                    p[1],
                    p[2],
                    self.start_yaw as f64,
                    self.start_pitch as f64,
                );
            } else {
                let _ = writeln!(out, "{k}={v}");
            }
        }
        for r in &self.rows {
            for a in self.arrivals.iter().filter(|a| a.tick == r.t) {
                let _ = match a.event {
                    crate::server::ServerEvent::SetVelocity { v } => {
                        writeln!(out, "{}    # {:.6} {:.6} {:.6}", a.line(), v[0], v[1], v[2])
                    }
                    crate::server::ServerEvent::Teleport { pos: v } => {
                        writeln!(out, "{}    # {:.6} {:.6} {:.6}", a.line(), v[0], v[1], v[2])
                    }
                    _ => writeln!(out, "{}", a.line()),
                };
            }
            out.push_str(&before(r.t));
            let _ = writeln!(
                out,
                "{} {:08x} {:08x} {} {:016x} {:016x} {:016x}    # {:.4} {:.4} {:.4} yaw {:.3} pitch {:.3}{}",
                r.t,
                r.yaw.to_bits(),
                r.pitch.to_bits(),
                r.keys,
                r.pos[0].to_bits(),
                r.pos[1].to_bits(),
                r.pos[2].to_bits(),
                r.pos[0],
                r.pos[1],
                r.pos[2],
                r.yaw as f64,
                r.pitch as f64,
                if r.ground { " ground" } else { "" },
            );
        }
        out
    }
}
