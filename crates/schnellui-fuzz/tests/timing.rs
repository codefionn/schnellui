//! Mouse and keyboard input at different paces, with animations running.
//!
//! `twin`: the decorated/undecorated twin under input that also waits,
//! bursts, forces frames and fades themes, with animations on.
//!
//! `animation`: an animated host and a reduced-motion host get the same
//! input. Their accessibility trees match after every step, the
//! retained-state checks hold on every animation frame, and both end
//! painting the same.
//!
//! `pace`: the same input as one burst, one event every 400 ms, and a
//! random mix of the two ends in the same state. Its input includes wheel,
//! page keys and AccessKit scroll actions on a viewport, and fast clicks
//! aimed at widgets.
//!
//! `multi_click`: one double or triple click inside a single frame and the
//! same click spread over frames, every press inside the double-click
//! interval of the one before, end the same.
//!
//! `not_a_multi_click`: a click that comes after the interval or lands
//! outside the slop starts over as a single click.

use proptest::prelude::*;
use schnellui_fuzz::components::Mount;
use schnellui_fuzz::{clicks, components, config, mount, ops, oracle, serial, verdict};

macro_rules! timing_properties {
    ($($component:ident),* $(,)?) => {$(
        mod $component {
            use super::*;

            fn component() -> components::Component {
                components::named(stringify!($component))
            }

            proptest! {
                #![proptest_config(config(24))]

                #[test]
                fn twin(m in mount(&component()), ops in ops::timed_ops(40)) {
                    let _serial = serial();
                    let m = Mount { animate: true, ..m };
                    verdict(oracle::twin(&component(), m, &ops))?;
                }

                #[test]
                fn animation(m in mount(&component()), ops in ops::timed_ops(40)) {
                    let _serial = serial();
                    verdict(oracle::animation(&component(), m, &ops))?;
                }
            }
        }
    )*};
}

/// The twin property alone, for components whose end state depends on how many
/// frames the input took.
macro_rules! twin_property {
    ($($component:ident),* $(,)?) => {$(
        mod $component {
            use super::*;

            fn component() -> components::Component {
                components::named(stringify!($component))
            }

            proptest! {
                #![proptest_config(config(24))]

                #[test]
                fn twin(m in mount(&component()), ops in ops::timed_ops(40)) {
                    let _serial = serial();
                    let m = Mount { animate: true, ..m };
                    verdict(oracle::twin(&component(), m, &ops))?;
                }
            }
        }
    )*};
}

macro_rules! pace_properties {
    ($($component:ident),* $(,)?) => {$(
        mod $component {
            use super::*;

            fn component() -> components::Component {
                components::named(stringify!($component))
            }

            proptest! {
                #![proptest_config(config(24))]

                #[test]
                fn pace(
                    m in mount(&component()),
                    ops in proptest::collection::vec(ops::pace_op(), 1..40),
                    seed in any::<u64>(),
                ) {
                    let _serial = serial();
                    verdict(oracle::pace(&component(), m, &ops, seed))?;
                }
            }
        }
    )*};
}

macro_rules! click_properties {
    ($($component:ident),* $(,)?) => {$(
        mod $component {
            use super::*;

            fn component() -> components::Component {
                components::named(stringify!($component))
            }

            proptest! {
                #![proptest_config(config(24))]

                #[test]
                fn multi_click(
                    m in mount(&component()),
                    before in proptest::collection::vec(ops::pace_op(), 0..12),
                    click in clicks::any_multi_click(),
                ) {
                    let _serial = serial();
                    verdict(clicks::multi_click(&component(), m, &before, &click))?;
                }

                #[test]
                fn not_a_multi_click(
                    m in mount(&component()),
                    before in proptest::collection::vec(ops::pace_op(), 0..12),
                    broken in clicks::any_broken(),
                ) {
                    let _serial = serial();
                    verdict(clicks::not_a_multi_click(&component(), m, &before, &broken))?;
                }
            }
        }
    )*};
}

timing_properties!(
    buttons,
    toggles,
    sliders,
    tabs,
    list,
    table,
    text_input,
    text_area,
    scroll,
    scroll_form,
    scroll_nested,
    scroll_area,
    scroll_resize,
    dropdown,
    context_menu,
    dialogs,
    grouped_tabs,
    todo,
    rich_text,
    rich_links,
    click_targets,
);

// A virtual list measures its rows as it mounts them and keeps its viewport
// anchored on a row while it does, so where it ends depends on how many
// frames and rebuilds the input took. `animation` replays one input as a
// fade of many frames and as one jump; `pace` as one frame and as many. Both
// end a pixel apart (`probes.rs` keeps ignored reproducers), so it takes part
// in `twin`, whose two hosts see the same frames, and in the other properties.
twin_property!(scroll_virtual);

/// The pace property for every component but `scroll_virtual` (see above).
mod pace {
    use super::*;

    pace_properties!(
        buttons,
        toggles,
        sliders,
        tabs,
        list,
        table,
        text_input,
        text_area,
        scroll,
        scroll_form,
        scroll_nested,
        scroll_area,
        scroll_resize,
        dropdown,
        context_menu,
        dialogs,
        grouped_tabs,
        todo,
        rich_text,
        rich_links,
        click_targets,
    );
}

/// The multi-click properties, for the same components as `pace`.
mod click {
    use super::*;

    click_properties!(
        buttons,
        toggles,
        sliders,
        tabs,
        list,
        table,
        text_input,
        text_area,
        scroll,
        scroll_form,
        scroll_nested,
        scroll_area,
        scroll_resize,
        dropdown,
        context_menu,
        dialogs,
        grouped_tabs,
        todo,
        rich_text,
        rich_links,
        click_targets,
    );
}
