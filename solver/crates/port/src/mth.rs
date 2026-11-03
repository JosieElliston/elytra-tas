//! `net.minecraft.util.Mth`: the parts movement uses.

use std::sync::OnceLock;

/// `Mth.SIN`, built as the Java builds it: `(float)Math.sin(i / 10430.378350470453)`.
fn sin_table() -> &'static [f32; 65536] {
    static SIN: OnceLock<Box<[f32; 65536]>> = OnceLock::new();
    SIN.get_or_init(|| {
        let mut t = Box::new([0.0f32; 65536]);
        for (i, v) in t.iter_mut().enumerate() {
            *v = (i as f64 / 10430.378350470453).sin() as f32;
        }
        t
    })
}

/// `Mth.sin(double)`: `SIN[(int)((long)(i * 10430.378350470453) & 65535L)]`.
pub fn sin(i: f64) -> f32 {
    sin_table()[((i * 10430.378350470453) as i64 & 65535) as usize]
}

/// `Mth.cos(double)`: `SIN[(int)((long)(i * 10430.378350470453 + 16384.0) & 65535L)]`.
pub fn cos(i: f64) -> f32 {
    sin_table()[((i * 10430.378350470453 + 16384.0) as i64 & 65535) as usize]
}

/// `Mth.sqrt(float)`: `(float)Math.sqrt(x)`.
pub fn sqrt(x: f32) -> f32 {
    (x as f64).sqrt() as f32
}

/// `Mth.floor(double)`: `(int)Math.floor(v)`.
pub fn floor(v: f64) -> i32 {
    v.floor() as i32
}

/// `Mth.clamp(float, float, float)`.
pub fn clamp_f32(value: f32, min: f32, max: f32) -> f32 {
    if value < min {
        min
    } else {
        java_min_f32(value, max)
    }
}

/// `Mth.clamp(double, double, double)`.
pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        min
    } else {
        java_min(value, max)
    }
}

/// `Mth.equal(double, double)`: `Math.abs(b - a) < 1.0E-5F`.
pub fn equal(a: f64, b: f64) -> bool {
    (b - a).abs() < 1.0E-5f32 as f64
}

/// `Mth.square(double)`.
pub fn square(x: f64) -> f64 {
    x * x
}

/// `Mth.square(float)`.
pub fn square_f32(x: f32) -> f32 {
    x * x
}

/// `Mth.lerp(double, double, double)`.
pub fn lerp(alpha1: f64, p0: f64, p1: f64) -> f64 {
    p0 + alpha1 * (p1 - p0)
}

/// `Mth.binarySearch(int, int, IntPredicate)`.
pub fn binary_search(mut from: i32, to: i32, condition: impl Fn(i32) -> bool) -> i32 {
    let mut len = to - from;
    while len > 0 {
        let half = len / 2;
        let middle = from + half;
        if condition(middle) {
            len = half;
        } else {
            from = middle + 1;
            len -= half + 1;
        }
    }
    from
}

/// `Math.min(double, double)`: NaN if either is NaN, and -0.0 below 0.0.
pub fn java_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == 0.0 && b == 0.0 {
        if a.is_sign_negative() { a } else { b }
    } else if a <= b {
        a
    } else {
        b
    }
}

/// `Math.max(double, double)`: NaN if either is NaN, and 0.0 above -0.0.
pub fn java_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == 0.0 && b == 0.0 {
        if a.is_sign_positive() { a } else { b }
    } else if a >= b {
        a
    } else {
        b
    }
}

/// `Math.min(float, float)`.
pub fn java_min_f32(a: f32, b: f32) -> f32 {
    java_min(a as f64, b as f64) as f32
}

/// `Math.signum(double)`.
pub fn signum(d: f64) -> f64 {
    if d == 0.0 || d.is_nan() {
        d
    } else {
        1.0f64.copysign(d)
    }
}

/// `Math.round(double)` for the finite, moderate values the shape code passes it.
pub fn round(a: f64) -> i64 {
    (a + 0.5).floor() as i64
}

/// `Math.cos(double)`, as fdlibm computes it (`StrictMath.cos`, `FdLibm.Cos`), for
/// |x| < 3pi/4: the elytra lift's `Math.cos(leanAngle)`, where the lean is a pitch in
/// [-90, 90] degrees. why not `f64::cos`? the platform libm and the JVM differ in the
/// last place often enough to move a replay: on the Burrowed Grove tape macOS libm
/// disagreed with Java's `Math.cos` at 29 of 715 lean angles and the continuous replay
/// drifted by 1e-14 from tick 493; with fdlibm (or Java's own values) it is bit-exact.
/// `Math.cos` itself isn't fdlibm: on this arm64 Mac, HotSpot's `Math.cos` (Temurin
/// 25.0.5, and Modrinth's Zulu 25.0.4.1, which the game runs on, identical to it on 6M
/// pitches) differs from fdlibm in the last place at 0.55% of float pitches, even under
/// `-Xint`, and no fdlibm variant tried reproduces it (oracle, 2026-11-03). So this is
/// closer to the game than libm, not the game; `tests/oracle.rs` counts those ticks as
/// noise.
pub fn java_cos(x: f64) -> f64 {
    let hx = (x.to_bits() >> 32) as i32;
    let ix = hx & 0x7fffffff;
    if ix <= 0x3fe921fb {
        // |x| <= pi/4
        return kernel_cos(x, 0.0);
    }
    if ix >= 0x7ff00000 {
        // inf or NaN: fdlibm's `x - x`
        return f64::NAN;
    }
    assert!(
        ix < 0x4002d97c,
        "java_cos: |x| >= 3pi/4 is not ported ({x})"
    );
    // `__ieee754_rem_pio2`, the |x| < 3pi/4 branch
    const PIO2_1: f64 = f64::from_bits(0x3FF921FB54400000);
    const PIO2_1T: f64 = f64::from_bits(0x3DD0B4611A626331);
    const PIO2_2: f64 = f64::from_bits(0x3DD0B4611A600000);
    const PIO2_2T: f64 = f64::from_bits(0x3BA3198A2E037073);
    let (n, y0, y1);
    if hx > 0 {
        let mut z = x - PIO2_1;
        if ix != 0x3ff921fb {
            y0 = z - PIO2_1T;
            y1 = (z - y0) - PIO2_1T;
        } else {
            z -= PIO2_2;
            y0 = z - PIO2_2T;
            y1 = (z - y0) - PIO2_2T;
        }
        n = 1;
    } else {
        let mut z = x + PIO2_1;
        if ix != 0x3ff921fb {
            y0 = z + PIO2_1T;
            y1 = (z - y0) + PIO2_1T;
        } else {
            z += PIO2_2;
            y0 = z + PIO2_2T;
            y1 = (z - y0) + PIO2_2T;
        }
        n = -1;
    }
    match n & 3 {
        0 => kernel_cos(y0, y1),
        1 => -kernel_sin(y0, y1),
        2 => -kernel_cos(y0, y1),
        _ => kernel_sin(y0, y1),
    }
}

/// fdlibm `__kernel_cos(x, y)`.
fn kernel_cos(x: f64, y: f64) -> f64 {
    const C1: f64 = f64::from_bits(0x3FA555555555554C);
    const C2: f64 = f64::from_bits(0xBF56C16C16C15177);
    const C3: f64 = f64::from_bits(0x3EFA01A019CB1590);
    const C4: f64 = f64::from_bits(0xBE927E4F809C52AD);
    const C5: f64 = f64::from_bits(0x3E21EE9EBDB4B1C4);
    const C6: f64 = f64::from_bits(0xBDA8FAE9BE8838D4);
    let ix = ((x.to_bits() >> 32) as i32) & 0x7fffffff;
    if ix < 0x3e400000 && x as i32 == 0 {
        return 1.0;
    }
    let z = x * x;
    let r = z * (C1 + z * (C2 + z * (C3 + z * (C4 + z * (C5 + z * C6)))));
    if ix < 0x3FD33333 {
        1.0 - (0.5 * z - (z * r - x * y))
    } else {
        let qx = if ix > 0x3fe90000 {
            0.28125
        } else {
            f64::from_bits(((ix - 0x00200000) as u64) << 32)
        };
        let hz = 0.5 * z - qx;
        let a = 1.0 - qx;
        a - (hz - (z * r - x * y))
    }
}

/// fdlibm `__kernel_sin(x, y, 1)`.
fn kernel_sin(x: f64, y: f64) -> f64 {
    const S1: f64 = f64::from_bits(0xBFC5555555555549);
    const S2: f64 = f64::from_bits(0x3F8111111110F8A6);
    const S3: f64 = f64::from_bits(0xBF2A01A019C161D5);
    const S4: f64 = f64::from_bits(0x3EC71DE357B1FE7D);
    const S5: f64 = f64::from_bits(0xBE5AE5E68A2B9CEB);
    const S6: f64 = f64::from_bits(0x3DE5D93A5ACFD57C);
    let ix = ((x.to_bits() >> 32) as i32) & 0x7fffffff;
    if ix < 0x3e400000 && x as i32 == 0 {
        return x;
    }
    let z = x * x;
    let v = z * x;
    let r = S2 + z * (S3 + z * (S4 + z * (S5 + z * S6)));
    x - ((z * (0.5 * y - v * r) - y) - v * S1)
}

/// `(float)(Math.PI / 180.0)`, the degrees-to-radians factor the Java writes inline.
pub const DEG_TO_RAD: f32 = (std::f64::consts::PI / 180.0) as f32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_endpoints() {
        assert_eq!(sin(0.0), 0.0);
        assert_eq!(cos(0.0), 1.0);
        // index 16384 is pi/2 up to the table step
        assert_eq!(sin(std::f64::consts::FRAC_PI_2), 1.0);
        // negative arguments wrap through the long cast and the mask
        assert_eq!(
            sin(-0.5),
            sin_table()[((-0.5f64 * 10430.378350470453) as i64 & 65535) as usize]
        );
    }

    /// `(pitch * DEG_TO_RAD, StrictMath.cos(that))` bits from Temurin 25.
    const STRICT_COS: [(u64, u64); 24] = [
        (0x0000000000000000u64, 0x3ff0000000000000u64),
        (0x3ff921fb60000000u64, 0xbe6777a5cf72ceccu64),
        (0xbff921fb60000000u64, 0xbe6777a5cf72ceccu64),
        (0x3fe921fb60000000u64, 0x3fe6a09e5e333598u64),
        (0xbfe921fb60000000u64, 0x3fe6a09e5e333598u64),
        (0x3fd3366c00000000u64, 0x3fee91a3eae609d0u64),
        (0x3fe392eda0000000u64, 0x3fea32a301c80a7fu64),
        (0x3e1dfc0a00000000u64, 0x3ff0000000000000u64),
        (0xbff0c15240000000u64, 0x3fdfffffe4e6f746u64),
        (0x3ff9214440000000u64, 0x3f26e28883afa63eu64),
        (0x3fe74f3960000000u64, 0x3fe7e1166f369122u64),
        (0xbff20a1200000000u64, 0x3fdb7407bfd1c801u64),
        (0x3fc221fa60000000u64, 0x3fefadf03cdcdd1du64),
        (0x3fd32e6220000000u64, 0x3fee92d3f1096b02u64),
        (0x3ff5d66600000000u64, 0x3fca2d0eefbfd5a1u64),
        (0xbff7434320000000u64, 0x3fbdda163b36f742u64),
        (0x3fe6e27000000000u64, 0x3fe828f7033bf3acu64),
        (0x3fe3f203a0000000u64, 0x3fe9fb9550ea4b12u64),
        (0x3ff3e07ee0000000u64, 0x3fd4a5ad244c9323u64),
        (0xbfa0e9f840000000u64, 0x3feffb87c72ca3aau64),
        (0x3fd4f4f060000000u64, 0x3fee4cb91cf52c16u64),
        (0xbfe8d03f80000000u64, 0x3fe6da1fba743763u64),
        (0x3fd41ce160000000u64, 0x3fee6ecb05843861u64),
        (0xbff1b68180000000u64, 0x3fdca07d7d32afa2u64),
    ];

    #[test]
    fn java_cos_matches_strict_math() {
        for (x, c) in STRICT_COS {
            assert_eq!(
                java_cos(f64::from_bits(x)).to_bits(),
                c,
                "cos({})",
                f64::from_bits(x)
            );
        }
    }

    #[test]
    fn java_min_max_signed_zero() {
        assert!(java_min(0.0, -0.0).is_sign_negative());
        assert!(java_max(-0.0, 0.0).is_sign_positive());
    }
}
