//! Eight `f32` lanes for the CPU rasterizer, behind one small trait so every
//! kernel is written once and compiled per instruction set.
//!
//! [`Portable`] is plain arrays that LLVM vectorizes for the baseline target (SSE2
//! on x86_64, NEON on aarch64). On x86_64, [`Avx`] spells the same operations
//! with 256-bit AVX intrinsics; [`SimdLevel::detect`] picks it at run time.
//!
//! Both implementations do the same IEEE operations in the same order: no fused
//! multiply-add, no reciprocal estimates, and `min`/`max` defined as the compare
//! and select that `minps`/`maxps` perform. A frame is therefore bit for bit the
//! same on every machine and every level, which is what golden screenshots need.

use std::ops::{Add, Div, Mul, Sub};

/// Which lane implementation the rasterizer runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SimdLevel {
    /// 256-bit AVX intrinsics (x86_64 only, detected at run time).
    Avx,
    /// Array code the compiler vectorizes for the build target.
    Portable,
}

impl SimdLevel {
    /// The widest level this CPU supports. `SCHNELLUI_SIMD=portable` forces the
    /// portable lanes.
    pub fn detect() -> SimdLevel {
        if std::env::var("SCHNELLUI_SIMD").is_ok_and(|v| v.eq_ignore_ascii_case("portable")) {
            return SimdLevel::Portable;
        }
        SimdLevel::best()
    }

    /// The widest level this CPU supports, ignoring the environment.
    pub fn best() -> SimdLevel {
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("avx") {
            return SimdLevel::Avx;
        }
        SimdLevel::Portable
    }

    /// `self`, or [`SimdLevel::Portable`] when this CPU cannot run it.
    pub(crate) fn supported(self) -> SimdLevel {
        match self {
            SimdLevel::Avx if SimdLevel::best() == SimdLevel::Avx => SimdLevel::Avx,
            _ => SimdLevel::Portable,
        }
    }
}

/// Number of lanes.
pub(crate) const LANES: usize = 8;

/// Eight `f32` lanes. A *mask* is a value whose lanes are all ones (true) or all
/// zeros (false), as the comparisons return.
pub(crate) trait Lanes:
    Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self>
{
    fn splat(v: f32) -> Self;
    /// `start, start + 1, ..., start + 7`.
    fn ramp(start: f32) -> Self;
    fn load(a: &[f32; LANES]) -> Self;
    fn store(self, out: &mut [f32; LANES]);
    /// Lane-wise `if self < o { self } else { o }`.
    fn min(self, o: Self) -> Self;
    /// Lane-wise `if self > o { self } else { o }`.
    fn max(self, o: Self) -> Self;
    fn abs(self) -> Self;
    fn sqrt(self) -> Self;
    /// Mask of `self < o`.
    fn lt(self, o: Self) -> Self;
    /// Mask of `self <= o`.
    fn le(self, o: Self) -> Self;
    /// Bitwise and: with a mask, zeroes the lanes where it is false.
    fn and(self, o: Self) -> Self;
    /// One bit per lane: the lane's sign bit (set for a true mask lane).
    fn bits(self) -> u32;

    #[inline(always)]
    fn clamp(self, lo: f32, hi: f32) -> Self {
        self.max(Self::splat(lo)).min(Self::splat(hi))
    }
}

/// Plain arrays; the compiler vectorizes the loops for the target's baseline.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Portable([f32; LANES]);

macro_rules! portable_binop {
    ($trait:ident, $fn:ident, $op:tt) => {
        impl $trait for Portable {
            type Output = Portable;
            #[inline(always)]
            fn $fn(self, o: Portable) -> Portable {
                Portable(std::array::from_fn(|i| self.0[i] $op o.0[i]))
            }
        }
    };
}
portable_binop!(Add, add, +);
portable_binop!(Sub, sub, -);
portable_binop!(Mul, mul, *);
portable_binop!(Div, div, /);

#[inline(always)]
fn mask(b: bool) -> f32 {
    f32::from_bits(if b { u32::MAX } else { 0 })
}

impl Lanes for Portable {
    #[inline(always)]
    fn splat(v: f32) -> Self {
        Portable([v; LANES])
    }
    #[inline(always)]
    fn ramp(start: f32) -> Self {
        let mut out = [0.0; LANES];
        for (i, o) in out.iter_mut().enumerate() {
            *o = start + i as f32;
        }
        Portable(out)
    }
    #[inline(always)]
    fn load(a: &[f32; LANES]) -> Self {
        Portable(*a)
    }
    #[inline(always)]
    fn store(self, out: &mut [f32; LANES]) {
        *out = self.0;
    }
    #[inline(always)]
    fn min(self, o: Self) -> Self {
        Portable(std::array::from_fn(|i| {
            if self.0[i] < o.0[i] {
                self.0[i]
            } else {
                o.0[i]
            }
        }))
    }
    #[inline(always)]
    fn max(self, o: Self) -> Self {
        Portable(std::array::from_fn(|i| {
            if self.0[i] > o.0[i] {
                self.0[i]
            } else {
                o.0[i]
            }
        }))
    }
    #[inline(always)]
    fn abs(self) -> Self {
        Portable(std::array::from_fn(|i| {
            f32::from_bits(self.0[i].to_bits() & 0x7FFF_FFFF)
        }))
    }
    #[inline(always)]
    fn sqrt(self) -> Self {
        Portable(std::array::from_fn(|i| self.0[i].sqrt()))
    }
    #[inline(always)]
    fn lt(self, o: Self) -> Self {
        Portable(std::array::from_fn(|i| mask(self.0[i] < o.0[i])))
    }
    #[inline(always)]
    fn le(self, o: Self) -> Self {
        Portable(std::array::from_fn(|i| mask(self.0[i] <= o.0[i])))
    }
    #[inline(always)]
    fn and(self, o: Self) -> Self {
        Portable(std::array::from_fn(|i| {
            f32::from_bits(self.0[i].to_bits() & o.0[i].to_bits())
        }))
    }
    #[inline(always)]
    fn bits(self) -> u32 {
        let mut bits = 0;
        for i in 0..LANES {
            bits |= (self.0[i].to_bits() >> 31) << i;
        }
        bits
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) use avx::Avx;

#[cfg(target_arch = "x86_64")]
mod avx {
    use super::{Lanes, LANES};
    use std::arch::x86_64::*;
    use std::ops::{Add, Div, Mul, Sub};

    /// 256-bit AVX lanes.
    ///
    /// Every method runs an AVX instruction, so a value of this type may only be
    /// created inside a function compiled with `#[target_feature(enable = "avx")]`
    /// that is reached after [`super::SimdLevel::best`] reported AVX. The methods
    /// are `#[inline(always)]` so they compile into that function.
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct Avx(__m256);

    macro_rules! avx_binop {
        ($trait:ident, $fn:ident, $intr:ident) => {
            impl $trait for Avx {
                type Output = Avx;
                #[inline(always)]
                fn $fn(self, o: Avx) -> Avx {
                    // SAFETY: see the type's documentation.
                    Avx(unsafe { $intr(self.0, o.0) })
                }
            }
        };
    }
    avx_binop!(Add, add, _mm256_add_ps);
    avx_binop!(Sub, sub, _mm256_sub_ps);
    avx_binop!(Mul, mul, _mm256_mul_ps);
    avx_binop!(Div, div, _mm256_div_ps);

    // SAFETY (every block below): see the type's documentation.
    impl Lanes for Avx {
        #[inline(always)]
        fn splat(v: f32) -> Self {
            Avx(unsafe { _mm256_set1_ps(v) })
        }
        #[inline(always)]
        fn ramp(start: f32) -> Self {
            unsafe {
                Avx(_mm256_add_ps(
                    _mm256_set1_ps(start),
                    _mm256_setr_ps(0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0),
                ))
            }
        }
        #[inline(always)]
        fn load(a: &[f32; LANES]) -> Self {
            Avx(unsafe { _mm256_loadu_ps(a.as_ptr()) })
        }
        #[inline(always)]
        fn store(self, out: &mut [f32; LANES]) {
            unsafe { _mm256_storeu_ps(out.as_mut_ptr(), self.0) }
        }
        #[inline(always)]
        fn min(self, o: Self) -> Self {
            // minps: a < b ? a : b, the portable definition.
            Avx(unsafe { _mm256_min_ps(self.0, o.0) })
        }
        #[inline(always)]
        fn max(self, o: Self) -> Self {
            Avx(unsafe { _mm256_max_ps(self.0, o.0) })
        }
        #[inline(always)]
        fn abs(self) -> Self {
            Avx(unsafe { _mm256_andnot_ps(_mm256_set1_ps(-0.0), self.0) })
        }
        #[inline(always)]
        fn sqrt(self) -> Self {
            Avx(unsafe { _mm256_sqrt_ps(self.0) })
        }
        #[inline(always)]
        fn lt(self, o: Self) -> Self {
            Avx(unsafe { _mm256_cmp_ps::<_CMP_LT_OQ>(self.0, o.0) })
        }
        #[inline(always)]
        fn le(self, o: Self) -> Self {
            Avx(unsafe { _mm256_cmp_ps::<_CMP_LE_OQ>(self.0, o.0) })
        }
        #[inline(always)]
        fn and(self, o: Self) -> Self {
            Avx(unsafe { _mm256_and_ps(self.0, o.0) })
        }
        #[inline(always)]
        fn bits(self) -> u32 {
            unsafe { _mm256_movemask_ps(self.0) as u32 }
        }
    }
}
