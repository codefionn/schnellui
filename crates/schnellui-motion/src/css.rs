//! CSS backend: compiles one backend-neutral [Motion](crate::Motion)
//! declaration into the equivalent CSS @keyframes block and animation
//! shorthand. The browser compositor then advances the animation with
//! zero JS involvement - the "implemented with CSS animations" half of
//! the framework dual-backend contract.

use crate::{Motion, Property, Repeat};

/// The compiled CSS artifacts for one declaration.
pub struct CssAnimation {
    /// The complete @keyframes name { ... } rule.
    pub keyframes: String,
    /// The complete animation shorthand value (no trailing semicolon).
    pub shorthand: String,
}

/// Compiles motion into CSS, naming the keyframes name.
pub fn css_animation(motion: &Motion, name: &str) -> CssAnimation {
    CssAnimation {
        keyframes: css_keyframes(motion, name),
        shorthand: css_shorthand(motion, name),
    }
}

fn css_shorthand(motion: &Motion, name: &str) -> String {
    let iterations = match motion.repeat {
        Repeat::Infinite => "infinite".to_string(),
        Repeat::Finite(n) => n.to_string(),
    };
    let delay = if motion.delay_ms > 0.0 {
        format!(" {}ms", motion.delay_ms as u32)
    } else {
        String::new()
    };
    format!(
        "{name} {}ms{delay} {} {iterations}",
        motion.duration_ms as u32,
        motion.easing.css_keyword()
    )
}

/// Compiles just the @keyframes rule for motion.
pub fn css_keyframes(motion: &Motion, name: &str) -> String {
    match motion.property {
        Property::Rotate { turns } => {
            let end = (turns * 360.0) % 360.0;
            let end = if end == 0.0 { 360.0 } else { end };
            format!(
                "@keyframes {name} {{ from {{ transform: rotate(0deg); }} to {{ transform: rotate({end}deg); }} }}"
            )
        }
        Property::Fade { from, to } => {
            format!("@keyframes {name} {{ from {{ opacity: {from}; }} to {{ opacity: {to}; }} }}")
        }
    }
}
