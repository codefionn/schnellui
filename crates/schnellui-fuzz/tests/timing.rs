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
//! random mix of the two ends in the same state.

use proptest::prelude::*;
use schnellui_fuzz::components::Mount;
use schnellui_fuzz::{components, config, mount, ops, oracle, serial, verdict};

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
    dropdown,
    context_menu,
    dialogs,
    grouped_tabs,
    todo,
    rich_text,
    rich_links,
);
