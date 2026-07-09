//! Tests for the per-node retained instance ranges: byte-proportionality over a
//! large document and an incremental-vs-fresh equivalence property test.

use crate::render_wgpu::gpu_core::GpuCore;
use crate::render_wgpu::types::Buf;
use crate::render_wgpu::*;
use schnellui::scene::{Color, LayoutBox, Point, Primitive, Rect, Scene, WidgetId, WidgetKind};
use schnellui::text::GlyphAtlas;

fn try_renderer(w: u32, h: u32) -> Option<Renderer> {
    match Renderer::try_new(w, h, Backend::Auto) {
        Ok(mut r) => {
            r.set_clear_color(Color::rgba(0, 0, 0, 255));
            Some(r)
        }
        Err(error) => {
            eprintln!("skipping GPU test: {error}");
            None
        }
    }
}

/// Instance bytes the last frame uploaded.
fn uploaded(r: &Renderer) -> u64 {
    r.last_upload_bytes().instance_bytes
}

const ROWS: usize = 1000;
const ROW_H: f32 = 16.0;

fn row_prims(i: usize, color: Color) -> Vec<Primitive> {
    let y = i as f32 * ROW_H;
    let mut prims = vec![Primitive::SolidRect {
        rect: Rect::new(0.0, y, 128.0, ROW_H - 1.0),
        color,
        corner_radius: 0.0,
    }];
    for g in 0..6 {
        prims.push(Primitive::GlyphQuad {
            rect: Rect::new(2.0 + g as f32 * 5.0, y + 2.0, 4.0, 8.0),
            atlas_uv: Rect::new(0.0, 0.0, 4.0, 8.0),
            color: Color::WHITE,
        });
    }
    prims
}

/// A 1000-row document. With `scrolled` the rows sit in a 128px `Scroll`
/// viewport (only visible rows become instances); without it the rows are a plain
/// tall column, so every row is an instance whether or not it is on screen.
fn document(scrolled: bool, count: usize) -> (Scene, WidgetId, Vec<WidgetId>) {
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Stack, None);
    scene.set_root(root);
    let scroll = if scrolled {
        scene.insert(WidgetKind::Scroll, Some(root))
    } else {
        scene.insert(WidgetKind::Column, Some(root))
    };
    scene.set_layout(
        scroll,
        LayoutBox {
            rect: Rect::new(
                0.0,
                0.0,
                128.0,
                if scrolled {
                    128.0
                } else {
                    count as f32 * ROW_H
                },
            ),
            content: Rect::new(0.0, 0.0, 128.0, count as f32 * ROW_H),
        },
    );
    let mut rows = Vec::new();
    for i in 0..count {
        let row = scene.insert(WidgetKind::Column, Some(scroll));
        scene.set_layout(
            row,
            LayoutBox {
                rect: Rect::new(0.0, i as f32 * ROW_H, 128.0, ROW_H),
                content: Rect::ZERO,
            },
        );
        scene.replace_primitives(row, row_prims(i, Color::rgb(40, 40, 40)));
        rows.push(row);
    }
    (scene, scroll, rows)
}

fn doc_atlas() -> GlyphAtlas {
    let mut atlas = GlyphAtlas::new(32, 32);
    atlas.write_coverage(
        schnellui::text::AtlasRect {
            x: 0,
            y: 0,
            width: 4,
            height: 8,
        },
        &[255u8; 32],
    );
    atlas
}

/// Upload sizes of one update of each kind over a `rows`-row document.
struct Measured {
    first: u64,
    hover: u64,
    scroll_step: u64,
    moved_node: u64,
}

fn measure(scrolled: bool, rows: usize) -> Option<Measured> {
    let mut r = try_renderer(128, 128)?;
    let atlas = doc_atlas();
    let (mut scene, scroll, row_ids) = document(scrolled, rows);
    let _ = r.render_rgba8(&scene, &atlas);
    let first = uploaded(&r);
    scene.clear_dirty();

    // A fill colour change on one visible row.
    scene.set_color(row_ids[3], Color::rgb(200, 30, 30));
    let _ = r.render_rgba8(&scene, &atlas);
    let hover = uploaded(&r);
    assert_eq!(r.core.last_upload_work.full_gathers, 0);
    scene.clear_dirty();

    // One scroll step of one row.
    let mut scroll_step = 0;
    if scrolled {
        scene.set_scroll_offset(scroll, Point { x: 0.0, y: ROW_H });
        let _ = r.render_rgba8(&scene, &atlas);
        scroll_step = uploaded(&r);
        assert_eq!(r.core.last_upload_work.full_gathers, 0);
        scene.clear_dirty();
    }

    // One row moved by 3px (layout box and its absolute paint together).
    let moved = row_ids[5];
    scene.set_layout(
        moved,
        LayoutBox {
            rect: Rect::new(3.0, 5.0 * ROW_H, 128.0, ROW_H),
            content: Rect::ZERO,
        },
    );
    let shifted: Vec<Primitive> = row_prims(5, Color::rgb(40, 40, 40))
        .into_iter()
        .map(|p| match p {
            Primitive::SolidRect {
                rect,
                color,
                corner_radius,
            } => Primitive::SolidRect {
                rect: Rect::new(rect.x + 3.0, rect.y, rect.width, rect.height),
                color,
                corner_radius,
            },
            Primitive::GlyphQuad {
                rect,
                atlas_uv,
                color,
            } => Primitive::GlyphQuad {
                rect: Rect::new(rect.x + 3.0, rect.y, rect.width, rect.height),
                atlas_uv,
                color,
            },
            other => other,
        })
        .collect();
    scene.replace_primitives(moved, shifted);
    let _ = r.render_rgba8(&scene, &atlas);
    let moved_node = uploaded(&r);
    assert_eq!(r.core.last_upload_work.full_gathers, 0);

    eprintln!(
        "UPLOAD BYTES scrolled={scrolled} rows={rows} first={first} hover={hover} \
         scroll_step={scroll_step} moved_node={moved_node}"
    );
    Some(Measured {
        first,
        hover,
        scroll_step,
        moved_node,
    })
}

/// A colour change or a moved node in a 1000-row document uploads only that node's
/// instances: one quad and its six glyphs, a few hundred bytes, not the document.
#[test]
fn hover_and_move_upload_only_the_node_in_a_long_column() {
    let Some(m) = measure(false, ROWS) else {
        return;
    };
    assert!(m.first > 100_000, "the column really is large: {}", m.first);
    assert!(m.hover <= 512, "hover uploaded {} bytes", m.hover);
    assert!(
        m.moved_node <= 512,
        "moved node uploaded {} bytes",
        m.moved_node
    );
}

const TERM_COLS: usize = 200;
const TERM_ROWS: usize = 60;

/// One terminal row of glyph quads. `phase` changes every glyph's colour and atlas
/// position, as an echoed line of new characters does.
fn term_row(row: usize, phase: u8) -> Vec<Primitive> {
    (0..TERM_COLS)
        .map(|col| Primitive::GlyphQuad {
            rect: Rect::new(col as f32 * 4.0, row as f32 * 8.0, 4.0, 8.0),
            atlas_uv: Rect::new(phase as f32 % 2.0 * 4.0, 0.0, 4.0, 8.0),
            color: Color::rgb(phase, 200, 200),
        })
        .collect()
}

/// A 200x60 terminal laid out like `TerminalGrid`'s display list: a backdrop, one
/// background run per row, then every row's glyphs. Returns the scene, the node
/// and the primitive range of row 30's glyphs.
fn terminal_scene() -> (Scene, WidgetId, std::ops::Range<usize>) {
    let mut scene = Scene::new();
    let term = scene.insert(WidgetKind::TerminalGrid, None);
    scene.set_root(term);
    let rect = Rect::new(0.0, 0.0, TERM_COLS as f32 * 4.0, TERM_ROWS as f32 * 8.0);
    scene.set_layout(
        term,
        LayoutBox {
            rect,
            content: rect,
        },
    );
    let mut prims = vec![Primitive::SolidRect {
        rect,
        color: Color::BLACK,
        corner_radius: 0.0,
    }];
    for row in 0..TERM_ROWS {
        prims.push(Primitive::SolidRect {
            rect: Rect::new(0.0, row as f32 * 8.0, 40.0, 8.0),
            color: Color::rgb(20, 20, row as u8),
            corner_radius: 0.0,
        });
    }
    let glyphs_start = prims.len();
    for row in 0..TERM_ROWS {
        prims.extend(term_row(row, 0));
    }
    scene.replace_primitives(term, prims);
    let start = glyphs_start + 30 * TERM_COLS;
    (scene, term, start..start + TERM_COLS)
}

/// A one-row echo in a 200x60 terminal re-encodes and uploads one row. Written
/// through `Scene::paint_range_mut`, the renderer re-encodes just that row's 200
/// primitives and uploads its 200 glyph instances; the whole-node path (`paint_mut`)
/// re-encodes all 12,061 primitives to find the same bytes.
#[test]
fn terminal_row_echo_reencodes_and_uploads_one_row() {
    let Some(mut r) = try_renderer(800, 480) else {
        return;
    };
    let atlas = doc_atlas();
    let (mut scene, term, row) = terminal_scene();
    let total = scene.paint(term).unwrap().primitives.len();
    let _ = r.render_rgba8(&scene, &atlas);
    let first = uploaded(&r);
    scene.clear_dirty();
    let row_bytes = (TERM_COLS * std::mem::size_of::<GlyphInstance>()) as u64;

    // Whole-node path: the node is re-encoded, `patch_slot` finds the changed span.
    scene.paint_mut(term).primitives[row.clone()].copy_from_slice(&term_row(30, 1));
    let _ = r.render_rgba8(&scene, &atlas);
    let whole = r.core.last_upload_work;
    scene.clear_dirty();

    // Ranged path, twice: the first ranged frame builds the primitive map.
    let mut ranged = whole;
    for phase in [2, 3] {
        scene
            .paint_range_mut(term, row.clone())
            .copy_from_slice(&term_row(30, phase));
        let _ = r.render_rgba8(&scene, &atlas);
        ranged = r.core.last_upload_work;
        scene.clear_dirty();
    }
    eprintln!(
        "TERMINAL ECHO first={first}B whole: {}B / {} prims encoded;          ranged: {}B / {} prims encoded (row = {row_bytes}B, {total} prims)",
        whole.bytes.instance_bytes,
        whole.primitives_encoded,
        ranged.bytes.instance_bytes,
        ranged.primitives_encoded,
    );
    assert_eq!(whole.full_gathers, 0);
    assert_eq!(whole.primitives_encoded, total);
    assert_eq!(ranged.full_gathers, 0);
    assert_eq!(ranged.ranges_patched, 1);
    assert_eq!(ranged.primitives_encoded, TERM_COLS);
    assert!(first > 20 * row_bytes, "the grid really is large: {first}");
    assert!(
        ranged.bytes.instance_bytes <= row_bytes,
        "one-row echo uploaded {} bytes (row = {row_bytes})",
        ranged.bytes.instance_bytes
    );

    // Typing one character fills one invisible padding glyph of the row (the
    // terminal grid pads rows to a grow-only capacity): one primitive, one instance.
    let typed = row.end - 1;
    scene.paint_range_mut(term, typed..typed + 1)[0] = Primitive::GlyphQuad {
        rect: Rect::new(796.0, 240.0, 4.0, 8.0),
        atlas_uv: Rect::new(0.0, 0.0, 4.0, 8.0),
        color: Color::rgb(255, 255, 0),
    };
    let _ = r.render_rgba8(&scene, &atlas);
    let typed_work = r.core.last_upload_work;
    eprintln!(
        "TERMINAL TYPE one glyph: {}B / {} prims encoded",
        typed_work.bytes.instance_bytes, typed_work.primitives_encoded
    );
    assert_eq!(typed_work.ranges_patched, 1);
    assert_eq!(typed_work.primitives_encoded, 1);
    assert_eq!(
        typed_work.bytes.instance_bytes,
        std::mem::size_of::<GlyphInstance>() as u64
    );
    scene.clear_dirty();

    // The retained instances equal a fresh gather of the same scene.
    let mut fresh = try_renderer(800, 480).unwrap();
    assert!(r.render_rgba8(&scene, &atlas) == fresh.render_rgba8(&scene, &atlas));
    assert_same_instances(&r, &fresh, "after ranged echo");
    scene.clear_dirty();

    // For comparison, the unpadded case: a typed glyph inserted into the row
    // changes the primitive count, shifts every later glyph instance and is
    // re-encoded and uploaded from the insertion point to the end of the node.
    scene.paint_mut(term).primitives.insert(
        row.end,
        Primitive::GlyphQuad {
            rect: Rect::new(0.0, 240.0, 4.0, 8.0),
            atlas_uv: Rect::new(0.0, 0.0, 4.0, 8.0),
            color: Color::WHITE,
        },
    );
    let _ = r.render_rgba8(&scene, &atlas);
    let inserted = r.core.last_upload_work;
    eprintln!(
        "TERMINAL INSERT (unpadded) one glyph: {}B / {} prims encoded",
        inserted.bytes.instance_bytes, inserted.primitives_encoded
    );
    assert!(inserted.bytes.instance_bytes > 20 * row_bytes);
}

/// A paint-dirty `Scroll` that the last gather never reached (it is hidden) has no
/// run to re-walk and draws nothing, so it must not force a full gather.
#[test]
fn paint_dirty_hidden_scroll_does_not_force_a_full_gather() {
    let Some(mut r) = try_renderer(128, 128) else {
        return;
    };
    let atlas = doc_atlas();
    let (mut scene, _scroll, rows) = document(true, 20);
    let root = scene.root().unwrap();
    let hidden = scene.insert(WidgetKind::Scroll, Some(root));
    let inner = scene.insert(WidgetKind::Column, Some(hidden));
    scene.set_layout(
        hidden,
        LayoutBox {
            rect: Rect::new(0.0, 0.0, 128.0, 128.0),
            content: Rect::new(0.0, 0.0, 128.0, 256.0),
        },
    );
    scene.set_layout(
        inner,
        LayoutBox {
            rect: Rect::new(0.0, 0.0, 128.0, 16.0),
            content: Rect::ZERO,
        },
    );
    scene.replace_primitives(inner, row_prims(0, Color::rgb(90, 90, 90)));
    scene.set_visible(hidden, false);
    let _ = r.render_rgba8(&scene, &atlas);
    assert_eq!(r.core.last_upload_work.full_gathers, 1);
    assert!(!r.core.fragments.by_node.contains_key(&hidden));
    scene.clear_dirty();

    // The hidden scroll repaints alongside a visible row.
    scene.replace_primitives(hidden, row_prims(1, Color::rgb(10, 20, 30)));
    scene.replace_primitives(rows[3], row_prims(3, Color::rgb(200, 0, 0)));
    assert!(scene.paint_dirty().contains(&hidden));
    let got = r.render_rgba8(&scene, &atlas);
    let work = r.core.last_upload_work;
    assert_eq!(work.full_gathers, 0, "hidden scroll forced a full gather");
    assert_eq!(work.nodes_patched, 1);
    assert_eq!(work.scroll_regions, 0);

    let mut fresh = try_renderer(128, 128).unwrap();
    assert!(got == fresh.render_rgba8(&scene, &atlas));
}

/// Inside a scroll viewport the same two edits stay node-sized, and a scroll step
/// uploads the visible content only: the size does not depend on the document.
#[test]
fn scroll_step_uploads_visible_content_not_the_document() {
    let (Some(small), Some(large)) = (measure(true, 100), measure(true, ROWS)) else {
        return;
    };
    for m in [&small, &large] {
        assert!(m.hover <= 512, "hover uploaded {} bytes", m.hover);
        assert!(
            m.moved_node <= 512,
            "moved node uploaded {} bytes",
            m.moved_node
        );
        assert!(m.scroll_step > 0);
    }
    assert_eq!(
        small.scroll_step, large.scroll_step,
        "a scroll step costs the same over 100 and 1000 rows"
    );
    // The viewport shows 8 rows of 7 instances; a step may rewrite those plus slack.
    assert!(
        large.scroll_step <= 2 * large.first,
        "scroll step {} vs first upload {}",
        large.scroll_step,
        large.first
    );
}

// --- incremental vs fresh equivalence ---------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() >> 33) as usize % n
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn color(&mut self) -> Color {
        Color::rgba(
            self.below(256) as u8,
            self.below(256) as u8,
            self.below(256) as u8,
            if self.below(4) == 0 { 160 } else { 255 },
        )
    }
}

struct World {
    scene: Scene,
    nodes: Vec<WidgetId>,
    scrolls: Vec<WidgetId>,
    overlays: Vec<WidgetId>,
    image: (f32, f32),
}

fn random_rect(rng: &mut Rng, max: f32) -> Rect {
    Rect::new(
        rng.unit() * (max + 30.0) - 20.0,
        rng.unit() * (max + 30.0) - 20.0,
        2.0 + rng.unit() * 36.0,
        2.0 + rng.unit() * 36.0,
    )
}

fn random_prims(rng: &mut Rng, image: (f32, f32)) -> Vec<Primitive> {
    let count = rng.below(13);
    (0..count)
        .map(|_| match rng.below(5) {
            0 | 1 => Primitive::SolidRect {
                rect: random_rect(rng, 128.0),
                color: rng.color(),
                corner_radius: rng.below(5) as f32,
            },
            2 => {
                let r = random_rect(rng, 128.0);
                Primitive::GlyphQuad {
                    rect: Rect::new(r.x, r.y, 4.0 + rng.below(4) as f32, 8.0),
                    atlas_uv: Rect::new(0.0, 0.0, 4.0, 8.0),
                    color: rng.color(),
                }
            }
            3 => Primitive::Line {
                from: Point {
                    x: rng.unit() * 128.0,
                    y: rng.unit() * 128.0,
                },
                to: Point {
                    x: rng.unit() * 128.0,
                    y: rng.unit() * 128.0,
                },
                width: 1.0 + rng.below(4) as f32,
                color: rng.color(),
            },
            _ => Primitive::ImageQuad {
                rect: random_rect(rng, 128.0),
                atlas_uv: Rect::new(image.0, image.1, 2.0, 2.0),
                tint: if rng.below(2) == 0 {
                    Color::WHITE
                } else {
                    rng.color()
                },
            },
        })
        .collect()
}

fn leaf_kind(rng: &mut Rng) -> WidgetKind {
    match rng.below(3) {
        0 => WidgetKind::Text,
        1 => WidgetKind::RichText,
        _ => WidgetKind::Chart,
    }
}

/// About 300 nodes: plain containers, two scrolls (one with a nested scroll) and two
/// overlays, every leaf with random paint and a random layout box.
fn build_world(rng: &mut Rng) -> World {
    let mut scene = Scene::new();
    let image = scene
        .images_mut()
        .insert(
            2,
            2,
            &[
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        )
        .expect("insert image");
    let image = (image.x as f32, image.y as f32);
    let root = scene.insert(WidgetKind::Stack, None);
    scene.set_root(root);
    let mut nodes = vec![root];
    let mut scrolls = Vec::new();
    let mut overlays = Vec::new();

    let mut containers: Vec<(WidgetId, usize)> = Vec::new();
    let mut add_container = |scene: &mut Scene,
                             nodes: &mut Vec<WidgetId>,
                             kind: WidgetKind,
                             parent: WidgetId,
                             leaves: usize| {
        let id = scene.insert(kind, Some(parent));
        nodes.push(id);
        containers.push((id, leaves));
        id
    };
    let plain = add_container(&mut scene, &mut nodes, WidgetKind::Column, root, 70);
    let s1 = add_container(&mut scene, &mut nodes, WidgetKind::Scroll, root, 70);
    let s2 = add_container(&mut scene, &mut nodes, WidgetKind::Scroll, s1, 40);
    let s3 = add_container(&mut scene, &mut nodes, WidgetKind::Scroll, root, 30);
    let o1 = add_container(&mut scene, &mut nodes, WidgetKind::Column, root, 25);
    let o2 = add_container(&mut scene, &mut nodes, WidgetKind::Column, root, 15);
    let tail = add_container(&mut scene, &mut nodes, WidgetKind::Column, root, 50);
    let _ = (plain, tail);
    scrolls.extend([s1, s2, s3]);
    overlays.extend([o1, o2]);
    scene.set_overlay_level(o1, 5);
    scene.set_overlay_level(o2, 5);
    for (container, rect) in [
        (root, Rect::new(0.0, 0.0, 128.0, 128.0)),
        (s1, Rect::new(4.0, 4.0, 100.0, 100.0)),
        (s2, Rect::new(10.0, 20.0, 60.0, 50.0)),
        (s3, Rect::new(70.0, 70.0, 50.0, 50.0)),
    ] {
        scene.set_layout(
            container,
            LayoutBox {
                rect,
                content: rect,
            },
        );
    }
    for (container, leaves) in containers {
        for _ in 0..leaves {
            let kind = leaf_kind(rng);
            let leaf = scene.insert(kind, Some(container));
            nodes.push(leaf);
            let rect = random_rect(rng, 128.0);
            scene.set_layout(
                leaf,
                LayoutBox {
                    rect,
                    content: Rect::ZERO,
                },
            );
            let prims = random_prims(rng, image);
            scene.replace_primitives(leaf, prims);
        }
    }
    // Chrome for the scrolls, drawn after their content.
    for &s in &scrolls {
        scene.replace_primitives(
            s,
            [Primitive::SolidRect {
                rect: Rect::new(60.0, 4.0, 4.0, 30.0),
                color: Color::rgb(250, 250, 250),
                corner_radius: 2.0,
            }],
        );
    }
    World {
        scene,
        nodes,
        scrolls,
        overlays,
        image,
    }
}

fn mutate(world: &mut World, rng: &mut Rng) -> &'static str {
    world.nodes.retain(|&id| world.scene.node(id).is_some());
    let pick = |rng: &mut Rng, nodes: &[WidgetId]| nodes[rng.below(nodes.len())];
    let op = rng.below(100);
    match op {
        0..=9 => {
            let id = world
                .nodes
                .get(8 + rng.below(3))
                .copied()
                .unwrap_or_else(|| pick(rng, &world.nodes));
            ranged_edit(world, rng, id);
            "ranged repaint"
        }
        10..=29 => {
            let id = pick(rng, &world.nodes);
            let prims = random_prims(rng, world.image);
            world.scene.replace_primitives(id, prims);
            "repaint"
        }
        30..=44 => {
            let id = pick(rng, &world.nodes);
            let color = rng.color();
            world.scene.set_color(id, color);
            "recolor"
        }
        45..=59 => {
            let id = pick(rng, &world.nodes);
            if id != world.scene.root().unwrap() {
                let rect = random_rect(rng, 128.0);
                world.scene.set_layout(
                    id,
                    LayoutBox {
                        rect,
                        content: Rect::ZERO,
                    },
                );
                let prims = random_prims(rng, world.image);
                world.scene.replace_primitives(id, prims);
            }
            "move"
        }
        60..=71 => {
            let live: Vec<WidgetId> = world
                .scrolls
                .iter()
                .copied()
                .filter(|&s| world.scene.node(s).is_some())
                .collect();
            if !live.is_empty() {
                let s = live[rng.below(live.len())];
                let to = Point {
                    x: rng.below(40) as f32,
                    y: rng.below(160) as f32,
                };
                world.scene.set_scroll_offset(s, to);
            }
            "scroll"
        }
        72..=79 => {
            let parent = pick(rng, &world.nodes);
            let kind = leaf_kind(rng);
            let id = world.scene.insert(kind, Some(parent));
            let rect = random_rect(rng, 128.0);
            world.scene.set_layout(
                id,
                LayoutBox {
                    rect,
                    content: Rect::ZERO,
                },
            );
            let prims = random_prims(rng, world.image);
            world.scene.replace_primitives(id, prims);
            world.nodes.push(id);
            "insert"
        }
        80..=85 => {
            let id = pick(rng, &world.nodes);
            if id != world.scene.root().unwrap() {
                let _ = world.scene.remove_subtree(id);
            }
            "remove"
        }
        86..=91 => {
            let o = world.overlays[rng.below(world.overlays.len())];
            if world.scene.node(o).is_some() {
                world.scene.bring_overlay_to_front(o);
            }
            "raise"
        }
        92..=93 => {
            // Restack an overlay, usually together with an unrelated repaint.
            let o = world.overlays[rng.below(world.overlays.len())];
            if world.scene.node(o).is_some() {
                world.scene.set_overlay_level(o, 3 + rng.below(8) as u8);
            }
            let id = pick(rng, &world.nodes);
            world.scene.set_color(id, rng.color());
            "overlay level"
        }
        94..=95 => {
            let id = pick(rng, &world.nodes);
            let radius = rng.below(6) as f32;
            world.scene.set_corner_radius(id, radius);
            "radius"
        }
        _ => {
            // Several leaves repainted in one frame.
            for _ in 0..4 {
                let id = pick(rng, &world.nodes);
                let prims = random_prims(rng, world.image);
                world.scene.replace_primitives(id, prims);
            }
            "burst"
        }
    }
}

/// A count-preserving edit of a primitive sub-range of `id` through
/// `Scene::paint_range_mut`, as the terminal grid writes a row. Half of the edits
/// keep every primitive's kind (the ranged renderer path); the rest may change
/// kinds, which must fall back to a whole-node re-encode.
fn ranged_edit(world: &mut World, rng: &mut Rng, id: WidgetId) {
    let len = world.scene.paint(id).map_or(0, |p| p.primitives.len());
    if len == 0 {
        return;
    }
    let a = rng.below(len);
    let b = a + 1 + rng.below(len - a);
    if rng.below(2) == 0 {
        let color = rng.color();
        let dx = rng.below(9) as f32 - 4.0;
        for prim in world.scene.paint_range_mut(id, a..b) {
            match prim {
                Primitive::SolidRect { rect, color: c, .. }
                | Primitive::GlyphQuad { rect, color: c, .. } => {
                    rect.x += dx;
                    *c = color;
                }
                Primitive::ImageQuad { rect, .. } => rect.x += dx,
                Primitive::Line { color: c, .. } => *c = color,
            }
        }
    } else {
        let mut fresh = random_prims(rng, world.image);
        while fresh.len() < b - a {
            fresh.extend(random_prims(rng, world.image));
        }
        world
            .scene
            .paint_range_mut(id, a..b)
            .copy_from_slice(&fresh[..b - a]);
    }
}

/// Ranged edits (`Scene::paint_range_mut`) on leaves anywhere in the world, mixed
/// with recolours and scroll steps but no structural change: the renderer patches
/// recorded ranges in place and must still match a full gather after every step.
#[test]
fn ranged_edits_match_a_full_gather() {
    let (Some(mut inc), Some(mut full)) = (try_renderer(128, 128), try_renderer(128, 128)) else {
        return;
    };
    let atlas = doc_atlas();
    let mut rng = Rng(0x5EED_0F7E_5700_0001);
    let mut world = build_world(&mut rng);
    let leaves: Vec<WidgetId> = world.nodes[8..].to_vec();
    // Each leaf reports ranges, so the first edit of a leaf builds its map.
    let hot: Vec<WidgetId> = (0..12).map(|_| leaves[rng.below(leaves.len())]).collect();
    let (mut ranged, mut gathers) = (0, 0);
    for step in 0..300 {
        let op = match rng.below(10) {
            0 => {
                let id = leaves[rng.below(leaves.len())];
                world.scene.set_color(id, rng.color());
                "recolor"
            }
            1 => {
                let sid = world.scrolls[rng.below(world.scrolls.len())];
                let to = Point {
                    x: 0.0,
                    y: rng.below(120) as f32,
                };
                world.scene.set_scroll_offset(sid, to);
                "scroll"
            }
            _ => {
                let id = hot[rng.below(hot.len())];
                ranged_edit(&mut world, &mut rng, id);
                "ranged"
            }
        };
        let got = inc.render_rgba8(&world.scene, &atlas);
        ranged += inc.core.last_upload_work.ranges_patched;
        gathers += inc.core.last_upload_work.full_gathers;
        full.core.invalidate_retained();
        let want = full.render_rgba8(&world.scene, &atlas);
        assert_same_instances(&inc, &full, &format!("step {step} ({op})"));
        assert!(got == want, "step {step} ({op}): ranged render differs");
        if rng.below(4) != 0 {
            world.scene.clear_dirty();
        }
    }
    eprintln!("ranged equivalence: {ranged} ranged patches, {gathers} full gathers");
    assert!(ranged > 40, "ranged path exercised: {ranged}");
}

/// The retained instances of `inc` must hold, node by node, what a full gather puts
/// in `full`, and its slots must tile each buffer in draw order without overlap.
fn assert_same_instances(inc: &Renderer, full: &Renderer, what: &str) {
    let (a, b) = (&inc.core, &full.core);
    for (&id, &fb) in &b.fragments.by_node {
        let Some(fa) = a.fragments.get(&id) else {
            panic!("{what}: node missing from the incremental table");
        };
        let fb = &b.fragments.frags[fb as usize];
        assert_eq!(fa.live, fb.live, "{what}: live counts of {id:?}");
        let slice =
            |c: &GpuCore, f: &crate::render_wgpu::types::GpuFragment, buf: Buf| -> Vec<u8> {
                let r = f.range(buf);
                let live = f.live[buf as usize] as usize;
                let s = r.start as usize;
                match buf {
                    Buf::Quad => bytemuck::cast_slice(&c.quad_scratch[s..s + live]).to_vec(),
                    Buf::Glyph => bytemuck::cast_slice(&c.glyph_scratch[s..s + live]).to_vec(),
                    Buf::Image => bytemuck::cast_slice(&c.image_scratch[s..s + live]).to_vec(),
                }
            };
        for buf in crate::render_wgpu::types::BUFS {
            assert!(
                slice(a, fa, buf) == slice(b, fb, buf),
                "{what}: {buf:?} instances of {id:?}"
            );
        }
    }
    for buf in crate::render_wgpu::types::BUFS {
        let t = &a.fragments;
        let mut cursor = 0u32;
        let mut seen = 0;
        // Quad slots appear base content, base chrome, overlay content ... in list
        // order, which is also buffer order.
        for &fi in &t.order[buf as usize] {
            let r = t.frags[fi as usize].range(buf);
            assert!(
                r.start >= cursor && r.end >= r.start,
                "{what}: {buf:?} slot {r:?} overlaps or precedes {cursor}: {:?} (list pos {seen})",
                t.frags[fi as usize]
            );
            cursor = r.end;
            seen += 1;
        }
        assert!(seen > 0);
        assert_eq!(
            cursor as usize,
            match buf {
                Buf::Quad => a.quad_scratch.len(),
                Buf::Glyph => a.glyph_scratch.len(),
                Buf::Image => a.image_scratch.len(),
            },
            "{what}: {buf:?} slots tile the whole buffer"
        );
    }
}

/// Deterministic random edits over a ~300-node scene with scrolls, a nested scroll
/// and overlays: after every step the incrementally patched renderer must draw the
/// same pixels as a renderer forced through a full gather.
#[test]
fn incremental_renderer_matches_a_full_gather_on_random_edits() {
    for seed in [
        0x9E37_79B9_7F4A_7C15,
        0x1234_5678_9ABC_DEF1,
        0xDEAD_BEEF_CAFE_F00D,
    ] {
        run_equivalence(seed, 260);
    }
}

fn run_equivalence(seed: u64, steps: usize) {
    let (Some(mut inc), Some(mut full)) = (try_renderer(128, 128), try_renderer(128, 128)) else {
        return;
    };
    let atlas = doc_atlas();
    let mut rng = Rng(seed);
    let mut world = build_world(&mut rng);
    assert!(world.nodes.len() >= 300, "{} nodes", world.nodes.len());
    let (mut incremental_frames, mut gathers, mut regions, mut grown, mut ranged) = (0, 0, 0, 0, 0);
    for step in 0..steps {
        let op = mutate(&mut world, &mut rng);
        let got = inc.render_rgba8(&world.scene, &atlas);
        gathers += inc.core.last_upload_work.full_gathers;
        regions += inc.core.last_upload_work.scroll_regions;
        grown += inc.core.last_upload_work.grown_slots;
        ranged += inc.core.last_upload_work.ranges_patched;
        incremental_frames += 1 - inc.core.last_upload_work.full_gathers;
        full.core.invalidate_retained();
        let want = full.render_rgba8(&world.scene, &atlas);
        assert_same_instances(&inc, &full, &format!("step {step} ({op})"));
        assert!(
            got == want,
            "step {step} ({op}): incremental render differs from a full gather"
        );
        if rng.below(3) != 0 {
            world.scene.clear_dirty();
        }
    }
    // A brand new renderer agrees too.
    let mut fresh = try_renderer(128, 128).unwrap();
    assert!(inc.render_rgba8(&world.scene, &atlas) == fresh.render_rgba8(&world.scene, &atlas));
    eprintln!(
        "seed {seed:#x}: {incremental_frames} incremental frames, {gathers} full gathers, \
         {regions} scroll re-walks, {grown} slot growths, {ranged} ranged patches"
    );
    assert!(
        incremental_frames > steps / 2,
        "most steps must take the incremental path: {incremental_frames} of {steps}"
    );
    assert!(
        regions > 5 && grown > 5,
        "scroll runs and slot growth are exercised"
    );
}

#[test]
fn tile_cache_reuses_clean_tiles_and_clears_old_bounds() {
    let Some(mut renderer) = try_renderer(384, 256) else {
        return;
    };
    let atlas = GlyphAtlas::new(32, 32);
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Stack, None);
    scene.set_root(root);
    let leaf = scene.insert(WidgetKind::Column, Some(root));
    let paint = |x, color| {
        [Primitive::SolidRect {
            rect: Rect::new(x, 10.0, 30.0, 30.0),
            color,
            corner_radius: 0.0,
        }]
    };
    scene.replace_primitives(leaf, paint(10.0, Color::WHITE));
    renderer.render_rgba8(&scene, &atlas);
    assert_eq!(renderer.last_tile_cache_stats().rasterized, 6);
    scene.clear_dirty();
    renderer.render_rgba8(&scene, &atlas);
    assert_eq!(
        renderer.last_tile_cache_stats(),
        TileCacheStats {
            hits: 6,
            rasterized: 0
        }
    );
    scene.replace_primitives(leaf, paint(10.0, Color::rgba(255, 0, 0, 255)));
    renderer.render_rgba8(&scene, &atlas);
    assert_eq!(
        renderer.last_tile_cache_stats(),
        TileCacheStats {
            hits: 5,
            rasterized: 1
        }
    );
    scene.clear_dirty();
    scene.replace_primitives(leaf, paint(150.0, Color::WHITE));
    let moved = renderer.render_rgba8(&scene, &atlas);
    assert_eq!(
        renderer.last_tile_cache_stats(),
        TileCacheStats {
            hits: 4,
            rasterized: 2
        }
    );
    assert_eq!(
        &moved[(10 * 384 + 10) * 4..(10 * 384 + 10) * 4 + 4],
        &[0, 0, 0, 255]
    );
    scene.remove_subtree(leaf);
    let removed = renderer.render_rgba8(&scene, &atlas);
    assert_eq!(
        renderer.last_tile_cache_stats(),
        TileCacheStats {
            hits: 5,
            rasterized: 1
        }
    );
    assert!(removed.chunks_exact(4).all(|p| p == [0, 0, 0, 255]));
    renderer.resize(400, 300);
    renderer.render_rgba8(&scene, &atlas);
    assert_eq!(renderer.last_tile_cache_stats().rasterized, 12);
}

#[test]
fn nested_property_translation_clip_matches_fresh_render() {
    use schnellui::scene::CompositeProperties;
    let Some(mut renderer) = try_renderer(256, 128) else {
        return;
    };
    let atlas = GlyphAtlas::new(32, 32);
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Stack, None);
    scene.set_root(root);
    let parent = scene.insert(WidgetKind::Column, Some(root));
    let child = scene.insert(WidgetKind::Column, Some(parent));
    scene.replace_primitives(
        child,
        [Primitive::SolidRect {
            rect: Rect::new(0.0, 0.0, 100.0, 100.0),
            color: Color::WHITE,
            corner_radius: 0.0,
        }],
    );
    scene.set_composite_properties(
        parent,
        CompositeProperties {
            translation: Point { x: 20.0, y: 10.0 },
            clip: Some(Rect::new(0.0, 0.0, 40.0, 40.0)),
        },
    );
    scene.set_composite_properties(
        child,
        CompositeProperties {
            translation: Point { x: 10.0, y: 10.0 },
            clip: Some(Rect::new(0.0, 0.0, 20.0, 20.0)),
        },
    );
    let first = renderer.render_rgba8(&scene, &atlas);
    let pixel =
        |bytes: &[u8], x: usize, y: usize| bytes[(y * 256 + x) * 4..(y * 256 + x) * 4 + 4].to_vec();
    assert_eq!(pixel(&first, 30, 20), [255, 255, 255, 255]);
    assert_eq!(pixel(&first, 29, 20), [0, 0, 0, 255]);
    assert_eq!(pixel(&first, 50, 20), [0, 0, 0, 255]);
    scene.clear_dirty();
    scene.set_composite_properties(
        parent,
        CompositeProperties {
            translation: Point { x: 140.0, y: 10.0 },
            clip: Some(Rect::new(0.0, 0.0, 40.0, 40.0)),
        },
    );
    assert!(scene.layout_dirty().is_empty());
    let updated = renderer.render_rgba8(&scene, &atlas);
    let Some(mut fresh) = try_renderer(256, 128) else {
        return;
    };
    assert_eq!(updated, fresh.render_rgba8(&scene, &atlas));
    assert_eq!(pixel(&updated, 30, 20), [0, 0, 0, 255]);
    assert_eq!(pixel(&updated, 150, 20), [255, 255, 255, 255]);
}

#[test]
fn required_gpu_adapter_is_available() {
    if std::env::var("SCHNELLUI_REQUIRE_GPU").as_deref() == Ok("1") {
        Renderer::try_new(16, 16, Backend::Auto).expect("CI requires a usable GPU adapter");
    }
}

#[test]
fn tile_resources_invalidate_without_primitive_edits() {
    use schnellui::text::AtlasRect;
    let Some(mut renderer) = try_renderer(384, 128) else {
        return;
    };
    let Some(mut fresh) = try_renderer(384, 128) else {
        return;
    };
    let mut atlas = GlyphAtlas::new(32, 32);
    let glyph_rect = AtlasRect {
        x: 0,
        y: 0,
        width: 2,
        height: 2,
    };
    atlas.write_coverage(glyph_rect, &[255; 4]);
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Column, None);
    scene.set_root(root);
    let image = scene.images_mut().insert(2, 2, &[255; 16]).unwrap();
    scene.replace_primitives(
        root,
        [
            Primitive::GlyphQuad {
                rect: Rect::new(10.0, 10.0, 20.0, 20.0),
                atlas_uv: Rect::new(0.0, 0.0, 2.0, 2.0),
                color: Color::WHITE,
            },
            Primitive::ImageQuad {
                rect: Rect::new(150.0, 10.0, 20.0, 20.0),
                atlas_uv: Rect::new(image.x as f32, image.y as f32, 2.0, 2.0),
                tint: Color::WHITE,
            },
        ],
    );
    renderer.render_rgba8(&scene, &atlas);
    scene.clear_dirty();
    atlas.write_coverage(glyph_rect, &[0; 4]);
    let changed = renderer.render_rgba8(&scene, &atlas);
    assert_eq!(
        renderer.last_tile_cache_stats(),
        TileCacheStats {
            hits: 2,
            rasterized: 1
        }
    );
    assert_eq!(changed, fresh.render_rgba8(&scene, &atlas));
    assert!(scene.images_mut().write_rect(image, &[0; 16]));
    let changed = renderer.render_rgba8(&scene, &atlas);
    assert_eq!(
        renderer.last_tile_cache_stats(),
        TileCacheStats {
            hits: 2,
            rasterized: 1
        }
    );
    assert_eq!(changed, fresh.render_rgba8(&scene, &atlas));
}

#[test]
fn tile_cache_matches_transparent_overlay_reorder_and_rotated_clip() {
    use schnellui::scene::CompositeProperties;
    let Some(mut renderer) = try_renderer(384, 256) else {
        return;
    };
    let Some(mut fresh) = try_renderer(384, 256) else {
        return;
    };
    let atlas = GlyphAtlas::new(32, 32);
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Stack, None);
    scene.set_root(root);
    let lower = scene.insert(WidgetKind::Column, Some(root));
    let upper = scene.insert(WidgetKind::Column, Some(root));
    for (id, color) in [
        (lower, Color::rgba(255, 0, 0, 120)),
        (upper, Color::rgba(0, 255, 0, 160)),
    ] {
        scene.set_overlay(id);
        scene.set_composite_properties(
            id,
            CompositeProperties {
                translation: Point::default(),
                clip: Some(Rect::new(105.0, 105.0, 70.0, 70.0)),
            },
        );
        scene.replace_primitives(
            id,
            [Primitive::Line {
                from: Point { x: 95.0, y: 95.0 },
                to: Point { x: 190.0, y: 185.0 },
                width: 30.0,
                color,
            }],
        );
    }
    let before = renderer.render_rgba8(&scene, &atlas);
    scene.clear_dirty();
    assert!(scene.bring_overlay_to_front(lower));
    let reordered = renderer.render_rgba8(&scene, &atlas);
    assert_ne!(reordered, before);
    assert_eq!(reordered, fresh.render_rgba8(&scene, &atlas));
    assert_eq!(renderer.last_tile_cache_stats().hits, 2);
    scene.clear_dirty();
    scene.set_composite_properties(
        lower,
        CompositeProperties {
            translation: Point { x: 140.0, y: 0.0 },
            clip: Some(Rect::new(105.0, 105.0, 70.0, 70.0)),
        },
    );
    let moved = renderer.render_rgba8(&scene, &atlas);
    assert_eq!(moved, fresh.render_rgba8(&scene, &atlas));
    renderer.set_scale(1.5);
    fresh.set_scale(1.5);
    assert_eq!(
        renderer.render_rgba8(&scene, &atlas),
        fresh.render_rgba8(&scene, &atlas)
    );
    assert_eq!(renderer.last_tile_cache_stats().rasterized, 6);
}
