//! Value-axis math for charts (pure and deterministic, SOUL §7.3): "nice" tick
//! selection and tick/value label formatting. No scene, text or GPU state lives here,
//! so every function is unit-tested directly.

/// A chosen value axis: the extended `[lo, hi]` range (rounded outward to whole tick
/// steps, so the first and last tick sit on the plot edges) and the tick values.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct NiceAxis {
    pub lo: f32,
    pub hi: f32,
    pub step: f32,
    pub ticks: Vec<f32>,
}

/// Hard cap on tick count, so a pathological range can never emit an unbounded list.
const MAX_TICKS: usize = 12;

/// The 1-2-5 step closest to (and not below) `raw`.
fn nice_step(raw: f32) -> f32 {
    if !raw.is_finite() || raw <= 0.0 {
        return 1.0;
    }
    let mag = 10f32.powf(raw.log10().floor());
    let f = raw / mag;
    let nice = if f <= 1.0 {
        1.0
    } else if f <= 2.0 {
        2.0
    } else if f <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

/// Picks 1-2-5 ticks covering `[lo, hi]` with about `target` intervals. A degenerate
/// range (all-equal data, or empty data folded to `(0, 0)`) is widened around its
/// value first, so the axis is never a divide-by-zero and never `NaN` (SOUL §7.3).
pub(super) fn nice_axis(lo: f32, hi: f32, target: usize) -> NiceAxis {
    let (mut lo, mut hi) = if lo.is_finite() && hi.is_finite() && lo <= hi {
        (lo, hi)
    } else {
        (0.0, 0.0)
    };
    if (hi - lo).abs() <= f32::EPSILON {
        if lo == 0.0 {
            hi = 1.0;
        } else {
            let pad = lo.abs() * 0.5;
            lo -= pad;
            hi += pad;
        }
    }
    let step = nice_step((hi - lo) / target.max(1) as f32);
    let k0 = (lo / step).floor();
    let k1 = (hi / step).ceil();
    let count = (((k1 - k0) as usize) + 1).min(MAX_TICKS);
    let ticks: Vec<f32> = (0..count).map(|i| (k0 + i as f32) * step).collect();
    NiceAxis {
        lo: k0 * step,
        hi: ticks.last().copied().unwrap_or(k1 * step),
        step,
        ticks,
    }
}

/// Number of decimals needed to tell ticks `step` apart (`0` for steps of 1 or more).
fn decimals_for(step: f32) -> usize {
    if !step.is_finite() || step >= 1.0 || step <= 0.0 {
        0
    } else {
        ((-step.log10().floor()) as usize).min(4)
    }
}

/// Formats a tick value for an axis whose ticks are `step` apart: just enough decimals
/// to separate ticks, and a `k`/`M` suffix once the step reaches a thousand / million.
pub(super) fn format_tick(v: f32, step: f32) -> String {
    let (div, suffix) = if step >= 1.0e6 {
        (1.0e6, "M")
    } else if step >= 1.0e3 {
        (1.0e3, "k")
    } else {
        (1.0, "")
    };
    let d = decimals_for(step / div);
    let mut s = format!("{:.*}", d, v / div);
    if s.starts_with('-') && s[1..].chars().all(|c| c == '0' || c == '.') {
        s.remove(0);
    }
    s.push_str(suffix);
    typographic_minus(s)
}

/// Swaps a leading ASCII hyphen for the minus sign (U+2212), which is as wide as a
/// digit and sits on the figure's centre line, so negative ticks align with positive ones.
fn typographic_minus(s: String) -> String {
    match s.strip_prefix('-') {
        Some(rest) => format!("\u{2212}{rest}"),
        None => s,
    }
}

/// Formats a data value for a tooltip: up to two decimals, trailing zeros trimmed.
pub(super) fn format_value(v: f32) -> String {
    if !v.is_finite() {
        return "n/a".to_string();
    }
    let mut s = format!("{v:.2}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".to_string();
    }
    typographic_minus(s)
}

/// Whether `unit` attaches to the number without a space: a single symbol such as
/// `%` or `°C`, or a unit that starts with a magnitude suffix (`k`, `M`, `B`) such
/// as `"k"` or `"k users"`, which reads as `12k users`.
fn unit_attaches(unit: &str) -> bool {
    let first_word = unit.split(' ').next().unwrap_or("");
    let symbol = !unit.starts_with(char::is_alphanumeric) && first_word.chars().count() <= 2;
    symbol || matches!(first_word, "k" | "M" | "B")
}

/// `prefix`, `value` and `unit` as one label, such as `$7k`, `12 ms` or `10k users`.
/// A unit is spaced from the number unless [`unit_attaches`] says otherwise.
pub(super) fn with_unit(value: &str, prefix: Option<&str>, unit: Option<&str>) -> String {
    let prefix = prefix.unwrap_or("");
    // the prefix goes after the minus sign: −$3k, not $−3k
    let (sign, digits) = match value.strip_prefix('\u{2212}') {
        Some(rest) => ("\u{2212}", rest),
        None => ("", value),
    };
    match unit {
        None | Some("") => format!("{sign}{prefix}{digits}"),
        Some(u) if unit_attaches(u) => format!("{sign}{prefix}{digits}{u}"),
        Some(u) => format!("{sign}{prefix}{digits} {u}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_1_2_5_steps_covering_the_range() {
        let a = nice_axis(0.0, 9.0, 4);
        assert_eq!(a.step, 5.0);
        assert_eq!(a.ticks, vec![0.0, 5.0, 10.0]);
        assert_eq!((a.lo, a.hi), (0.0, 10.0));
        let a = nice_axis(-6.0, 8.0, 4);
        assert_eq!(a.ticks, vec![-10.0, -5.0, 0.0, 5.0, 10.0]);
        let a = nice_axis(4.0, 12.0, 4);
        assert_eq!(a.ticks, vec![4.0, 6.0, 8.0, 10.0, 12.0]);
    }

    #[test]
    fn degenerate_ranges_stay_finite_and_ordered() {
        for (lo, hi) in [(0.0, 0.0), (5.0, 5.0), (-3.0, -3.0), (f32::NAN, 1.0)] {
            let a = nice_axis(lo, hi, 4);
            assert!(a.lo < a.hi, "{lo}..{hi} -> {a:?}");
            assert!(a.ticks.len() >= 2 && a.ticks.len() <= MAX_TICKS);
            assert!(a.ticks.iter().all(|t| t.is_finite()));
        }
    }

    #[test]
    fn tick_labels_use_just_enough_decimals() {
        assert_eq!(format_tick(5.0, 5.0), "5");
        assert_eq!(format_tick(-0.0, 5.0), "0");
        assert_eq!(format_tick(0.5, 0.5), "0.5");
        assert_eq!(format_tick(0.25, 0.05), "0.25");
        assert_eq!(format_tick(20000.0, 5000.0), "20k");
        assert_eq!(format_tick(2.0e6, 1.0e6), "2M");
        assert_eq!(format_tick(-5.0, 5.0), "\u{2212}5");
    }

    #[test]
    fn values_and_units_format_compactly() {
        assert_eq!(format_value(3.0), "3");
        assert_eq!(format_value(2.5678), "2.57");
        assert_eq!(format_value(-0.001), "0");
        assert_eq!(format_value(f32::NAN), "n/a");
        assert_eq!(format_value(-3.5), "\u{2212}3.5");
        assert_eq!(with_unit("12", None, Some("ms")), "12 ms");
        assert_eq!(with_unit("12", None, Some("%")), "12%");
        assert_eq!(with_unit("12", None, Some("°C")), "12°C");
        assert_eq!(with_unit("12", None, None), "12");
        assert_eq!(with_unit("10", None, Some("k users")), "10k users");
        assert_eq!(with_unit("7", Some("$"), Some("k")), "$7k");
        assert_eq!(with_unit("\u{2212}3", Some("$"), Some("k")), "\u{2212}$3k");
    }
}
