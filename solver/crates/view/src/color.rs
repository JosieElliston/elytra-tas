//! Colors, the categorical series order, and magnitude/delta ramps.

/// sRGB color with straight alpha, 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }

    /// From `0xRRGGBB`.
    pub const fn hex(v: u32) -> Color {
        Color {
            r: ((v >> 16) & 255) as f32 / 255.0,
            g: ((v >> 8) & 255) as f32 / 255.0,
            b: (v & 255) as f32 / 255.0,
            a: 1.0,
        }
    }

    pub const fn alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    pub fn lerp(self, o: Color, t: f32) -> Color {
        Color {
            r: self.r + (o.r - self.r) * t,
            g: self.g + (o.g - self.g) * t,
            b: self.b + (o.b - self.b) * t,
            a: self.a + (o.a - self.a) * t,
        }
    }
}

pub const WHITE: Color = Color::hex(0xFFFFFF);
pub const BLACK: Color = Color::hex(0x000000);
pub const INK: Color = Color::hex(0x14171A);
pub const RED: Color = Color::hex(0xE5213B);
pub const YELLOW: Color = Color::hex(0xFFD21F);
pub const GREEN: Color = Color::hex(0x10D070);

/// Categorical colors for runs, assigned by position and never cycled: [`series`].
pub const SERIES: [Color; 6] = [
    Color::hex(0xFF2D55),
    Color::hex(0x00C2FF),
    Color::hex(0xFFD400),
    Color::hex(0xB14CFF),
    Color::hex(0x00E08A),
    Color::hex(0xFF8A00),
];

/// The `i`th categorical color. Panics past the end. why? a seventh run needs small
/// multiples or a highlight-one-run view, and a cycled color would silently say two
/// runs are the same.
pub fn series(i: usize) -> Color {
    assert!(
        i < SERIES.len(),
        "series({i}): only {} categorical colors; use panels",
        SERIES.len()
    );
    SERIES[i]
}

/// A piecewise-linear color ramp over 0 to 1.
#[derive(Debug, Clone)]
pub struct Ramp(pub Vec<Color>);

impl Ramp {
    /// Magnitude, single hue: dark low, bright high. Reads on mid-gray stone.
    pub fn magnitude() -> Ramp {
        Ramp(vec![
            Color::hex(0x4A0620),
            Color::hex(0xB00F3F),
            Color::hex(0xFF4A5E),
            Color::hex(0xFFB0A0),
            Color::hex(0xFFF3E6),
        ])
    }

    /// Deltas: blue negative, red positive, neutral at 0.5.
    pub fn diverging() -> Ramp {
        Ramp(vec![
            Color::hex(0x0A5BD8),
            Color::hex(0x79B2FF),
            Color::hex(0xEDEDED),
            Color::hex(0xFF8D7A),
            Color::hex(0xD81414),
        ])
    }

    pub fn at(&self, t: f64) -> Color {
        let n = self.0.len();
        let x = t.clamp(0.0, 1.0) * (n - 1) as f64;
        let i = (x.floor() as usize).min(n - 2);
        self.0[i].lerp(self.0[i + 1], (x - i as f64) as f32)
    }
}
