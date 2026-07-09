//! # schnellui-macro
//!
//! The `view!` proc-macro (SOUL §3.3). It parses via [`schnellui_view_parser`]
//! (a separate crate, for tooling/hot-reload), then **codegens a statically-typed
//! builder chain** with a compile-time static/dynamic split — *not* sugar over a
//! runtime interpreter (SOUL §3.3, Directive #4).
//!
//! - Static subtrees → hoisted to a `const` / built once, never traversed on update.
//! - Dynamic sites (`(count)`, `move || …`) → a `RenderEffect`-equivalent that runs
//!   once on first render, carries its previous value, and mutates the existing
//!   retained node in place on later runs.
//!
//! A `proc-macro` crate can only export macros, so the codegen itself lives in the
//! parser crate ([`schnellui_view_parser::Codegen`]); this crate is the thin
//! `#[proc_macro]` shim (SOUL §3.3).

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use schnellui_view_parser::{parse_view, Codegen, RenderMode};

/// `view! { … }` — the entry macro. Expands to a `schnellui-widgets` builder chain
/// whose static skeleton is hoisted and whose dynamic slots are reactive (§3.3).
#[proc_macro]
pub fn view(input: TokenStream) -> TokenStream {
    let input2: TokenStream2 = input.into();
    match parse_view(input2) {
        Ok(tree) => Codegen::new(RenderMode::Native).emit(&tree).into(),
        Err(e) => e.to_compile_error().into(),
    }
}
