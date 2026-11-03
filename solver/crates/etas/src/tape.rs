//! `.tas` tapes (format in `extract_run.py`'s docstring): the recorded runs, and the
//! branchless plan `tas_play` flies. A plan with reactions is `plan::Plan`, which
//! parses its `@` lines and hands the rest here.

use std::fmt::Write as _;
use std::path::Path;

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
        for (n, line) in text.lines().enumerate() {
            let (data, comment) = match line.split_once('#') {
                Some((d, c)) => (d.trim(), c),
                None => (line.trim(), ""),
            };
            if data.is_empty() {
                continue;
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
        Ok(Tape {
            start_pos: [f64_hex(s[0])?, f64_hex(s[1])?, f64_hex(s[2])?],
            start_yaw: f32_hex(s[3])?,
            start_pitch: f32_hex(s[4])?,
            header,
            rows,
        })
    }

    /// The tape as `extract_run.py` writes it, human-readable comments included.
    pub fn write(&self) -> String {
        self.write_with(|_| String::new())
    }

    /// [`Tape::write`], with `before(t)`'s lines written before tick line `t` (a
    /// plan's directives).
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
