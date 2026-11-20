//! The CPU renderer against the GPU renderer, against a fresh repaint of
//! itself, and across its lane implementations.

use super::{SimdLevel, SoftRenderer};
use crate::render_wgpu::retained_tests::{
    build_world, child_of, doc_atlas, grow_node, mutate, Rng, World,
};
use crate::render_wgpu::{Backend, Renderer};
use schnellui::scene::{Color, Scene};
use schnellui::text::GlyphAtlas;

const BLACK: Color = Color::rgba(0, 0, 0, 255);

fn soft(w: u32, h: u32, scale: f32) -> SoftRenderer {
    let mut r = SoftRenderer::new(w, h);
    r.set_clear_color(BLACK);
    r.set_scale(scale);
    r
}

fn gpu(w: u32, h: u32, scale: f32, clear: Color) -> Option<Renderer> {
    match Renderer::try_new(w, h, Backend::Auto) {
        Ok(mut r) => {
            r.set_clear_color(clear);
            r.set_scale(scale);
            Some(r)
        }
        Err(error) => {
            eprintln!("skipping GPU comparison: {error}");
            None
        }
    }
}

/// How far two RGBA8 pictures are apart.
#[derive(Debug)]
struct Diff {
    /// Largest difference of one channel.
    max: u8,
    /// Pixels with a channel more than 2 codes off.
    far: usize,
    pixels: usize,
}

fn diff(a: &[u8], b: &[u8]) -> Diff {
    assert_eq!(a.len(), b.len());
    let mut d = Diff {
        max: 0,
        far: 0,
        pixels: a.len() / 4,
    };
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        let m = pa
            .iter()
            .zip(pb)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap();
        d.max = d.max.max(m);
        d.far += usize::from(m > 2);
    }
    d
}

/// Most pixels within rounding of the GPU; the rest are pixel centres on a
/// primitive's edge, which a rasterizer may count on either side.
fn assert_close(d: &Diff, what: &str) {
    eprintln!("{what}: {d:?}");
    assert!(
        d.far * 1000 <= d.pixels * 4,
        "{what}: {} of {} pixels differ from the GPU by more than 2 codes",
        d.far,
        d.pixels
    );
}

#[test]
fn matches_the_gpu_on_random_worlds() {
    let atlas = doc_atlas();
    for (seed, scale) in [(1u64, 3.0), (2, 2.5), (3, 1.0), (4, 1.5)] {
        let Some(mut want) = gpu(384, 384, scale, BLACK) else {
            return;
        };
        let world = build_world(&mut Rng(0x5EED_0000 + seed));
        let mut got = soft(384, 384, scale);
        let d = diff(
            got.render(&world.scene, &atlas),
            &want.render_rgba8(&world.scene, &atlas),
        );
        assert_close(&d, &format!("world {seed} at {scale}x"));
    }
}

#[test]
fn matches_the_gpu_on_widgets_and_text() {
    use crate::widgets::{Button, Checkbox, Column, Slider, Text, TextInput};
    for scale in [1.0, 2.0] {
        let mut app = crate::App::mount_with_size_scaled(
            Column::new()
                .child(Text::new("The quick brown fox jumps over the lazy dog"))
                .child(Button::new("Press me"))
                .child(Checkbox::new(true).name("Checked"))
                .child(Slider::new(4.0, 0.0, 10.0))
                .child(TextInput::new("typed text").label("Field")),
            360,
            240,
            scale,
        );
        app.frame();
        let (pw, ph) = (
            (app.size.width * scale).round() as u32,
            (app.size.height * scale).round() as u32,
        );
        let Some(mut want) = gpu(pw, ph, scale, app.clear) else {
            return;
        };
        let mut got = SoftRenderer::new(pw, ph);
        got.set_scale(scale);
        got.set_clear_color(app.clear);
        let gpu_px = want.render_rgba8(&app.scene, &app.atlas);
        let d = diff(got.render(&app.scene, &app.atlas), &gpu_px);
        assert_close(&d, &format!("widgets at {scale}x"));
    }
}

/// One random edit of the kinds the GPU tile-cache test uses.
fn edit(world: &mut World, atlas: &mut GlyphAtlas, rng: &mut Rng) -> &'static str {
    match rng.below(100) {
        0..=59 => mutate(world, rng),
        60..=69 => {
            let id = child_of(world, rng, world.nodes[1]);
            grow_node(world, rng, id);
            "grow in base"
        }
        70..=79 => {
            let o = world.overlays[rng.below(world.overlays.len())];
            let id = child_of(world, rng, o);
            grow_node(world, rng, id);
            "grow in overlay"
        }
        80..=89 => {
            let value = rng.below(256) as u8;
            atlas.write_coverage(
                schnellui::text::AtlasRect {
                    x: 0,
                    y: 0,
                    width: 4,
                    height: 8,
                },
                &[value; 32],
            );
            "glyph atlas write"
        }
        _ => {
            let rect = schnellui::scene::TexelRect {
                x: world.image.0 as u32,
                y: world.image.1 as u32,
                width: 2,
                height: 2,
            };
            let texels: Vec<u8> = (0..16).map(|_| rng.below(256) as u8).collect();
            assert!(world.scene.images_mut().write_rect(rect, &texels));
            "image atlas write"
        }
    }
}

fn fresh(scene: &Scene, atlas: &GlyphAtlas, size: (u32, u32), scale: f32) -> Vec<u8> {
    soft(size.0, size.1, scale).render_rgba8(scene, atlas)
}

/// After every random edit, the retained picture (only damaged tiles redrawn,
/// instances patched in place) equals a fresh renderer's full repaint, byte for
/// byte. Also switches scenes, scales and sizes under the same renderer.
#[test]
fn retained_picture_matches_a_fresh_repaint_on_random_edits() {
    for seed in [0x50F7_0001u64, 0x50F7_0002] {
        let mut rng = Rng(seed);
        let mut atlas = doc_atlas();
        let mut world = build_world(&mut rng);
        let other = build_world(&mut Rng(seed ^ 0xFFFF_0000));
        let (mut size, mut scale) = ((288, 288), 2.0);
        let mut inc = soft(size.0, size.1, scale);
        let mut partial = 0;
        for step in 0..80 {
            let op = match rng.below(20) {
                0 => {
                    let got = inc.render_rgba8(&other.scene, &atlas);
                    assert!(
                        got == fresh(&other.scene, &atlas, size, scale),
                        "step {step}: the other scene differs"
                    );
                    "other scene"
                }
                1 => {
                    scale = if scale == 2.0 { 2.5 } else { 2.0 };
                    inc.set_scale(scale);
                    "rescale"
                }
                2 => {
                    size = if size == (288, 288) {
                        (300, 250)
                    } else {
                        (288, 288)
                    };
                    inc.resize(size.0, size.1);
                    "resize"
                }
                _ => edit(&mut world, &mut atlas, &mut rng),
            };
            let got = inc.render_rgba8(&world.scene, &atlas);
            let stats = inc.last_tile_cache_stats();
            partial += usize::from(stats.hits > 0 && stats.rasterized > 0);
            assert!(
                got == fresh(&world.scene, &atlas, size, scale),
                "step {step} ({op}): the retained picture differs from a full repaint"
            );
            if rng.below(3) != 0 {
                world.scene.clear_dirty();
            }
        }
        assert!(partial > 20, "partial repaints exercised: {partial}");
    }
}

#[test]
fn avx_and_portable_lanes_draw_the_same_bytes() {
    if SimdLevel::best() != SimdLevel::Avx {
        eprintln!("skipping: this CPU has no AVX");
        return;
    }
    let mut rng = Rng(0xA5A5_0003);
    let mut atlas = doc_atlas();
    let mut world = build_world(&mut rng);
    let mut avx = SoftRenderer::with_simd_level(320, 320, SimdLevel::Avx);
    let mut portable = SoftRenderer::with_simd_level(320, 320, SimdLevel::Portable);
    assert_eq!(avx.simd_level(), SimdLevel::Avx);
    assert_eq!(portable.simd_level(), SimdLevel::Portable);
    for step in 0..24 {
        let scale = [1.0, 2.5, 1.5][step % 3];
        avx.set_scale(scale);
        portable.set_scale(scale);
        let a = avx.render_rgba8(&world.scene, &atlas);
        let p = portable.render_rgba8(&world.scene, &atlas);
        assert!(a == p, "step {step}: AVX and portable lanes differ");
        edit(&mut world, &mut atlas, &mut rng);
    }
}

#[test]
fn an_unchanged_frame_repaints_nothing_and_an_edit_repaints_its_tiles() {
    let atlas = doc_atlas();
    let mut world = build_world(&mut Rng(0x7117_0004));
    let mut r = soft(768, 768, 6.0);
    r.render(&world.scene, &atlas);
    let total = r.last_tile_cache_stats().rasterized;
    assert_eq!(total, 36, "a first frame paints every tile");
    world.scene.clear_dirty();
    r.render(&world.scene, &atlas);
    assert_eq!(r.last_tile_cache_stats().rasterized, 0);
    assert_eq!(r.last_tile_cache_stats().hits, 36);
    // A small leaf in the plain column changes colour.
    let leaf = world.scene.node(world.nodes[1]).unwrap().children[3];
    world.scene.replace_primitives(
        leaf,
        [schnellui::scene::Primitive::SolidRect {
            rect: schnellui::scene::Rect::new(20.0, 20.0, 4.0, 4.0),
            color: Color::rgb(10, 200, 30),
            corner_radius: 0.0,
        }],
    );
    r.render(&world.scene, &atlas);
    let stats = r.last_tile_cache_stats();
    assert!(
        stats.rasterized >= 1 && stats.rasterized < 12,
        "one small edit repainted {stats:?}"
    );
}

#[cfg(feature = "count-allocations")]
#[test]
fn steady_rerender_allocates_nothing() {
    let atlas = doc_atlas();
    let mut rng = Rng(0xA110_0005);
    let mut world = build_world(&mut rng);
    let mut r = soft(384, 384, 3.0);
    let leaf = world.scene.node(world.nodes[1]).unwrap().children[5];
    let recolor = |world: &mut World, i: u8| {
        world.scene.set_color(leaf, Color::rgb(i, 255 - i, 40));
    };
    for i in 0..4 {
        recolor(&mut world, i);
        r.render(&world.scene, &atlas);
        world.scene.clear_dirty();
    }
    for i in 4..12 {
        recolor(&mut world, i);
        let info = allocation_counter::measure(|| {
            r.render(&world.scene, &atlas);
        });
        assert_eq!(info.count_total, 0, "allocations on re-render {i}");
        assert!(r.last_tile_cache_stats().rasterized > 0);
        world.scene.clear_dirty();
    }
}
