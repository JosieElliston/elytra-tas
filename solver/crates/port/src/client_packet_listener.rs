//! `net.minecraft.client.multiplayer.ClientPacketListener`: the two server packets
//! that change the local player's movement state. The client handles packets between
//! ticks (at a frame's start, before its ticks), so a caller replaying real play
//! applies each before the physics of the first tick that sees it.

use crate::{LocalPlayer, Pose, Vec3};

impl LocalPlayer {
    /// `handleSetEntityMotion` for the local player: `Entity.lerpMotion`, which for a
    /// `LocalPlayer` is `setDeltaMovement`. `movement` is the packet's `LpVec3`
    /// decoding, exactly what the client applies (boosters, the wall-hit echo).
    pub fn handle_set_entity_motion(&mut self, movement: Vec3) {
        self.set_delta_movement(movement);
    }

    /// `handleSetEntityData` carrying the server's glide end: shared flag 7 cleared
    /// and `DATA_POSE` set to the server's pose (`refreshDimensions` follows; the box
    /// is recomputed from the pose, see [`LocalPlayer::get_bounding_box`]). The pose
    /// is the server's `updatePlayerPose` after the glide ended, run on its copy of
    /// the player: standing if it fits there, else crouching, else swimming. The
    /// packet's flag byte also carries the server's sprint bit; it is taken to be the
    /// client's (the server's follows the client's sprint commands).
    pub fn handle_glide_off(&mut self, pose: Pose) {
        self.fall_flying = false;
        self.set_pose(pose);
    }
}
