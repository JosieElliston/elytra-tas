//! Shared by the tests: the Burrowed Grove course crate's course and the recorded tape.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use etas::course::Course;
use etas::tape::Tape;
use world::World;

pub fn course_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../courses/burrowed_grove")
}

pub fn tape_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tas/burrowed_grove.tas")
}

pub fn load() -> (Course, World, Tape) {
    let (c, w) = Course::load(&course_dir()).unwrap();
    (c, w, Tape::read(&tape_path()).unwrap())
}
