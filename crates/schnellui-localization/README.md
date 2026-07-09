# schnellui-localization

Renderer-independent localization primitives for SchnellUI applications. The
crate owns locale parsing and negotiation, fallback catalogs, and named message
formatting; applications continue to own their wording.

The umbrella crate re-exports this crate as `schnellui::localization`:

```rust
use schnellui::localization::{Catalog, Locale};

let en = Locale::parse("en").unwrap();
let de = Locale::parse("de").unwrap();
let catalog = Catalog::new(en.clone())
    .with_message(en, "welcome", "Welcome, {name}!")
    .with_message(de.clone(), "welcome", "Willkommen, {name}!");

let messages = catalog.localizer(&[de]);
assert_eq!(
    messages.format("welcome", &[("name", &"Ada")]),
    "Willkommen, Ada!"
);
```

Locale negotiation prefers an exact locale, then a language-neutral locale,
then a deterministic regional match, and finally the catalog fallback. Missing
messages fall back independently, so a partially translated locale remains
usable while making missing message identifiers visible during development.

Language-pack extensions should be merged with `Catalog::extend_missing`. The
fill-only operation lets an extension contribute a new locale or missing
messages without replacing host-owned translations.
