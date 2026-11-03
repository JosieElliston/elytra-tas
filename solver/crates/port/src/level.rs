//! The world as movement sees it: `Level.getBlockState`, `BlockCollisions` and the
//! `CollisionGetter` queries built on it.

use std::collections::HashMap;

use world::{Aabb, BlockDetail, BlockInfo, BlockPos, Cell, Effect, OffsetType, UnknownSpace};

use crate::Unsupported;
use crate::mth;
use crate::phys::AABB;
use crate::shapes::{self, BooleanOp, VoxelShape};

/// A block state at a position: its table row and `BlockState.getOffset(pos)`.
#[derive(Debug, Clone, Copy)]
pub struct BlockState<'a> {
    pub info: &'a BlockInfo,
    pub detail: &'a BlockDetail,
    /// The shape offset at this position; zero unless `info.offset` says otherwise.
    pub offset: [f64; 3],
}

impl BlockState<'_> {
    /// The id without properties, e.g. `minecraft:oak_fence`.
    pub fn name(&self) -> &str {
        let s = self.info.state.as_str();
        s.split('[').next().unwrap_or(s)
    }

    /// `state.is(BlockTags.FENCES)`, as the class `FenceBlock`: vanilla's tag is
    /// exactly those blocks, and world files carry only the climbing tags.
    pub fn is_fence(&self) -> bool {
        self.detail.class == "FenceBlock"
    }

    /// `state.is(BlockTags.WALLS)`, as the class `WallBlock` (see [`Self::is_fence`]).
    pub fn is_wall(&self) -> bool {
        self.detail.class == "WallBlock"
    }

    /// `state.getBlock() instanceof FenceGateBlock`.
    pub fn is_fence_gate(&self) -> bool {
        self.detail.class == "FenceGateBlock"
    }

    /// `getCollisionShape(level, pos, context)`, before `BlockCollisions` moves it to
    /// `pos`. An offset block's shape is its local shape moved by the offset, so its
    /// coordinates come out as `(local + offset) + pos`, the game's order.
    pub fn collision_shape(&self) -> VoxelShape {
        let local = VoxelShape::from_boxes(&self.info.boxes);
        if self.info.offset == OffsetType::None {
            local
        } else {
            local.move_(self.offset[0], self.offset[1], self.offset[2])
        }
    }

    /// `BlockStateBase.hasLargeCollisionShape`.
    pub fn has_large_collision_shape(&self) -> bool {
        self.detail.large_shape
    }

    /// The state holds water or lava (`getFluidState()` not empty).
    pub fn has_fluid(&self) -> bool {
        self.detail.fluid != "-"
    }
}

/// Block lookups. Unknown space is an error, never air. why? a route through a chunk
/// the source never loaded must not pass `verify`.
pub trait Level {
    fn get_block_state(&self, pos: BlockPos) -> Result<BlockState<'_>, UnknownSpace>;
}

impl Level for world::World {
    fn get_block_state(&self, pos: BlockPos) -> Result<BlockState<'_>, UnknownSpace> {
        let Cell::Known(id) = self.cell(pos) else {
            return Err(UnknownSpace(pos));
        };
        Ok(BlockState {
            info: self.info(id),
            detail: self.detail(id),
            offset: self.offset(id, pos),
        })
    }
}

/// A hand-built level for tests: blocks by position, air elsewhere, and optionally
/// unknown outside `known`. Blocks have no position offset.
#[derive(Debug, Clone)]
pub struct BlockMap {
    pub blocks: HashMap<BlockPos, (BlockInfo, BlockDetail)>,
    pub known: Option<(BlockPos, BlockPos)>,
    air: (BlockInfo, BlockDetail),
}

impl Default for BlockMap {
    fn default() -> Self {
        let air = block_info("minecraft:air", &[]);
        let air_detail = block_detail(&air);
        BlockMap {
            blocks: HashMap::new(),
            known: None,
            air: (air, air_detail),
        }
    }
}

/// A plain block (friction 0.6, factors 1, no effect) with the given local boxes.
pub fn block_info(state: &str, boxes: &[Aabb]) -> BlockInfo {
    BlockInfo {
        state: state.to_string(),
        friction: 0.6,
        speed_factor: 1.0,
        jump_factor: 1.0,
        effect: Effect::None,
        offset: OffsetType::None,
        boxes: boxes.to_vec(),
    }
}

/// The table row a hand-built block implies: class `Block`, fluid and effects from
/// `info.effect`, large and suffocating from the boxes (vanilla's defaults).
pub fn block_detail(info: &BlockInfo) -> BlockDetail {
    let unit = [Aabb {
        min: [0.0; 3],
        max: [1.0; 3],
    }];
    BlockDetail {
        class: "Block".to_string(),
        fluid: match info.effect {
            Effect::Water => "water",
            Effect::Lava => "lava",
            _ => "-",
        }
        .to_string(),
        own_effect: if info.effect == Effect::Climbable {
            Effect::None
        } else {
            info.effect
        },
        climbable: info.effect == Effect::Climbable,
        glide_through: false,
        large_shape: info
            .boxes
            .iter()
            .any(|b| (0..3).any(|i| b.min[i] < 0.0 || b.max[i] > 1.0)),
        max_horizontal_offset: 0.0,
        max_vertical_offset: 0.0,
        block_id: 0,
        is_default: true,
        suffocating: info.boxes == unit,
    }
}

impl BlockMap {
    /// Set a block, with the detail [`block_detail`] implies.
    pub fn set(&mut self, pos: BlockPos, info: BlockInfo) {
        let detail = block_detail(&info);
        self.blocks.insert(pos, (info, detail));
    }

    /// Set a block with an explicit table row.
    pub fn set_with_detail(&mut self, pos: BlockPos, info: BlockInfo, detail: BlockDetail) {
        self.blocks.insert(pos, (info, detail));
    }

    /// Full stone blocks over `[x0, x1] x {y} x [z0, z1]`.
    pub fn floor(&mut self, x0: i32, x1: i32, y: i32, z0: i32, z1: i32) {
        let stone = block_info(
            "minecraft:stone",
            &[Aabb {
                min: [0.0; 3],
                max: [1.0; 3],
            }],
        );
        for x in x0..=x1 {
            for z in z0..=z1 {
                self.set(BlockPos { x, y, z }, stone.clone());
            }
        }
    }
}

impl Level for BlockMap {
    fn get_block_state(&self, pos: BlockPos) -> Result<BlockState<'_>, UnknownSpace> {
        if let Some((lo, hi)) = self.known {
            let inside = (lo.x..=hi.x).contains(&pos.x)
                && (lo.y..=hi.y).contains(&pos.y)
                && (lo.z..=hi.z).contains(&pos.z);
            if !inside {
                return Err(UnknownSpace(pos));
            }
        }
        let (info, detail) = self.blocks.get(&pos).unwrap_or(&self.air);
        Ok(BlockState {
            info,
            detail,
            offset: [0.0; 3],
        })
    }
}

/// `new BlockCollisions(level, entity, box, onlySuffocatingBlocks, (pos, shape) -> ...)`,
/// drained: every block whose moved collision shape overlaps `bx`, in the cursor's
/// order (x fastest, then y, then z).
///
/// Differs from the game in one way: an unknown cell is an error (the game skips an
/// unloaded chunk).
pub fn block_collisions<L: Level + ?Sized>(
    level: &L,
    bx: &AABB,
    only_suffocating: bool,
) -> Result<Vec<(BlockPos, VoxelShape)>, Unsupported> {
    let entity_shape = VoxelShape::create_aabb(bx);
    let x0 = mth::floor(bx.min_x - 1.0E-7) - 1;
    let x1 = mth::floor(bx.max_x + 1.0E-7) + 1;
    let y0 = mth::floor(bx.min_y - 1.0E-7) - 1;
    let y1 = mth::floor(bx.max_y + 1.0E-7) + 1;
    let z0 = mth::floor(bx.min_z - 1.0E-7) - 1;
    let z1 = mth::floor(bx.max_z + 1.0E-7) + 1;
    let mut out = Vec::new();
    // `Cursor3D.advance`: x fastest, then y, then z.
    for z in z0..=z1 {
        for y in y0..=y1 {
            for x in x0..=x1 {
                // `Cursor3D.getNextType`: how many coordinates are on the cursor's edge
                let cursor_face_type = (x == x0 || x == x1) as i32
                    + (y == y0 || y == y1) as i32
                    + (z == z0 || z == z1) as i32;
                if cursor_face_type == 3 {
                    continue;
                }
                let pos = BlockPos { x, y, z };
                let block_state = level.get_block_state(pos)?;
                if only_suffocating && !block_state.detail.suffocating {
                    continue;
                }
                let block_shape = block_state.collision_shape();
                if cursor_face_type == 1 && !block_state.has_large_collision_shape() {
                    continue;
                }
                if cursor_face_type == 2 {
                    // only Blocks.MOVING_PISTON passes here
                    continue;
                }
                let (fx, fy, fz) = (x as f64, y as f64, z as f64);
                if block_shape.is_block {
                    if bx.intersects(fx, fy, fz, fx + 1.0, fy + 1.0, fz + 1.0) {
                        out.push((pos, block_shape.move_(fx, fy, fz)));
                    }
                } else {
                    let shape = block_shape.move_(fx, fy, fz);
                    if !shape.is_empty()
                        && shapes::join_is_not_empty(&shape, &entity_shape, BooleanOp::And)
                    {
                        out.push((pos, shape));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// `CollisionGetter.getBlockCollisions(entity, box)`.
pub fn get_block_collisions<L: Level + ?Sized>(
    level: &L,
    bx: &AABB,
) -> Result<Vec<VoxelShape>, Unsupported> {
    Ok(block_collisions(level, bx, false)?
        .into_iter()
        .map(|(_, s)| s)
        .collect())
}

/// `CollisionGetter.noCollision(entity, box)`: blocks only (no entities, world border
/// ignored).
pub fn no_collision<L: Level + ?Sized>(level: &L, bx: &AABB) -> Result<bool, Unsupported> {
    Ok(block_collisions(level, bx, false)?
        .iter()
        .all(|(_, s)| s.is_empty()))
}

/// `CollisionGetter.collidesWithSuffocatingBlock(entity, box)`.
pub fn collides_with_suffocating_block<L: Level + ?Sized>(
    level: &L,
    bx: &AABB,
) -> Result<bool, Unsupported> {
    Ok(block_collisions(level, bx, true)?
        .iter()
        .any(|(_, s)| !s.is_empty()))
}

/// `Vec3i.compareTo`: y, then z, then x.
fn compare_to(a: BlockPos, b: BlockPos) -> i32 {
    if a.y == b.y {
        if a.z == b.z { a.x - b.x } else { a.z - b.z }
    } else {
        a.y - b.y
    }
}

/// `CollisionGetter.findSupportingBlock(entity, box)`: the colliding block whose
/// center is nearest `position`, ties to the larger position.
pub fn find_supporting_block<L: Level + ?Sized>(
    level: &L,
    position: crate::phys::Vec3,
    bx: &AABB,
) -> Result<Option<BlockPos>, Unsupported> {
    let mut main_support: Option<BlockPos> = None;
    let mut main_support_distance = f64::MAX;
    for (pos, _) in block_collisions(level, bx, false)? {
        // `Vec3i.distToCenterSqr`
        let dx = pos.x as f64 + 0.5 - position.x;
        let dy = pos.y as f64 + 0.5 - position.y;
        let dz = pos.z as f64 + 0.5 - position.z;
        let distance = dx * dx + dy * dy + dz * dz;
        if distance < main_support_distance
            || distance == main_support_distance
                && (main_support.is_none() || compare_to(main_support.unwrap(), pos) < 0)
        {
            main_support = Some(pos);
            main_support_distance = distance;
        }
    }
    Ok(main_support)
}

/// The first cell overlapping `bx` whose state passes `test` (cells `floor(min)` to
/// `ceil(max) - 1`, as `EntityFluidInteraction.update` scans): how the port detects
/// the movement it doesn't simulate (fluids, stuck blocks).
pub fn find_block<L: Level + ?Sized>(
    level: &L,
    bx: &AABB,
    test: impl Fn(&BlockState) -> bool,
) -> Result<Option<BlockPos>, Unsupported> {
    let ceil = |v: f64| v.ceil() as i32;
    for x in mth::floor(bx.min_x)..ceil(bx.max_x) {
        for y in mth::floor(bx.min_y)..ceil(bx.max_y) {
            for z in mth::floor(bx.min_z)..ceil(bx.max_z) {
                let pos = BlockPos { x, y, z };
                if test(&level.get_block_state(pos)?) {
                    return Ok(Some(pos));
                }
            }
        }
    }
    Ok(None)
}
