//! Plans, version 2 (`solver/PLAN-FORMAT.md` is the spec): a `.tas` tape with the
//! plan's header keys and directive lines, optionally split into named segments that
//! playback switches between on what the server sends (`@branch`, `@goto`). A plain
//! tape and a version-1 plan (one segment, `@react` only) are version-2 plans.
//!
//! [`Plan::parse`] reads and checks one, [`Plan::write`] writes one (as version 1 when
//! it has no branches, so older readers still fly it), [`Plan::paths`] lists every
//! root-to-leaf path as the tape and the [`Arrival`]s that path assumes, for `verify`,
//! and [`plan_from`] makes a branchless plan from a trajectory, with a reaction for
//! every planned perfect bounce `verify` finds.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use world::World;

use crate::course::Course;
use crate::server::{Arrival, ServerEvent};
use crate::tape::{Tape, TapeRow};
use crate::trajectory::Trajectory;
use crate::verify::{self, Report};

/// The `format=` value of the version this module writes when a plan branches.
pub const FORMAT: &str = "etas-plan 2";
/// Version 1: one segment, `@react` only. Still read, and written for such plans.
pub const FORMAT_1: &str = "etas-plan 1";
/// The account playback refuses to fly on any other (AGENTS.md: the alt).
pub const ACCOUNT: &str = "deftenvy";
/// `diverge_tol=`'s default, blocks.
pub const DIVERGE_TOL: f64 = 1e-4;
/// What logs and paths call the segment before the first `@segment` line. Not a
/// segment name a file may use.
pub const ROOT: &str = "root";

/// A kind of server event a plan names: what a reaction waits for, what a branch or
/// an `@ignore` matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Event {
    /// The server's entity data clearing the local player's fall-flying flag.
    GlideOff,
    /// `set_entity_motion` for the local player (boosters, the wall-hit echo).
    SetVelocity,
}

/// What a reaction does, in the tick the event is first handled, before its physics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Jump down for that tick.
    PressJump,
}

impl Event {
    pub fn name(self) -> &'static str {
        match self {
            Event::GlideOff => "glide_off",
            Event::SetVelocity => "set_velocity",
        }
    }

    pub fn of(e: &ServerEvent) -> Event {
        match e {
            ServerEvent::GlideOff => Event::GlideOff,
            ServerEvent::SetVelocity { .. } => Event::SetVelocity,
            ServerEvent::Teleport { .. } => unreachable!("a plan's branch can't be a teleport"),
        }
    }

    fn parse(s: &str) -> Option<Event> {
        match s {
            "glide_off" => Some(Event::GlideOff),
            "set_velocity" => Some(Event::SetVelocity),
            _ => None,
        }
    }
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::PressJump => "press_jump",
        }
    }
}

/// `@react <t0> <t1> <event> <action>`: during ticks `t0..=t1`, on the tick on which
/// the client first handles `event`, do `action`. At most one firing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct React {
    pub t0: usize,
    pub t1: usize,
    pub event: Event,
    pub action: Action,
}

impl React {
    pub fn line(&self) -> String {
        format!(
            "@react {} {} {} {}",
            self.t0,
            self.t1,
            self.event.name(),
            self.action.name()
        )
    }
}

/// `@branch <tick> <event> [payload] <child>`: if `event` was handled before line
/// `tick` (a velocity within `velocity_tol=` of the payload), playback goes on in
/// `child`, whose first line is `tick`.
#[derive(Debug, Clone, PartialEq)]
pub struct Branch {
    pub tick: usize,
    pub event: ServerEvent,
    pub child: String,
}

impl Branch {
    pub fn line(&self) -> String {
        match self.event {
            ServerEvent::GlideOff => format!("@branch {} glide_off {}", self.tick, self.child),
            ServerEvent::Teleport { .. } => unreachable!("a plan's branch can't be a teleport"),
            ServerEvent::SetVelocity { v } => format!(
                "@branch {} set_velocity {:016x} {:016x} {:016x} {}    # {:?} {:?} {:?}",
                self.tick,
                v[0].to_bits(),
                v[1].to_bits(),
                v[2].to_bits(),
                self.child,
                v[0],
                v[1],
                v[2]
            ),
        }
    }
}

/// `@ignore <t0> <t1> <event>`: `event` handled before a line in `t0..=t1` that no
/// branch or reaction takes is logged, not an abort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ignore {
    pub t0: usize,
    pub t1: usize,
    pub event: Event,
}

impl Ignore {
    pub fn line(&self) -> String {
        format!("@ignore {} {} {}", self.t0, self.t1, self.event.name())
    }
}

/// A segment's ways out: its branches (file order), `@ignore`s, and `@goto`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Links {
    pub branches: Vec<Branch>,
    pub ignores: Vec<Ignore>,
    /// Where playback goes on after the segment's last line when no branch took it.
    pub goto: Option<String>,
}

impl Links {
    fn is_empty(&self) -> bool {
        self.branches.is_empty() && self.ignores.is_empty() && self.goto.is_none()
    }
}

/// A named segment (`@segment <name> <start>`): tick lines `start..start + rows.len()`,
/// in the plan's tick numbering, and its own directives.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub name: String,
    pub start: usize,
    pub rows: Vec<TapeRow>,
    pub reacts: Vec<React>,
    pub links: Links,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The header (the plan's keys too, in file order), start, and the root segment's
    /// tick lines (from line 1).
    pub tape: Tape,
    /// The root segment's reactions, in file order.
    pub reacts: Vec<React>,
    /// The root segment's branches, ignores and goto.
    pub links: Links,
    /// The named segments, in file order.
    pub segments: Vec<Segment>,
}

/// How a path ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leaf {
    /// The plan's end: the last segment ran out of lines with nothing expected after.
    End,
    /// Playback aborts before line `tick`: the segment ended, a branch expected an
    /// event before `tick`, and none came.
    Missing { tick: usize },
}

/// One root-to-leaf path: what playback flies when exactly `arrivals` happen.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanPath {
    /// The segments it goes through, root first.
    pub segments: Vec<String>,
    /// The plan's header and start, and the lines flown, ticks 1 onward (`ticks=` is
    /// this path's).
    pub tape: Tape,
    /// The branch events taken, in tick order: each is handled before its tick's
    /// physics.
    pub arrivals: Vec<Arrival>,
    /// The reactions of the segments it goes through, each window cut at the tick the
    /// path leaves its segment.
    pub reacts: Vec<React>,
    pub leaf: Leaf,
}

impl PlanPath {
    /// The path as a branchless plan (its tape and reactions), for `verify`, which
    /// takes the arrivals separately.
    pub fn plan(&self) -> Plan {
        let mut p = Plan::from_tape(self.tape.clone());
        p.reacts = self.reacts.clone();
        p
    }
}

/// A segment as `paths` and the checks walk it: the root and named ones alike.
#[derive(Clone, Copy)]
struct View<'a> {
    name: &'a str,
    start: usize,
    rows: &'a [TapeRow],
    reacts: &'a [React],
    links: &'a Links,
}

impl View<'_> {
    /// The first line past the segment: where its goto target starts, and the last
    /// tick its branches may name.
    fn end(&self) -> usize {
        self.start + self.rows.len()
    }
}

impl Plan {
    /// A plan with no reactions or branches: what a plain `.tas` tape is.
    pub fn from_tape(tape: Tape) -> Plan {
        Plan {
            tape,
            reacts: Vec::new(),
            links: Links::default(),
            segments: Vec::new(),
        }
    }

    pub fn read(path: &Path) -> Result<Plan, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Plan::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// True when the plan needs version 2: segments, branches, `@ignore` or `@goto`.
    pub fn branches(&self) -> bool {
        !self.segments.is_empty() || !self.links.is_empty()
    }

    /// The `format=` this plan is written with: version 1 when it doesn't branch.
    pub fn format(&self) -> &'static str {
        if self.branches() { FORMAT } else { FORMAT_1 }
    }

    /// Tick lines in the file, every segment's: what `ticks=` must say.
    pub fn tick_lines(&self) -> usize {
        self.tape.rows.len() + self.segments.iter().map(|s| s.rows.len()).sum::<usize>()
    }

    /// Parse a plan and check it (PLAN-FORMAT.md, "Checks"). Refuses an `@` line its
    /// version doesn't have, a `format=` other than versions 1 and 2, and directives
    /// in a file without `format=`.
    pub fn parse(text: &str) -> Result<Plan, String> {
        // the root part (header and root lines) goes to Tape::parse, the rest is ours;
        // blank lines keep the line numbers of the tape's error messages
        let mut root_text = String::with_capacity(text.len());
        let mut root_dirs: Vec<(usize, String)> = Vec::new();
        let mut parts: Vec<SegmentText> = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let data = line.split('#').next().unwrap_or("").trim();
            let in_segment = !parts.is_empty();
            if data.starts_with("@segment") && data.split_whitespace().next() == Some("@segment") {
                parts.push(SegmentText {
                    line: n + 1,
                    head: data.to_string(),
                    body: Vec::new(),
                });
                root_text.push('\n');
            } else if in_segment {
                if !data.is_empty() {
                    parts
                        .last_mut()
                        .unwrap()
                        .body
                        .push((n + 1, line.to_string()));
                }
                root_text.push('\n');
            } else if data.starts_with('@') {
                root_dirs.push((n + 1, data.to_string()));
                root_text.push('\n');
            } else {
                root_text.push_str(line);
                root_text.push('\n');
            }
        }
        let tape = Tape::parse(&root_text)?;
        let v2 = match tape.get("format") {
            Some(FORMAT) => true,
            Some(FORMAT_1) => false,
            Some(f) => return Err(format!("format={f}, reader knows {FORMAT_1} and {FORMAT}")),
            None => {
                if let Some((line, _)) = root_dirs.first() {
                    return Err(format!(
                        "line {line}: a directive in a plan without format={FORMAT}"
                    ));
                }
                if let Some(p) = parts.first() {
                    return Err(format!(
                        "line {}: a directive in a plan without format={FORMAT}",
                        p.line
                    ));
                }
                false
            }
        };
        if !v2 && let Some(p) = parts.first() {
            return Err(format!(
                "line {}: unknown directive @segment (format {FORMAT_1} has @react only)",
                p.line
            ));
        }
        let tol = |key: &str| -> Result<Option<f64>, String> {
            match tape.get(key) {
                None => Ok(None),
                Some(v) => {
                    let x: f64 = v.parse().map_err(|e| format!("{key}={v}: {e}"))?;
                    if x.is_nan() || x < 0.0 {
                        return Err(format!("{key}={v}: not a tolerance"));
                    }
                    Ok(Some(x))
                }
            }
        };
        tol("diverge_tol")?;
        let vtol = tol("velocity_tol")?;

        let mut plan = Plan::from_tape(tape);
        let mut d = Directives::default();
        for (line, text) in &root_dirs {
            parse_directive(*line, text, v2, &mut d)?;
        }
        (plan.reacts, plan.links) = d.finish(ROOT)?;

        for p in parts {
            let at = |e: String| format!("line {}: {e}", p.line);
            let f: Vec<&str> = p.head.split_whitespace().collect();
            if f.len() != 3 {
                return Err(at(format!("@segment has {} fields, want 2", f.len() - 1)));
            }
            let name = f[1].to_string();
            if !valid_name(&name) || name == ROOT {
                return Err(at(format!(
                    "segment name {name:?}: want [A-Za-z0-9_.+-]+, not {ROOT}"
                )));
            }
            let start: usize = f[2]
                .parse()
                .map_err(|e| at(format!("start {:?}: {e}", f[2])))?;
            if start == 0 {
                return Err(at("start 0: lines are ticks 1 onward".into()));
            }
            let mut rows = Vec::new();
            let mut d = Directives::default();
            for (line, raw) in &p.body {
                let (data, comment) = match raw.split_once('#') {
                    Some((d, c)) => (d.trim(), c),
                    None => (raw.trim(), ""),
                };
                if data.starts_with('@') {
                    parse_directive(*line, data, v2, &mut d)?;
                } else if data.contains('=') {
                    return Err(format!(
                        "line {line}: a header line inside segment {name}; the header comes before the first @segment"
                    ));
                } else {
                    let r = parse_row(data, comment).map_err(|e| format!("line {line}: {e}"))?;
                    let want = start + rows.len();
                    if r.t != want {
                        return Err(format!("line {line}: tick {}, want {want}", r.t));
                    }
                    rows.push(r);
                }
            }
            let (reacts, links) = d.finish(&name)?;
            plan.segments.push(Segment {
                name,
                start,
                rows,
                reacts,
                links,
            });
        }

        if let Some(v) = plan.tape.get("ticks") {
            let n = plan.tick_lines();
            if v != n.to_string() {
                return Err(format!("ticks={v}, but the file has {n} tick lines"));
            }
        }
        plan.check(vtol)?;
        Ok(plan)
    }

    fn root(&self) -> View<'_> {
        View {
            name: ROOT,
            start: 1,
            rows: &self.tape.rows,
            reacts: &self.reacts,
            links: &self.links,
        }
    }

    fn views(&self) -> HashMap<&str, View<'_>> {
        let mut m = HashMap::new();
        m.insert(ROOT, self.root());
        for s in &self.segments {
            m.insert(
                s.name.as_str(),
                View {
                    name: &s.name,
                    start: s.start,
                    rows: &s.rows,
                    reacts: &s.reacts,
                    links: &s.links,
                },
            );
        }
        m
    }

    /// The checks of PLAN-FORMAT.md that span directives and segments.
    fn check(&self, vtol: Option<f64>) -> Result<(), String> {
        let mut names = HashSet::new();
        for s in &self.segments {
            if !names.insert(s.name.as_str()) {
                return Err(format!("segment {} is defined twice", s.name));
            }
        }
        let views = self.views();
        let mut order: Vec<View> = vec![self.root()];
        order.extend(self.segments.iter().map(|s| views[s.name.as_str()]));
        for s in &order {
            let (a, end) = (s.start, s.end());
            let at = |what: String| format!("segment {}: {what}", s.name);
            for r in s.reacts {
                // the tick before the window must be this segment's, so the edge rule
                // can be checked (the root's tick 0 is the start, no keys)
                let lo = if s.name == ROOT { 1 } else { a + 1 };
                if !(lo <= r.t0 && r.t0 <= r.t1 && r.t1 < end) {
                    return Err(at(format!(
                        "{}: window not within ticks {lo}..{}",
                        r.line(),
                        end - 1
                    )));
                }
                for t in r.t0..=r.t1 {
                    if t >= 2 && s.rows[t - 1 - a].keys.contains('J') {
                        return Err(at(format!(
                            "{}: press_jump at tick {t}, but tick {} holds J",
                            r.line(),
                            t - 1
                        )));
                    }
                }
            }
            for i in &s.links.ignores {
                if !(a <= i.t0 && i.t0 <= i.t1 && i.t1 <= end) {
                    return Err(at(format!("{}: not within ticks {a}..{end}", i.line())));
                }
            }
            for (k, b) in s.links.branches.iter().enumerate() {
                if !(a <= b.tick && b.tick <= end) {
                    return Err(at(format!(
                        "@branch {}: tick not within {a}..{end}",
                        b.tick
                    )));
                }
                let child = views
                    .get(b.child.as_str())
                    .ok_or_else(|| at(format!("@branch {}: no segment {}", b.tick, b.child)))?;
                if child.start != b.tick {
                    return Err(at(format!(
                        "@branch {} to {}: the child starts at {}, not at the branch's tick",
                        b.tick, b.child, child.start
                    )));
                }
                match b.event {
                    ServerEvent::GlideOff => {
                        if let Some(r) = s.reacts.iter().find(|r| {
                            r.event == Event::GlideOff && r.t0 <= b.tick && b.tick <= r.t1
                        }) {
                            return Err(at(format!(
                                "@branch {} glide_off is inside {}",
                                b.tick,
                                r.line()
                            )));
                        }
                    }
                    ServerEvent::SetVelocity { v } => {
                        vtol.ok_or_else(|| {
                            at(format!(
                                "@branch {} set_velocity, but no velocity_tol= in the header",
                                b.tick
                            ))
                        })?;
                        if v.iter().any(|x| !x.is_finite()) {
                            return Err(at(format!("@branch {}: velocity {v:?}", b.tick)));
                        }
                    }
                    ServerEvent::Teleport { .. } => {
                        return Err(at(format!("@branch {}: a teleport", b.tick)));
                    }
                }
                // at most one branch can match what was handled before a tick
                for q in &s.links.branches[..k] {
                    if q.tick != b.tick || Event::of(&q.event) != Event::of(&b.event) {
                        continue;
                    }
                    let clash = match (q.event, b.event) {
                        (ServerEvent::SetVelocity { v: p }, ServerEvent::SetVelocity { v }) => {
                            let tol = vtol.unwrap_or(0.0);
                            (0..3).all(|i| (p[i] - v[i]).abs() <= 2.0 * tol)
                        }
                        _ => true,
                    };
                    if clash {
                        return Err(at(format!(
                            "@branch {} {} to {} and to {}: one observation would match both",
                            b.tick,
                            Event::of(&b.event).name(),
                            q.child,
                            b.child
                        )));
                    }
                }
            }
            if let Some(g) = &s.links.goto {
                let t = views
                    .get(g.as_str())
                    .ok_or_else(|| at(format!("@goto {g}: no such segment")))?;
                if t.start != end {
                    return Err(at(format!(
                        "@goto {g}: it starts at {}, not at {end}, the line after this segment's last",
                        t.start
                    )));
                }
            }
        }

        // a DAG, every segment reachable from the root; edges never go back in ticks,
        // so a cycle can only be among segments entered at the same tick
        let edges = |s: &View| -> Vec<String> {
            let mut e: Vec<String> = s.links.branches.iter().map(|b| b.child.clone()).collect();
            e.extend(s.links.goto.clone());
            e
        };
        let mut state: HashMap<&str, u8> = HashMap::new(); // 1 on the stack, 2 done
        fn dfs<'a>(
            n: &'a str,
            views: &HashMap<&'a str, View<'a>>,
            edges: &dyn Fn(&View) -> Vec<String>,
            state: &mut HashMap<&'a str, u8>,
        ) -> Result<(), String> {
            match state.get(n) {
                Some(1) => return Err(format!("segment {n} is reachable from itself")),
                Some(_) => return Ok(()),
                None => {}
            }
            state.insert(n, 1);
            let v = views[n];
            for c in edges(&v) {
                let (k, _) = views.get_key_value(c.as_str()).unwrap();
                dfs(k, views, edges, state)?;
            }
            state.insert(n, 2);
            Ok(())
        }
        dfs(ROOT, &views, &edges, &mut state)?;
        if let Some(s) = self
            .segments
            .iter()
            .find(|s| !state.contains_key(s.name.as_str()))
        {
            return Err(format!("segment {} is not reachable from the root", s.name));
        }

        // each observation is taken once: a segment entered at tick t by a branch on
        // event E (through any chain of same-tick entries) can't branch on E at t again
        let mut consumed: HashMap<&str, HashSet<Event>> = HashMap::new();
        loop {
            let mut changed = false;
            for s in &order {
                let here = consumed.get(s.name).cloned().unwrap_or_default();
                let mut add = |child: &str, extra: Option<Event>, tick: usize| {
                    let child_v = views[child];
                    let mut set = if s.start == tick {
                        here.clone()
                    } else {
                        HashSet::new()
                    };
                    set.extend(extra);
                    if child_v.start != tick {
                        return;
                    }
                    let e = consumed
                        .entry(views.get_key_value(child).unwrap().0)
                        .or_default();
                    for k in set {
                        changed |= e.insert(k);
                    }
                };
                for b in &s.links.branches {
                    add(&b.child, Some(Event::of(&b.event)), b.tick);
                }
                if let Some(g) = &s.links.goto {
                    add(g, None, s.end());
                }
            }
            if !changed {
                break;
            }
        }
        for s in &order {
            if let Some(set) = consumed.get(s.name)
                && let Some(b) = s
                    .links
                    .branches
                    .iter()
                    .find(|b| b.tick == s.start && set.contains(&Event::of(&b.event)))
            {
                return Err(format!(
                    "segment {}: @branch {} {} can never match: entering it took that observation",
                    s.name,
                    b.tick,
                    Event::of(&b.event).name()
                ));
            }
        }
        Ok(())
    }

    /// The plan as text: the header (with `format=` and `ticks=` set to what the plan
    /// needs, if present), the root's lines, then each segment. Each directive is on
    /// the line before the tick line it names first; a segment's goto, and branches
    /// past its last line, come after its lines.
    pub fn write(&self) -> String {
        let mut tape = self.tape.clone();
        if tape.get("format").is_some() {
            tape.set("format", self.format().to_string());
        }
        if tape.get("ticks").is_some() {
            tape.set("ticks", self.tick_lines().to_string());
        }
        let mut out = write_segment(&tape, self.root());
        let views = self.views();
        for s in &self.segments {
            let _ = writeln!(out, "@segment {} {}", s.name, s.start);
            let bare = Tape {
                header: Vec::new(),
                start_pos: tape.start_pos,
                start_yaw: tape.start_yaw,
                start_pitch: tape.start_pitch,
                rows: Vec::new(),
                arrivals: Vec::new(),
            };
            out.push_str(&write_segment(&bare, views[s.name.as_str()]));
        }
        out
    }

    /// `diverge_tol=`, or the default.
    pub fn diverge_tol(&self) -> f64 {
        self.tape
            .get("diverge_tol")
            .and_then(|v| v.parse().ok())
            .unwrap_or(DIVERGE_TOL)
    }

    /// `velocity_tol=` (blocks per tick, per component), if the header has one.
    pub fn velocity_tol(&self) -> Option<f64> {
        self.tape.get("velocity_tol").and_then(|v| v.parse().ok())
    }

    /// How many root-to-leaf paths [`Plan::paths`] would list (a DAG can have many).
    pub fn path_count(&self) -> u128 {
        let views = self.views();
        let mut memo: HashMap<&str, u128> = HashMap::new();
        fn count<'a>(
            n: &'a str,
            views: &HashMap<&'a str, View<'a>>,
            memo: &mut HashMap<&'a str, u128>,
        ) -> u128 {
            if let Some(&c) = memo.get(n) {
                return c;
            }
            let v = views[n];
            let mut c: u128 = 0;
            for b in &v.links.branches {
                c += count(
                    views.get_key_value(b.child.as_str()).unwrap().0,
                    views,
                    memo,
                );
            }
            c += match &v.links.goto {
                Some(g) => count(views.get_key_value(g.as_str()).unwrap().0, views, memo),
                None => 1,
            };
            memo.insert(n, c);
            c
        }
        count(ROOT, &views, &mut memo)
    }

    /// Every root-to-leaf path, depth first: at each segment the paths through its
    /// branches in file order, then the one where none fires. A path that ends in
    /// [`Leaf::Missing`] is where playback aborts; the others should verify.
    pub fn paths(&self) -> Vec<PlanPath> {
        let views = self.views();
        let mut acc = Walk::default();
        let mut out = Vec::new();
        self.walk(&views, self.root(), &mut acc, &mut out);
        out
    }

    fn walk(&self, views: &HashMap<&str, View>, s: View, acc: &mut Walk, out: &mut Vec<PlanPath>) {
        acc.segments.push(s.name.to_string());
        let mark = (acc.rows.len(), acc.arrivals.len(), acc.reacts.len());
        let cut = |acc: &mut Walk, leave: usize| {
            acc.rows.extend_from_slice(&s.rows[..leave - s.start]);
            acc.reacts
                .extend(s.reacts.iter().filter(|r| r.t0 < leave).map(|r| React {
                    t1: r.t1.min(leave - 1),
                    ..*r
                }));
        };
        for b in &s.links.branches {
            cut(acc, b.tick);
            acc.arrivals.push(Arrival {
                tick: b.tick,
                event: b.event,
            });
            self.walk(views, views[b.child.as_str()], acc, out);
            acc.rows.truncate(mark.0);
            acc.arrivals.truncate(mark.1);
            acc.reacts.truncate(mark.2);
        }
        cut(acc, s.end());
        match &s.links.goto {
            Some(g) => self.walk(views, views[g.as_str()], acc, out),
            None => {
                let leaf = if s.links.branches.iter().any(|b| b.tick == s.end()) {
                    Leaf::Missing { tick: s.end() }
                } else {
                    Leaf::End
                };
                let mut tape = Tape {
                    rows: acc.rows.clone(),
                    ..self.tape.clone()
                };
                if tape.get("ticks").is_some() {
                    tape.set("ticks", tape.rows.len().to_string());
                }
                out.push(PlanPath {
                    segments: acc.segments.clone(),
                    tape,
                    arrivals: acc.arrivals.clone(),
                    reacts: acc.reacts.clone(),
                    leaf,
                });
            }
        }
        acc.rows.truncate(mark.0);
        acc.arrivals.truncate(mark.1);
        acc.reacts.truncate(mark.2);
        acc.segments.pop();
    }
}

#[derive(Default)]
struct Walk {
    segments: Vec<String>,
    rows: Vec<TapeRow>,
    arrivals: Vec<Arrival>,
    reacts: Vec<React>,
}

struct SegmentText {
    line: usize,
    head: String,
    body: Vec<(usize, String)>,
}

#[derive(Default)]
struct Directives {
    reacts: Vec<React>,
    links: Links,
    gotos: usize,
}

impl Directives {
    fn finish(self, segment: &str) -> Result<(Vec<React>, Links), String> {
        if self.gotos > 1 {
            return Err(format!("segment {segment}: more than one @goto"));
        }
        Ok((self.reacts, self.links))
    }
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-'))
}

fn parse_directive(line: usize, data: &str, v2: bool, d: &mut Directives) -> Result<(), String> {
    let f: Vec<&str> = data.split_whitespace().collect();
    let at = |e: String| format!("line {line}: {e}");
    let tick = |s: &str| -> Result<usize, String> {
        s.parse::<usize>()
            .map_err(|e| at(format!("tick {s:?}: {e}")))
    };
    let event = |s: &str| Event::parse(s).ok_or_else(|| at(format!("unknown event {s}")));
    match f[0] {
        "@react" => {
            if f.len() != 5 {
                return Err(at(format!("@react has {} fields, want 4", f.len() - 1)));
            }
            let (t0, t1) = (tick(f[1])?, tick(f[2])?);
            if t1 < t0 {
                return Err(at(format!("window {t0}..{t1} not within its segment")));
            }
            let ev = match event(f[3])? {
                Event::GlideOff => Event::GlideOff,
                e => return Err(at(format!("unknown event {} for @react", e.name()))),
            };
            let action = match f[4] {
                "press_jump" => Action::PressJump,
                a => return Err(at(format!("unknown action {a}"))),
            };
            d.reacts.push(React {
                t0,
                t1,
                event: ev,
                action,
            });
        }
        "@branch" if v2 => {
            if f.len() < 4 {
                return Err(at("want @branch <tick> <event> [payload] <child>".into()));
            }
            let t = tick(f[1])?;
            let ev = match event(f[2])? {
                Event::GlideOff if f.len() == 4 => ServerEvent::GlideOff,
                Event::SetVelocity if f.len() == 7 => {
                    let mut v = [0.0; 3];
                    for (k, s) in f[3..6].iter().enumerate() {
                        v[k] = f64_hex(s).map_err(at)?;
                    }
                    ServerEvent::SetVelocity { v }
                }
                e => {
                    return Err(at(format!(
                        "@branch {} has {} fields, want {}",
                        e.name(),
                        f.len() - 1,
                        if e == Event::GlideOff { 3 } else { 6 }
                    )));
                }
            };
            d.links.branches.push(Branch {
                tick: t,
                event: ev,
                child: f[f.len() - 1].to_string(),
            });
        }
        "@ignore" if v2 => {
            if f.len() != 4 {
                return Err(at(format!("@ignore has {} fields, want 3", f.len() - 1)));
            }
            let (t0, t1) = (tick(f[1])?, tick(f[2])?);
            d.links.ignores.push(Ignore {
                t0,
                t1,
                event: event(f[3])?,
            });
        }
        "@goto" if v2 => {
            if f.len() != 2 {
                return Err(at(format!("@goto has {} fields, want 1", f.len() - 1)));
            }
            d.links.goto = Some(f[1].to_string());
            d.gotos += 1;
        }
        other => {
            return Err(at(format!(
                "unknown directive {other}{}",
                if v2 {
                    ""
                } else {
                    " (format etas-plan 1 has @react only)"
                }
            )));
        }
    }
    Ok(())
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

/// A tick line, as `Tape::parse` reads one (it reads only the root's, numbered from 1).
fn parse_row(data: &str, comment: &str) -> Result<TapeRow, String> {
    let f: Vec<&str> = data.split_whitespace().collect();
    if f.len() != 7 {
        return Err(format!("{} fields, want 7", f.len()));
    }
    Ok(TapeRow {
        t: f[0].parse().map_err(|e| format!("tick {:?}: {e}", f[0]))?,
        yaw: f32_hex(f[1])?,
        pitch: f32_hex(f[2])?,
        keys: f[3].to_string(),
        pos: [f64_hex(f[4])?, f64_hex(f[5])?, f64_hex(f[6])?],
        ground: comment.split_whitespace().any(|w| w == "ground"),
    })
}

/// A segment's lines and directives under `tape`'s header (empty for named segments).
fn write_segment(tape: &Tape, s: View) -> String {
    let mut t = tape.clone();
    t.rows = s.rows.to_vec();
    let before = |tick: usize| {
        let mut out = String::new();
        for r in s.reacts.iter().filter(|r| r.t0 == tick) {
            let _ = writeln!(out, "{}", r.line());
        }
        for i in s.links.ignores.iter().filter(|i| i.t0 == tick) {
            let _ = writeln!(out, "{}", i.line());
        }
        for b in s.links.branches.iter().filter(|b| b.tick == tick) {
            let _ = writeln!(out, "{}", b.line());
        }
        out
    };
    let mut out = t.write_with(before);
    // what names the line past the last: ignores and branches there, and the goto
    let end = s.end();
    for i in s.links.ignores.iter().filter(|i| i.t0 >= end) {
        let _ = writeln!(out, "{}", i.line());
    }
    for b in s.links.branches.iter().filter(|b| b.tick >= end) {
        let _ = writeln!(out, "{}", b.line());
    }
    if let Some(g) = &s.links.goto {
        let _ = writeln!(out, "@goto {g}");
    }
    out
}

/// `verify` of every path that playback can finish ([`Leaf::End`]), under the
/// arrivals that path assumes ([`verify::verify_path`]). A path ending in
/// [`Leaf::Missing`] is an abort, which needs no verifying.
pub fn verify_paths(course: &Course, world: &World, plan: &Plan) -> Vec<(PlanPath, Report)> {
    plan.paths()
        .into_iter()
        .filter(|p| p.leaf == Leaf::End)
        .map(|p| {
            let flat = p.plan();
            let r = verify::verify_path(course, world, verify::Plan::Reactive(&flat), &p.arrivals);
            (p, r)
        })
        .collect()
}

/// The plan for a trajectory under a course: its tape, the plan header (format,
/// account, course and world hashes, `diverge_tol`), and for every planned perfect
/// bounce the window `verify` says a jump press rescues
/// ([`verify::Bounce::window_to_plan`]). Returns the trajectory's verify report too;
/// the plan is only as good as that report (and the plan's own, which the caller
/// should run).
pub fn plan_from(course: &Course, world: &World, traj: &Trajectory) -> (Plan, Report) {
    let report = verify::verify(course, world, verify::Plan::Trajectory(traj));
    let mut tape = traj.to_tape();
    let mut header = vec![
        // branchless, so version 1 (`Plan::format`)
        ("format".to_string(), FORMAT_1.to_string()),
        ("account".to_string(), ACCOUNT.to_string()),
        ("course".to_string(), course.name.clone()),
        ("course_sha256".to_string(), course.sha256.clone()),
        (
            "world_sha256".to_string(),
            course.world.content_sha256.clone(),
        ),
        ("diverge_tol".to_string(), format!("{DIVERGE_TOL:e}")),
    ];
    let keys: Vec<String> = header.iter().map(|(k, _)| k.clone()).collect();
    header.extend(tape.header.drain(..).filter(|(k, _)| !keys.contains(k)));
    tape.header = header;
    let reacts = report
        .bounces
        .iter()
        .filter_map(|b| b.window_to_plan())
        .map(|(t0, t1)| React {
            t0,
            t1,
            event: Event::GlideOff,
            action: Action::PressJump,
        })
        .collect();
    let mut plan = Plan::from_tape(tape);
    plan.reacts = reacts;
    (plan, report)
}

/// One path for [`branched`]: what to fly (ticks 1 onward, no gaps) when exactly
/// `arrivals` (sorted by tick, at most one per tick) happen.
#[derive(Debug, Clone, PartialEq)]
pub struct PathTape {
    pub rows: Vec<TapeRow>,
    pub arrivals: Vec<Arrival>,
}

/// The plan that flies each of `paths` under its arrivals, every other observation an
/// abort: a segment per run of ticks the paths in it agree on, and a branch where
/// their arrivals part (a child per event; the paths without one go on in the
/// segment). `header` gives the header and start (its rows are ignored, `ticks=` is
/// set on write), `reacts` are the root's. Fails when two paths part without an
/// observation telling them apart (their lines differ while their arrivals so far
/// agree), when one ends where another with the same arrivals goes on, or when a
/// reaction's window leaves the root. `paths()` of the result lists the input paths,
/// as [`Leaf::End`]s.
pub fn branched(header: &Tape, reacts: Vec<React>, paths: &[PathTape]) -> Result<Plan, String> {
    struct Out {
        segments: Vec<Segment>,
        named: usize,
    }
    fn same(a: &TapeRow, b: &TapeRow) -> bool {
        a.t == b.t
            && a.yaw.to_bits() == b.yaw.to_bits()
            && a.pitch.to_bits() == b.pitch.to_bits()
            && crate::state::keys_of(&a.keys) == crate::state::keys_of(&b.keys)
            && (0..3).all(|k| a.pos[k].to_bits() == b.pos[k].to_bits())
            && a.ground == b.ground
    }
    // the segment of `members` from tick `t`, its children pushed into `out`
    fn build(
        paths: &[PathTape],
        name: String,
        start: usize,
        mut members: Vec<usize>,
        out: &mut Out,
    ) -> Result<Segment, String> {
        let mut seg = Segment {
            name,
            start,
            rows: Vec::new(),
            reacts: Vec::new(),
            links: Links::default(),
        };
        let mut t = start;
        loop {
            // what each member observes before line t (entering at `start` took it)
            let mut groups: Vec<(ServerEvent, Vec<usize>)> = Vec::new();
            let mut none = Vec::new();
            for &m in &members {
                let here: Vec<&Arrival> =
                    paths[m].arrivals.iter().filter(|a| a.tick == t).collect();
                let ev = match (here.as_slice(), t == start && seg.name != ROOT) {
                    ([], _) | ([_], true) => None,
                    ([a], false) => Some(a.event),
                    _ => return Err(format!("path {m}: {} arrivals at tick {t}", here.len())),
                };
                match ev {
                    None => none.push(m),
                    Some(e) => match groups.iter_mut().find(|(g, _)| *g == e) {
                        Some((_, v)) => v.push(m),
                        None => groups.push((e, vec![m])),
                    },
                }
            }
            for (e, g) in groups {
                out.named += 1;
                let child = format!("t{t}_{}", out.named);
                let s = build(paths, child.clone(), t, g, out)?;
                out.segments.push(s);
                seg.links.branches.push(Branch {
                    tick: t,
                    event: e,
                    child,
                });
            }
            if none.is_empty() {
                return Ok(seg);
            }
            let ended: Vec<bool> = none.iter().map(|&m| paths[m].rows.len() < t).collect();
            if ended.iter().all(|&e| e) {
                return Ok(seg);
            }
            if ended.iter().any(|&e| e) {
                return Err(format!(
                    "tick {t}: paths {none:?} have the same arrivals, and some end there"
                ));
            }
            let first = &paths[none[0]].rows[t - 1];
            if let Some(&m) = none.iter().find(|&&m| !same(&paths[m].rows[t - 1], first)) {
                return Err(format!(
                    "tick {t}: paths {} and {m} fly different lines on the same arrivals",
                    none[0]
                ));
            }
            seg.rows.push(first.clone());
            members = none;
            t += 1;
        }
    }
    for (i, p) in paths.iter().enumerate() {
        if p.rows.iter().enumerate().any(|(k, r)| r.t != k + 1) {
            return Err(format!("path {i}: rows aren't ticks 1, 2, ..."));
        }
        if p.arrivals.windows(2).any(|w| w[1].tick <= w[0].tick) {
            return Err(format!("path {i}: arrivals not one per tick in order"));
        }
    }
    let mut out = Out {
        segments: Vec::new(),
        named: 0,
    };
    let root = build(
        paths,
        ROOT.to_string(),
        1,
        (0..paths.len()).collect(),
        &mut out,
    )?;
    let end = 1 + root.rows.len();
    if let Some(r) = reacts.iter().find(|r| r.t1 >= end) {
        return Err(format!("{}: window past the root's end {end}", r.line()));
    }
    let mut tape = header.clone();
    tape.rows = root.rows;
    tape.arrivals.clear();
    // children were pushed depth first; file order by start tick reads better
    out.segments.sort_by_key(|s| s.start);
    Ok(Plan {
        tape,
        reacts,
        links: root.links,
        segments: out.segments,
    })
}

/// A segment the root branches to, for [`with_children`]: on `event` observed before
/// line `tick` (`@branch <tick> …`), playback goes on with `rows` (ticks `tick`, `tick
/// + 1`, … to the plan's end on that path) and their own reactions and ignores.
#[derive(Debug, Clone, PartialEq)]
pub struct Child {
    pub name: String,
    pub tick: usize,
    pub event: ServerEvent,
    pub rows: Vec<TapeRow>,
    pub reacts: Vec<React>,
    pub ignores: Vec<Ignore>,
}

/// `main` (a plan without segments) with a root branch to each of `children`, in the
/// order given (file order of the branches, and of the segments). Fails when a child's
/// rows don't run from its tick without gaps, or its tick isn't within the root
/// (`1..=` its end). The result is checked as `Plan::parse` checks a file.
pub fn with_children(main: Plan, children: Vec<Child>) -> Result<Plan, String> {
    if !main.segments.is_empty() {
        return Err("main plan already has segments".into());
    }
    let mut plan = main;
    let end = 1 + plan.tape.rows.len();
    for c in children {
        if !(1..=end).contains(&c.tick) {
            return Err(format!(
                "child {}: tick {} outside the root 1..={end}",
                c.name, c.tick
            ));
        }
        if c.rows.iter().enumerate().any(|(k, r)| r.t != c.tick + k) {
            return Err(format!(
                "child {}: rows don't run from tick {}",
                c.name, c.tick
            ));
        }
        plan.links.branches.push(Branch {
            tick: c.tick,
            event: c.event,
            child: c.name.clone(),
        });
        plan.segments.push(Segment {
            name: c.name,
            start: c.tick,
            rows: c.rows,
            reacts: c.reacts,
            links: Links {
                ignores: c.ignores,
                ..Links::default()
            },
        });
    }
    if plan.tape.get("format").is_some() {
        plan.tape.set("format", plan.format().to_string());
    }
    Plan::parse(&plan.write())
}
