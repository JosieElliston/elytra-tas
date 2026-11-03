//! `net.minecraft.world.phys.shapes`: `VoxelShape` and the parts of `Shapes` that
//! building block shapes and colliding against them use.
//!
//! A shape is a grid of coordinates per axis and a bitset of full cells
//! (`DiscreteVoxelShape`). The grid matters beyond the filled region: `collide` scans
//! cells from the one holding the moving box's face, and step-up takes every grid
//! coordinate of a nearby shape as a candidate height.

use crate::mth;
use crate::phys::{AABB, Axis};

/// `BitSetDiscreteVoxelShape`. Index `(x * ySize + y) * zSize + z`.
#[derive(Debug, Clone, PartialEq)]
pub struct BitSetDiscreteVoxelShape {
    size: [i32; 3],
    storage: Vec<bool>,
}

impl BitSetDiscreteVoxelShape {
    /// `new BitSetDiscreteVoxelShape(xSize, ySize, zSize)`: all empty.
    pub fn new(x_size: i32, y_size: i32, z_size: i32) -> Self {
        BitSetDiscreteVoxelShape {
            size: [x_size, y_size, z_size],
            storage: vec![false; (x_size * y_size * z_size) as usize],
        }
    }

    /// `BitSetDiscreteVoxelShape.withFilledBounds`.
    #[allow(clippy::too_many_arguments)]
    pub fn with_filled_bounds(
        x_size: i32,
        y_size: i32,
        z_size: i32,
        x_min: i32,
        y_min: i32,
        z_min: i32,
        x_max: i32,
        y_max: i32,
        z_max: i32,
    ) -> Self {
        let mut s = Self::new(x_size, y_size, z_size);
        for x in x_min..x_max {
            for y in y_min..y_max {
                for z in z_min..z_max {
                    let i = s.get_index(x, y, z);
                    s.storage[i] = true;
                }
            }
        }
        s
    }

    fn get_index(&self, x: i32, y: i32, z: i32) -> usize {
        ((x * self.size[1] + y) * self.size[2] + z) as usize
    }

    pub fn get_size(&self, axis: Axis) -> i32 {
        self.size[axis.ordinal()]
    }

    /// `DiscreteVoxelShape.isFullWide(int, int, int)`: false outside the grid.
    pub fn is_full_wide(&self, x: i32, y: i32, z: i32) -> bool {
        if x < 0 || y < 0 || z < 0 {
            return false;
        }
        x < self.size[0]
            && y < self.size[1]
            && z < self.size[2]
            && self.storage[self.get_index(x, y, z)]
    }

    pub fn is_empty(&self) -> bool {
        !self.storage.iter().any(|&b| b)
    }

    /// `firstFull(axis)`: the stored lower bound, which `join` and `withFilledBounds`
    /// keep equal to the first full index (or the size when empty).
    pub fn first_full(&self, axis: Axis) -> i32 {
        let mut m = self.get_size(axis);
        for x in 0..self.size[0] {
            for y in 0..self.size[1] {
                for z in 0..self.size[2] {
                    if self.storage[self.get_index(x, y, z)] {
                        m = m.min(axis.choose(x, y, z));
                    }
                }
            }
        }
        m
    }

    /// `lastFull(axis)`: one past the last full index (0 when empty).
    pub fn last_full(&self, axis: Axis) -> i32 {
        let mut m = 0;
        for x in 0..self.size[0] {
            for y in 0..self.size[1] {
                for z in 0..self.size[2] {
                    if self.storage[self.get_index(x, y, z)] {
                        m = m.max(axis.choose(x, y, z) + 1);
                    }
                }
            }
        }
        m
    }

    /// `BitSetDiscreteVoxelShape.join` for one boolean op.
    fn join(
        first: &Self,
        second: &Self,
        x_merger: &IndexMerger,
        y_merger: &IndexMerger,
        z_merger: &IndexMerger,
        op: BooleanOp,
    ) -> Self {
        let mut shape = Self::new(
            x_merger.size() - 1,
            y_merger.size() - 1,
            z_merger.size() - 1,
        );
        x_merger.for_merged_indexes(&mut |x1, x2, xr| {
            y_merger.for_merged_indexes(&mut |y1, y2, yr| {
                z_merger.for_merged_indexes(&mut |z1, z2, zr| {
                    if op.apply(
                        first.is_full_wide(x1, y1, z1),
                        second.is_full_wide(x2, y2, z2),
                    ) {
                        let i = shape.get_index(xr, yr, zr);
                        shape.storage[i] = true;
                    }
                    true
                });
                true
            });
            true
        });
        shape
    }
}

/// `BooleanOp`, the two `Shapes` uses here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanOp {
    Or,
    And,
}

impl BooleanOp {
    fn apply(self, a: bool, b: bool) -> bool {
        match self {
            BooleanOp::Or => a || b,
            BooleanOp::And => a && b,
        }
    }
}

/// A shape's coordinate list on one axis (`DoubleList`). `cube_parts` is set when the
/// list is a `CubePointRange` (`i / parts`), which `createIndexMerger` treats specially.
#[derive(Debug, Clone, PartialEq)]
pub struct Coords {
    pub values: Vec<f64>,
    pub cube_parts: Option<i32>,
}

impl Coords {
    /// `new CubePointRange(parts)`.
    fn cube_point_range(parts: i32) -> Coords {
        Coords {
            values: (0..=parts).map(|i| i as f64 / parts as f64).collect(),
            cube_parts: Some(parts),
        }
    }

    fn array(values: Vec<f64>) -> Coords {
        Coords {
            values,
            cube_parts: None,
        }
    }

    fn size(&self) -> i32 {
        self.values.len() as i32
    }

    fn get_double(&self, i: i32) -> f64 {
        self.values[i as usize]
    }
}

/// `IndexMerger` and its four implementations.
enum IndexMerger {
    Identical(Coords),
    NonOverlapping {
        lower: Coords,
        upper: Coords,
        swap: bool,
    },
    Indirect {
        result: Vec<f64>,
        first_indices: Vec<i32>,
        second_indices: Vec<i32>,
        result_length: i32,
    },
    DiscreteCube {
        result: Coords,
        first_div: i32,
        second_div: i32,
    },
}

fn gcd(a: i32, b: i32) -> i32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// `Shapes.lcm`.
fn lcm(first: i32, second: i32) -> i64 {
    first as i64 * (second / gcd(first, second)) as i64
}

impl IndexMerger {
    /// `Shapes.createIndexMerger`.
    fn create(
        cost: i32,
        first: &Coords,
        second: &Coords,
        first_only_matters: bool,
        second_only_matters: bool,
    ) -> Self {
        let first_size = first.size() - 1;
        let second_size = second.size() - 1;
        if first.cube_parts.is_some() && second.cube_parts.is_some() {
            let size = lcm(first_size, second_size);
            if cost as i64 * size <= 256 {
                // `new DiscreteCubeMerger(firstSize, secondSize)`
                let g = gcd(first_size, second_size);
                return IndexMerger::DiscreteCube {
                    result: Coords::cube_point_range(size as i32),
                    first_div: first_size / g,
                    second_div: second_size / g,
                };
            }
        }
        if first.get_double(first_size) < second.get_double(0) - 1.0E-7 {
            IndexMerger::NonOverlapping {
                lower: first.clone(),
                upper: second.clone(),
                swap: false,
            }
        } else if second.get_double(second_size) < first.get_double(0) - 1.0E-7 {
            IndexMerger::NonOverlapping {
                lower: second.clone(),
                upper: first.clone(),
                swap: true,
            }
        } else if first_size == second_size && first.values == second.values {
            IndexMerger::Identical(first.clone())
        } else {
            Self::indirect(first, second, first_only_matters, second_only_matters)
        }
    }

    /// `new IndirectMerger(first, second, firstOnlyMatters, secondOnlyMatters)`.
    fn indirect(
        first: &Coords,
        second: &Coords,
        first_only_matters: bool,
        second_only_matters: bool,
    ) -> Self {
        let mut last_value = f64::NAN;
        let first_size = first.size();
        let second_size = second.size();
        let capacity = (first_size + second_size) as usize;
        let mut result = vec![0.0; capacity];
        let mut first_indices = vec![0; capacity];
        let mut second_indices = vec![0; capacity];
        let can_skip_first = !first_only_matters;
        let can_skip_second = !second_only_matters;
        let mut result_index = 0usize;
        let mut first_index = 0;
        let mut second_index = 0;
        loop {
            let ran_out_of_first = first_index >= first_size;
            let ran_out_of_second = second_index >= second_size;
            if ran_out_of_first && ran_out_of_second {
                let result_length = (result_index as i32).max(1);
                return IndexMerger::Indirect {
                    result,
                    first_indices,
                    second_indices,
                    result_length,
                };
            }
            let chose_first = !ran_out_of_first
                && (ran_out_of_second
                    || first.get_double(first_index) < second.get_double(second_index) + 1.0E-7);
            if chose_first {
                first_index += 1;
                if can_skip_first && (second_index == 0 || ran_out_of_second) {
                    continue;
                }
            } else {
                second_index += 1;
                if can_skip_second && (first_index == 0 || ran_out_of_first) {
                    continue;
                }
            }
            let current_first_index = first_index - 1;
            let current_second_index = second_index - 1;
            let next_value = if chose_first {
                first.get_double(current_first_index)
            } else {
                second.get_double(current_second_index)
            };
            if !(last_value >= next_value - 1.0E-7) {
                first_indices[result_index] = current_first_index;
                second_indices[result_index] = current_second_index;
                result[result_index] = next_value;
                result_index += 1;
                last_value = next_value;
            } else {
                first_indices[result_index - 1] = current_first_index;
                second_indices[result_index - 1] = current_second_index;
            }
        }
    }

    fn size(&self) -> i32 {
        match self {
            IndexMerger::Identical(c) => c.size(),
            IndexMerger::NonOverlapping { lower, upper, .. } => lower.size() + upper.size(),
            IndexMerger::Indirect { result_length, .. } => *result_length,
            IndexMerger::DiscreteCube { result, .. } => result.size(),
        }
    }

    /// `IndexMerger.getList`.
    fn get_list(&self) -> Coords {
        match self {
            IndexMerger::Identical(c) => c.clone(),
            IndexMerger::NonOverlapping { lower, upper, .. } => {
                let mut v = lower.values.clone();
                v.extend_from_slice(&upper.values);
                Coords::array(v)
            }
            IndexMerger::Indirect {
                result,
                result_length,
                ..
            } => {
                if *result_length <= 1 {
                    Coords::array(vec![0.0])
                } else {
                    Coords::array(result[..*result_length as usize].to_vec())
                }
            }
            IndexMerger::DiscreteCube { result, .. } => result.clone(),
        }
    }

    /// `IndexMerger.forMergedIndexes`: calls `consumer(firstIndex, secondIndex,
    /// resultIndex)` per merged cell until it returns false.
    fn for_merged_indexes(&self, consumer: &mut dyn FnMut(i32, i32, i32) -> bool) -> bool {
        match self {
            IndexMerger::Identical(c) => {
                for i in 0..c.size() - 1 {
                    if !consumer(i, i, i) {
                        return false;
                    }
                }
                true
            }
            IndexMerger::NonOverlapping { lower, upper, swap } => {
                let mut call = |a: i32, b: i32, r: i32| {
                    if *swap {
                        consumer(b, a, r)
                    } else {
                        consumer(a, b, r)
                    }
                };
                let lower_size = lower.size();
                for i in 0..lower_size {
                    if !call(i, -1, i) {
                        return false;
                    }
                }
                let upper_size = upper.size() - 1;
                for i in 0..upper_size {
                    if !call(lower_size - 1, i, lower_size + i) {
                        return false;
                    }
                }
                true
            }
            IndexMerger::Indirect {
                first_indices,
                second_indices,
                result_length,
                ..
            } => {
                for i in 0..(*result_length - 1) as usize {
                    if !consumer(first_indices[i], second_indices[i], i as i32) {
                        return false;
                    }
                }
                true
            }
            IndexMerger::DiscreteCube {
                result,
                first_div,
                second_div,
            } => {
                for i in 0..result.size() - 1 {
                    if !consumer(i / second_div, i / first_div, i) {
                        return false;
                    }
                }
                true
            }
        }
    }
}

/// `VoxelShape` (`ArrayVoxelShape`, `CubeVoxelShape`): a cell bitset and the
/// coordinates of its cell boundaries on each axis.
#[derive(Debug, Clone, PartialEq)]
pub struct VoxelShape {
    pub shape: BitSetDiscreteVoxelShape,
    pub coords: [Coords; 3],
    /// This is `Shapes.block()` itself, which `BlockCollisions` tests by identity.
    pub is_block: bool,
}

impl VoxelShape {
    /// `Shapes.empty()`.
    pub fn empty() -> VoxelShape {
        VoxelShape {
            shape: BitSetDiscreteVoxelShape::new(0, 0, 0),
            coords: [
                Coords::array(vec![0.0]),
                Coords::array(vec![0.0]),
                Coords::array(vec![0.0]),
            ],
            is_block: false,
        }
    }

    /// `Shapes.block()`.
    pub fn block() -> VoxelShape {
        VoxelShape {
            shape: BitSetDiscreteVoxelShape::with_filled_bounds(1, 1, 1, 0, 0, 0, 1, 1, 1),
            coords: [
                Coords::cube_point_range(1),
                Coords::cube_point_range(1),
                Coords::cube_point_range(1),
            ],
            is_block: true,
        }
    }

    /// `new CubeVoxelShape(shape)`: coordinates `i / size` on each axis.
    fn cube(shape: BitSetDiscreteVoxelShape) -> VoxelShape {
        let coords = [
            Coords::cube_point_range(shape.size[0]),
            Coords::cube_point_range(shape.size[1]),
            Coords::cube_point_range(shape.size[2]),
        ];
        VoxelShape {
            shape,
            coords,
            is_block: false,
        }
    }

    /// `Shapes.create(minX, minY, minZ, maxX, maxY, maxZ)`.
    pub fn create(
        min_x: f64,
        min_y: f64,
        min_z: f64,
        max_x: f64,
        max_y: f64,
        max_z: f64,
    ) -> VoxelShape {
        if !(max_x - min_x < 1.0E-7) && !(max_y - min_y < 1.0E-7) && !(max_z - min_z < 1.0E-7) {
            let x_bits = find_bits(min_x, max_x);
            let y_bits = find_bits(min_y, max_y);
            let z_bits = find_bits(min_z, max_z);
            if x_bits < 0 || y_bits < 0 || z_bits < 0 {
                return VoxelShape {
                    shape: BitSetDiscreteVoxelShape::with_filled_bounds(1, 1, 1, 0, 0, 0, 1, 1, 1),
                    coords: [
                        Coords::array(vec![min_x, max_x]),
                        Coords::array(vec![min_y, max_y]),
                        Coords::array(vec![min_z, max_z]),
                    ],
                    is_block: false,
                };
            }
            if x_bits == 0 && y_bits == 0 && z_bits == 0 {
                return VoxelShape::block();
            }
            let x_size = 1 << x_bits;
            let y_size = 1 << y_bits;
            let z_size = 1 << z_bits;
            let voxel_shape = BitSetDiscreteVoxelShape::with_filled_bounds(
                x_size,
                y_size,
                z_size,
                mth::round(min_x * x_size as f64) as i32,
                mth::round(min_y * y_size as f64) as i32,
                mth::round(min_z * z_size as f64) as i32,
                mth::round(max_x * x_size as f64) as i32,
                mth::round(max_y * y_size as f64) as i32,
                mth::round(max_z * z_size as f64) as i32,
            );
            VoxelShape::cube(voxel_shape)
        } else {
            VoxelShape::empty()
        }
    }

    /// `Shapes.create(AABB)`.
    pub fn create_aabb(b: &AABB) -> VoxelShape {
        VoxelShape::create(b.min_x, b.min_y, b.min_z, b.max_x, b.max_y, b.max_z)
    }

    /// A block's collision shape from its block-local boxes, as `VoxelShape.optimize`
    /// rebuilds one: `joinUnoptimized(acc, Shapes.box(box), OR)` over the boxes in
    /// order, starting from empty. The block table's boxes are the game shape's
    /// `toAabbs()`, so this is that shape after `optimize()`; for the shapes vanilla
    /// builds with `Shapes.or` (which optimizes) that is the game's own shape.
    pub fn from_boxes(boxes: &[world::Aabb]) -> VoxelShape {
        let mut acc = VoxelShape::empty();
        for b in boxes {
            let s = VoxelShape::create(b.min[0], b.min[1], b.min[2], b.max[0], b.max[1], b.max[2]);
            acc = join_unoptimized(&acc, &s, BooleanOp::Or);
        }
        acc
    }

    pub fn is_empty(&self) -> bool {
        self.shape.is_empty()
    }

    pub fn get_coords(&self, axis: Axis) -> &[f64] {
        &self.coords[axis.ordinal()].values
    }

    /// `VoxelShape.get(axis, i)`.
    pub fn get(&self, axis: Axis, i: i32) -> f64 {
        self.coords[axis.ordinal()].get_double(i)
    }

    /// `VoxelShape.min(axis)`.
    pub fn min(&self, axis: Axis) -> f64 {
        let i = self.shape.first_full(axis);
        if i >= self.shape.get_size(axis) {
            f64::INFINITY
        } else {
            self.get(axis, i)
        }
    }

    /// `VoxelShape.max(axis)`.
    pub fn max(&self, axis: Axis) -> f64 {
        let i = self.shape.last_full(axis);
        if i <= 0 {
            f64::NEG_INFINITY
        } else {
            self.get(axis, i)
        }
    }

    /// `VoxelShape.move(double, double, double)`: an `ArrayVoxelShape` over
    /// `OffsetDoubleList`s (`coord + offset`).
    pub fn move_(&self, dx: f64, dy: f64, dz: f64) -> VoxelShape {
        if self.is_empty() {
            return VoxelShape::empty();
        }
        let off = |c: &Coords, d: f64| Coords::array(c.values.iter().map(|v| v + d).collect());
        VoxelShape {
            shape: self.shape.clone(),
            coords: [
                off(&self.coords[0], dx),
                off(&self.coords[1], dy),
                off(&self.coords[2], dz),
            ],
            is_block: false,
        }
    }

    /// `VoxelShape.findIndex`: the largest `i` with `get(axis, i) <= coord`, from -1
    /// to the size.
    pub fn find_index(&self, axis: Axis, coord: f64) -> i32 {
        mth::binary_search(0, self.shape.get_size(axis) + 1, |index| {
            coord < self.get(axis, index)
        }) - 1
    }

    /// `VoxelShape.collide(axis, moving, distance)` (`collideX` through `AxisCycle`).
    pub fn collide(&self, axis: Axis, moving: &AABB, mut distance: f64) -> f64 {
        if self.is_empty() {
            return distance;
        }
        if distance.abs() < 1.0E-7 {
            return 0.0;
        }
        // `inverse.cycle(X/Y/Z)` for `AxisCycle.between(axis, X)`: a is the movement
        // axis, then b and c in cyclic order.
        let (a_axis, b_axis, c_axis) = match axis {
            Axis::X => (Axis::X, Axis::Y, Axis::Z),
            Axis::Y => (Axis::Y, Axis::Z, Axis::X),
            Axis::Z => (Axis::Z, Axis::X, Axis::Y),
        };
        let full = |a: i32, b: i32, c: i32| {
            let mut xyz = [0; 3];
            xyz[a_axis.ordinal()] = a;
            xyz[b_axis.ordinal()] = b;
            xyz[c_axis.ordinal()] = c;
            self.shape.is_full_wide(xyz[0], xyz[1], xyz[2])
        };
        let max_a = moving.max(a_axis);
        let min_a = moving.min(a_axis);
        let a_min = self.find_index(a_axis, min_a + 1.0E-7);
        let a_max = self.find_index(a_axis, max_a - 1.0E-7);
        let b_min = 0.max(self.find_index(b_axis, moving.min(b_axis) + 1.0E-7));
        let b_max = self
            .shape
            .get_size(b_axis)
            .min(self.find_index(b_axis, moving.max(b_axis) - 1.0E-7) + 1);
        let c_min = 0.max(self.find_index(c_axis, moving.min(c_axis) + 1.0E-7));
        let c_max = self
            .shape
            .get_size(c_axis)
            .min(self.find_index(c_axis, moving.max(c_axis) - 1.0E-7) + 1);
        let a_size = self.shape.get_size(a_axis);
        if distance > 0.0 {
            for a in a_max + 1..a_size {
                for b in b_min..b_max {
                    for c in c_min..c_max {
                        if full(a, b, c) {
                            let new_distance = self.get(a_axis, a) - max_a;
                            if new_distance >= -1.0E-7 {
                                distance = mth::java_min(distance, new_distance);
                            }
                            return distance;
                        }
                    }
                }
            }
        } else if distance < 0.0 {
            let mut a = a_min - 1;
            while a >= 0 {
                for b in b_min..b_max {
                    for c in c_min..c_max {
                        if full(a, b, c) {
                            let new_distance = self.get(a_axis, a + 1) - min_a;
                            if new_distance <= 1.0E-7 {
                                distance = mth::java_max(distance, new_distance);
                            }
                            return distance;
                        }
                    }
                }
                a -= 1;
            }
        }
        distance
    }
}

/// `Shapes.findBits`: the grid resolution (0..=3 bits) a box's bounds sit on, or -1.
pub fn find_bits(min: f64, max: f64) -> i32 {
    if !(min < -1.0E-7) && !(max > 1.0000001) {
        for bits in 0..=3 {
            let intervals = 1 << bits;
            let sh_min = min * intervals as f64;
            let sh_max = max * intervals as f64;
            let found_min = (sh_min - mth::round(sh_min) as f64).abs() < 1.0E-7 * intervals as f64;
            let found_max = (sh_max - mth::round(sh_max) as f64).abs() < 1.0E-7 * intervals as f64;
            if found_min && found_max {
                return bits;
            }
        }
        -1
    } else {
        -1
    }
}

/// `Shapes.joinUnoptimized(first, second, op)`, without the `first == second`
/// identity shortcut (block shapes don't repeat a box).
pub fn join_unoptimized(first: &VoxelShape, second: &VoxelShape, op: BooleanOp) -> VoxelShape {
    let first_only_matters = op.apply(true, false);
    let second_only_matters = op.apply(false, true);
    if first.is_empty() {
        return if second_only_matters {
            second.clone()
        } else {
            VoxelShape::empty()
        };
    }
    if second.is_empty() {
        return if first_only_matters {
            first.clone()
        } else {
            VoxelShape::empty()
        };
    }
    let x_merger = IndexMerger::create(
        1,
        &first.coords[0],
        &second.coords[0],
        first_only_matters,
        second_only_matters,
    );
    let y_merger = IndexMerger::create(
        x_merger.size() - 1,
        &first.coords[1],
        &second.coords[1],
        first_only_matters,
        second_only_matters,
    );
    let z_merger = IndexMerger::create(
        (x_merger.size() - 1) * (y_merger.size() - 1),
        &first.coords[2],
        &second.coords[2],
        first_only_matters,
        second_only_matters,
    );
    let voxel_shape = BitSetDiscreteVoxelShape::join(
        &first.shape,
        &second.shape,
        &x_merger,
        &y_merger,
        &z_merger,
        op,
    );
    let all_cube = [&x_merger, &y_merger, &z_merger]
        .iter()
        .all(|m| matches!(m, IndexMerger::DiscreteCube { .. }));
    if all_cube {
        VoxelShape::cube(voxel_shape)
    } else {
        VoxelShape {
            shape: voxel_shape,
            coords: [
                x_merger.get_list(),
                y_merger.get_list(),
                z_merger.get_list(),
            ],
            is_block: false,
        }
    }
}

/// `Shapes.joinIsNotEmpty(first, second, op)`.
pub fn join_is_not_empty(first: &VoxelShape, second: &VoxelShape, op: BooleanOp) -> bool {
    let first_empty = first.is_empty();
    let second_empty = second.is_empty();
    if !first_empty && !second_empty {
        let first_only_matters = op.apply(true, false);
        let second_only_matters = op.apply(false, true);
        for axis in Axis::VALUES {
            if first.max(axis) < second.min(axis) - 1.0E-7 {
                return first_only_matters || second_only_matters;
            }
            if second.max(axis) < first.min(axis) - 1.0E-7 {
                return first_only_matters || second_only_matters;
            }
        }
        let x_merger = IndexMerger::create(
            1,
            &first.coords[0],
            &second.coords[0],
            first_only_matters,
            second_only_matters,
        );
        let y_merger = IndexMerger::create(
            x_merger.size() - 1,
            &first.coords[1],
            &second.coords[1],
            first_only_matters,
            second_only_matters,
        );
        let z_merger = IndexMerger::create(
            (x_merger.size() - 1) * (y_merger.size() - 1),
            &first.coords[2],
            &second.coords[2],
            first_only_matters,
            second_only_matters,
        );
        // `!xMerger.forMergedIndexes(... !op.apply(first.isFullWide, second.isFullWide))`
        !x_merger.for_merged_indexes(&mut |x1, x2, _| {
            y_merger.for_merged_indexes(&mut |y1, y2, _| {
                z_merger.for_merged_indexes(&mut |z1, z2, _| {
                    !op.apply(
                        first.shape.is_full_wide(x1, y1, z1),
                        second.shape.is_full_wide(x2, y2, z2),
                    )
                })
            })
        })
    } else {
        op.apply(!first_empty, !second_empty)
    }
}

/// `Shapes.collide(axis, moving, shapes, distance)`.
pub fn collide(axis: Axis, moving: &AABB, shapes: &[VoxelShape], mut distance: f64) -> f64 {
    for shape in shapes {
        if distance.abs() < 1.0E-7 {
            return 0.0;
        }
        distance = shape.collide(axis, moving, distance);
    }
    distance
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(min: [f64; 3], max: [f64; 3]) -> world::Aabb {
        world::Aabb { min, max }
    }

    #[test]
    fn slab_is_a_cube_grid() {
        let s = VoxelShape::from_boxes(&[bx([0.0; 3], [1.0, 0.5, 1.0])]);
        assert_eq!(s.get_coords(Axis::Y), &[0.0, 0.5, 1.0]);
        assert_eq!(s.get_coords(Axis::X), &[0.0, 1.0]);
        assert_eq!(s.max(Axis::Y), 0.5);
    }

    #[test]
    fn unit_box_is_block() {
        assert!(VoxelShape::from_boxes(&[bx([0.0; 3], [1.0; 3])]).is_block);
    }

    #[test]
    fn stairs_join() {
        // a straight stair: bottom slab and a back half on top
        let s = VoxelShape::from_boxes(&[
            bx([0.0; 3], [1.0, 0.5, 1.0]),
            bx([0.0, 0.5, 0.5], [1.0, 1.0, 1.0]),
        ]);
        assert_eq!(s.get_coords(Axis::Z), &[0.0, 0.5, 1.0]);
        assert!(s.shape.is_full_wide(0, 1, 1));
        assert!(!s.shape.is_full_wide(0, 1, 0));
        assert!(s.shape.is_full_wide(0, 0, 0));
    }

    #[test]
    fn collide_stops_at_face_and_ignores_overlap() {
        let s = VoxelShape::block().move_(0.0, 0.0, 2.0);
        let moving = AABB::new(0.2, 0.0, 0.5, 0.8, 1.0, 1.1);
        assert_eq!(s.collide(Axis::Z, &moving, 3.0), 2.0 - 1.1);
        // already inside along z: the full block has no cell boundary past the face
        let inside = AABB::new(0.2, 0.0, 2.1, 0.8, 1.0, 2.7);
        assert_eq!(s.collide(Axis::Z, &inside, 3.0), 3.0);
    }

    #[test]
    fn non_grid_box_is_array_shape() {
        let s = VoxelShape::from_boxes(&[bx([0.3, 0.0, 0.3], [0.7, 1.0, 0.7])]);
        assert_eq!(s.get_coords(Axis::X), &[0.3, 0.7]);
        assert!(s.coords[0].cube_parts.is_none());
    }
}
