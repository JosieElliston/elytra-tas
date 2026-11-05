//! The toolkit: course files, artifacts, `verify` and one
//! solver so far.
//!
//! - [`course`]: course files from `fit_course.py`, and their world
//! - [`booster`]: boosters' velocities as cases, from the moves the server saw
//! - [`trajectory`]: the trajectory artifact; [`tape`]: `.tas` tapes, the branchless plan
//! - [`plan`]: plans for the `/etas` mod (tapes, reactions, branches; `PLAN-FORMAT.md`)
//! - [`server`]: server events (glide-off, velocity) as the client handled them
//! - [`record`]: replay a recorded run under port and the faithful kernel, no course
//! - [`verify`]: the gate every plan passes before it goes to the mod
//! - [`solve`]: the first solver, re-solving a stretch of a recorded run
//! - [`draw`]: runs, courses and reports as annotations on a `view::Frame`

pub mod booster;
pub mod course;
pub mod draw;
pub mod plan;
pub mod record;
pub mod server;
pub mod solve;
pub mod state;
pub mod tape;
pub mod trajectory;
pub mod verify;

/// The commit of this source tree and whether the tree has changes, for artifact
/// headers; ("unknown", true) without git. The tree is the one this crate was built
/// from (`CARGO_MANIFEST_DIR`).
pub fn git_state() -> (String, bool) {
    let dir = env!("CARGO_MANIFEST_DIR");
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    match (run(&["rev-parse", "HEAD"]), run(&["status", "--porcelain"])) {
        (Some(c), Some(s)) => (c, !s.is_empty()),
        _ => ("unknown".into(), true),
    }
}
