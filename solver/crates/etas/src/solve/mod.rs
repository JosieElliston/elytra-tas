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
//! - [`smooth`]: a plan's rotation smoothed by priced windows, at no cost in time
//! - [`stretch`]: a stretch of a trajectory as a problem, and the splice back
//! - [`reach`]: the least ticks from a state to a point, velocity free (into a booster)
//! - [`cases`]: a booster pass's outcomes and how often each comes, for branching plans
//! - [`bounces`]: a recording's lost-glide bounces re-solved as perfect ones, and the
//!   lost-glide flights a plan branches to

pub mod bounces;
pub mod cases;
pub mod ddp;
pub mod reach;
pub mod refly;
pub mod smooth;
pub mod stretch;
pub mod walls;
