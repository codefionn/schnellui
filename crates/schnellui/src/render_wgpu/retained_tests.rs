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
                | Primitive::StrokeRect { rect, color: c, .. }
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
/// in `full`, and its non-empty slots must tile each buffer in draw order. An empty
/// slot's position carries no data: a scroll re-walk leaves those of the nodes it
/// skips (outside a child index window) where they were.
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
            seen += 1;
            if r.is_empty() {
                assert!(r.end >= r.start, "{what}: {buf:?} slot {r:?} is inverted");
                continue;
            }
            assert!(
                r.start == cursor,
                "{what}: {buf:?} slot {r:?} overlaps or precedes {cursor}: {:?} (list pos {seen})",
                t.frags[fi as usize]
            );
            cursor = r.end;
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
            ..Default::default()
        },
    );
    scene.set_composite_properties(
        child,
        CompositeProperties {
            translation: Point { x: 10.0, y: 10.0 },
            clip: Some(Rect::new(0.0, 0.0, 20.0, 20.0)),
            ..Default::default()
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
            ..Default::default()
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
                ..Default::default()
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
            ..Default::default()
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

#[test]
fn repaint_of_adjacent_nodes_is_one_instance_write() {
    let Some(mut renderer) = try_renderer(1920, 1080) else {
        return;
    };
    let atlas = doc_atlas();
    let (mut scene, leaves) = grid_scene();
    let prims = grid_prims;
    renderer.render_rgba8(&scene, &atlas);
    scene.clear_dirty();

    // Every node changes colour: 2000 one-quad slots that sit side by side in
    // the quad buffer. They go to the GPU as one write, not 2000.
    for (i, leaf) in leaves.iter().enumerate() {
        scene.replace_primitives(*leaf, prims(i, Color::rgb(200, 30, 30)));
    }
    let incremental = renderer.render_rgba8(&scene, &atlas);
    let work = renderer.core.last_upload_work;
    assert_eq!(work.full_gathers, 0);
    assert_eq!(work.nodes_patched, 2000);
    assert_eq!(work.instance_writes, 1);
    assert_eq!(work.instances_written, 2000);
    assert_eq!(renderer.last_tile_cache_stats().hits, 0);

    let mut fresh = try_renderer(1920, 1080).expect("a second renderer");
    assert_eq!(incremental, fresh.render_rgba8(&scene, &atlas));

    // Two nodes far apart in the buffer stay two small writes.
    scene.clear_dirty();
    for i in [3, 1500] {
        scene.replace_primitives(leaves[i], prims(i, Color::WHITE));
    }
    renderer.render_rgba8(&scene, &atlas);
    let work = renderer.core.last_upload_work;
    assert_eq!((work.instance_writes, work.instances_written), (2, 2));
    assert_eq!(renderer.last_tile_cache_stats().rasterized, 2);
}

/// Node `i` of a 2000-node grid filling 1920x1080: a bar and six glyphs.
fn grid_prims(i: usize, color: Color) -> Vec<Primitive> {
    let x = (i % 15) as f32 * 128.0;
    let y = (i / 15) as f32 * 8.0;
    let mut prims = vec![Primitive::SolidRect {
        rect: Rect::new(x, y, 120.0, 7.0),
        color,
        corner_radius: 0.0,
    }];
    for g in 0..6 {
        prims.push(Primitive::GlyphQuad {
            rect: Rect::new(x + 2.0 + g as f32 * 5.0, y + 1.0, 4.0, 5.0),
            atlas_uv: Rect::new(0.0, 0.0, 4.0, 5.0),
            color: Color::WHITE,
        });
    }
    prims
}

/// 2000 sibling nodes of [`grid_prims`], 14,000 instances.
fn grid_scene() -> (Scene, Vec<WidgetId>) {
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Stack, None);
    scene.set_root(root);
    let leaves = (0..2000)
        .map(|i| {
            let leaf = scene.insert(WidgetKind::Column, Some(root));
            scene.replace_primitives(leaf, grid_prims(i, Color::WHITE));
            leaf
        })
        .collect();
    (scene, leaves)
}

/// The tile cache re-hashes what the upload changed, not the scene: one recoloured
/// node of 14,000 instances hashes its one quad, and a node that outgrows its slot
/// at the end of the buffer hashes only the instances from its splice on.
#[test]
fn one_changed_node_hashes_its_instances_not_the_scene() {
    let Some(mut renderer) = try_renderer(1920, 1080) else {
        return;
    };
    let atlas = doc_atlas();
    let (mut scene, leaves) = grid_scene();
    renderer.render_rgba8(&scene, &atlas);
    let core = &renderer.core;
    let total = core.quad_scratch.len() + core.glyph_scratch.len() + core.image_scratch.len();
    assert!(total >= 14_000, "{total} instances");
    assert_eq!(renderer.tiles.instances_hashed, total);
    scene.clear_dirty();

    renderer.render_rgba8(&scene, &atlas);
    assert_eq!(renderer.tiles.instances_hashed, 0, "nothing painted");

    scene.replace_primitives(leaves[1000], grid_prims(1000, Color::rgb(200, 30, 30)));
    let recoloured = renderer.render_rgba8(&scene, &atlas);
    assert_eq!(renderer.core.last_upload_work.full_gathers, 0);
    assert_eq!(renderer.tiles.instances_hashed, 1, "the node's one quad");
    assert_eq!(renderer.last_tile_cache_stats().rasterized, 1);
    let mut fresh = try_renderer(1920, 1080).expect("a second renderer");
    assert!(recoloured == fresh.render_rgba8(&scene, &atlas));
    scene.clear_dirty();

    // The last node gains six glyphs: its glyph slot of 8 overflows and splices.
    let last = leaves.len() - 1;
    let mut grown = grid_prims(last, Color::WHITE);
    grown.extend(
        grid_prims(last, Color::WHITE)
            .into_iter()
            .skip(1)
            .map(|p| match p {
                Primitive::GlyphQuad { rect, atlas_uv, .. } => Primitive::GlyphQuad {
                    rect: Rect::new(rect.x + 32.0, rect.y, rect.width, rect.height),
                    atlas_uv,
                    color: Color::rgb(0, 200, 255),
                },
                other => other,
            }),
    );
    scene.replace_primitives(leaves[last], grown);
    let grown = renderer.render_rgba8(&scene, &atlas);
    let work = renderer.core.last_upload_work;
    assert_eq!((work.full_gathers, work.grown_slots), (0, 1));
    let hashed = renderer.tiles.instances_hashed;
    assert!((12..=32).contains(&hashed), "hashed {hashed}");
    let mut fresh = try_renderer(1920, 1080).expect("a second renderer");
    assert!(grown == fresh.render_rgba8(&scene, &atlas));
}

/// A nested scroll's scrollbar is the last quad of the buffer. Scrolling the outer
/// viewport clips it away, so the incremental upload cuts it off the end of the
/// mirror without writing anything: its old tile must still be repainted.
#[test]
fn tile_cache_clears_instances_cut_from_the_end_of_a_buffer() {
    let (Some(mut renderer), Some(mut fresh)) = (try_renderer(384, 384), try_renderer(384, 384))
    else {
        return;
    };
    let atlas = doc_atlas();
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Stack, None);
    scene.set_root(root);
    let outer = scene.insert(WidgetKind::Scroll, Some(root));
    let inner = scene.insert(WidgetKind::Scroll, Some(outer));
    for (id, rect) in [
        (outer, Rect::new(0.0, 0.0, 384.0, 384.0)),
        (inner, Rect::new(0.0, 0.0, 384.0, 40.0)),
    ] {
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: Rect::new(0.0, 0.0, 384.0, 1000.0),
            },
        );
    }
    scene.replace_primitives(
        inner,
        [Primitive::SolidRect {
            rect: Rect::new(300.0, 10.0, 20.0, 20.0),
            color: Color::WHITE,
            corner_radius: 0.0,
        }],
    );
    let before = renderer.render_rgba8(&scene, &atlas);
    assert!(before != fresh.render_rgba8(&Scene::new(), &atlas));
    let quads = renderer.core.quad_scratch.len();
    scene.clear_dirty();

    scene.set_scroll_offset(outer, Point { x: 0.0, y: 100.0 });
    let after = renderer.render_rgba8(&scene, &atlas);
    let work = renderer.core.last_upload_work;
    assert_eq!((work.full_gathers, work.scroll_regions), (0, 1));
    assert!(
        renderer.core.quad_scratch.len() < quads,
        "the mirror shrank"
    );
    assert_eq!(renderer.last_tile_cache_stats().rasterized, 1);
    assert!(after == fresh.render_rgba8(&scene, &atlas));
}

/// Appends 13 to 36 random primitives to `id`, so its slots usually overflow and
/// splice the mirrors, moving every slot and layer boundary behind it.
fn grow_node(world: &mut World, rng: &mut Rng, id: WidgetId) {
    let mut prims = world
        .scene
        .paint(id)
        .map_or_else(Vec::new, |p| p.primitives.clone());
    while prims.len() < 13 {
        prims.extend(random_prims(rng, world.image));
    }
    for _ in 0..3 {
        prims.extend(random_prims(rng, world.image));
    }
    world.scene.replace_primitives(id, prims);
}

/// A live child of `parent`, or any live node.
fn child_of(world: &World, rng: &mut Rng, parent: WidgetId) -> WidgetId {
    match world.scene.node(parent).map(|n| n.children.clone()) {
        Some(children) if !children.is_empty() => children[rng.below(children.len())],
        _ => world.nodes[rng.below(world.nodes.len())],
    }
}

/// Random edits on a surface of 3x3 or more tiles: after every frame the tile
/// cache's picture must equal a full repaint of a full gather. This is the test
/// that catches a missed invalidation. Besides `mutate`, the steps grow nodes past
/// their slots in the base layer and inside an overlay (the splice moves the layer
/// boundaries behind it), write both atlases, upload a frame without drawing it,
/// draw a different scene, rescale and resize.
#[test]
fn tile_cache_matches_a_full_repaint_on_random_edits() {
    for seed in [0x7115_CA5E_0000_0001, 0xA11A_5000_FEED_0002] {
        tile_equivalence(seed, 220);
    }
}

fn tile_equivalence(seed: u64, steps: usize) {
    let (Some(mut inc), Some(mut full)) = (try_renderer(384, 384), try_renderer(384, 384)) else {
        return;
    };
    let mut atlas = doc_atlas();
    let mut rng = Rng(seed);
    let mut world = build_world(&mut rng);
    let mut other = build_world(&mut Rng(seed ^ 0xFFFF_0000));
    let (mut size, mut scale) = ((384, 384), 3.0);
    inc.set_scale(scale);
    full.set_scale(scale);
    let (mut partial, mut grown, mut shifted) = (0, 0, 0);
    for step in 0..steps {
        let op = match rng.below(100) {
            0..=54 => mutate(&mut world, &mut rng),
            55..=64 => {
                let id = child_of(&world, &mut rng, world.nodes[1]);
                grow_node(&mut world, &mut rng, id);
                "grow in base"
            }
            65..=74 => {
                let o = world.overlays[rng.below(world.overlays.len())];
                let id = child_of(&world, &mut rng, o);
                grow_node(&mut world, &mut rng, id);
                "grow in overlay"
            }
            75..=79 => {
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
            80..=84 => {
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
            85..=89 => {
                // A window frame that uploads and then fails to acquire its
                // surface: the tile cache never sees that upload's spans.
                mutate(&mut world, &mut rng);
                let _ = inc
                    .core
                    .upload_scene(&world.scene, &atlas, size.0, size.1, scale);
                if rng.below(2) == 0 {
                    world.scene.clear_dirty();
                }
                mutate(&mut world, &mut rng);
                "upload without draw"
            }
            90..=93 => {
                let got = inc.render_rgba8(&other.scene, &atlas);
                full.core.invalidate_retained();
                full.tiles.invalidate();
                let want = full.render_rgba8(&other.scene, &atlas);
                assert!(got == want, "step {step}: the other scene differs");
                other.scene.clear_dirty();
                "other scene"
            }
            94..=96 => {
                scale = if scale == 3.0 { 2.5 } else { 3.0 };
                inc.set_scale(scale);
                full.set_scale(scale);
                "rescale"
            }
            _ => {
                size = if size == (384, 384) {
                    (400, 330)
                } else {
                    (384, 384)
                };
                inc.resize(size.0, size.1);
                full.resize(size.0, size.1);
                "resize"
            }
        };
        let layers_before = inc.core.overlay_layers.clone();
        let got = inc.render_rgba8(&world.scene, &atlas);
        let work = inc.core.last_upload_work;
        let core = &inc.core;
        let total = core.quad_scratch.len() + core.glyph_scratch.len() + core.image_scratch.len();
        if work.full_gathers == 0 && inc.tiles.instances_hashed < total {
            partial += 1;
            grown += usize::from(work.grown_slots > 0);
            shifted += usize::from(core.overlay_layers != layers_before);
        }
        full.core.invalidate_retained();
        full.tiles.invalidate();
        let want = full.render_rgba8(&world.scene, &atlas);
        assert!(
            got == want,
            "step {step} ({op}): the tile cache's picture differs from a full repaint"
        );
        if rng.below(3) != 0 {
            world.scene.clear_dirty();
        }
    }
    eprintln!(
        "tile seed {seed:#x}: {partial} frames hashed part of the scene, {grown} of them \
         grew a slot, {shifted} moved overlay boundaries"
    );
    assert!(partial > steps / 3, "partial re-hash exercised: {partial}");
    assert!(
        grown > 10 && shifted > 5,
        "splices and layer shifts exercised"
    );
}

// --- scroll re-walk cost ----------------------------------------------------------

const LIST_W: u32 = 1920;
const LIST_H: u32 = 1080;

/// Row `i`'s colour: distinct for neighbouring rows, so a scroll step changes bytes.
fn list_color(i: usize) -> Color {
    Color::rgb(
        (i * 37 % 251) as u8,
        (i * 91 % 241) as u8,
        (i * 53 % 239) as u8,
    )
}

/// A `rows`-row list filling a 1920x1080 viewport: `Scroll > Column > row`, each row
/// a bar in its own colour with a six-glyph `Text` label as its child.
fn long_list(rows: usize) -> (Scene, WidgetId, Vec<WidgetId>) {
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Stack, None);
    scene.set_root(root);
    let scroll = scene.insert(WidgetKind::Scroll, Some(root));
    let column = scene.insert(WidgetKind::Column, Some(scroll));
    let height = rows as f32 * ROW_H;
    let viewport = Rect::new(0.0, 0.0, LIST_W as f32, LIST_H as f32);
    scene.set_layout(
        scroll,
        LayoutBox {
            rect: viewport,
            content: Rect::new(0.0, 0.0, LIST_W as f32, height),
        },
    );
    scene.set_layout(
        column,
        LayoutBox {
            rect: Rect::new(0.0, 0.0, LIST_W as f32, height),
            content: Rect::ZERO,
        },
    );
    let mut ids = Vec::with_capacity(rows);
    for i in 0..rows {
        let y = i as f32 * ROW_H;
        let row = scene.insert(WidgetKind::Row, Some(column));
        scene.set_layout(
            row,
            LayoutBox {
                rect: Rect::new(0.0, y, LIST_W as f32, ROW_H),
                content: Rect::ZERO,
            },
        );
        scene.replace_primitives(
            row,
            [Primitive::SolidRect {
                rect: Rect::new(0.0, y, 400.0, ROW_H - 1.0),
                color: list_color(i),
                corner_radius: 0.0,
            }],
        );
        let label = scene.insert(WidgetKind::Text, Some(row));
        scene.set_layout(
            label,
            LayoutBox {
                rect: Rect::new(2.0, y + 2.0, 30.0, 8.0),
                content: Rect::ZERO,
            },
        );
        scene.replace_primitives(label, row_prims(i, list_color(i + 1)).split_off(1));
        ids.push(row);
    }
    (scene, scroll, ids)
}

/// One-row scroll steps over a 5000- and a 50,000-row list visit the rows in or
/// leaving the viewport plus a small constant, not every row, and big jumps visit
/// the old and the new window. Pixels equal a fresh renderer after each phase.
#[test]
fn scroll_step_walks_the_visible_rows_not_the_list() {
    let atlas = doc_atlas();
    // 1080 / 16 = 67.5 rows show; each row is two nodes.
    let window = (LIST_H as f32 / ROW_H) as usize + 2;
    let mut walked_by_size = Vec::new();
    for rows in [5000, 50_000] {
        let (Some(mut r), Some(mut fresh)) =
            (try_renderer(LIST_W, LIST_H), try_renderer(LIST_W, LIST_H))
        else {
            return;
        };
        let (mut scene, scroll, _) = long_list(rows);
        let _ = r.render_rgba8(&scene, &atlas);
        scene.clear_dirty();
        let mut most = 0;
        for step in 1..=4 {
            scene.set_scroll_offset(
                scroll,
                Point {
                    x: 0.0,
                    y: step as f32 * ROW_H,
                },
            );
            let _ = r.core.upload_scene(&scene, &atlas, LIST_W, LIST_H, 1.0);
            let work = r.core.last_upload_work;
            assert_eq!((work.full_gathers, work.scroll_regions), (0, 1));
            most = most.max(work.walked_nodes);
            scene.clear_dirty();
        }
        assert!(
            most <= 2 * (window + 2) + 8,
            "{rows} rows: a one-row step walked {most} nodes"
        );
        let got = r.render_rgba8(&scene, &atlas);
        assert!(
            got == fresh.render_rgba8(&scene, &atlas),
            "{rows} rows: step"
        );
        scene.clear_dirty();

        // A jump to the middle, then past the end: the old and the new window.
        for y in [rows as f32 * ROW_H / 2.0, rows as f32 * ROW_H * 2.0] {
            scene.set_scroll_offset(scroll, Point { x: 0.0, y });
            let got = r.render_rgba8(&scene, &atlas);
            let work = r.core.last_upload_work;
            assert_eq!(work.full_gathers, 0);
            assert!(
                work.walked_nodes <= 4 * (window + 2) + 8,
                "{rows} rows: a jump walked {} nodes",
                work.walked_nodes
            );
            fresh.core.invalidate_retained();
            assert!(
                got == fresh.render_rgba8(&scene, &atlas),
                "{rows} rows: jump"
            );
            scene.clear_dirty();
        }
        walked_by_size.push(most);
    }
    assert_eq!(
        walked_by_size[0], walked_by_size[1],
        "a one-row step walks the same nodes at 5000 and 50,000 rows"
    );
}

/// A list world for the windowed scroll re-walk: an outer `Scroll > Column` of
/// rows with uneven heights, each a bar and a text label. Every 23rd row holds a
/// horizontal scroll of 20 cells, row 10 a vertical scroll of 40 sub-rows, and
/// some rows carry a composite translation or clip.
struct ListWorld {
    scene: Scene,
    outer: WidgetId,
    inners: Vec<WidgetId>,
    column: WidgetId,
    rows: Vec<WidgetId>,
    height: f32,
}

const LW_W: f32 = 256.0;

fn list_row(scene: &mut Scene, column: WidgetId, i: usize, y: f32, h: f32) -> WidgetId {
    let row = scene.insert(WidgetKind::Row, Some(column));
    scene.set_layout(
        row,
        LayoutBox {
            rect: Rect::new(0.0, y, LW_W + 60.0, h),
            content: Rect::ZERO,
        },
    );
    scene.replace_primitives(
        row,
        [Primitive::SolidRect {
            rect: Rect::new(4.0, y, LW_W + 50.0, h - 1.0),
            color: list_color(i),
            corner_radius: (i % 3) as f32,
        }],
    );
    let label = scene.insert(WidgetKind::Text, Some(row));
    scene.set_layout(
        label,
        LayoutBox {
            rect: Rect::new(8.0, y + 1.0, 30.0, h.min(8.0)),
            content: Rect::ZERO,
        },
    );
    let glyphs: Vec<Primitive> = (0..5)
        .map(|g| Primitive::GlyphQuad {
            rect: Rect::new(8.0 + g as f32 * 5.0, y + 1.0, 4.0, 8.0),
            atlas_uv: Rect::new(0.0, 0.0, 4.0, 8.0),
            color: list_color(i + 7),
        })
        .collect();
    scene.replace_primitives(label, glyphs);
    row
}

fn build_list_world(rng: &mut Rng, count: usize) -> ListWorld {
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Stack, None);
    scene.set_root(root);
    let outer = scene.insert(WidgetKind::Scroll, Some(root));
    let column = scene.insert(WidgetKind::Column, Some(outer));
    let mut rows = Vec::new();
    let mut inners = Vec::new();
    let mut y = 0.0;
    for i in 0..count {
        let h = 6.0 + rng.below(25) as f32;
        let h = if i % 23 == 0 || i == 10 {
            h.max(40.0)
        } else {
            h
        };
        let row = list_row(&mut scene, column, i, y, h);
        if i % 23 == 0 {
            // A horizontal scroll of 20 cells, wider than its viewport.
            let inner = scene.insert(WidgetKind::Scroll, Some(row));
            scene.set_layout(
                inner,
                LayoutBox {
                    rect: Rect::new(40.0, y + 2.0, 150.0, h - 4.0),
                    content: Rect::new(40.0, y + 2.0, 600.0, h - 4.0),
                },
            );
            scene.replace_primitives(
                inner,
                [Primitive::SolidRect {
                    rect: Rect::new(40.0, y + h - 4.0, 30.0, 2.0),
                    color: Color::WHITE,
                    corner_radius: 1.0,
                }],
            );
            for j in 0..20 {
                let cell = scene.insert(WidgetKind::Column, Some(inner));
                scene.replace_primitives(
                    cell,
                    [Primitive::SolidRect {
                        rect: Rect::new(40.0 + j as f32 * 30.0, y + 3.0, 26.0, h - 8.0),
                        color: list_color(i * 31 + j),
                        corner_radius: 0.0,
                    }],
                );
            }
            inners.push(inner);
        }
        if i == 10 {
            // A vertical scroll holding its own list of 40 sub-rows.
            let inner = scene.insert(WidgetKind::Scroll, Some(row));
            scene.set_layout(
                inner,
                LayoutBox {
                    rect: Rect::new(100.0, y + 2.0, 120.0, h - 4.0),
                    content: Rect::new(100.0, y + 2.0, 120.0, 400.0),
                },
            );
            for j in 0..40 {
                let sub = scene.insert(WidgetKind::Row, Some(inner));
                scene.replace_primitives(
                    sub,
                    [Primitive::SolidRect {
                        rect: Rect::new(100.0, y + 2.0 + j as f32 * 10.0, 120.0, 9.0),
                        color: list_color(i * 17 + j),
                        corner_radius: 0.0,
                    }],
                );
            }
            inners.push(inner);
        }
        match i % 37 {
            5 => scene.set_composite_properties(
                row,
                schnellui::scene::CompositeProperties {
                    translation: Point { x: 12.0, y: 3.0 },
                    clip: None,
                    clip_spares_overlays: false,
                },
            ),
            9 => scene.set_composite_properties(
                row,
                schnellui::scene::CompositeProperties {
                    translation: Point::default(),
                    clip: Some(Rect::new(0.0, y, 90.0, h)),
                    clip_spares_overlays: false,
                },
            ),
            _ => {}
        }
        rows.push(row);
        y += h;
    }
    scene.set_layout(
        outer,
        LayoutBox {
            rect: Rect::new(0.0, 0.0, LW_W, LW_W),
            content: Rect::new(0.0, 0.0, LW_W + 60.0, y),
        },
    );
    scene.replace_primitives(
        outer,
        [Primitive::SolidRect {
            rect: Rect::new(LW_W - 6.0, 4.0, 4.0, 40.0),
            color: Color::rgb(250, 250, 250),
            corner_radius: 2.0,
        }],
    );
    ListWorld {
        scene,
        outer,
        inners,
        column,
        rows,
        height: y,
    }
}

/// One random step on a list world: scrolls of every size and direction on the
/// outer and the nested viewports, repaints, moves and ranged edits of rows that
/// are usually off screen, inserts and removes.
fn list_step(world: &mut ListWorld, rng: &mut Rng) -> &'static str {
    world.rows.retain(|&id| world.scene.node(id).is_some());
    world.inners.retain(|&id| world.scene.node(id).is_some());
    let at = |w: &ListWorld| w.scene.scroll_offset(w.outer);
    let row = world.rows[rng.below(world.rows.len())];
    match rng.below(100) {
        0..=24 => {
            let now = at(world);
            let dy = (rng.below(7) as f32 - 3.0) * 7.5;
            let dx = if rng.below(5) == 0 {
                rng.below(9) as f32 - 4.0
            } else {
                0.0
            };
            world.scene.set_scroll_offset(
                world.outer,
                Point {
                    x: now.x + dx,
                    y: now.y + dy,
                },
            );
            "small step"
        }
        25..=36 => {
            let y = rng.unit() * (world.height + 600.0) - 300.0;
            world
                .scene
                .set_scroll_offset(world.outer, Point { x: 0.0, y });
            "jump"
        }
        37..=41 => {
            let y = if rng.below(2) == 0 {
                0.0
            } else {
                world.height - LW_W
            };
            world
                .scene
                .set_scroll_offset(world.outer, Point { x: 0.0, y });
            "to an end"
        }
        42..=51 => {
            if !world.inners.is_empty() {
                let inner = world.inners[rng.below(world.inners.len())];
                let to = Point {
                    x: rng.below(500) as f32 - 20.0,
                    y: rng.below(400) as f32 - 20.0,
                };
                world.scene.set_scroll_offset(inner, to);
                if rng.below(2) == 0 {
                    let now = at(world);
                    world.scene.set_scroll_offset(
                        world.outer,
                        Point {
                            x: now.x,
                            y: now.y + 5.0,
                        },
                    );
                }
            }
            "nested scroll"
        }
        52..=63 => {
            world.scene.set_color(row, rng.color());
            "recolor"
        }
        64..=71 => {
            // Repaint a row so it draws where the viewport is now: a row far off
            // screen whose bounds suddenly reach the clip.
            let top = at(world).y;
            let rect = Rect::new(
                rng.unit() * 200.0,
                top + rng.unit() * 200.0,
                20.0 + rng.unit() * 40.0,
                4.0 + rng.unit() * 20.0,
            );
            world.scene.replace_primitives(
                row,
                [Primitive::SolidRect {
                    rect,
                    color: rng.color(),
                    corner_radius: 0.0,
                }],
            );
            "repaint into view"
        }
        72..=77 => {
            if let Some(label) = world.scene.node(row).map(|n| n.children[0]) {
                let len = world.scene.paint(label).map_or(0, |p| p.primitives.len());
                if len > 0 {
                    let color = rng.color();
                    for prim in world.scene.paint_range_mut(label, 0..len.min(2)) {
                        if let Primitive::GlyphQuad { color: c, .. } = prim {
                            *c = color;
                        }
                    }
                }
            }
            "ranged"
        }
        78..=83 => {
            // Move a row's layout box and paint (no relayout of the others).
            let b = world.scene.layout(row).copied().unwrap_or_default();
            let dy = rng.below(41) as f32 - 20.0;
            world.scene.set_layout(
                row,
                LayoutBox {
                    rect: Rect::new(b.rect.x, b.rect.y + dy, b.rect.width, b.rect.height),
                    content: Rect::ZERO,
                },
            );
            let prims: Vec<Primitive> = world
                .scene
                .paint(row)
                .map_or_else(Vec::new, |p| p.primitives.clone())
                .into_iter()
                .map(|p| match p {
                    Primitive::SolidRect {
                        rect,
                        color,
                        corner_radius,
                    } => Primitive::SolidRect {
                        rect: Rect::new(rect.x, rect.y + dy, rect.width, rect.height),
                        color,
                        corner_radius,
                    },
                    other => other,
                })
                .collect();
            world.scene.replace_primitives(row, prims);
            "move"
        }
        84..=89 => {
            let y = rng.unit() * world.height;
            let i = world.rows.len() + rng.below(1000);
            let id = list_row(&mut world.scene, world.column, i, y, 12.0);
            world.rows.push(id);
            "insert"
        }
        90..=93 => {
            if world.rows.len() > 30 {
                let _ = world.scene.remove_subtree(row);
            }
            "remove"
        }
        94..=96 => {
            world.scene.replace_primitives(
                world.outer,
                [Primitive::SolidRect {
                    rect: Rect::new(LW_W - 6.0, rng.unit() * 200.0, 4.0, 40.0),
                    color: rng.color(),
                    corner_radius: 2.0,
                }],
            );
            "scrollbar"
        }
        _ => {
            for _ in 0..3 {
                let row = world.rows[rng.below(world.rows.len())];
                world.scene.set_color(row, rng.color());
            }
            let now = at(world);
            world.scene.set_scroll_offset(
                world.outer,
                Point {
                    x: now.x,
                    y: now.y - 11.0,
                },
            );
            "burst and step"
        }
    }
}

/// Random scroll steps, jumps, nested scrolls, repaints, moves, inserts and
/// removes on a list of uneven rows: after every frame the windowed re-walk must
/// hold, node by node, what a full gather holds, and draw the same pixels.
#[test]
fn windowed_scroll_matches_a_full_gather_on_random_scrolls() {
    for seed in [
        0x5C70_11ED_0000_0001,
        0x0BAD_5EED_1234_5678,
        0x7777_AAAA_0000_0003,
    ] {
        list_equivalence(seed, 300);
    }
}

fn list_equivalence(seed: u64, steps: usize) {
    let (Some(mut inc), Some(mut full)) = (try_renderer(256, 256), try_renderer(256, 256)) else {
        return;
    };
    let atlas = doc_atlas();
    let mut rng = Rng(seed);
    let mut world = build_list_world(&mut rng, 400);
    let nodes = world.scene.len();
    let (mut windowed, mut gathers) = (0, 0);
    for step in 0..steps {
        let op = list_step(&mut world, &mut rng);
        let got = inc.render_rgba8(&world.scene, &atlas);
        let work = inc.core.last_upload_work;
        gathers += work.full_gathers;
        if work.scroll_regions > 0 && work.walked_nodes < nodes / 4 {
            windowed += 1;
        }
        full.core.invalidate_retained();
        let want = full.render_rgba8(&world.scene, &atlas);
        assert_same_instances(&inc, &full, &format!("seed {seed:#x} step {step} ({op})"));
        assert!(
            got == want,
            "seed {seed:#x} step {step} ({op}): windowed render differs from a full gather"
        );
        if rng.below(4) != 0 {
            world.scene.clear_dirty();
        }
    }
    eprintln!("list seed {seed:#x}: {windowed} windowed re-walks, {gathers} full gathers");
    assert!(
        windowed > steps / 4,
        "windowed re-walks exercised: {windowed}"
    );
}
