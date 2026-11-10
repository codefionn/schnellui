//! Window resizing, per component: one jump at a time, a gradual drag with a
//! random acceleration curve, and a gradual drag while hover, press or focus
//! is live. Every frame passes the retained-state checks, and the final
//! window paints and reads like a fresh mount at its final size.

use proptest::prelude::*;
use schnellui_fuzz::{components, config, curve, mount, ops, oracle, serial, verdict};

macro_rules! resize_properties {
    ($($component:ident),* $(,)?) => {$(
        mod $component {
            use super::*;

            fn component() -> components::Component {
                components::named(stringify!($component))
            }

            proptest! {
                #![proptest_config(config(16))]

                #[test]
                fn jump(m in mount(&component()), sizes in curve::jumps()) {
                    let _serial = serial();
                    verdict(oracle::resize(&component(), m, None, &sizes))?;
                }

                #[test]
                fn gradual(m in mount(&component()), mut drag in curve::gradual()) {
                    let _serial = serial();
                    drag.from = (m.size.0 as f32, m.size.1 as f32);
                    verdict(oracle::resize(&component(), m, None, &drag.sizes()))?;
                }

                #[test]
                fn gradual_while_held(
                    m in mount(&component()),
                    before in ops::ops(10),
                    mut drag in curve::gradual(),
                ) {
                    let _serial = serial();
                    drag.from = (m.size.0 as f32, m.size.1 as f32);
                    let grip = oracle::Grip { before };
                    verdict(oracle::resize(&component(), m, Some(&grip), &drag.sizes()))?;
                }
            }
        }
    )*};
}

resize_properties!(
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
);
