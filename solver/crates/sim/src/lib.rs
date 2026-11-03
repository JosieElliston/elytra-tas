//! Flight kernels for the solver tools: see ARCHITECTURE.md, "Physics". There is no
//! global state in this crate; a kernel is a value you pass in.
//!
//! [`faithful`] is the game's arithmetic and collision for normal flight, exact
//! against `port`; [`generic`] is plain math over a float trait, contact-free.

pub mod faithful;
pub mod generic;
pub mod state;
