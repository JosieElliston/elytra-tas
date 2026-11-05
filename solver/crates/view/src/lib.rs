//! Pictures of the world and of what a run does in it, for checking routes and
//! optimizers by eye. CPU ray traced, so it needs no GPU and has no window.
//!
//! ```no_run
//! # fn main() -> Result<(), String> {
//! use view::{Camera, LabelStyle, Scene, Stroke, WorldStyle, color::RED, tools};
//! # let world = world::World::air(
//! #     world::BlockPos { x: 0, y: 0, z: 0 }, world::BlockPos { x: 9, y: 9, z: 9 });
//! # let pts = vec![[1.0, 1.0, 1.0], [8.0, 8.0, 8.0]];
//! let scene = Scene::new(&world)?;
//! let mut f = scene.frame(&Camera::top(&pts), 1400, 900);
//! f.style(WorldStyle::new().below_y(120.0));
//! f.polyline(&pts, Stroke::new(RED, 2.0));
//! f.label(pts[1], "tick 120", LabelStyle::default());
//! tools::rulers(&mut f);
//! f.finish().save("out.png")?;
//! # Ok(()) }
//! ```
//!
//! [`Frame`] is immediate mode: call draw functions, then [`Frame::finish`]. Helpers are
//! plain functions taking `&mut Frame`, like those in [`tools`] and `etas::draw`, so the
//! primitives stay available beside them.
//!
//! Speed, as of 2026-11-04 on an M5 with Burrowed Grove and Super Classic Downtown:
//! 0.2 to 0.6 s for a 1400x900 frame (2x antialiasing, shadows), 50 ms to prepare a
//! [`Scene`].
//!
//! Annotations are depth-tested against the world. A stroke's `occluded` sets how much
//! of it shows behind blocks, so a path inside a canyon isn't lost.
//!
//! Textures and models come from the client jar at runtime ([`find_jar`]). Without one,
//! the world is flat-colored collision boxes. Not drawn, because world files don't have
//! them: entities (boats, shulkers) and biome colors (tints are plains' constants).
//! Block entities whose model has no geometry (chests, beds, shulker boxes) are their
//! collision boxes in their particle texture; one with neither a model nor collision
//! doesn't appear. Barriers are hidden unless a style asks for them
//! ([`WorldStyle::show_barriers`]), and they still collide.

// colors and axes are fixed-size arrays indexed together
#![allow(clippy::needless_range_loop)]

mod assets;
pub mod camera;
pub mod color;
mod frame;
mod image;
mod scene;
mod style;
pub mod tools;

pub use assets::find_jar;
pub use camera::{Camera, V3, View, look_vector};
pub use color::{Color, Ramp};
pub use frame::{Align, Frame, LabelStyle, Marker, Shape, Stroke, TextStyle};
pub use image::Image;
pub use scene::Scene;
pub use style::{CellMask, Geometry, WorldStyle};

#[cfg(test)]
mod tests;
