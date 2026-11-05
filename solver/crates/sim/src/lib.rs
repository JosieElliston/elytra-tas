//! Flight kernels for the solver tools: see ARCHITECTURE.md, "Physics". There is no
//! global state in this crate; a kernel is a value you pass in.
//!
//! [`faithful`] is the game's arithmetic and collision, exact against `port`: normal
//! flight, perfect-ebounce glides, and any tick from a full state (walking, lost
//! glides, server events); [`generic`] is plain math over a float trait,
//! contact-free flight.

pub mod faithful;
pub mod generic;
pub mod state;
