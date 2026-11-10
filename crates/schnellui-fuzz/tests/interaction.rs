//! Random pointer, key, wheel, resize and theme input, per component.
//!
//! `twin`: a decorated host and its undecorated twin read the same after
//! every step and paint the same once quiet, and the retained-state checks
//! and the component's own accessibility rules hold throughout.
//!
//! `routes`: the same input through the window host's pointer handlers and
//! through the debug server's pointer commands reads, selects and paints the
//! same after every step.
//!
//! `history`: whatever the input, driving the component to its canonical
//! state leaves it painting and reading like a fresh mount driven there.

use proptest::prelude::*;
use schnellui_fuzz::{components, config, mount, ops, oracle, serial, verdict};

macro_rules! interaction_properties {
    ($($component:ident),* $(,)?) => {$(
        mod $component {
            use super::*;

            fn component() -> components::Component {
                components::named(stringify!($component))
            }

            proptest! {
                #![proptest_config(config(32))]

                #[test]
                fn twin(m in mount(&component()), ops in ops::ops(40)) {
                    let _serial = serial();
                    verdict(oracle::twin(&component(), m, &ops))?;
                }

                #[test]
                fn routes(
                    m in mount(&component()),
                    ops in proptest::collection::vec(ops::route_op(), 1..40),
                ) {
                    let _serial = serial();
                    verdict(oracle::routes(&component(), m, &ops))?;
                }

                #[test]
                fn history(m in mount(&component()), ops in ops::ops(40)) {
                    let _serial = serial();
                    verdict(oracle::history(&component(), m, &ops))?;
                }
            }
        }
    )*};
}

interaction_properties!(
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
    scroll_virtual,
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
