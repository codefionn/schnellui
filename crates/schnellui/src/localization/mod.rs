//! Small, renderer-independent localization primitives for schnellui applications.
//!
//! Applications own their wording and pass translated strings to widgets. This
//! crate supplies locale parsing and negotiation, fallback catalogs, and named
//! placeholder interpolation without imposing a translation file format.

use std::borrow::Borrow;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fmt::{self, Write as _};

/// A normalized BCP-47-style locale identifier such as `en` or `de-DE`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Locale(String);

impl Locale {
    /// Parses and normalizes a locale identifier. POSIX suffixes such as
    /// `.UTF-8` and `@euro`, and `_` separators, are accepted.
    pub fn parse(value: impl AsRef<str>) -> Option<Self> {
        let value = value.as_ref().trim();
        let value = value.split(['.', '@']).next().unwrap_or(value);
        if value.is_empty()
            || value.eq_ignore_ascii_case("c")
            || value.eq_ignore_ascii_case("posix")
        {
            return None;
        }
        let parts: Vec<_> = value.split(['-', '_']).collect();
        if parts.is_empty()
            || parts.iter().any(|part| {
                part.is_empty()
                    || part.len() > 8
                    || !part.bytes().all(|byte| byte.is_ascii_alphanumeric())
            })
            || !(2..=8).contains(&parts[0].len())
            || !parts[0].bytes().all(|byte| byte.is_ascii_alphabetic())
        {
            return None;
        }
        let normalized = parts
            .iter()
            .enumerate()
            .map(|(index, part)| {
                if index == 0 {
                    part.to_ascii_lowercase()
                } else if part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                    part.to_ascii_uppercase()
                } else if part.len() == 4 && part.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                    let mut chars = part.chars();
                    let first = chars.next().unwrap().to_ascii_uppercase();
                    format!("{first}{}", chars.as_str().to_ascii_lowercase())
                } else {
                    part.to_ascii_lowercase()
                }
            })
            .collect::<Vec<_>>()
            .join("-");
        Some(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The language-only parent used for fallback (`de-DE` becomes `de`).
    pub fn language(&self) -> &str {
        self.0.split('-').next().unwrap_or(&self.0)
    }

    /// Finds the best supported locale, preferring exact, language-neutral,
    /// regional language, and finally fallback matches.
    pub fn negotiate(requested: &[Locale], supported: &[Locale], fallback: &Locale) -> Locale {
        for request in requested {
            if let Some(locale) = supported.iter().find(|locale| *locale == request) {
                return locale.clone();
            }
            // Prefer a language-neutral catalog over an arbitrary regional
            // sibling (for example, `de` before `de-DE` for `de-AT`).
            if let Some(locale) = supported
                .iter()
                .find(|locale| locale.as_str() == request.language())
            {
                return locale.clone();
            }
            if let Some(locale) = supported
                .iter()
                .find(|locale| locale.language() == request.language())
            {
                return locale.clone();
            }
        }
        fallback.clone()
    }

    /// Reads the conventional locale environment variables in priority order.
    pub fn from_environment() -> Option<Self> {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(|name| env::var(name).ok().and_then(Self::parse))
    }
}

// Lets the catalog look up a language-only key by `&str` without allocating.
// Derived `Ord`/`Eq` on the inner `String` agree with those of `str`.
impl Borrow<str> for Locale {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Locale {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// An application-owned collection of messages with a required fallback locale.
#[derive(Clone, Debug)]
pub struct Catalog {
    fallback: Locale,
    // Locale iteration participates in language fallback. Sorted keys keep
    // regional fallback stable across runs and insertion orders.
    messages: BTreeMap<Locale, HashMap<String, String>>,
    // Language to the catalog locales sharing it, in the same sorted order as
    // `messages`. Built on insertion so a message miss or locale negotiation
    // never scans every locale.
    by_language: HashMap<String, Vec<Locale>>,
}

impl Catalog {
    pub fn new(fallback: Locale) -> Self {
        Self {
            fallback,
            messages: BTreeMap::new(),
            by_language: HashMap::new(),
        }
    }

    /// Returns the message table for `locale`, registering the locale in the
    /// language index the first time it is seen.
    fn table_mut(&mut self, locale: Locale) -> &mut HashMap<String, String> {
        if !self.messages.contains_key(&locale) {
            let siblings = self
                .by_language
                .entry(locale.language().to_owned())
                .or_default();
            let at = siblings.partition_point(|existing| *existing < locale);
            siblings.insert(at, locale.clone());
        }
        self.messages.entry(locale).or_default()
    }

    /// Adds or replaces one message and returns the catalog for fluent setup.
    pub fn with_message(
        mut self,
        locale: Locale,
        id: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        self.insert(locale, id, message);
        self
    }

    pub fn insert(&mut self, locale: Locale, id: impl Into<String>, message: impl Into<String>) {
        self.table_mut(locale).insert(id.into(), message.into());
    }

    /// Adds a message only when that locale does not already define the id.
    ///
    /// Hosts can use this when merging extension-provided language packs: core
    /// translations remain authoritative while extensions fill missing entries.
    pub fn insert_if_missing(
        &mut self,
        locale: Locale,
        id: impl Into<String>,
        message: impl Into<String>,
    ) -> bool {
        use std::collections::hash_map::Entry;

        match self.table_mut(locale).entry(id.into()) {
            Entry::Vacant(entry) => {
                entry.insert(message.into());
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    /// Fill several missing messages for one locale, returning how many were
    /// accepted. Iteration order does not affect existing core translations.
    pub fn extend_missing<I, K, V>(&mut self, locale: Locale, messages: I) -> usize
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        messages.into_iter().fold(0, |inserted, (id, message)| {
            inserted
                + usize::from(self.insert_if_missing(locale.clone(), id.into(), message.into()))
        })
    }

    pub fn supported_locales(&self) -> impl Iterator<Item = &Locale> {
        self.messages.keys()
    }

    pub fn fallback_locale(&self) -> &Locale {
        &self.fallback
    }

    /// Resolves the active locale without constructing a localizer.
    pub fn resolve_locale(&self, requested: &[Locale]) -> Locale {
        // Same order as `Locale::negotiate`: exact, language-neutral, first
        // regional sibling, then the fallback. Uses the index, no scan or Vec.
        for request in requested {
            if let Some((locale, _)) = self.messages.get_key_value(request) {
                return locale.clone();
            }
            if let Some((locale, _)) = self.messages.get_key_value(request.language()) {
                return locale.clone();
            }
            if let Some(locale) = self
                .by_language
                .get(request.language())
                .and_then(|siblings| siblings.first())
            {
                return locale.clone();
            }
        }
        self.fallback.clone()
    }

    pub fn localizer(&self, requested: &[Locale]) -> Localizer<'_> {
        Localizer {
            catalog: self,
            locale: self.resolve_locale(requested),
        }
    }

    fn message(&self, locale: &Locale, id: &str) -> Option<&str> {
        self.messages
            .get(locale)
            .and_then(|messages| messages.get(id).map(String::as_str))
            .or_else(|| {
                self.by_language
                    .get(locale.language())?
                    .iter()
                    .find_map(|candidate| self.messages.get(candidate)?.get(id).map(String::as_str))
            })
            .or_else(|| {
                self.messages
                    .get(&self.fallback)?
                    .get(id)
                    .map(String::as_str)
            })
    }
}

/// A catalog view resolved to one active locale.
#[derive(Clone, Debug)]
pub struct Localizer<'a> {
    catalog: &'a Catalog,
    locale: Locale,
}

impl<'a> Localizer<'a> {
    pub fn locale(&self) -> &Locale {
        &self.locale
    }

    /// Resolves a message. Missing identifiers remain visible as their id.
    pub fn text(&self, id: &str) -> Cow<'a, str> {
        self.catalog
            .message(&self.locale, id)
            .map_or_else(|| Cow::Owned(id.to_owned()), Cow::Borrowed)
    }

    /// Resolves a message and replaces `{name}` placeholders. Unknown placeholders
    /// are preserved, which makes incomplete argument sets easy to diagnose.
    /// Values are inserted in one pass and are never interpreted as templates.
    /// Double braces (`{{` and `}}`) produce literal braces.
    pub fn format(&self, id: &str, arguments: &[(&str, &dyn fmt::Display)]) -> String {
        // Reserve a little per argument so short values do not regrow the
        // output; the output string is the only allocation.
        // The message is resolved once and shared with the writer.
        let template = self.catalog.message(&self.locale, id).unwrap_or(id);
        let mut result = String::with_capacity(template.len() + 16 * arguments.len());
        write_template(&mut result, template, arguments);
        result
    }

    /// Like [`Localizer::format`] but appends to `out`, so a caller can reuse
    /// one buffer across calls. Argument values are written straight into `out`
    /// and the (tiny) argument list is scanned linearly; the last argument with
    /// a matching name wins.
    pub fn format_into(&self, out: &mut String, id: &str, arguments: &[(&str, &dyn fmt::Display)]) {
        let template = self.catalog.message(&self.locale, id).unwrap_or(id);
        write_template(out, template, arguments);
    }
}

/// Appends `template` to `result` with `{name}` placeholders filled from
/// `arguments` and doubled braces collapsed.
fn write_template(result: &mut String, template: &str, arguments: &[(&str, &dyn fmt::Display)]) {
    let bytes = template.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'{' {
            if bytes.get(index + 1) == Some(&b'{') {
                result.push('{');
                index += 2;
                continue;
            }
            if let Some(end_offset) = bytes[index + 1..].iter().position(|byte| *byte == b'}') {
                let end = index + 1 + end_offset;
                let name = &template[index + 1..end];
                if let Some((_, value)) = arguments.iter().rev().find(|(key, _)| *key == name) {
                    let _ = write!(result, "{value}");
                } else {
                    result.push_str(&template[index..=end]);
                }
                index = end + 1;
                continue;
            }
        } else if bytes[index] == b'}' && bytes.get(index + 1) == Some(&b'}') {
            result.push('}');
            index += 2;
            continue;
        }

        let character = template[index..]
            .chars()
            .next()
            .expect("index remains on a UTF-8 character boundary");
        result.push(character);
        index += character.len_utf8();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counts allocations made by the current thread, per-thread so parallel tests
    /// do not disturb each other. The counting `#[global_allocator]` behind this is
    /// the one `allocation_counter` installs on link.
    fn allocations_during<R>(f: impl FnOnce() -> R) -> (usize, R) {
        let mut out = None;
        let info = allocation_counter::measure(|| out = Some(f()));
        (info.count_total as usize, out.unwrap())
    }

    #[test]
    fn format_with_three_arguments_allocates_only_the_output() {
        let catalog = Catalog::new(locale("en")).with_message(
            locale("en"),
            "row",
            "{a} owes {b} {c} credits",
        );
        let localizer = catalog.localizer(&[locale("en")]);
        let args: [(&str, &dyn fmt::Display); 3] = [("a", &"Ada"), ("b", &"Bob"), ("c", &42u32)];
        let (allocs, text) = allocations_during(|| localizer.format("row", &args));
        assert_eq!(text, "Ada owes Bob 42 credits");
        assert_eq!(allocs, 1);

        // A reused buffer needs none once it has capacity.
        let mut buffer = String::with_capacity(64);
        let (allocs, ()) = allocations_during(|| localizer.format_into(&mut buffer, "row", &args));
        assert_eq!(allocs, 0);
        assert_eq!(buffer, "Ada owes Bob 42 credits");
    }

    #[test]
    fn regional_miss_uses_the_language_index() {
        let mut catalog = Catalog::new(locale("en")).with_message(locale("en"), "k", "fallback");
        catalog.insert(locale("de-DE"), "k", "Hallo");
        catalog.insert(locale("de-CH"), "other", "x");
        // de-AT has no table; siblings are tried in sorted order (de-CH, de-DE).
        let localizer = catalog.localizer(&[locale("de-AT")]);
        assert_eq!(localizer.locale(), &locale("de-CH"));
        assert_eq!(localizer.text("k"), "Hallo");
        assert_eq!(localizer.text("nothing"), "nothing");
    }

    fn locale(value: &str) -> Locale {
        Locale::parse(value).unwrap()
    }

    #[test]
    fn parses_posix_and_normalizes_tags() {
        assert_eq!(locale("de_DE.UTF-8").as_str(), "de-DE");
        assert_eq!(locale("zh-hans-cn").as_str(), "zh-Hans-CN");
        assert!(Locale::parse("C").is_none());
        assert!(Locale::parse("not valid").is_none());
    }

    #[test]
    fn negotiates_exact_then_language_then_fallback() {
        let supported = [locale("en"), locale("de")];
        assert_eq!(
            Locale::negotiate(&[locale("de-AT")], &supported, &supported[0]),
            supported[1]
        );
        assert_eq!(
            Locale::negotiate(&[locale("fr")], &supported, &supported[0]),
            supported[0]
        );
    }

    #[test]
    fn negotiation_prefers_language_neutral_catalog_and_is_deterministic() {
        let fallback = locale("en");
        let supported = [locale("de-DE"), locale("de"), locale("de-CH")];
        assert_eq!(
            Locale::negotiate(&[locale("de-AT")], &supported, &fallback),
            locale("de")
        );

        let mut catalog = Catalog::new(fallback);
        catalog.insert(locale("de-CH"), "save", "Sichere");
        catalog.insert(locale("de-DE"), "save", "Speichern");
        assert_eq!(catalog.resolve_locale(&[locale("de-AT")]), locale("de-CH"));
    }

    #[test]
    fn translates_formats_and_falls_back_per_message() {
        let catalog = Catalog::new(locale("en"))
            .with_message(locale("en"), "greeting", "Hello, {name}!")
            .with_message(locale("en"), "save", "Save")
            .with_message(locale("de"), "greeting", "Hallo, {name}!");
        let localizer = catalog.localizer(&[locale("de-DE")]);
        assert_eq!(localizer.locale(), &locale("de"));
        assert_eq!(
            localizer.format("greeting", &[("name", &"Ada")]),
            "Hallo, Ada!"
        );
        assert_eq!(localizer.text("save"), "Save");
        assert_eq!(localizer.text("missing.id"), "missing.id");
    }

    #[test]
    fn formatting_is_single_pass_and_supports_literal_braces() {
        let catalog = Catalog::new(locale("en")).with_message(
            locale("en"),
            "summary",
            "{{user}} {name}: {value}; {missing}",
        );
        let localizer = catalog.localizer(&[locale("en")]);
        assert_eq!(
            localizer.format("summary", &[("name", &"Ada"), ("value", &"{name}")]),
            "{user} Ada: {name}; {missing}"
        );
    }

    #[test]
    fn extension_messages_fill_but_never_replace_host_messages() {
        let en = locale("en");
        let fr = locale("fr");
        let mut catalog = Catalog::new(en.clone()).with_message(en.clone(), "settings", "Settings");

        assert_eq!(
            catalog.extend_missing(
                en.clone(),
                [("settings", "Extension settings"), ("notes", "Notes")],
            ),
            1
        );
        assert_eq!(
            catalog.extend_missing(fr.clone(), [("settings", "Paramètres"), ("notes", "Notes")],),
            2
        );
        assert_eq!(catalog.localizer(&[en]).text("settings"), "Settings");
        assert_eq!(catalog.localizer(&[fr]).text("settings"), "Paramètres");
    }
}
