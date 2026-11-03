//! `net.minecraft.world.phys` value types (`Vec3`, `Vec2`, `AABB`) and
//! `Direction.Axis`, with the methods movement calls.

use crate::mth;

/// `net.minecraft.core.Direction.Axis`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub const VALUES: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    pub fn ordinal(self) -> usize {
        self as usize
    }

    /// `Direction.Axis.choose`.
    pub fn choose<T>(self, x: T, y: T, z: T) -> T {
        match self {
            Axis::X => x,
            Axis::Y => y,
            Axis::Z => z,
        }
    }
}

/// `Direction.axisStepOrder(Vec3)`: y first, then the larger horizontal axis.
pub fn axis_step_order(movement: Vec3) -> [Axis; 3] {
    if movement.x.abs() < movement.z.abs() {
        [Axis::Y, Axis::Z, Axis::X]
    } else {
        [Axis::Y, Axis::X, Axis::Z]
    }
}

/// `net.minecraft.world.phys.Vec3`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    pub const fn new(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3 { x, y, z }
    }

    pub fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }

    pub fn add_xyz(self, x: f64, y: f64, z: f64) -> Vec3 {
        Vec3::new(self.x + x, self.y + y, self.z + z)
    }

    pub fn subtract_xyz(self, x: f64, y: f64, z: f64) -> Vec3 {
        self.add_xyz(-x, -y, -z)
    }

    pub fn scale(self, s: f64) -> Vec3 {
        self.multiply(s, s, s)
    }

    pub fn multiply(self, xs: f64, ys: f64, zs: f64) -> Vec3 {
        Vec3::new(self.x * xs, self.y * ys, self.z * zs)
    }

    /// `Vec3.normalize`: zero below a length of `1.0E-5F`.
    pub fn normalize(self) -> Vec3 {
        let dist = (self.x * self.x + self.y * self.y + self.z * self.z).sqrt();
        if dist < 1.0E-5f32 as f64 {
            Vec3::ZERO
        } else {
            Vec3::new(self.x / dist, self.y / dist, self.z / dist)
        }
    }

    pub fn length_sqr(self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    pub fn horizontal_distance(self) -> f64 {
        (self.x * self.x + self.z * self.z).sqrt()
    }

    pub fn horizontal_distance_sqr(self) -> f64 {
        self.x * self.x + self.z * self.z
    }

    pub fn get(self, axis: Axis) -> f64 {
        axis.choose(self.x, self.y, self.z)
    }

    pub fn with(self, axis: Axis, value: f64) -> Vec3 {
        Vec3::new(
            if axis == Axis::X { value } else { self.x },
            if axis == Axis::Y { value } else { self.y },
            if axis == Axis::Z { value } else { self.z },
        )
    }

    /// `Vec3.isFinite`, which `Entity.setDeltaMovement` checks.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

/// `net.minecraft.world.phys.Vec2` (float).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Vec2 {
        Vec2 { x, y }
    }

    pub fn scale(self, s: f32) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }

    /// `Vec2.normalized`: zero below a length of `1.0E-4F`.
    pub fn normalized(self) -> Vec2 {
        let dist = mth::sqrt(self.x * self.x + self.y * self.y);
        if dist < 1.0E-4 {
            Vec2::ZERO
        } else {
            Vec2::new(self.x / dist, self.y / dist)
        }
    }

    pub fn length(self) -> f32 {
        mth::sqrt(self.x * self.x + self.y * self.y)
    }

    pub fn length_squared(self) -> f32 {
        self.x * self.x + self.y * self.y
    }
}

/// `net.minecraft.world.phys.AABB`.
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AABB {
    pub min_x: f64,
    pub min_y: f64,
    pub min_z: f64,
    pub max_x: f64,
    pub max_y: f64,
    pub max_z: f64,
}

impl AABB {
    /// `new AABB(...)`, which sorts each axis' bounds.
    pub fn new(min_x: f64, min_y: f64, min_z: f64, max_x: f64, max_y: f64, max_z: f64) -> AABB {
        AABB {
            min_x: mth::java_min(min_x, max_x),
            min_y: mth::java_min(min_y, max_y),
            min_z: mth::java_min(min_z, max_z),
            max_x: mth::java_max(min_x, max_x),
            max_y: mth::java_max(min_y, max_y),
            max_z: mth::java_max(min_z, max_z),
        }
    }

    pub fn min(&self, axis: Axis) -> f64 {
        axis.choose(self.min_x, self.min_y, self.min_z)
    }

    pub fn max(&self, axis: Axis) -> f64 {
        axis.choose(self.max_x, self.max_y, self.max_z)
    }

    /// `AABB.move(double, double, double)`.
    pub fn move_(&self, xa: f64, ya: f64, za: f64) -> AABB {
        AABB::new(
            self.min_x + xa,
            self.min_y + ya,
            self.min_z + za,
            self.max_x + xa,
            self.max_y + ya,
            self.max_z + za,
        )
    }

    pub fn move_vec(&self, v: Vec3) -> AABB {
        self.move_(v.x, v.y, v.z)
    }

    /// `AABB.expandTowards(double, double, double)`.
    pub fn expand_towards(&self, xa: f64, ya: f64, za: f64) -> AABB {
        let (mut min_x, mut min_y, mut min_z) = (self.min_x, self.min_y, self.min_z);
        let (mut max_x, mut max_y, mut max_z) = (self.max_x, self.max_y, self.max_z);
        if xa < 0.0 {
            min_x += xa;
        } else if xa > 0.0 {
            max_x += xa;
        }
        if ya < 0.0 {
            min_y += ya;
        } else if ya > 0.0 {
            max_y += ya;
        }
        if za < 0.0 {
            min_z += za;
        } else if za > 0.0 {
            max_z += za;
        }
        AABB::new(min_x, min_y, min_z, max_x, max_y, max_z)
    }

    pub fn expand_towards_vec(&self, v: Vec3) -> AABB {
        self.expand_towards(v.x, v.y, v.z)
    }

    /// `AABB.inflate(double, double, double)`.
    pub fn inflate(&self, x: f64, y: f64, z: f64) -> AABB {
        AABB::new(
            self.min_x - x,
            self.min_y - y,
            self.min_z - z,
            self.max_x + x,
            self.max_y + y,
            self.max_z + z,
        )
    }

    /// `AABB.deflate(double)`.
    pub fn deflate(&self, amount: f64) -> AABB {
        self.inflate(-amount, -amount, -amount)
    }

    /// `AABB.minmax(AABB)`: the smallest box containing both.
    pub fn minmax(&self, o: &AABB) -> AABB {
        AABB::new(
            mth::java_min(self.min_x, o.min_x),
            mth::java_min(self.min_y, o.min_y),
            mth::java_min(self.min_z, o.min_z),
            mth::java_max(self.max_x, o.max_x),
            mth::java_max(self.max_y, o.max_y),
            mth::java_max(self.max_z, o.max_z),
        )
    }

    /// `AABB.intersects(double...)`: strict overlap.
    pub fn intersects(
        &self,
        min_x: f64,
        min_y: f64,
        min_z: f64,
        max_x: f64,
        max_y: f64,
        max_z: f64,
    ) -> bool {
        self.min_x < max_x
            && self.max_x > min_x
            && self.min_y < max_y
            && self.max_y > min_y
            && self.min_z < max_z
            && self.max_z > min_z
    }
}
