//! Random pointer, key, wheel, resize and theme input, per component.
//!
//! `twin`: a decorated host and its undecorated twin read the same after
//! every step and paint the same once quiet, and the retained-state checks
//! and the component's own accessibility rules hold throughout.
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
    dropdown,
    context_menu,
    dialogs,
    grouped_tabs,
    todo,
    rich_text,
    rich_links,
);
