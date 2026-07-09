//! # Rich text (SOUL §8.1)
//!
//! A formatted-document surface: the [`RichDoc`] model (blocks of styled
//! spans) and the [`RichText`] widget that renders it through the same
//! deferred, width-aware paint discipline as wrapping text (measure during
//! Taffy's pass, emit after layout, skip when nothing changed — SOUL §1,
//! §8.1). **Format importers (Markdown, OpenDocument, syntax highlighting)
//! are the implementor's responsibility for now** — application code maps its
//! format onto the model; the framework renders it.
//!
//! The editing counterpart is [`TextArea`](crate::TextArea) (multi-line source
//! editor with a pluggable highlight hook); pair the two for an
//! edit-and-preview surface.

mod doc;
mod view;

pub use doc::{ListMarker, RichBlock, RichDoc, RichSpan, SpanStyle};
pub use view::RichText;

pub(crate) use view::{
    emit_rich_paint, measure_rich, poll_dynamic_source, push_rich_glyphs, RichState,
};
