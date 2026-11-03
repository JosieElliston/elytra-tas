//! The toolkit (ARCHITECTURE.md, Tools): course files, artifacts and `verify` so far.
//!
//! - [`course`]: course files from `fit_course.py`, and their world
//! - [`trajectory`]: the trajectory artifact; [`tape`]: `.tas` tapes, the branchless plan
//! - [`verify`]: the gate every plan passes before it goes to the mod

pub mod course;
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
