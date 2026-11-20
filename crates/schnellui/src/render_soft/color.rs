//! sRGB <-> linear tables for blending into an 8-bit sRGB picture.
//!
//! The GPU target is `Rgba8UnormSrgb` with `BlendState::ALPHA_BLENDING`: the
//! hardware decodes the stored texel to linear, blends, and stores the result
//! rounded back to 8-bit sRGB after every draw. The CPU picture does the same so
//! translucent layers quantize the way they do on the GPU.

use std::sync::OnceLock;

/// The decode and encode tables, built once per process.
pub(crate) struct ColorLuts {
    /// 8-bit sRGB -> linear.
    pub(crate) decode: [f32; 256],
    /// 8-bit unorm -> `[0, 1]` (the alpha channel and R8 glyph coverage).
    pub(crate) unorm: [f32; 256],
    /// `upper[k]` is the linear value at which the rounded sRGB code moves from
    /// `k` to `k + 1` (infinite for 255).
    upper: [f32; 256],
    /// First guess of the code for linear `i / 65535`; at most one step off.
    guess: Box<[u8]>,
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(l: f64) -> f64 {
    if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

/// The process-wide tables.
pub(crate) fn luts() -> &'static ColorLuts {
    static LUTS: OnceLock<ColorLuts> = OnceLock::new();
    LUTS.get_or_init(|| {
        let mut decode = [0.0; 256];
        let mut unorm = [0.0; 256];
        let mut upper = [f32::INFINITY; 256];
        for k in 0..256 {
            decode[k] = srgb_to_linear(k as f64 / 255.0) as f32;
            unorm[k] = k as f32 / 255.0;
            if k < 255 {
                upper[k] = srgb_to_linear((k as f64 + 0.5) / 255.0) as f32;
            }
        }
        let guess = (0..65536u32)
            .map(|i| (linear_to_srgb(i as f64 / 65535.0) * 255.0).round() as u8)
            .collect();
        ColorLuts {
            decode,
            unorm,
            upper,
            guess,
        }
    })
}

impl ColorLuts {
    /// Linear `l` to the nearest 8-bit sRGB code. Exact against the f32
    /// thresholds: the table's guess is corrected by one comparison either way
    /// (the thresholds are further apart than one table step).
    #[inline(always)]
    pub(crate) fn encode(&self, l: f32) -> u8 {
        let l = if l > 0.0 { l.min(1.0) } else { 0.0 };
        let mut c = self.guess[(l * 65535.0 + 0.5) as usize] as usize;
        if l >= self.upper[c] {
            c += 1;
        } else if c > 0 && l < self.upper[c - 1] {
            c -= 1;
        }
        c as u8
    }

    /// `[0, 1]` to the nearest 8-bit unorm code.
    #[inline(always)]
    pub(crate) fn encode_unorm(&self, a: f32) -> u8 {
        let a = if a > 0.0 { a.min(1.0) } else { 0.0 };
        (a * 255.0 + 0.5) as u8
    }
}

/// Packs RGBA bytes into the picture's pixel word (bytes in memory order).
#[inline(always)]
pub(crate) fn pack(r: u8, g: u8, b: u8, a: u8) -> u32 {
    u32::from_le_bytes([r, g, b, a])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_rounds_like_the_exact_formula() {
        let luts = luts();
        for k in 0..=255u8 {
            assert_eq!(luts.encode(luts.decode[k as usize]), k, "round trip of {k}");
        }
        let mut worst = 0.0f64;
        for i in 0..=200_000u32 {
            let l = i as f32 / 200_000.0;
            let got = luts.encode(l) as f64;
            let exact = linear_to_srgb(l as f64) * 255.0;
            worst = worst.max((got - exact).abs());
        }
        assert!(worst <= 0.5 + 1e-4, "encode is off by {worst} codes");
    }
}
