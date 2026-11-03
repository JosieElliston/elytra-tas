//! The first solver (ARCHITECTURE.md, build order step 5): re-solve a stretch of a
//! recorded run on the generic kernel, re-fly it under the faithful kernel, splice it
//! back. The interface is this method's own; shared solver interfaces come once a
//! second solver composes with it.
//!
//! - [`walls`]: the obstacles near a path, for the ghost constraints
//! - [`ddp`]: DDP through ghost walls under an augmented Lagrangian, controls
//!   `(lead, pitch)`, goal a plane crossing (secant time) or a state to reach
//! - [`refly`]: a plan under the faithful kernel, where it parts from the generic one,
//!   and the repair by re-solving from the flown state each tick
//! - [`stretch`]: a stretch of a trajectory as a problem, and the splice back

pub mod ddp;
pub mod refly;
pub mod stretch;
pub mod walls;
