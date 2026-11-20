//! # schnellui::render_soft
//!
//! The CPU renderer: the same retained scene the wgpu backend draws, rasterised
//! on the CPU with eight-lane SIMD kernels. It is the fallback for machines with
//! no usable GPU adapter, and what `SCHNELLUI_BACKEND=cpu` (or [`Backend::Cpu`])
//! selects, headless and windowed alike.
//!
//! The GPU stays the renderer of record (SOUL §1.2); this one follows it:
//!
//! - **Same instances.** [`SoftRenderer`] owns the renderer-neutral instance
//!   mirrors of [`crate::render_wgpu`], so a paint change re-encodes one node's
//!   slot here too, and draw order (layers, then quads, images, glyphs, scroll
//!   chrome) is identical.
//! - **Same damage.** The tile fingerprints that pick which GPU tiles to redraw
//!   pick which CPU tiles to clear and redraw. Unchanged tiles are not touched.
//! - **Same pixels, near enough.** Each kernel transcribes its fragment shader:
//!   pixel-centre coverage, the rounded-rect distance field, the texel pick, and
//!   the `SrcAlpha, OneMinusSrcAlpha` blend into 8-bit sRGB after every draw.
//!   Results differ from a GPU by float rounding only.
//! - **Deterministic.** No FMA, no estimates: the AVX and portable lanes produce
//!   identical bytes, on any machine.
//! - **Zero allocations** on a steady re-render, like the GPU path (SOUL §1.1).
//!
//! [`Backend::Cpu`]: crate::render_wgpu::Backend::Cpu

mod color;
mod raster;
mod renderer;
mod simd;
mod surface;

pub use renderer::SoftRenderer;
pub use simd::SimdLevel;
pub use surface::SoftSurface;

#[cfg(test)]
mod tests;
