//! SVG to PNG `data:` URLs for the Svg component.
//!
//! The crate's CPU rasteriser turns the source into RGBA without a window or a
//! GPU. The result is cached by source and pixel size, because a render runs on
//! every update and rasterising is the slow part.

use std::collections::HashMap;

use schnellui::widgets::{parse_svg, rasterize_svg};

/// Most entries kept. A full cache is cleared, which is simple and enough for
/// the few images an app shows.
const MAX_ENTRIES: usize = 32;
/// Longest side of a rasterised image, in pixels.
const MAX_SIDE: u32 = 4096;

/// Source, then the requested size and density as float bits.
type Key = (String, Option<u32>, Option<u32>, u32);

/// Rasterised SVGs, keyed by source and the size asked for.
#[derive(Default)]
pub(crate) struct SvgCache {
    entries: HashMap<Key, Option<String>>,
    /// How many times a source was rasterised. Tests read it.
    pub(crate) rasterisations: usize,
}

impl SvgCache {
    /// A PNG `data:` URL for `source` drawn at `width` by `height` dp times
    /// `density`. A missing size falls back to the document's own. `None` when
    /// the source does not parse.
    pub(crate) fn data_url(
        &mut self,
        source: &str,
        width: Option<f32>,
        height: Option<f32>,
        density: f32,
    ) -> Option<String> {
        let key = (
            source.to_owned(),
            width.map(f32::to_bits),
            height.map(f32::to_bits),
            density.to_bits(),
        );
        if let Some(hit) = self.entries.get(&key) {
            return hit.clone();
        }
        let url = self.rasterise(source, width, height, density);
        if self.entries.len() >= MAX_ENTRIES {
            self.entries.clear();
        }
        self.entries.insert(key, url.clone());
        url
    }

    fn rasterise(
        &mut self,
        source: &str,
        width: Option<f32>,
        height: Option<f32>,
        density: f32,
    ) -> Option<String> {
        let doc = parse_svg(source).ok()?;
        let density = if density.is_finite() && density > 0.0 {
            density
        } else {
            1.0
        };
        let side = |dp: Option<f32>, own: f32| {
            let dp = dp.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(own);
            ((dp * density).round() as u32).clamp(1, MAX_SIDE)
        };
        let (pw, ph) = (side(width, doc.width), side(height, doc.height));
        let rgba = rasterize_svg(&doc, pw, ph);
        self.rasterisations += 1;
        encode_png(&rgba, pw, ph).map(|png| {
            let mut url = String::from("data:image/png;base64,");
            base64_into(&mut url, &png);
            url
        })
    }
}

fn encode_png(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(rgba).ok()?;
    writer.finish().ok()?;
    Some(out)
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Appends standard padded base64.
pub(crate) fn base64_into(out: &mut String, data: &[u8]) {
    out.reserve(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
}
