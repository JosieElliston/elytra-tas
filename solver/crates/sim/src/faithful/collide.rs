//! Vanilla collision for a gliding box, against the world's collision boxes, and the
//! block checks around a normal-flight move.
//!
//! The game collides against `VoxelShape`s: per block, a grid of cells, scanned from
//! the cell holding the moving face. Here a block is its list of boxes (the table's
//! `toAabbs()` boxes). The two agree whenever the moving box is not inside one of
//! the boxes by more than 1e-7 on every axis: the grid's inner lines only matter
//! from inside. A start inside is [`Event::InsideBlock`], and a move never leads
//! inside (it can sink up to 1e-7 into a box on one axis, which both handle alike).
//! Blocks stay separate in the cursor's order, because `Shapes.collide` snaps the
//! distance to 0 once it is under 1e-7 before the next block, so the order of the
//! blocks matters at that scale.

// `!(a < b)` is not `a >= b` for NaN, and the merge starts from NaN as the Java does.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use port::mth;
use world::{Aabb, BlockPos, Cell, Effect, OffsetType, StateId, World};

use super::Event;

/// `Mth`'s and `Shapes`' collision tolerance.
const EPS: f64 = 1.0E-7;

const UNIT: Aabb = Aabb {
    min: [0.0; 3],
    max: [1.0; 3],
};

/// `EntityDimensions.makeBoundingBox` for the gliding pose (0.6 x 0.6): the half
/// width is a float.
pub(super) fn player_box(pos: [f64; 3]) -> Aabb {
    let w = (0.6f32 / 2.0) as f64;
    let h = 0.6f32 as f64;
    Aabb {
        min: [pos[0] - w, pos[1], pos[2] - w],
        max: [pos[0] + w, pos[1] + h, pos[2] + w],
    }
}

/// `AABB.deflate(amount)`.
pub(super) fn deflate(b: &Aabb, amount: f64) -> Aabb {
    let lo: [f64; 3] = std::array::from_fn(|i| b.min[i] - -amount);
    let hi: [f64; 3] = std::array::from_fn(|i| b.max[i] + -amount);
    Aabb {
        min: std::array::from_fn(|i| mth::java_min(lo[i], hi[i])),
        max: std::array::from_fn(|i| mth::java_max(lo[i], hi[i])),
    }
}

/// `AABB.expandTowards(v)`.
fn expand_towards(b: &Aabb, v: [f64; 3]) -> Aabb {
    let mut out = *b;
    for (i, &d) in v.iter().enumerate() {
        if d < 0.0 {
            out.min[i] += d;
        } else if d > 0.0 {
            out.max[i] += d;
        }
    }
    out
}

/// Positive-volume overlap (`AABB.intersects`).
fn intersects(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|i| a.min[i] < b.max[i] && a.max[i] > b.min[i])
}

/// `Shapes.joinIsNotEmpty(shape, area, AND)` for one box of a shape: on each axis,
/// the two intervals still overlap after `IndirectMerger` merges coordinates within
/// 1e-7. The game merges the whole shape's grid with the area's; the grid's other
/// lines move a merged boundary inside the box, which keeps the overlap.
fn joins(b: &Aabb, area: &Aabb) -> bool {
    (0..3).all(|i| merged_overlap([b.min[i], b.max[i]], [area.min[i], area.max[i]]))
}

/// Whether `IndirectMerger(first, second, false, false)` (an AND's) has a merged cell
/// in both intervals, after `Shapes.createIndexMerger`'s disjoint shortcut.
fn merged_overlap(first: [f64; 2], second: [f64; 2]) -> bool {
    if first[1] < second[0] - EPS || second[1] < first[0] - EPS {
        return false;
    }
    if first == second {
        return true;
    }
    // (first cell, second cell) of each merged coordinate
    let mut cells: [(usize, usize); 4] = [(0, 0); 4];
    let mut n = 0;
    let mut last = f64::NAN;
    let (mut i, mut j) = (0, 0);
    loop {
        let (out_first, out_second) = (i >= 2, j >= 2);
        if out_first && out_second {
            break;
        }
        let chose_first = !out_first && (out_second || first[i] < second[j] + EPS);
        if chose_first {
            i += 1;
            if j == 0 || out_second {
                continue;
            }
        } else {
            j += 1;
            if i == 0 || out_first {
                continue;
            }
        }
        let next = if chose_first {
            first[i - 1]
        } else {
            second[j - 1]
        };
        if !(last >= next - EPS) {
            cells[n] = (i - 1, j - 1);
            n += 1;
            last = next;
        } else {
            cells[n - 1] = (i - 1, j - 1);
        }
    }
    // merged cell r lies between coordinates r and r + 1
    cells[..n.saturating_sub(1)].contains(&(0, 0))
}

fn floor_pos(p: [f64; 3]) -> BlockPos {
    BlockPos {
        x: mth::floor(p[0]),
        y: mth::floor(p[1]),
        z: mth::floor(p[2]),
    }
}

pub(super) fn state_at(world: &World, pos: BlockPos) -> Result<StateId, Event> {
    match world.cell(pos) {
        Cell::Known(id) => Ok(id),
        Cell::Unknown => Err(Event::UnknownSpace(pos)),
    }
}

/// The first cell overlapping `bx` (cells `floor(min)` to `ceil(max) - 1`, x outer,
/// as port's detectors scan) whose state passes `test`.
fn find_cell(
    world: &World,
    bx: &Aabb,
    test: impl Fn(StateId) -> bool,
) -> Result<Option<BlockPos>, Event> {
    let lo: [i32; 3] = std::array::from_fn(|i| mth::floor(bx.min[i]));
    let hi: [i32; 3] = std::array::from_fn(|i| bx.max[i].ceil() as i32);
    for x in lo[0]..hi[0] {
        for y in lo[1]..hi[1] {
            for z in lo[2]..hi[2] {
                let pos = BlockPos { x, y, z };
                if test(state_at(world, pos)?) {
                    return Ok(Some(pos));
                }
            }
        }
    }
    Ok(None)
}

/// `Entity.updateFluidInteraction` as port detects it: a state holding water or
/// lava in a cell overlapping `bx`.
pub(super) fn find_fluid(world: &World, bx: &Aabb) -> Result<(), Event> {
    match find_cell(world, bx, |id| world.detail(id).fluid != "-")? {
        Some(pos) => Err(Event::Fluid(pos)),
        None => Ok(()),
    }
}

/// `Entity.applyEffectsFromBlocks`' stuck blocks: between the box before and after
/// the move, deflated by `1.0E-5F`.
pub(super) fn find_stuck(world: &World, before: &Aabb, after: &Aabb) -> Result<(), Event> {
    let swept = Aabb {
        min: std::array::from_fn(|i| mth::java_min(before.min[i], after.min[i])),
        max: std::array::from_fn(|i| mth::java_max(before.max[i], after.max[i])),
    };
    let swept = deflate(&swept, 1.0E-5f32 as f64);
    match find_cell(world, &swept, |id| {
        world.detail(id).own_effect == Effect::Stuck
    })? {
        Some(pos) => Err(Event::Stuck(pos)),
        None => Ok(()),
    }
}

/// `LivingEntity.onClimbable()` at the start of `travelFallFlying`, gliding: the block
/// at the position, unless the world's `can_glide_through` tag has it.
pub(super) fn check_climbable(world: &World, pos: [f64; 3]) -> Result<(), Event> {
    let at = floor_pos(pos);
    let d = world.detail(state_at(world, at)?);
    if d.climbable && !d.glide_through {
        Err(Event::Climbable(at))
    } else {
        Ok(())
    }
}

/// A block [`for_blocks`] visits.
#[derive(Clone, Copy)]
struct Visit {
    pos: BlockPos,
    id: StateId,
    offset: [f64; 3],
}

/// The shape is `Shapes.block()`, which `BlockCollisions` tests as a cube, not by
/// joining.
fn is_unit(world: &World, id: StateId, offset: [f64; 3]) -> bool {
    let boxes = &world.info(id).boxes;
    boxes.len() == 1 && boxes[0] == UNIT && offset == [0.0; 3]
}

/// The blocks `BlockCollisions` visits for `area`, in its cursor order (x fastest,
/// then y, then z): cells overlapping `area` with its 1e-7 margin, plus the
/// face-adjacent shell for large shapes (fences, walls). `each` gets a block's
/// position and its boxes in world coordinates, `(local + offset) + pos` as the game
/// adds them, when one of them overlaps `area`. Every cell the game reads is read
/// here, so unknown space fails the same way.
///
/// Inclusion is the game's: a full cube by overlap, another shape by
/// [`joins`], which merges coordinates within 1e-7. why exact? a block that clips
/// nothing still matters, since `Shapes.collide` snaps a distance under 1e-7 to 0
/// before each further block.
fn for_blocks(
    world: &World,
    area: &Aabb,
    buf: &mut Vec<Aabb>,
    mut each: impl FnMut(Visit, &[Aabb]) -> Result<(), Event>,
) -> Result<(), Event> {
    let lo: [i32; 3] = std::array::from_fn(|i| mth::floor(area.min[i] - EPS) - 1);
    let hi: [i32; 3] = std::array::from_fn(|i| mth::floor(area.max[i] + EPS) + 1);
    for z in lo[2]..=hi[2] {
        let fz = (z == lo[2] || z == hi[2]) as u8;
        for y in lo[1]..=hi[1] {
            let fy = fz + (y == lo[1] || y == hi[1]) as u8;
            for x in lo[0]..=hi[0] {
                let face = fy + (x == lo[0] || x == hi[0]) as u8;
                if face == 3 {
                    continue;
                }
                let pos = BlockPos { x, y, z };
                let id = state_at(world, pos)?;
                let info = world.info(id);
                if face == 2 || info.boxes.is_empty() || face == 1 && !world.detail(id).large_shape
                {
                    continue;
                }
                let o = if info.offset == OffsetType::None {
                    [0.0; 3]
                } else {
                    world.offset(id, pos)
                };
                let p = [x as f64, y as f64, z as f64];
                buf.clear();
                buf.extend(info.boxes.iter().map(|b| Aabb {
                    min: std::array::from_fn(|i| (b.min[i] + o[i]) + p[i]),
                    max: std::array::from_fn(|i| (b.max[i] + o[i]) + p[i]),
                }));
                let unit = is_unit(world, id, o);
                if buf.iter().any(|b| {
                    if unit {
                        intersects(b, area)
                    } else {
                        joins(b, area)
                    }
                }) {
                    each(Visit { pos, id, offset: o }, buf)?;
                }
            }
        }
    }
    Ok(())
}

/// The blocks a move can hit, each a run of boxes, in cursor order.
struct Colliders {
    boxes: Vec<Aabb>,
    blocks: Vec<Block>,
}

struct Block {
    /// Its boxes are `boxes[previous end..end]`.
    end: usize,
    id: StateId,
    pos: BlockPos,
    /// The shape offset.
    offset: [f64; 3],
}

impl Colliders {
    fn iter(&self) -> impl Iterator<Item = (&Block, &[Aabb])> {
        let mut start = 0;
        self.blocks.iter().map(move |b| {
            let s = &self.boxes[start..b.end];
            start = b.end;
            (b, s)
        })
    }
}

/// `Entity.collide` for an airborne box: the L-shaped move of `bb` by `v`, y first,
/// then the larger horizontal axis (`Direction.axisStepOrder`), each axis clipped by
/// the blocks around the swept box. No step-up: that needs `onGround` before the
/// move, or a landing (which ends normal flight anyway).
pub(super) fn collide(
    world: &World,
    pos: [f64; 3],
    bb: &Aabb,
    v: [f64; 3],
) -> Result<[f64; 3], Event> {
    if v[0] * v[0] + v[1] * v[1] + v[2] * v[2] == 0.0 {
        return Ok(v);
    }
    let cols = colliders(world, &expand_towards(bb, v))?;
    if cols.blocks.is_empty() {
        return Ok(v);
    }
    check_suffocation(world, pos, bb, &cols)?;
    collide_with(world, bb, &cols, v)
}

/// `Entity.collide` with step-up: [`collide`], then, when the move landed or started
/// on the ground (`on_ground`) and clipped horizontally, the step-up heights the
/// game tries (the colliders' grid heights up to `maxUpStep`, lowest first), taking
/// the first that moves further horizontally.
pub(super) fn collide_step(
    world: &World,
    pos: [f64; 3],
    bb: &Aabb,
    v: [f64; 3],
    on_ground: bool,
) -> Result<[f64; 3], Event> {
    let step = collide(world, pos, bb, v)?;
    let (x_coll, z_coll) = (v[0] != step[0], v[2] != step[2]);
    let landed = v[1] != step[1] && v[1] < 0.0;
    if !((landed || on_ground) && (x_coll || z_coll)) {
        return Ok(step);
    }
    let grounded = if landed {
        move_box(bb, [0.0, step[1], 0.0])
    } else {
        *bb
    };
    let mut area = expand_towards(&grounded, [v[0], MAX_UP_STEP as f64, v[2]]);
    if !landed {
        area = expand_towards(&area, [0.0, -1.0E-5f32 as f64, 0.0]);
    }
    let cols = colliders(world, &area)?;
    let skip = step[1] as f32;
    // `collectCandidateStepUpHeights`: each block's grid heights from the box bottom,
    // as floats, 0 to the step height, deduplicated by bits, sorted
    let mut heights: Vec<f32> = Vec::with_capacity(4);
    let mut lines = Vec::new();
    for (block, _) in cols.iter() {
        lines.clear();
        lines.extend(grid_lines(world, block, 1));
        lines.sort_by(f64::total_cmp);
        let mut last = f64::NAN;
        for &l in &lines {
            // `IndirectMerger` keeps one coordinate of any within 1e-7
            if last >= l - EPS {
                continue;
            }
            last = l;
            let h = (l - grounded.min[1]) as f32;
            if !(h < 0.0) && h != skip {
                if h > MAX_UP_STEP {
                    break;
                }
                if !heights.iter().any(|c| c.to_bits() == h.to_bits()) {
                    heights.push(h);
                }
            }
        }
    }
    heights.sort_by(f32::total_cmp);
    let hor = |m: [f64; 3]| m[0] * m[0] + m[2] * m[2];
    for h in heights {
        let up = collide_with(world, &grounded, &cols, [v[0], h as f64, v[2]])?;
        if hor(up) > hor(step) {
            let to_ground = bb.min[1] - grounded.min[1];
            return Ok([up[0] - 0.0, up[1] - to_ground, up[2] - 0.0]);
        }
    }
    Ok(step)
}

/// `LivingEntity.maxUpStep()`: the `STEP_HEIGHT` default, as a float.
const MAX_UP_STEP: f32 = 0.6;

fn move_box(b: &Aabb, d: [f64; 3]) -> Aabb {
    Aabb {
        min: std::array::from_fn(|i| b.min[i] + d[i]),
        max: std::array::from_fn(|i| b.max[i] + d[i]),
    }
}

/// The blocks around `area` that a move can hit, in cursor order.
fn colliders(world: &World, area: &Aabb) -> Result<Colliders, Event> {
    let mut cols = Colliders {
        boxes: Vec::new(),
        blocks: Vec::new(),
    };
    let mut buf = Vec::new();
    for_blocks(world, area, &mut buf, |b, boxes| {
        cols.boxes.extend_from_slice(boxes);
        cols.blocks.push(Block {
            end: cols.boxes.len(),
            id: b.id,
            pos: b.pos,
            offset: b.offset,
        });
        Ok(())
    })?;
    Ok(cols)
}

/// `Entity.collideWithShapes`: the L-shaped move against a fixed collider list.
fn collide_with(
    world: &World,
    bb: &Aabb,
    cols: &Colliders,
    v: [f64; 3],
) -> Result<[f64; 3], Event> {
    if cols.blocks.is_empty() {
        return Ok(v);
    }
    let order = if v[0].abs() < v[2].abs() {
        [1, 2, 0]
    } else {
        [1, 0, 2]
    };
    let mut r = [0.0; 3];
    for a in order {
        if v[a] != 0.0 {
            r[a] = collide_axis(world, a, &move_box(bb, r), cols, v[a])?;
        }
    }
    Ok(r)
}

/// `CollisionGetter.noCollision` for blocks: no block's shape meets `bx`.
pub(super) fn no_collision(world: &World, bx: &Aabb) -> Result<bool, Event> {
    let mut any = false;
    let mut buf = Vec::new();
    for_blocks(world, bx, &mut buf, |_, _| {
        any = true;
        Ok(())
    })?;
    Ok(!any)
}

/// [`check_suffocation`] on its own, for a tick whose move may not meet the block:
/// the colliders around the four corner columns of `bb`.
pub(super) fn check_inside(world: &World, pos: [f64; 3], bb: &Aabb) -> Result<(), Event> {
    let w = 0.6f32 as f64 * 0.35;
    let area = Aabb {
        min: [
            mth::floor(pos[0] - w) as f64,
            bb.min[1],
            mth::floor(pos[2] - w) as f64,
        ],
        max: [
            mth::floor(pos[0] + w) as f64 + 1.0,
            bb.max[1],
            mth::floor(pos[2] + w) as f64 + 1.0,
        ],
    };
    let cols = colliders(world, &area)?;
    check_suffocation(world, pos, bb, &cols)
}

/// `LocalPlayer.moveTowardsClosestSpace`'s test, at the four columns under the box's
/// corners (inset to 0.35 of the width): a suffocating block overlapping the column
/// within the box's height. The game then pushes the player sideways.
fn check_suffocation(
    world: &World,
    pos: [f64; 3],
    bb: &Aabb,
    cols: &Colliders,
) -> Result<(), Event> {
    let w = 0.6f32 as f64;
    let corners = [
        (pos[0] - w * 0.35, pos[2] + w * 0.35),
        (pos[0] - w * 0.35, pos[2] - w * 0.35),
        (pos[0] + w * 0.35, pos[2] - w * 0.35),
        (pos[0] + w * 0.35, pos[2] + w * 0.35),
    ];
    for (b, boxes) in cols.iter() {
        if !world.detail(b.id).suffocating {
            continue;
        }
        let unit = is_unit(world, b.id, b.offset);
        for &(x, z) in &corners {
            let (cx, cz) = (mth::floor(x) as f64, mth::floor(z) as f64);
            let column = deflate(
                &Aabb {
                    min: [cx, bb.min[1], cz],
                    max: [cx + 1.0, bb.max[1], cz + 1.0],
                },
                EPS,
            );
            if boxes.iter().any(|bx| {
                if unit {
                    intersects(bx, &column)
                } else {
                    joins(bx, &column)
                }
            }) {
                return Err(Event::InsideBlock(b.pos));
            }
        }
    }
    Ok(())
}

/// A box is beside `mv` on axis `i` when their ranges overlap by more than the 1e-7
/// margins (`VoxelShape.findIndex` on `min + 1e-7` and `max - 1e-7`).
fn beside(bx: &Aabb, mv: &Aabb, i: usize) -> bool {
    bx.min[i] <= mv.max[i] - EPS && bx.max[i] > mv.min[i] + EPS
}

/// `Shapes.collide(axis, moving, shapes, distance)` over the blocks, each block as
/// `VoxelShape.collide` would scan its cells.
///
/// A box the moving face is already inside (along `a`, and beside on the other two
/// axes) is where boxes and the game's cells part: the game clips at the next grid
/// line of the shape ahead of the face, and boxes have no inner lines. If the block
/// has a line there, closer than the move's end, that is [`Event::InsideBlock`].
/// Without one the two agree, and the tick goes on.
fn collide_axis(
    world: &World,
    a: usize,
    mv: &Aabb,
    cols: &Colliders,
    mut d: f64,
) -> Result<f64, Event> {
    let (b, c) = ((a + 1) % 3, (a + 2) % 3);
    let beside = |bx: &Aabb, i: usize| beside(bx, mv, i);
    for (block, boxes) in cols.iter() {
        if d.abs() < EPS {
            return Ok(0.0);
        }
        if d > 0.0 {
            let ahead = mv.max[a] - EPS;
            let mut near = f64::INFINITY;
            for bx in boxes {
                if !(beside(bx, b) && beside(bx, c)) {
                    continue;
                }
                if bx.min[a] > ahead {
                    near = near.min(bx.min[a]);
                } else if bx.max[a] > ahead
                    && grid_lines(world, block, a)
                        .any(|g| g > ahead && g < bx.max[a] && g - mv.max[a] < d)
                {
                    return Err(Event::InsideBlock(block.pos));
                }
            }
            if near < f64::INFINITY {
                let nd = near - mv.max[a];
                if nd >= -EPS {
                    d = mth::java_min(d, nd);
                }
            }
        } else {
            let ahead = mv.min[a] + EPS;
            let mut near = f64::NEG_INFINITY;
            for bx in boxes {
                if !(beside(bx, b) && beside(bx, c)) {
                    continue;
                }
                if bx.max[a] <= ahead {
                    near = near.max(bx.max[a]);
                } else if bx.min[a] < ahead
                    && grid_lines(world, block, a)
                        .any(|g| g <= ahead && g > bx.min[a] && g - mv.min[a] > d)
                {
                    return Err(Event::InsideBlock(block.pos));
                }
            }
            if near > f64::NEG_INFINITY {
                let nd = near - mv.min[a];
                if nd <= EPS {
                    d = mth::java_max(d, nd);
                }
            }
        }
    }
    Ok(d)
}

/// Every grid line the game's shape for `block` can have on axis `a`, in world
/// coordinates (a superset): its boxes' faces, and for a box on a 1/2, 1/4 or 1/8
/// grid every line of that grid, which `Shapes.create` makes a `CubeVoxelShape` and
/// joining keeps.
fn grid_lines<'a>(world: &'a World, block: &'a Block, a: usize) -> impl Iterator<Item = f64> + 'a {
    let p = [block.pos.x as f64, block.pos.y as f64, block.pos.z as f64][a];
    let o = block.offset[a];
    world.info(block.id).boxes.iter().flat_map(move |bx| {
        let bits: [i32; 3] = std::array::from_fn(|i| find_bits(bx.min[i], bx.max[i]));
        let n = if bits.iter().all(|&k| k >= 0) {
            1 << bits[a]
        } else {
            0
        };
        [bx.min[a], bx.max[a]]
            .into_iter()
            .chain(
                (0..=n)
                    .filter(move |_| n > 0)
                    .map(move |i| i as f64 / n as f64),
            )
            .map(move |l| (l + o) + p)
    })
}

/// `Shapes.findBits`: the grid (0 to 3 bits) a box's bounds sit on, or -1.
fn find_bits(min: f64, max: f64) -> i32 {
    if !(min < -EPS) && !(max > 1.0000001) {
        for bits in 0..=3 {
            let n = (1 << bits) as f64;
            let on = |v: f64| (v * n - (v * n + 0.5).floor()).abs() < EPS * n;
            if on(min) && on(max) {
                return bits;
            }
        }
    }
    -1
}

/// `Entity.getOnPosLegacy()` right after a landing: the supporting block
/// (`Entity.checkSupportingBlock`, which the landing move sets) at the height 0.2
/// below the feet, or the block under the position if there is none.
pub(super) fn landing_on_pos(
    world: &World,
    pos: [f64; 3],
    bb: &Aabb,
    movement: [f64; 3],
) -> Result<BlockPos, Event> {
    // `onGroundNoBlocks` was clear: it is in flight
    let (support, _) = check_supporting_block(world, pos, bb, movement, false)?;
    on_pos(world, pos, support, 0.2)
}

/// `Entity.checkSupportingBlock(true, movement)` after a move that ends on the
/// ground: the supporting block and the new `onGroundNoBlocks`. The test slab under
/// the box, and if it finds nothing and `no_blocks` (the last value) is clear, the
/// slab where the box came from.
pub(super) fn check_supporting_block(
    world: &World,
    pos: [f64; 3],
    bb: &Aabb,
    movement: [f64; 3],
    no_blocks: bool,
) -> Result<(Option<BlockPos>, bool), Event> {
    let test = Aabb {
        min: [bb.min[0], bb.min[1] - 1.0E-6, bb.min[2]],
        max: [bb.max[0], bb.min[1], bb.max[2]],
    };
    let mut support = supporting_block(world, pos, &test)?;
    if support.is_none() && !no_blocks {
        let back = Aabb {
            min: [
                test.min[0] - movement[0],
                test.min[1],
                test.min[2] - movement[2],
            ],
            max: [
                test.max[0] - movement[0],
                test.max[1],
                test.max[2] - movement[2],
            ],
        };
        support = supporting_block(world, pos, &back)?;
    }
    Ok((support, support.is_none()))
}

/// `Entity.getOnPos(offset)`: the supporting block at the height `offset` below the
/// feet (fences keep their own position for offsets up to 0.5, walls and gates for
/// any), or the block there under the position without one.
pub(super) fn on_pos(
    world: &World,
    pos: [f64; 3],
    support: Option<BlockPos>,
    offset: f32,
) -> Result<BlockPos, Event> {
    let y = mth::floor(pos[1] - offset as f64);
    Ok(match support {
        Some(p) => {
            let class = world.detail(state_at(world, p)?).class.as_str();
            let keep = class == "WallBlock"
                || class == "FenceGateBlock"
                || offset as f64 <= 0.5 && class == "FenceBlock";
            if keep { p } else { BlockPos { y, ..p } }
        }
        None => BlockPos {
            x: mth::floor(pos[0]),
            y,
            z: mth::floor(pos[2]),
        },
    })
}

/// `CollisionGetter.findSupportingBlock`: the colliding block whose center is nearest
/// `pos`, ties to the larger position (`Vec3i.compareTo`: y, then z, then x).
fn supporting_block(world: &World, pos: [f64; 3], area: &Aabb) -> Result<Option<BlockPos>, Event> {
    let mut best: Option<(BlockPos, f64)> = None;
    let mut buf = Vec::new();
    for_blocks(world, area, &mut buf, |v, _| {
        let p = v.pos;
        let d = [
            p.x as f64 + 0.5 - pos[0],
            p.y as f64 + 0.5 - pos[1],
            p.z as f64 + 0.5 - pos[2],
        ];
        let dist = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        let take = match best {
            None => true,
            Some((b, bd)) => dist < bd || dist == bd && (b.y, b.z, b.x) < (p.y, p.z, p.x),
        };
        if take {
            best = Some((p, dist));
        }
        Ok(())
    })?;
    Ok(best.map(|(p, _)| p))
}
