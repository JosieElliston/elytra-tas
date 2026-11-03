//! The float trait the generic kernel is written over.

use num_dual::DualNum;
use num_traits::NumCast;
use std::fmt::Debug;
use std::ops::{Add, Div, Mul, Neg, Sub};

/// A number the generic kernel can run on: `f64` (precise physics), `f32` (fast,
/// approximate), or any `Copy` num-dual type over either (`Dual64` for one
/// directional derivative, `DualSVec64<N>` for a gradient, `HyperDual64` /
/// `Dual2_64` / `HyperDualSVec64<M, N>` for second derivatives). It is implemented for
/// every `num_dual::DualNum + Copy`, which covers all of those.
///
/// Branches in the kernel compare [`Scalar::re`], so a dual number takes the branch
/// its value takes and carries that branch's derivative.
pub trait Scalar:
    Copy
    + Debug
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Neg<Output = Self>
{
    /// A constant. On `f32` (and duals over it) this rounds to the nearest `f32`.
    fn cst(x: f64) -> Self;
    /// The value, without derivatives.
    fn re(self) -> f64;
    fn sin_cos(self) -> (Self, Self);
    fn sqrt(self) -> Self;
    fn powf(self, k: f64) -> Self;
}

impl<D> Scalar for D
where
    D: DualNum + Copy,
{
    #[inline]
    fn cst(x: f64) -> Self {
        D::from(<D::Primitive as NumCast>::from(x).expect("an f64 constant fits"))
    }
    #[inline]
    fn re(self) -> f64 {
        <f64 as NumCast>::from(num_dual::DualStruct::re(&self)).expect("f32/f64 widen to f64")
    }
    #[inline]
    fn sin_cos(self) -> (Self, Self) {
        DualNum::sin_cos(&self)
    }
    #[inline]
    fn sqrt(self) -> Self {
        DualNum::sqrt(&self)
    }
    #[inline]
    fn powf(self, k: f64) -> Self {
        DualNum::powf(
            &self,
            <D::Primitive as NumCast>::from(k).expect("an f64 exponent fits"),
        )
    }
}

/// `a` if `a.re() >= b.re()`, else `b`.
#[inline]
pub(crate) fn max<T: Scalar>(a: T, b: T) -> T {
    if a.re() >= b.re() { a } else { b }
}
