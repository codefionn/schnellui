//! Property-based fuzzing for schnellui. Not published.
//!
//! Every property drives a real app through
//! [`HeadlessHost`](schnellui::testing::fuzz::HeadlessHost), which runs the
//! windowed host's own pointer, key, wheel and resize handlers without a
//! window. proptest generates the input and shrinks a failure to the shortest
//! sequence that still fails.
//!
//! These oracles judge a run:
//!
//! - Fresh mount. A window resized to its final size, by one jump or by a
//!   gradual drag, paints and reads (a11y) like an app mounted at that size.
//!   Components with state are first driven to a fixed canonical state on
//!   both sides, so any input history has to end where a fresh app ends.
//! - Undecorated twin. A second host wears the same theme with every hover,
//!   press and focus decoration removed and receives the same input. Their
//!   accessibility trees match after every step, and once the pointer is gone
//!   and focus is cleared their paint matches too.
//! - Still twin. An animated host and a reduced-motion host get the same
//!   input, with waits, bursts of events without a frame between them, and
//!   theme fades mixed in. Their trees match step by step until the first
//!   theme change; every animation frame passes the retained-state checks;
//!   no animation runs past ten seconds; both end painting the same.
//! - Pace. One input sequence delivered as a single burst, one event every
//!   400 ms, and a seeded mix of the two ends in the same state.
//! - Retained-state checks from `schnellui::testing::fuzz::invariants`, run
//!   after every step: decoration records match their paint, no decoration is
//!   sized for paint that moved, and tooltips and floating layers stay inside
//!   the window.
//!
//! The signal graph is process-global, so every property takes [`serial`]
//! first. `SCHNELLUI_FUZZ_CASES` sets the number of cases per property, and
//! `PROPTEST_RNG_SEED` replays a run:
//!
//! ```sh
//! SCHNELLUI_FUZZ_CASES=256 cargo test -p schnellui-fuzz -- --test-threads=1
//! cargo test -p schnellui-fuzz --test resize dropdown:: -- --test-threads=1
//! ```
//!
//! CI does not run this crate. It is for hunting, and a failing property
//! prints the shrunk input that reproduces it.

pub mod components;
pub mod curve;
pub mod ops;
pub mod oracle;

use std::sync::{Mutex, MutexGuard};

use proptest::test_runner::Config;
use schnellui::widgets::{ComponentInteractions, InteractionStates, InteractionStyle, Theme};

pub use schnellui::testing::fuzz::{ClipboardAction, HeadlessHost, HostKey, PointerButton};

static SERIAL: Mutex<()> = Mutex::new(());

/// Holds the process-wide lock that keeps two apps from sharing the signal
/// graph at once. A panicking case poisons nothing worth keeping.
pub fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The proptest configuration every property uses: `SCHNELLUI_FUZZ_CASES`
/// cases (default `default_cases`), no regression files written into the
/// source tree, and a generous shrink budget.
pub fn config(default_cases: u32) -> Config {
    let cases = std::env::var("SCHNELLUI_FUZZ_CASES")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(default_cases);
    Config {
        cases,
        failure_persistence: None,
        max_shrink_iters: 4096,
        ..Config::default()
    }
}

/// A theme that uses every decoration channel in every state, so washes,
/// recolours and strokes stack on every widget family at once. Its page is a
/// shade off the default's, so that with decorations stripped it still
/// differs from LIGHT and a switch between the two rebuilds both twins.
pub const EVERY: Theme = Theme {
    page: schnellui::scene::Color::rgb(0xf3, 0xf4, 0xf7),
    interactions: all_channels(10),
    component_interactions: ComponentInteractions {
        button: Some(all_channels(70)),
        navigation: Some(all_channels(130)),
        toggle: None,
        editable: Some(all_channels(190)),
        raw_surface: Some(all_channels(200)),
    },
    ..schnellui::widgets::DEFAULT_THEME
};

const fn all_channels(base: u8) -> InteractionStates {
    const fn style(r: u8) -> InteractionStyle {
        InteractionStyle {
            background: Some(schnellui::scene::Color::rgba(r, 40, 90, 0x50)),
            foreground: Some(schnellui::scene::Color::rgb(r, 10, 200)),
            border: Some(schnellui::scene::Color::rgb(r, 120, 30)),
        }
    }
    InteractionStates {
        hover: style(base),
        focus: style(base + 20),
        active: style(base + 40),
    }
}

/// The built-in themes plus [`EVERY`].
pub const THEMES: [Theme; 6] = [
    EVERY,
    schnellui::theme::LIGHT,
    schnellui::theme::DARK,
    schnellui::theme::FOREST,
    schnellui::theme::BRUTAL,
    schnellui::theme::CANDY,
];

/// How many of [`THEMES`] share the classic shape, from the front.
pub const CLASSIC_THEMES: usize = 4;

/// Scale factors a host may run at.
pub const SCALES: [f32; 4] = [1.0, 1.25, 1.5, 2.0];

/// A random theme and scale for `component`, at its designed size.
pub fn mount(
    component: &components::Component,
) -> impl proptest::strategy::Strategy<Value = components::Mount> {
    use proptest::prelude::*;
    let size = component.size;
    (0..THEMES.len(), 0..SCALES.len()).prop_map(move |(theme, scale)| components::Mount {
        theme: THEMES[theme],
        size,
        scale: SCALES[scale],
        animate: false,
    })
}

/// Turns an oracle verdict into a proptest outcome.
pub fn verdict(result: Result<(), String>) -> Result<(), proptest::test_runner::TestCaseError> {
    result.map_err(proptest::test_runner::TestCaseError::fail)
}
