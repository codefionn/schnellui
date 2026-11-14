use crate::text::types::*;
use parking_lot::{Condvar, Mutex};
use std::borrow::Cow;
use std::sync::{mpsc, Arc, OnceLock};

use parley::fontique::{
    Attributes, Blob, Collection, CollectionOptions, FamilyId, QueryFamily, QueryFont, QueryStatus,
    SourceCache,
};
use parley::FontData;
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, FontStyle, FontWeight,
    GenericFamily, Layout, LayoutContext, OverflowWrap, PositionedLayoutItem, RangedBuilder,
    StyleProperty, TextWrapMode,
};
use rustc_hash::FxHashMap;
use swash::scale::image::{Content, Image};
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::Format;

/// Line height of text that can wrap, as a multiple of its font size.
const PARAGRAPH_LINE_HEIGHT: f32 = 1.4;
use swash::FontRef;

/// The shaper/measurer, owning pooled Parley + Fontique contexts (SOUL §8.1). The
/// `LayoutContext`/`Layout` are pooled so warm shaping is amortized zero-alloc
/// (SOUL §4.1 `text_edit` budget); rasterized glyphs are cached by [`GlyphKey`].
pub struct TextShaper {
    /// The embedded faces plus the system fonts. Filled in from
    /// [`SystemFonts`] by the first text that needs them.
    font_cx: Option<FontContext>,
    /// The embedded faces alone, for text they cover ([`FaceCoverage`]).
    embedded_cx: FontContext,
    layout_cx: LayoutContext<Brush>,
    /// Reused layout scratch (`build_into` clears + refills — §4.4).
    scratch_layout: Layout<Brush>,
    /// swash scaling caches + scratch buffers (pooled).
    scale_cx: ScaleContext,
    /// Reused glyph-image buffer (grow-only — §4.4).
    scratch_image: Image,
    /// Rasterized-glyph cache: a repeated glyph is a hit, no atlas re-write.
    glyph_cache: FxHashMap<GlyphKey, RasterGlyph>,
    /// Glyph cache misses so far: each is one swash rasterization.
    rasterized: u64,
    /// Reused source-text buffer for [`TextShaper::truncate_to_width_into`].
    scratch_text: String,
    /// Reused cut-point list for [`TextShaper::truncate_to_width_into`].
    scratch_cuts: Vec<(u32, f32)>,
    /// Font data used by swash, indexed by the stable [`FontId`] stored on shaped glyphs.
    pub(crate) font_resources: Vec<FontData>,
    /// Maps Fontique blob/index identities to the rasterizer-facing id above.
    font_ids: FxHashMap<(u64, u32), FontId>,
    /// Per embedded face, the characters it holds (see [`FaceCoverage`]).
    coverage: [CoverageSlot; FACE_COUNT],
    /// The families the sans and the mono short stacks keep for the text
    /// being shaped, a bit per stack position ([`FamilyRows`]).
    masks: [Vec<u64>; 2],
    /// Shapes that took a narrowed family stack, so tests can tell the path ran.
    pub(crate) narrowed: u64,
    /// Test switch: shape everything with the full family stack.
    full_stack_only: bool,
}

const FACE_COUNT: usize = FontFace::MonoBold as usize + 1;

/// Every face, indexed by its discriminant.
const FONT_FACES: [FontFace; FACE_COUNT] = [
    FontFace::Sans,
    FontFace::SansBold,
    FontFace::SansItalic,
    FontFace::SansBoldItalic,
    FontFace::Mono,
    FontFace::MonoBold,
];

/// Which characters one embedded face holds, so text made only of them can be
/// shaped against the embedded faces alone.
///
/// With system fonts loaded, a generic family resolves to the embedded face
/// followed by every matching system family (188 on a desktop with many fonts
/// installed). Parley copies that list into its font query for each shaped
/// item and drops it afterwards, and does so again per cluster for digits and
/// other characters with an emoji form. That costs more than shaping a short
/// label does. The embedded face is first in the list and parley stops at the
/// first font that maps a whole cluster, so for text the face covers the rest
/// of the list is never read. A collection without the system fonts gives the
/// same glyphs from a list of one.
struct FaceCoverage {
    /// The font parley's query yields first for the face.
    font: QueryFont,
    /// One bit per 64-codepoint block of the BMP: `has` is filled in for it.
    known: [u64; 16],
    /// One bit per BMP codepoint: the font maps it to a glyph.
    has: [u64; 1024],
}

#[derive(Default)]
enum CoverageSlot {
    #[default]
    Unresolved,
    /// The face could not be resolved; its text takes the full stack.
    Unavailable,
    Ready(Box<FaceCoverage>),
}

impl FaceCoverage {
    fn resolve(font_cx: &mut FontContext, face: FontFace) -> Option<Box<FaceCoverage>> {
        let generic = if face.mono() {
            GenericFamily::Monospace
        } else {
            GenericFamily::SansSerif
        };
        let id = font_cx.collection.generic_families(generic).next()?;
        let mut first = None;
        let mut query = font_cx.collection.query(&mut font_cx.source_cache);
        query.set_families([QueryFamily::Id(id)]);
        query.set_attributes(face_attributes(face));
        query.matches_with(|font| {
            first = Some(font.clone());
            QueryStatus::Stop
        });
        Some(Box::new(FaceCoverage {
            font: first?,
            known: [0; 16],
            has: [0; 1024],
        }))
    }

    /// Whether parley's font selection finds every cluster of `text` complete
    /// in this face. Control characters count as held: they take no part in
    /// shaping, so they never send a cluster to another font.
    fn holds(&mut self, text: &str) -> bool {
        text.chars().all(|ch| {
            let cp = ch as u32;
            if cp > 0xFFFF {
                return false;
            }
            let block = (cp >> 6) as usize;
            if self.known[block >> 6] & (1 << (block & 63)) == 0 {
                self.fill(block);
            }
            self.has[block] & (1 << (cp & 63)) != 0 || ch.is_control()
        })
    }

    fn fill(&mut self, block: usize) {
        let mut bits = 0u64;
        if let Some(charmap) = self.font.charmap() {
            for bit in 0..64 {
                let mapped = char::from_u32((block as u32) << 6 | bit)
                    .and_then(|ch| charmap.map(ch))
                    .is_some_and(|glyph| glyph != 0);
                bits |= (mapped as u64) << bit;
            }
        }
        self.has[block] = bits;
        self.known[block >> 6] |= 1 << (block & 63);
    }

    /// Whether the face lacks `ch` (in the sense of [`FaceCoverage::holds`]).
    fn lacks(&mut self, ch: char) -> bool {
        let mut buf = [0u8; 4];
        !self.holds(ch.encode_utf8(&mut buf))
    }
}

/// The generic family stacks of the system collection. Built once per process.
///
/// Family ids come from the collection that [`SystemFonts`] built, and every
/// shaper's full collection is a clone of it, so the ids hold for all of them.
struct FamilyStacks {
    sans: FamilyStack,
    mono: FamilyStack,
    /// The two slots below resolve to nothing in the system collection, so a
    /// short stack written into them is all parley sees.
    usable: bool,
}

struct FamilyStack {
    /// The families the generic resolves to, in parley's order.
    ids: Vec<FamilyId>,
    /// How many leading families are the embedded ones.
    embedded: usize,
}

/// The generic family that carries the short stack of the sans faces. The
/// embedded faces leave it unset and fontconfig has no list for it.
const SANS_SLOT: GenericFamily = GenericFamily::UiSerif;
/// The same for the mono faces.
const MONO_SLOT: GenericFamily = GenericFamily::UiMonospace;

impl FamilyStacks {
    fn get() -> &'static FamilyStacks {
        static STACKS: OnceLock<FamilyStacks> = OnceLock::new();
        STACKS.get_or_init(|| {
            let mut system = SystemFonts::get().collection.clone();
            let mut embedded = BaseFonts::get().embedded.clone();
            let mut stack = |generic| FamilyStack {
                ids: system.generic_families(generic).collect(),
                embedded: embedded.generic_families(generic).count(),
            };
            let (sans, mono) = (
                stack(GenericFamily::SansSerif),
                stack(GenericFamily::Monospace),
            );
            let usable = system.generic_families(SANS_SLOT).next().is_none()
                && system.generic_families(MONO_SLOT).next().is_none();
            FamilyStacks { sans, mono, usable }
        })
    }

    fn stack(&self, mono: bool) -> &FamilyStack {
        if mono {
            &self.mono
        } else {
            &self.sans
        }
    }
}

/// The short family stacks: for text the embedded face does not cover, a
/// stack that shapes it exactly as the full generic stack would.
///
/// For each cluster parley walks the fonts of the stack (then the emoji
/// generic for emoji clusters, then the script fallbacks) and keeps the first
/// font that maps the whole cluster, else the font that mapped the largest
/// share first, else the first font. The embedded face `E` comes first. A
/// later font `F` that maps no character `E` lacks cannot change that
/// outcome for a cluster `E` did not complete: `F` maps a subset of what `E`
/// maps there, so its share is no larger than the best share already seen and
/// parley discards it. So the stack can drop every system family that maps
/// none of the characters the text needs from outside `E`, and the fonts
/// parley still meets come in the same order. The emoji and script fallback
/// lists parley appends are untouched.
///
/// Parley also maps a cluster's NFC and NFD forms, whose characters may not
/// be in the text. A missing kana therefore marks the families that map any
/// kana it composes with or decomposes into ([`KANA_PAIRS`]), and any other
/// missing character that takes part in canonical composition sends the text
/// down the full stack ([`normalization_inert`]).
///
/// The short stack reaches parley as a generic family ([`SANS_SLOT`],
/// [`MONO_SLOT`]) rewritten on the shaper's own collection before each build.
/// Names would not do: the embedded symbol face is called "DejaVu Sans", so
/// that name never resolves to the system DejaVu Sans.
///
/// Which families map a character is a row in [`FamilyRows`], computed off
/// the shaping thread. Until every row a text needs is there, the text takes
/// the full stack, which gives the same glyphs.
struct FamilyRows {
    state: Mutex<RowState>,
    ready: Condvar,
    /// The worker's request queue; `None` where there is no worker thread.
    requests: OnceLock<Option<mpsc::Sender<RowKey>>>,
}

/// A character and the face whose stack and attributes its row is for. The
/// face fixes both: no two faces share a stack and attributes.
type RowKey = (char, u8);

/// A row that was requested and has not arrived.
const PENDING: u32 = u32::MAX;

#[derive(Default)]
struct RowState {
    /// Per key, the offset of its row in `bits`, or [`PENDING`]. A row is a bit
    /// per stack position: the family yields a font that maps the character
    /// or, for kana, one of its [`KANA_PAIRS`] partners.
    rows: FxHashMap<RowKey, u32>,
    bits: Vec<u64>,
}

thread_local! {
    /// Family rows scanned on this thread, so a test can prove shaping never
    /// scans.
    static ROW_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl FamilyRows {
    fn get() -> &'static FamilyRows {
        static ROWS: OnceLock<FamilyRows> = OnceLock::new();
        ROWS.get_or_init(|| FamilyRows {
            state: Mutex::new(RowState::default()),
            ready: Condvar::new(),
            requests: OnceLock::new(),
        })
    }

    /// Asks the worker for the row of `key`, once. Without a worker the key
    /// stays pending and its text keeps the full stack.
    fn request(&'static self, state: &mut RowState, key: RowKey) {
        if state.rows.contains_key(&key) {
            return;
        }
        state.rows.insert(key, PENDING);
        if let Some(requests) = self.requests.get_or_init(|| self.spawn_worker()) {
            let _ = requests.send(key);
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn spawn_worker(&'static self) -> Option<mpsc::Sender<RowKey>> {
        None
    }

    /// One thread that scans the rows in request order, with its own clone of
    /// the system collection and its own source cache.
    #[cfg(not(target_arch = "wasm32"))]
    fn spawn_worker(&'static self) -> Option<mpsc::Sender<RowKey>> {
        let (send, receive) = mpsc::channel::<RowKey>();
        std::thread::Builder::new()
            .name("schnellui-font-rows".into())
            .spawn(move || {
                let mut font_cx = SystemFonts::context();
                let mut row = Vec::new();
                let mut next = receive.recv().ok();
                while let Some(key) = next {
                    self.scan_into(key, &mut font_cx, &mut row);
                    next = match receive.try_recv() {
                        Ok(key) => Some(key),
                        Err(mpsc::TryRecvError::Disconnected) => None,
                        Err(mpsc::TryRecvError::Empty) => {
                            // Unmap the font files once the queue is drained;
                            // the next burst maps what it needs again.
                            font_cx.source_cache.prune(0, false);
                            receive.recv().ok()
                        }
                    };
                }
            })
            .ok()
            .map(|_| send)
    }

    /// Scans and stores the row of `key`.
    fn scan_into(&self, key: RowKey, font_cx: &mut FontContext, row: &mut Vec<u64>) {
        scan_row(key, font_cx, row);
        let mut state = self.state.lock();
        let at = state.bits.len();
        state.bits.extend_from_slice(row);
        state.rows.insert(key, at as u32);
        drop(state);
        self.ready.notify_all();
    }
}

/// Fills `row` with a bit per position of the face's stack: the family yields
/// a font that maps the character or one of its [`KANA_PAIRS`] partners.
/// Queries every system family of the stack, which loads their files.
fn scan_row((ch, face): RowKey, font_cx: &mut FontContext, row: &mut Vec<u64>) {
    ROW_SCANS.with(|c| c.set(c.get() + 1));
    let face = FONT_FACES[face as usize];
    let stack = FamilyStacks::get().stack(face.mono());
    row.clear();
    row.resize(stack.ids.len().div_ceil(64), 0);
    let mut query = font_cx.collection.query(&mut font_cx.source_cache);
    query.set_attributes(face_attributes(face));
    // The character and, for kana, the characters parley may map in its
    // place (the NFC or NFD form of its cluster).
    let relatives = KANA_PAIRS
        .iter()
        .filter(|pair| pair.contains(&(ch as u16)))
        .flatten();
    let wanted =
        || std::iter::once(ch).chain(relatives.clone().filter_map(|&r| char::from_u32(r as u32)));
    for (at, &id) in stack.ids.iter().enumerate().skip(stack.embedded) {
        // The same fonts, in the same order, that this family yields inside
        // the full stack.
        query.set_families([QueryFamily::Id(id)]);
        let mut maps = false;
        query.matches_with(|font| {
            maps = font.charmap().is_some_and(|charmap| {
                wanted().any(|c| charmap.map(c).is_some_and(|glyph| glyph != 0))
            });
            if maps {
                QueryStatus::Stop
            } else {
                QueryStatus::Continue
            }
        });
        row[at / 64] |= (maps as u64) << (at % 64);
    }
}

/// The font attributes parley resolves for `face`.
fn face_attributes(face: FontFace) -> Attributes {
    Attributes {
        weight: if face.bold() {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        },
        style: if face.italic() {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
        ..Attributes::default()
    }
}

/// Whether a short family stack can stand in for the full one when the
/// embedded face lacks `ch`. Parley maps a cluster's NFC and NFD forms as well
/// as its text, so the stack has to hold the families for those characters
/// too. That holds for a character with no two-character canonical
/// decomposition that is neither half of one, so its clusters have no other
/// form, and for kana, whose forms [`KANA_PAIRS`] lists in full. The ranges
/// were checked against the Unicode 16 decomposition data; they leave out
/// accented letters, arrows and math operators (U+2190 to U+22FF, U+2ADC,
/// U+2ADD) and Hangul, which compose.
fn normalization_inert(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2000..=0x218F
            | 0x2300..=0x2ADB
            | 0x2ADE..=0x2BFF
            | 0x2E00..=0x2E7F
            | 0x3000..=0x30FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xE000..=0xF8FF
            | 0xF900..=0xFAFF
            | 0xFE00..=0xFE0F
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFFEF
            | 0x1F000..=0x1FBFF
            | 0x20000..=0x3FFFF
            | 0xE0000..=0xE01EF
            | 0xF0000..=0x10FFFF
    )
}

/// Every two-character canonical decomposition that involves kana, as
/// (composed, first, second), from the Unicode 16 data. None reaches outside
/// U+3040 to U+30FF.
const KANA_PAIRS: [[u16; 3]; 58] = [
    [0x304C, 0x304B, 0x3099],
    [0x304E, 0x304D, 0x3099],
    [0x3050, 0x304F, 0x3099],
    [0x3052, 0x3051, 0x3099],
    [0x3054, 0x3053, 0x3099],
    [0x3056, 0x3055, 0x3099],
    [0x3058, 0x3057, 0x3099],
    [0x305A, 0x3059, 0x3099],
    [0x305C, 0x305B, 0x3099],
    [0x305E, 0x305D, 0x3099],
    [0x3060, 0x305F, 0x3099],
    [0x3062, 0x3061, 0x3099],
    [0x3065, 0x3064, 0x3099],
    [0x3067, 0x3066, 0x3099],
    [0x3069, 0x3068, 0x3099],
    [0x3070, 0x306F, 0x3099],
    [0x3071, 0x306F, 0x309A],
    [0x3073, 0x3072, 0x3099],
    [0x3074, 0x3072, 0x309A],
    [0x3076, 0x3075, 0x3099],
    [0x3077, 0x3075, 0x309A],
    [0x3079, 0x3078, 0x3099],
    [0x307A, 0x3078, 0x309A],
    [0x307C, 0x307B, 0x3099],
    [0x307D, 0x307B, 0x309A],
    [0x3094, 0x3046, 0x3099],
    [0x309E, 0x309D, 0x3099],
    [0x30AC, 0x30AB, 0x3099],
    [0x30AE, 0x30AD, 0x3099],
    [0x30B0, 0x30AF, 0x3099],
    [0x30B2, 0x30B1, 0x3099],
    [0x30B4, 0x30B3, 0x3099],
    [0x30B6, 0x30B5, 0x3099],
    [0x30B8, 0x30B7, 0x3099],
    [0x30BA, 0x30B9, 0x3099],
    [0x30BC, 0x30BB, 0x3099],
    [0x30BE, 0x30BD, 0x3099],
    [0x30C0, 0x30BF, 0x3099],
    [0x30C2, 0x30C1, 0x3099],
    [0x30C5, 0x30C4, 0x3099],
    [0x30C7, 0x30C6, 0x3099],
    [0x30C9, 0x30C8, 0x3099],
    [0x30D0, 0x30CF, 0x3099],
    [0x30D1, 0x30CF, 0x309A],
    [0x30D3, 0x30D2, 0x3099],
    [0x30D4, 0x30D2, 0x309A],
    [0x30D6, 0x30D5, 0x3099],
    [0x30D7, 0x30D5, 0x309A],
    [0x30D9, 0x30D8, 0x3099],
    [0x30DA, 0x30D8, 0x309A],
    [0x30DC, 0x30DB, 0x3099],
    [0x30DD, 0x30DB, 0x309A],
    [0x30F4, 0x30A6, 0x3099],
    [0x30F7, 0x30EF, 0x3099],
    [0x30F8, 0x30F0, 0x3099],
    [0x30F9, 0x30F1, 0x3099],
    [0x30FA, 0x30F2, 0x3099],
    [0x30FE, 0x30FD, 0x3099],
];

thread_local! {
    /// Full shapers built on this thread (see [`TextShaper::full_builds`]).
    static FULL_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Default for TextShaper {
    fn default() -> Self {
        Self::new()
    }
}

/// The embedded part of the process-wide font setup every [`TextShaper`]
/// starts from. It reads no files, so building it takes microseconds.
struct BaseFonts {
    /// The embedded faces and the generic families that point at them.
    embedded: Collection,
    /// One blob per embedded face. [`SystemFonts`] registers the same blobs,
    /// so a face has one blob identity (and one `FontId`) whichever collection
    /// shaped it.
    blobs: Vec<Blob<u8>>,
    font_resources: Vec<FontData>,
    font_ids: FxHashMap<(u64, u32), FontId>,
}

impl BaseFonts {
    fn get() -> &'static BaseFonts {
        static BASE: OnceLock<BaseFonts> = OnceLock::new();
        BASE.get_or_init(BaseFonts::build)
    }

    fn build() -> BaseFonts {
        let mut font_resources = Vec::new();
        let mut font_ids = FxHashMap::default();
        // Static bytes, no copy.
        let blobs: Vec<Blob<u8>> = EMBEDDED_FACES
            .iter()
            .map(|bytes| Blob::new(Arc::new(*bytes) as Arc<dyn AsRef<[u8]> + Send + Sync>))
            .collect();
        for blob in &blobs {
            // FontFace::font_id and the symbol-font ids are indices into
            // EMBEDDED_FACES, so registration order is part of their contract.
            let id = FontId(font_resources.len() as u32);
            font_ids.insert((blob.id(), 0), id);
            font_resources.push(FontData::new(blob.clone(), 0));
        }
        let mut embedded = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        register_embedded(&mut embedded, &blobs);
        BaseFonts {
            embedded,
            blobs,
            font_resources,
            font_ids,
        }
    }
}

/// Registers the embedded faces in `collection` and maps the generic families
/// to them. The four sans faces share the family name "Liberation Sans", so
/// fontique folds them into one family and resolves bold/italic within it; the
/// two mono faces fold into "Liberation Mono" the same way. Nerd Fonts Symbols
/// follows it only as fallback, retaining stable terminal metrics.
fn register_embedded(collection: &mut Collection, blobs: &[Blob<u8>]) {
    let families: Vec<_> = blobs
        .iter()
        .map(|blob| {
            collection
                .register_fonts(blob.clone(), None)
                .first()
                .map(|(id, _)| *id)
                .expect("embedded face registered at least one family")
        })
        .collect();
    let family_of = |id: FontId| families[id.0 as usize];
    let family_id = family_of(EMBEDDED_FONT_ID);
    let mono_family_id = family_of(FontFace::Mono.font_id());
    let nerd_symbols_family_id = family_of(NERD_SYMBOLS_FONT_ID);
    let unicode_symbols_family_id = family_of(UNICODE_SYMBOLS_FONT_ID);

    // Point the generic families at the embedded fonts so any resolution
    // path (including the default "sans-serif") lands on them. Monospace
    // resolves to Liberation Mono — the code face rich text shapes with.
    for generic in [
        GenericFamily::SansSerif,
        GenericFamily::Serif,
        GenericFamily::SystemUi,
        GenericFamily::UiSansSerif,
    ] {
        collection.set_generic_families(generic, std::iter::once(family_id));
    }
    collection.set_generic_families(
        GenericFamily::Monospace,
        [
            mono_family_id,
            nerd_symbols_family_id,
            unicode_symbols_family_id,
        ]
        .into_iter(),
    );
}

/// The embedded faces plus the system fonts, which serve callers that name a
/// family and act as glyph fallback behind the embedded faces.
///
/// Building it scans the installed fonts (about 20 ms through fontconfig on a
/// desktop with many of them), and text the embedded faces cover never reads
/// it. So the first shaper starts the scan on a background thread instead of
/// running it, and a shaper asks for the collection only when it meets text
/// that needs it. That call waits for the scan if it is still running.
struct SystemFonts {
    collection: Collection,
}

impl SystemFonts {
    fn get() -> &'static SystemFonts {
        static SYSTEM: OnceLock<SystemFonts> = OnceLock::new();
        SYSTEM.get_or_init(|| {
            let mut collection = Collection::new(CollectionOptions {
                shared: false,
                system_fonts: true,
            });
            register_embedded(&mut collection, &BaseFonts::get().blobs);
            SystemFonts { collection }
        })
    }

    /// Starts the font scan in the background, once per process. Without
    /// threads (wasm), or if the thread cannot start, the first caller of
    /// [`SystemFonts::get`] runs the scan.
    fn prewarm() {
        #[cfg(not(target_arch = "wasm32"))]
        {
            static STARTED: std::sync::Once = std::sync::Once::new();
            STARTED.call_once(|| {
                let _ = std::thread::Builder::new()
                    .name("schnellui-fonts".into())
                    .spawn(|| {
                        SystemFonts::get();
                    });
            });
        }
    }

    fn context() -> FontContext {
        FontContext {
            collection: SystemFonts::get().collection.clone(),
            source_cache: SourceCache::default(),
        }
    }
}

/// Whether `ch` draws as a colour picture by default. The supplementary
/// pictograph blocks count as a whole; the BMP entries are the characters
/// Unicode gives the Emoji_Presentation property.
fn emoji_by_default(ch: char) -> bool {
    let cp = ch as u32;
    if cp < 0x231A {
        return false;
    }
    if cp > 0xFFFF {
        return (0x1F000..=0x1FAFF).contains(&cp);
    }
    const BMP: &[(u16, u16)] = &[
        (0x231A, 0x231B),
        (0x23E9, 0x23EC),
        (0x23F0, 0x23F0),
        (0x23F3, 0x23F3),
        (0x25FD, 0x25FE),
        (0x2614, 0x2615),
        (0x2648, 0x2653),
        (0x267F, 0x267F),
        (0x2693, 0x2693),
        (0x26A1, 0x26A1),
        (0x26AA, 0x26AB),
        (0x26BD, 0x26BE),
        (0x26C4, 0x26C5),
        (0x26CE, 0x26CE),
        (0x26D4, 0x26D4),
        (0x26EA, 0x26EA),
        (0x26F2, 0x26F3),
        (0x26F5, 0x26F5),
        (0x26FA, 0x26FA),
        (0x26FD, 0x26FD),
        (0x2705, 0x2705),
        (0x270A, 0x270B),
        (0x2728, 0x2728),
        (0x274C, 0x274C),
        (0x274E, 0x274E),
        (0x2753, 0x2755),
        (0x2757, 0x2757),
        (0x2795, 0x2797),
        (0x27B0, 0x27B0),
        (0x27BF, 0x27BF),
        (0x2B1B, 0x2B1C),
        (0x2B50, 0x2B50),
        (0x2B55, 0x2B55),
    ];
    let cp = cp as u16;
    BMP.iter().any(|&(lo, hi)| (lo..=hi).contains(&cp))
}

const VS16: char = '\u{FE0F}';
const ZWJ: char = '\u{200D}';
const KEYCAP: char = '\u{20E3}';

/// The next stretch of `text` at or after byte `from` that draws as emoji:
/// characters that are emoji by default, any character with the emoji
/// variation selector (U+FE0F) behind it, keycaps, and what joins onto them
/// (skin tones, joiners with their next character, tags). Neighbouring emoji
/// come back as one range. A digit, `#` or a heart without the selector stays
/// text.
pub(super) fn next_emoji_range(text: &str, from: usize) -> Option<std::ops::Range<usize>> {
    let rest = &text[from..];
    // Every emoji sequence holds a character of three or four UTF-8 bytes.
    if !rest.bytes().any(|b| b >= 0xE2) {
        return None;
    }
    let mut chars = rest.char_indices().peekable();
    let mut start = None;
    while let Some((at, ch)) = chars.next() {
        let next = chars.peek().map(|&(_, c)| c);
        let begins = emoji_by_default(ch)
            || (next == Some(VS16) && !ch.is_control() && ch != ' ')
            || (next == Some(KEYCAP) && (ch.is_ascii_digit() || ch == '#' || ch == '*'));
        if !begins {
            if let Some(start) = start {
                return Some(from + start..from + at);
            }
            continue;
        }
        start.get_or_insert(at);
        // What rides on the base character.
        while let Some(&(_, c)) = chars.peek() {
            let rides = c == VS16
                || c == KEYCAP
                || ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
                || ('\u{E0020}'..='\u{E007F}').contains(&c);
            if rides {
                chars.next();
            } else if c == ZWJ {
                chars.next();
                chars.next();
            } else {
                break;
            }
        }
    }
    start.map(|start| from + start..text.len())
}

impl TextShaper {
    /// Builds a shaper over a Fontique collection holding the embedded faces
    /// (deterministic, SOUL §7.3), mapped as every generic family so UI text
    /// resolves to Liberation Sans (Liberation Mono for monospace). The
    /// system fonts serve explicitly named families and glyph fallback for
    /// characters the embedded faces lack; they load in the background
    /// ([`SystemFonts`]).
    ///
    /// Both collections are built once per process and cloned: fontique keeps
    /// the system font index behind an `Arc`, so a clone shares the fontconfig
    /// scan instead of repeating it for every app, theme rebuild, or remount.
    pub fn new() -> TextShaper {
        FULL_BUILDS.with(|c| c.set(c.get() + 1));
        let base = BaseFonts::get();
        SystemFonts::prewarm();
        TextShaper {
            font_cx: None,
            embedded_cx: FontContext {
                collection: base.embedded.clone(),
                source_cache: SourceCache::default(),
            },
            layout_cx: LayoutContext::new(),
            scratch_layout: Layout::default(),
            scale_cx: ScaleContext::new(),
            scratch_image: Image::new(),
            glyph_cache: FxHashMap::default(),
            rasterized: 0,
            scratch_text: String::new(),
            scratch_cuts: Vec::new(),
            font_resources: base.font_resources.clone(),
            font_ids: base.font_ids.clone(),
            coverage: Default::default(),
            masks: Default::default(),
            narrowed: 0,
            full_stack_only: false,
        }
    }

    /// A shaper with no fonts, for a stand-in that is dropped without shaping.
    /// It skips the collection clone, so it is cheap. Shaping with it falls back
    /// to whatever parley does with an empty collection.
    #[doc(hidden)]
    pub fn detached() -> TextShaper {
        TextShaper {
            font_cx: Some(FontContext {
                collection: Collection::new(CollectionOptions {
                    shared: false,
                    system_fonts: false,
                }),
                source_cache: SourceCache::default(),
            }),
            embedded_cx: FontContext {
                collection: Collection::new(CollectionOptions {
                    shared: false,
                    system_fonts: false,
                }),
                source_cache: SourceCache::default(),
            },
            layout_cx: LayoutContext::new(),
            scratch_layout: Layout::default(),
            scale_cx: ScaleContext::new(),
            scratch_image: Image::new(),
            glyph_cache: FxHashMap::default(),
            rasterized: 0,
            scratch_text: String::new(),
            scratch_cuts: Vec::new(),
            font_resources: Vec::new(),
            font_ids: FxHashMap::default(),
            coverage: Default::default(),
            masks: Default::default(),
            narrowed: 0,
            full_stack_only: false,
        }
    }

    /// Number of full shapers ([`TextShaper::new`]) built on this thread; lets
    /// tests prove a path builds none.
    #[doc(hidden)]
    pub fn full_builds() -> usize {
        FULL_BUILDS.with(|c| c.get())
    }

    /// Returns installed font family names, including the bundled families.
    pub fn font_families(&mut self) -> Vec<String> {
        let mut families: Vec<String> = self
            .font_cx
            .get_or_insert_with(SystemFonts::context)
            .collection
            .family_names()
            .map(str::to_owned)
            .collect();
        families.sort_by_key(|family| family.to_lowercase());
        families.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        families
    }

    /// Returns installed families whose font metadata declares fixed-width metrics.
    pub fn monospace_font_families(&mut self) -> Vec<String> {
        let families = self.font_families();
        let font_cx = self.font_cx.get_or_insert_with(SystemFonts::context);
        families
            .into_iter()
            .filter(|family| {
                font_cx
                    .collection
                    .family_by_name(family)
                    .and_then(|family| family.default_font().cloned())
                    .and_then(|font| {
                        let index = font.index();
                        font.load(Some(&mut font_cx.source_cache))
                            .map(|blob| (blob, index))
                    })
                    .is_some_and(|(blob, index)| {
                        FontRef::from_index(blob.data(), index as usize).is_some_and(|font| {
                            font.metrics(&[]).is_monospace
                                && font.charmap().map('M') != 0
                                && font.charmap().map('0') != 0
                        })
                    })
            })
            .collect()
    }

    /// Shapes `text` at `size_px`, returning positioned glyphs + intrinsic size
    /// (SOUL §8.1). Runs through the pooled Parley context (warm ⇒ amortized
    /// zero-alloc). `max_width` (if given) is the wrap width in logical px.
    ///
    /// Back-compat thin wrapper: equivalent to [`TextShaper::shape_with`] with
    /// [`WrapMode::Word`] + [`TextAlign::Start`] (word wrap, long words broken), so
    /// existing callers (e.g. the widgets/chart glyph path) keep their behavior.
    pub fn shape(&mut self, text: &str, size_px: f32, max_width: Option<f32>) -> ShapedText {
        let mut out = ShapedText::default();
        self.shape_core(
            text,
            &ShapeOptions::new(size_px).max_width(max_width),
            None,
            &mut out,
        );
        out
    }

    /// Shapes `text` with explicit [`ShapeOptions`] (wrap mode + alignment +
    /// width), returning a [`ShapedText`] whose `lines` carry correct multi-line,
    /// aligned origins and whose `width`/`height` are the widest line / total
    /// height (SOUL §8.1). Warm re-shape of same/same-length text stays amortized
    /// zero-alloc through the pooled `LayoutContext`/`Layout` (§4.1).
    ///
    /// The returned value owns fresh glyph storage, which spills to the heap past
    /// 16 glyphs. Hot paths use [`shape_into`](Self::shape_into) instead.
    pub fn shape_with(&mut self, text: &str, opts: &ShapeOptions) -> ShapedText {
        let mut out = ShapedText::default();
        self.shape_core(text, opts, None, &mut out);
        out
    }

    /// [`shape_with`](Self::shape_with) into a caller-owned [`ShapedText`]. `out`
    /// is cleared first and keeps its glyph/line capacity, so a caller that holds
    /// one result per call site shapes without allocating once it has grown to
    /// the longest text it sees.
    pub fn shape_into(&mut self, text: &str, opts: &ShapeOptions, out: &mut ShapedText) {
        self.shape_core(text, opts, None, out);
    }

    /// Shapes with an explicitly selected installed font family.
    pub fn shape_with_family(
        &mut self,
        text: &str,
        opts: &ShapeOptions,
        family: &str,
    ) -> ShapedText {
        let mut out = ShapedText::default();
        self.shape_core(text, opts, Some(family), &mut out);
        out
    }

    /// Shapes everything with the full generic family stack, skipping the
    /// covered-text shortcut, so tests can compare the two.
    #[doc(hidden)]
    pub fn set_full_stack_only(&mut self, on: bool) {
        self.full_stack_only = on;
    }

    /// Whether `face` holds all of `text`, so the embedded faces alone shape
    /// it exactly as the full collection would ([`FaceCoverage`]).
    fn covered(&mut self, face: FontFace, text: &str) -> bool {
        if self.full_stack_only {
            return false;
        }
        match self.coverage(face) {
            CoverageSlot::Ready(coverage) => coverage.holds(text),
            _ => false,
        }
    }

    /// The coverage of `face`, resolved on first use.
    fn coverage(&mut self, face: FontFace) -> &mut CoverageSlot {
        let slot = &mut self.coverage[face as usize];
        if matches!(slot, CoverageSlot::Unresolved) {
            *slot = match FaceCoverage::resolve(&mut self.embedded_cx, face) {
                Some(coverage) => CoverageSlot::Ready(coverage),
                None => CoverageSlot::Unavailable,
            };
        }
        slot
    }

    /// Writes the short family stacks for `text` into [`SANS_SLOT`] and
    /// [`MONO_SLOT`] of the full collection. `faces` holds a bit per
    /// [`FontFace`] the text is shaped in; a slot gets the families that any
    /// face of its kind needs. False when the text has to take the full stack:
    /// it needs a family row that has not arrived (this requests it), or a
    /// character the short stack cannot stand in for.
    ///
    /// Only looks rows up, under one uncontended lock; the scans run on the
    /// row worker ([`FamilyRows`]).
    fn narrow_stacks(&mut self, text: &str, faces: u8) -> bool {
        if self.full_stack_only || !self.faces_ready(faces) {
            return false;
        }
        let stacks = FamilyStacks::get();
        if !stacks.usable {
            return false;
        }
        for (mono, mask) in [false, true].into_iter().zip(&mut self.masks) {
            let words = stacks.stack(mono).ids.len().div_ceil(64);
            if mask.len() != words {
                *mask = vec![0; words];
            }
        }
        let in_set = |face: usize| faces & (1 << face) != 0;
        let rows = FamilyRows::get();
        let mut state = rows.state.lock();
        let mut complete = true;
        for ch in text.chars() {
            let mut lacking = false;
            for (f, slot) in self.coverage.iter_mut().enumerate() {
                if let (true, CoverageSlot::Ready(coverage)) = (in_set(f), slot) {
                    lacking |= coverage.lacks(ch);
                }
            }
            if !lacking {
                continue;
            }
            if !normalization_inert(ch) {
                complete = false;
                break;
            }
            for f in (0..FACE_COUNT).filter(|&f| in_set(f)) {
                let key = (ch, f as u8);
                match state.rows.get(&key) {
                    Some(&PENDING) => complete = false,
                    Some(&at) => {
                        let mask = &mut self.masks[FONT_FACES[f].mono() as usize];
                        let row = &state.bits[at as usize..at as usize + mask.len()];
                        for (mask, bits) in mask.iter_mut().zip(row) {
                            *mask |= bits;
                        }
                    }
                    None => {
                        rows.request(&mut state, key);
                        complete = false;
                    }
                }
            }
        }
        drop(state);
        if complete {
            let font_cx = self.font_cx.get_or_insert_with(SystemFonts::context);
            for (mono, generic) in [(false, SANS_SLOT), (true, MONO_SLOT)] {
                if !(0..FACE_COUNT).any(|f| in_set(f) && FONT_FACES[f].mono() == mono) {
                    continue;
                }
                let stack = stacks.stack(mono);
                let mask = &self.masks[mono as usize];
                let keep = |at: usize| at < stack.embedded || mask[at / 64] & (1 << (at % 64)) != 0;
                let ids = stack.ids.iter().enumerate();
                font_cx.collection.set_generic_families(
                    generic,
                    ids.filter(|(at, _)| keep(*at)).map(|(_, id)| *id),
                );
            }
        }
        for mask in &mut self.masks {
            mask.fill(0);
        }
        complete
    }

    /// Whether every face in `faces` has its coverage resolved.
    fn faces_ready(&mut self, faces: u8) -> bool {
        (0..FACE_COUNT)
            .filter(|&f| faces & (1 << f) != 0)
            .all(|f| matches!(self.coverage(FONT_FACES[f]), CoverageSlot::Ready(_)))
    }

    /// Blocks until the family rows ([`FamilyRows`]) that `text` needs in any
    /// face have arrived, so the next shape of `text` can take the short
    /// stack. For tests; shaping itself never waits. Where there is no row
    /// worker it scans the rows on this thread.
    #[doc(hidden)]
    pub fn wait_for_family_rows(&mut self, text: &str) {
        let all = (1u8 << FACE_COUNT) - 1;
        if self.full_stack_only || !self.faces_ready(all) || !FamilyStacks::get().usable {
            return;
        }
        let rows = FamilyRows::get();
        let mut keys = Vec::new();
        for ch in text.chars() {
            for f in 0..FACE_COUNT {
                if let CoverageSlot::Ready(coverage) = &mut self.coverage[f] {
                    if coverage.lacks(ch) && normalization_inert(ch) {
                        keys.push((ch, f as u8));
                    }
                }
            }
        }
        let mut state = rows.state.lock();
        for &key in &keys {
            rows.request(&mut state, key);
        }
        if rows.requests.get().is_some_and(Option::is_none) {
            drop(state);
            let mut font_cx = SystemFonts::context();
            let mut row = Vec::new();
            for key in keys {
                rows.scan_into(key, &mut font_cx, &mut row);
            }
            return;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while keys.iter().any(|key| state.rows.get(key) == Some(&PENDING)) {
            let timed_out = rows.ready.wait_until(&mut state, deadline).timed_out();
            assert!(!timed_out, "family rows for {text:?} never arrived");
        }
    }

    /// Family rows scanned on the calling thread so far (see [`FamilyRows`]).
    #[doc(hidden)]
    pub fn family_row_scans_on_this_thread() -> usize {
        ROW_SCANS.with(|c| c.get())
    }

    /// Builds, breaks and aligns the pooled layout for `text`. `style` adds the
    /// font family and per-span properties; the size, wrap mode and alignment
    /// from `opts` are applied here, once, for every shaping entry point.
    /// `embedded` resolves fonts against the embedded faces alone; pass it only
    /// for text they cover ([`TextShaper::covered`]).
    fn layout_text(
        &mut self,
        text: &str,
        opts: &ShapeOptions,
        embedded: bool,
        base: GenericFamily,
        style: impl FnOnce(&mut RangedBuilder<'_, Brush>),
    ) {
        // Emoji take the system's colour emoji font, which the embedded faces
        // do not have.
        let has_emoji = next_emoji_range(text, 0).is_some();
        let emoji_stack = [
            FontFamilyName::Generic(GenericFamily::Emoji),
            FontFamilyName::Generic(base),
        ];
        // Split-borrow the pooled fields so the builder can hold `layout_cx` +
        // `font_cx` while `build_into` fills `scratch_layout` (§4.1 pooling).
        let font_cx = if embedded && !has_emoji {
            &mut self.embedded_cx
        } else {
            self.font_cx.get_or_insert_with(SystemFonts::context)
        };
        let layout_cx = &mut self.layout_cx;
        let layout = &mut self.scratch_layout;

        {
            // scale = 1.0 (logical == physical for deterministic shots, §7.3);
            // quantize = true rounds advances for stable, diffable output.
            let mut builder = layout_cx.ranged_builder(font_cx, text, 1.0, true);
            builder.push_default(StyleProperty::FontSize(opts.size_px));
            // `Word` is `overflow-wrap: break-word`: a word wider than the line
            // breaks inside the word instead of running past the edge, the way
            // a browser textarea does. `Anywhere` is the same break rule that
            // also counts for min-content sizing.
            match opts.wrap {
                WrapMode::NoWrap => {
                    builder.push_default(StyleProperty::TextWrapMode(TextWrapMode::NoWrap));
                }
                WrapMode::Word => {
                    builder.push_default(StyleProperty::OverflowWrap(OverflowWrap::BreakWord));
                }
                WrapMode::Anywhere => {
                    builder.push_default(StyleProperty::OverflowWrap(OverflowWrap::Anywhere));
                }
            }
            // Text that can flow onto several lines gets reading leading, so a
            // paragraph is not set as tight as a one-line label.
            if let Some(line_height) = opts.line_height {
                builder.push_default(StyleProperty::LineHeight(
                    parley::LineHeight::FontSizeRelative(line_height),
                ));
            } else if opts.max_width.is_some() && opts.wrap != WrapMode::NoWrap {
                builder.push_default(StyleProperty::LineHeight(
                    parley::LineHeight::FontSizeRelative(PARAGRAPH_LINE_HEIGHT),
                ));
            }
            style(&mut builder);
            if has_emoji {
                let mut at = 0;
                while let Some(range) = next_emoji_range(text, at) {
                    at = range.end;
                    builder.push(
                        StyleProperty::FontFamily(FontFamily::List(Cow::Borrowed(&emoji_stack))),
                        range,
                    );
                }
            }
            builder.build_into(layout, text);
        }

        layout.break_all_lines(opts.max_width);

        // `Start` leaves offsets at 0 (parley's post-break default), so skip the
        // extra pass — this also keeps `shape()` byte-identical to the pre-wrap
        // behavior. Other alignments shift each line's `metrics.offset`.
        let alignment = match opts.align {
            TextAlign::Start => return,
            TextAlign::Center => Alignment::Center,
            TextAlign::End => Alignment::End,
            TextAlign::Justify => Alignment::Justify,
        };
        layout.align(alignment, AlignmentOptions::default());
    }

    /// The shared shaping core (SOUL §8.1). All wrap/align config flows through
    /// here; [`shape`](Self::shape)/[`shape_with`](Self::shape_with)/
    /// [`truncate_to_width`](Self::truncate_to_width) are thin callers.
    fn shape_core(
        &mut self,
        text: &str,
        opts: &ShapeOptions,
        font_family: Option<&str>,
        out: &mut ShapedText,
    ) {
        let face = opts.face;
        let embedded = font_family.is_none() && self.covered(face, text);
        let narrow =
            font_family.is_none() && !embedded && self.narrow_stacks(text, 1 << face as u8);
        self.narrowed += narrow as u64;
        let base = match (font_family.is_some() || face.mono(), narrow) {
            (true, true) if font_family.is_none() => MONO_SLOT,
            (true, _) => GenericFamily::Monospace,
            (false, true) => SANS_SLOT,
            (false, false) => GenericFamily::SansSerif,
        };
        self.layout_text(text, opts, embedded, base, |builder| {
            // Borrowed names: no per-call Vec/String for a named family.
            let named;
            let family = if let Some(family) = font_family {
                named = [
                    FontFamilyName::Named(Cow::Borrowed(family)),
                    FontFamilyName::Generic(GenericFamily::Monospace),
                ];
                FontFamily::List(Cow::Borrowed(&named))
            } else if narrow {
                FontFamily::from(if face.mono() { MONO_SLOT } else { SANS_SLOT })
            } else if face.mono() {
                FontFamily::from(GenericFamily::Monospace)
            } else {
                FontFamily::from(GenericFamily::SansSerif)
            };
            builder.push_default(StyleProperty::FontFamily(family));
            if face.bold() {
                builder.push_default(StyleProperty::FontWeight(FontWeight::BOLD));
            }
            if face.italic() {
                builder.push_default(StyleProperty::FontStyle(FontStyle::Italic));
            }
        });
        let layout = &self.scratch_layout;

        out.clear();
        out.width = layout.width();
        out.height = layout.height();
        out.font = opts.face.font_id();
        let mut first_baseline: Option<f32> = None;
        for line in layout.lines() {
            let m = line.metrics();
            let glyph_start = out.glyphs.len() as u32;
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                // Single uniform style ⇒ each GlyphRun spans its whole (line-
                // scoped) Run, so walking the run's clusters (visual order) is
                // exact — and because the Run is scoped to this line, wrapping
                // never double-counts glyphs across lines. Cluster text ranges
                // attach the source byte index to every glyph.
                let run = glyph_run.run();
                let font =
                    font_id_for_data(run.font(), &mut self.font_ids, &mut self.font_resources);
                for cluster in run.visual_clusters() {
                    let cluster_byte = cluster.text_range().start as u32;
                    for g in cluster.glyphs() {
                        out.glyphs.push(ShapedGlyph {
                            glyph_id: g.id as u16,
                            font,
                            x_advance: g.advance,
                            x_offset: g.x,
                            y_offset: g.y,
                            cluster: cluster_byte,
                        });
                    }
                }
            }
            if first_baseline.is_none() {
                first_baseline = Some(m.baseline);
            }
            out.lines.push(ShapedLine {
                x: m.offset,
                baseline: m.baseline,
                top: m.block_min_coord,
                height: m.line_height,
                glyph_start,
                glyph_count: out.glyphs.len() as u32 - glyph_start,
            });
        }
        out.baseline = first_baseline.unwrap_or(0.0);
        if font_family.is_some() {
            if let Some(glyph) = out.glyphs.first() {
                out.font = glyph.font;
            }
        }
    }

    /// Shapes `text` with **per-span styles** — the rich-text path (SOUL §8.1).
    /// `spans` tile the string in order (each covering its next `len` bytes);
    /// any tail bytes not covered by a span shape in the option's default face.
    /// Faces map to real embedded weights/styles through fontique resolution;
    /// span colors ride parley's brush and come back per glyph; underline /
    /// strikethrough resolve against the run's font metrics into [`RichDecor`]s.
    ///
    /// Returned positions are **absolute** within the layout (line origin and
    /// [`TextAlign`] shift applied), so the caller emits quads by adding only
    /// its own paint origin. Runs through the same pooled `LayoutContext` /
    /// `Layout` as [`shape_with`](Self::shape_with) — warm re-shapes stay in
    /// the `text_edit` budget class (§4.1); the output `Vec`s are the caller's
    /// to cache.
    pub fn shape_spans(
        &mut self,
        text: &str,
        spans: &[SpanSpec],
        opts: &ShapeOptions,
    ) -> RichShapedText {
        // The embedded faces alone will do when each span's face holds its
        // text and the default face holds the rest.
        let mut embedded = true;
        let mut at = 0usize;
        for s in spans {
            let end = (at + s.len).min(text.len());
            embedded = text
                .get(at..end)
                .is_some_and(|part| self.covered(s.face, part));
            if !embedded {
                break;
            }
            at = end;
        }
        let embedded = embedded && self.covered(FontFace::Sans, &text[at..]);
        // Otherwise the short stacks will do when the text allows them. Each
        // gets the families for every character any of the faces lacks: a
        // cluster takes the face of its first character, so a character can
        // meet another span's stack.
        let faces = spans.iter().fold(1u8 << FontFace::Sans as u8, |faces, s| {
            faces | 1 << s.face as u8
        });
        let narrow = !embedded && self.narrow_stacks(text, faces);
        self.narrowed += narrow as u64;
        let base = if narrow {
            SANS_SLOT
        } else {
            GenericFamily::SansSerif
        };
        self.layout_text(text, opts, embedded, base, |builder| {
            let (sans, mono) = if narrow {
                (SANS_SLOT, MONO_SLOT)
            } else {
                (GenericFamily::SansSerif, GenericFamily::Monospace)
            };
            builder.push_default(StyleProperty::FontFamily(sans.into()));
            let mut at = 0usize;
            for s in spans {
                let end = (at + s.len).min(text.len());
                let range = at..end;
                at = end;
                if range.is_empty() {
                    continue;
                }
                if s.face.mono() {
                    builder.push(StyleProperty::FontFamily(mono.into()), range.clone());
                }
                if s.face.bold() {
                    builder.push(StyleProperty::FontWeight(FontWeight::BOLD), range.clone());
                }
                if s.face.italic() {
                    builder.push(StyleProperty::FontStyle(FontStyle::Italic), range.clone());
                }
                builder.push(
                    StyleProperty::Brush(Brush {
                        color: s.color,
                        tag: s.tag,
                    }),
                    range.clone(),
                );
                if s.underline {
                    builder.push(StyleProperty::Underline(true), range.clone());
                }
                if s.strikethrough {
                    builder.push(StyleProperty::Strikethrough(true), range.clone());
                }
            }
        });
        let layout = &self.scratch_layout;

        let mut out = RichShapedText {
            width: layout.width(),
            height: layout.height(),
            ..RichShapedText::default()
        };
        for line in layout.lines() {
            let m = line.metrics();
            let glyph_start = out.glyphs.len() as u32;
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let font =
                    font_id_for_data(run.font(), &mut self.font_ids, &mut self.font_resources);
                let style = glyph_run.style();
                let color = style.brush.color;
                let tag = style.brush.tag;
                let rm = run.metrics();
                // Decoration offsets are typographic (positive = up from the
                // baseline); screen y is down, so subtract.
                if let Some(d) = &style.underline {
                    out.decors.push(RichDecor {
                        x: glyph_run.offset(),
                        y: glyph_run.baseline() - d.offset.unwrap_or(rm.underline_offset),
                        width: glyph_run.advance(),
                        thickness: d.size.unwrap_or(rm.underline_size).max(0.5),
                        color: d.brush.color,
                        tag,
                    });
                }
                if let Some(d) = &style.strikethrough {
                    out.decors.push(RichDecor {
                        x: glyph_run.offset(),
                        y: glyph_run.baseline() - d.offset.unwrap_or(rm.strikethrough_offset),
                        width: glyph_run.advance(),
                        thickness: d.size.unwrap_or(rm.strikethrough_size).max(0.5),
                        color: d.brush.color,
                        tag,
                    });
                }
                for g in glyph_run.positioned_glyphs() {
                    out.glyphs.push(RichGlyph {
                        glyph_id: g.id as u16,
                        x: g.x,
                        y: g.y,
                        font,
                        color,
                        advance: g.advance,
                        tag,
                    });
                }
            }
            out.lines.push(ShapedLine {
                x: m.offset,
                baseline: m.baseline,
                top: m.block_min_coord,
                height: m.line_height,
                glyph_start,
                glyph_count: out.glyphs.len() as u32 - glyph_start,
            });
        }
        out
    }

    /// Single-line ellipsis truncation (SOUL §8.1). Shapes `text` on one line; if
    /// it fits `max_width` the full shape is returned unchanged, otherwise the
    /// longest prefix whose `prefix + "…"` fits `max_width` is shaped and returned
    /// (ending in the ellipsis glyph). Parley has no native ellipsis, so the cut is
    /// found by walking the full shape's cluster advances.
    pub fn truncate_to_width(&mut self, text: &str, size_px: f32, max_width: f32) -> ShapedText {
        let mut out = ShapedText::default();
        self.truncate_to_width_into(text, size_px, max_width, &mut out);
        out
    }

    /// [`truncate_to_width`](Self::truncate_to_width) into a caller-owned result.
    ///
    /// **Cost:** three shapes at most for any text length: the full text, the
    /// ellipsis alone, and the chosen `prefix + "…"`. The cut comes from summing
    /// the full shape's glyph advances at cluster boundaries instead of
    /// re-shaping prefixes. Kerning across the cut can make that sum a hair off,
    /// so the final shape is verified and the cut backs off one cluster at a time
    /// until it fits. Warm calls reuse the shaper's scratch buffers.
    pub fn truncate_to_width_into(
        &mut self,
        text: &str,
        size_px: f32,
        max_width: f32,
        out: &mut ShapedText,
    ) {
        self.truncate_to_width_face_into(text, size_px, max_width, FontFace::Sans, out);
    }

    /// [`truncate_to_width_into`](Self::truncate_to_width_into) in `face`, so a
    /// bold heading that is cut keeps its weight.
    pub fn truncate_to_width_face_into(
        &mut self,
        text: &str,
        size_px: f32,
        max_width: f32,
        face: FontFace,
        out: &mut ShapedText,
    ) {
        const ELLIPSIS: &str = "…";
        // Force a single line; alignment is irrelevant to truncation.
        let opts = ShapeOptions::new(size_px).wrap(WrapMode::NoWrap).face(face);

        self.shape_core(text, &opts, None, out);
        if text.is_empty() || out.width <= max_width {
            return;
        }

        // Cluster start offsets in logical order with the width of the text
        // before each. Glyphs come out in visual order, which runs backwards
        // through RTL runs, so sum advances per cluster byte offset and sort.
        let mut cuts = std::mem::take(&mut self.scratch_cuts);
        cuts.clear();
        cuts.extend(out.glyphs.iter().map(|g| (g.cluster, g.x_advance)));
        cuts.sort_unstable_by_key(|c| c.0);
        let (mut pen, mut w, mut r) = (0.0f32, 0usize, 0usize);
        while r < cuts.len() {
            let c = cuts[r].0;
            let mut adv = 0.0f32;
            while r < cuts.len() && cuts[r].0 == c {
                adv += cuts[r].1;
                r += 1;
            }
            cuts[w] = (c, pen);
            w += 1;
            pen += adv;
        }
        cuts.truncate(w);
        // A cut at byte 0 would be the ellipsis alone, handled below.
        cuts.retain(|c| c.0 != 0);

        // The ellipsis alone: its width is reserved, and it is the answer when
        // not even one character fits beside it.
        self.shape_core(ELLIPSIS, &opts, None, out);
        let budget = max_width - out.width;
        let mut fit = cuts.partition_point(|&(_, pen)| pen <= budget);

        // Verify with a real shape of `prefix + "…"`, backing off on a miss.
        let mut buf = std::mem::take(&mut self.scratch_text);
        let mut result_fits = false;
        while fit > 0 {
            let cut = cuts[fit - 1].0 as usize;
            buf.clear();
            buf.push_str(&text[..cut]);
            buf.push_str(ELLIPSIS);
            self.shape_core(&buf, &opts, None, out);
            if out.width <= max_width {
                result_fits = true;
                break;
            }
            fit -= 1;
        }
        if !result_fits {
            self.shape_core(ELLIPSIS, &opts, None, out);
        }
        self.scratch_text = buf;
        self.scratch_cuts = cuts;
    }

    /// Measures `text` at `size_px` under an optional width constraint, returning
    /// only the intrinsic `(width, height)` handed up to layout (SOUL §8.1).
    pub fn measure(&mut self, text: &str, size_px: f32, max_width: Option<f32>) -> (f32, f32) {
        let s = self.shape(text, size_px, max_width);
        (s.width, s.height)
    }

    /// Rasterizes one glyph via swash into `atlas`, marking the written sub-rect
    /// dirty (SOUL §8.1, §3.2), and returns the full [`RasterGlyph`] (atlas rect +
    /// placement bearing) the renderer needs to position the quad. A repeated key
    /// is a cache hit (idempotent, no re-write, §4.1).
    ///
    /// A full atlas grows ([`GlyphAtlas::grow`]) and the glyph is placed again. If
    /// the atlas cannot grow, the glyph comes back with an empty rect and is *not*
    /// cached, so it draws as soon as an atlas with room is used.
    pub fn rasterize_glyph(&mut self, key: GlyphKey, atlas: &mut GlyphAtlas) -> RasterGlyph {
        if let Some(g) = self.glyph_cache.get(&key) {
            return *g;
        }
        let (g, placed) = self.raster_glyph_into(key, atlas, true);
        // An atlas at its cap reports the placement but caches nothing, so
        // the glyph is not lost for good.
        if placed {
            self.rasterized += 1;
            self.glyph_cache.insert(key, g);
        }
        g
    }

    /// Rasterizes one glyph into `atlas` without reading or filling the glyph
    /// cache. For a throwaway atlas, such as an SVG raster's scratch atlas:
    /// a cached rect would point into it after it is gone, and the next
    /// lookup, from the app's own atlas, would read someone else's texels.
    pub fn rasterize_glyph_uncached(
        &mut self,
        key: GlyphKey,
        atlas: &mut GlyphAtlas,
    ) -> RasterGlyph {
        // Coverage only: the caller reads the scratch atlas's R8 texels, and a colour glyph
        // has none there.
        self.raster_glyph_into(key, atlas, false).0
    }

    /// The glyph rasterized into `atlas`, and whether it found a place there
    /// (false when the atlas is full and cannot grow).
    ///
    /// With `allow_color`, a glyph that has a colour picture in its font (an emoji in a
    /// colour bitmap or layered colour outline font) goes into the atlas's colour plane as
    /// RGBA. swash picks the nearest bitmap strike and scales it to the text size. Every
    /// other glyph takes the coverage path as before.
    fn raster_glyph_into(
        &mut self,
        key: GlyphKey,
        atlas: &mut GlyphAtlas,
        allow_color: bool,
    ) -> (RasterGlyph, bool) {
        // The key's FontId selects the embedded face's bytes (index 0 — each
        // Liberation face is a single-font file). If a face ever fails to parse
        // we cache an empty glyph so we don't retry every frame.
        let resource = self
            .font_resources
            .get(key.font.0 as usize)
            .unwrap_or(&self.font_resources[EMBEDDED_FONT_ID.0 as usize]);
        let Some(font) = FontRef::from_index(resource.data.data(), resource.index as usize) else {
            return (RasterGlyph::default(), true);
        };

        let img = &mut self.scratch_image;
        img.clear();

        // Deterministic: outline source, no hinting, 8-bit alpha (no subpixel AA).
        let mut scaler = self
            .scale_cx
            .builder(font)
            .size(key.size_px as f32)
            .hint(false)
            .build();
        // Colour first, so an emoji font that also carries plain outlines draws in colour.
        // A font with no colour tables skips both colour sources at no cost.
        let color_sources = [
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ];
        let plain_sources = [Source::Outline];
        let sources: &[Source] = if allow_color {
            &color_sources
        } else {
            &plain_sources
        };
        let mut render = Render::new(sources);
        render.format(Format::Alpha);
        let ok = render.render_into(&mut scaler, key.glyph_id, img);

        if ok && img.content == Content::Color {
            let (w, h) = (img.placement.width, img.placement.height);
            if w == 0 || h == 0 || img.data.len() < (w as usize) * (h as usize) * 4 {
                return (RasterGlyph::default(), true);
            }
            let unplaced = RasterGlyph {
                rect: AtlasRect::default(),
                left: img.placement.left,
                top: img.placement.top,
                width: w,
                height: h,
                color: true,
            };
            let Some(rect) = atlas.allocate_color(w, h) else {
                return (unplaced, false);
            };
            atlas.write_color(rect, &img.data);
            return (RasterGlyph { rect, ..unplaced }, true);
        }

        let g = if ok && !img.data.is_empty() {
            let w = img.placement.width;
            let h = img.placement.height;
            // Growing keeps placed texels in place, so retrying is sound.
            let mut slot = atlas.allocate(w, h);
            while slot.is_none() && atlas.can_ever_fit(w, h) && atlas.grow() {
                slot = atlas.allocate(w, h);
            }
            let Some(rect) = slot else {
                return (
                    RasterGlyph {
                        rect: AtlasRect::default(),
                        left: img.placement.left,
                        top: img.placement.top,
                        width: w,
                        height: h,
                        color: false,
                    },
                    false,
                );
            };
            atlas.write_coverage(rect, &img.data);
            RasterGlyph {
                rect,
                left: img.placement.left,
                top: img.placement.top,
                width: w,
                height: h,
                color: false,
            }
        } else {
            // Empty glyph (e.g. space): no coverage, zero-size rect, no atlas write.
            RasterGlyph {
                rect: AtlasRect::default(),
                left: img.placement.left,
                top: img.placement.top,
                width: 0,
                height: 0,
                color: false,
            }
        };
        (g, true)
    }

    /// A new shaper sharing the font setup with a copy of this one's glyph
    /// cache, valid against a clone of the atlas this shaper writes into.
    pub fn fork(&self) -> TextShaper {
        let mut forked = TextShaper::new();
        forked.glyph_cache = self.glyph_cache.clone();
        // Cached GlyphKeys name faces by FontId, a per-shaper index handed out in
        // first-seen order. The fork must resolve ids exactly as the donor did, or
        // a face it meets later reuses an id the donor gave another face and hits
        // that face's cached bitmap.
        forked.font_ids = self.font_ids.clone();
        // Share the loaded font blobs too, so a face the donor already loaded keeps
        // its blob identity (and thus its FontId and cached glyphs) in the fork.
        forked.font_cx = self.font_cx.as_ref().map(|font_cx| FontContext {
            collection: font_cx.collection.clone(),
            source_cache: font_cx.source_cache.clone(),
        });
        forked.font_resources = self.font_resources.clone();
        forked.rasterized = self.rasterized;
        forked
    }

    /// How many glyphs this shaper has rasterized (glyph cache misses). A
    /// shaper carried across a rebuild keeps its count, so a delta of zero
    /// proves the cache was reused.
    pub fn rasterized_glyph_count(&self) -> u64 {
        self.rasterized
    }

    /// The [`GlyphKey`] for `glyph_id` at `size_px` in `font`, with the
    /// deterministic subpixel bucket (0). The face comes from a
    /// [`ShapedGlyph::font`] / [`RichGlyph::font`].
    pub fn glyph_key_in(&self, font: FontId, glyph_id: u16, size_px: u32) -> GlyphKey {
        GlyphKey {
            font,
            glyph_id,
            size_px,
            subpixel: 0,
        }
    }
}
