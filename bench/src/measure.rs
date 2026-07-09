use super::*;

pub(crate) struct Raw {
    pub(crate) iters: u64,
    pub(crate) time_median_ns: u64,
    time_p99_ns: u64,
    pub(crate) allocs_median: u64,
    pub(crate) allocs_min: u64,
    pub(crate) allocs_max: u64,
    pub(crate) bytes_median: u64,
    pub(crate) bytes_min: u64,
    pub(crate) bytes_max: u64,
    pub(crate) net_min: i64,
    pub(crate) net_max: i64,
}

/// Warm `f` [`WARMUP`] times, then run the two measured passes (timing first,
/// counting second) of `iters` iterations each.
pub(crate) fn run_measure(f: &mut Box<dyn FnMut()>, iters: u64) -> Raw {
    let n = iters.max(1) as usize;

    // Warm the steady-state closure so both passes see the second-and-later state.
    for _ in 0..WARMUP {
        f();
    }

    // --- PASS 1: TIMING (no counting — counting perturbs timing). ---
    let mut times = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        f();
        times.push(t0.elapsed().as_nanos() as u64);
    }

    // --- PASS 2: COUNTING (same closure instance, still steady-state). ---
    let mut allocs = Vec::with_capacity(n);
    let mut bytes = Vec::with_capacity(n);
    let mut nets = Vec::with_capacity(n);
    for _ in 0..n {
        let info = allocation_counter::measure(&mut *f);
        allocs.push(info.count_total);
        bytes.push(info.bytes_total);
        nets.push(info.count_current);
    }

    times.sort_unstable();
    let allocs_min = *allocs.iter().min().unwrap();
    let allocs_max = *allocs.iter().max().unwrap();
    let bytes_min = *bytes.iter().min().unwrap();
    let bytes_max = *bytes.iter().max().unwrap();
    let net_min = *nets.iter().min().unwrap();
    let net_max = *nets.iter().max().unwrap();
    allocs.sort_unstable();
    bytes.sort_unstable();

    Raw {
        iters: n as u64,
        time_median_ns: percentile(&times, 50.0),
        time_p99_ns: percentile(&times, 99.0),
        allocs_median: percentile(&allocs, 50.0),
        allocs_min,
        allocs_max,
        bytes_median: percentile(&bytes, 50.0),
        bytes_min,
        bytes_max,
        net_min,
        net_max,
    }
}

/// The measured + judged result for one budget-table path.
pub(crate) struct PathResult {
    pub(crate) name: &'static str,
    pub(crate) raw: Raw,
    pub(crate) budget: Budget,
    pub(crate) verdict: Verdict,
    /// short reason when the verdict is a breach (drift / over-budget / free).
    pub(crate) reason: String,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Verdict {
    Pass,
    Fail,
    Info,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Info => "info",
        }
    }
}

pub(crate) fn measure(path: &BenchPath, iters: u64) -> PathResult {
    let effective = path.iters_cap.map(|c| iters.min(c)).unwrap_or(iters);
    let mut f = (path.make)();
    let raw = run_measure(&mut f, effective);
    let (verdict, reason) = judge(path.budget, &raw);
    PathResult {
        name: path.name,
        raw,
        budget: path.budget,
        verdict,
        reason,
    }
}

/// Applies the budget to the measured extremes and returns a verdict + breach reason.
pub(crate) fn judge(budget: Budget, r: &Raw) -> (Verdict, String) {
    match budget {
        Budget::Zero => {
            // Steady means steady: a zero path must be identical every iteration...
            if r.allocs_min != r.allocs_max || r.bytes_min != r.bytes_max {
                return (
                    Verdict::Fail,
                    format!(
                        "alloc count drifts across iterations ({}..={} allocs, {}..={} B) — steady-state is not steady",
                        r.allocs_min, r.allocs_max, r.bytes_min, r.bytes_max
                    ),
                );
            }
            // ...and that steady value must be literal zero, with no stray frees.
            if r.allocs_max != 0 || r.bytes_max != 0 {
                return (
                    Verdict::Fail,
                    format!(
                        "steady-state re-render allocated {} times / {} B (budget 0) — SOUL §1",
                        r.allocs_max, r.bytes_max
                    ),
                );
            }
            if r.net_min != 0 || r.net_max != 0 {
                return (
                    Verdict::Fail,
                    format!(
                        "net alloc balance != 0 ({}..={}) — a free breaches allocs+reallocs+frees==0 (SOUL §1)",
                        r.net_min, r.net_max
                    ),
                );
            }
            (Verdict::Pass, String::new())
        }
        Budget::Bounded { allocs, bytes } => {
            if r.allocs_max > allocs {
                return (
                    Verdict::Fail,
                    format!("{} allocs > budget {allocs} — SOUL §4.1", r.allocs_max),
                );
            }
            if r.bytes_max > bytes {
                return (
                    Verdict::Fail,
                    format!("{} B > budget {bytes} B — SOUL §4.1", r.bytes_max),
                );
            }
            (Verdict::Pass, String::new())
        }
        Budget::Report => (Verdict::Info, String::new()),
    }
}

// ---- proportionality (Directive #3) ----

/// One proportionality path measured at every n in [`SCALE_NS`].
pub(crate) struct ScaleResult {
    pub(crate) name: &'static str,
    pub(crate) series: Vec<(usize, Raw)>,
    /// median-time ratio of the largest n over the smallest n.
    pub(crate) ratio: f64,
    /// per-n alloc counts steady AND identical across n.
    pub(crate) alloc_flat: bool,
    /// ratio under [`FLAT_RATIO`].
    pub(crate) time_flat: bool,
    pub(crate) reason: String,
}

impl ScaleResult {
    pub(crate) fn is_flat(&self) -> bool {
        self.alloc_flat && self.time_flat
    }
    fn verdict_label(&self) -> &'static str {
        if self.is_flat() {
            "SCALES-FLAT"
        } else {
            "SCALES-WITH-N"
        }
    }
}

pub(crate) fn measure_scale(sp: &ScalePath, iters: u64) -> ScaleResult {
    let mut series = Vec::with_capacity(SCALE_NS.len());
    for &n in &SCALE_NS {
        let mut f = (sp.make)(n);
        series.push((n, run_measure(&mut f, iters)));
        // f (and its App) drop here before the next n mounts. Each App now owns its
        // widget runtime, so overlapping live apps do not alias retained slots.
    }

    let t_small = series.first().unwrap().1.time_median_ns.max(1);
    let t_large = series.last().unwrap().1.time_median_ns;
    let ratio = t_large as f64 / t_small as f64;
    let time_flat = ratio < FLAT_RATIO;

    // Hard gate: per-n steady (min==max) AND the same count at every n. An alloc
    // count that grows with document size means work ∝ size — the exact violation
    // Directive #3 forbids.
    let base = series[0].1.allocs_median;
    let alloc_flat = series
        .iter()
        .all(|(_, r)| r.allocs_min == r.allocs_max && r.allocs_median == base);

    let mut reasons = Vec::new();
    if !time_flat {
        reasons.push(format!(
            "median time ratio n={}..n={} is {ratio:.2}x (flat bound {FLAT_RATIO}x) — work scales with document size (SOUL Directive #3)",
            SCALE_NS[0],
            SCALE_NS[SCALE_NS.len() - 1]
        ));
    }
    if !alloc_flat {
        let counts: Vec<String> = series
            .iter()
            .map(|(n, r)| format!("n={n}:{}..{}", r.allocs_min, r.allocs_max))
            .collect();
        reasons.push(format!(
            "alloc counts not identical across n ({}) — allocation scales with document size",
            counts.join(", ")
        ));
    }

    ScaleResult {
        name: sp.name,
        series,
        ratio,
        alloc_flat,
        time_flat,
        reason: reasons.join("; "),
    }
}

/// Nearest-rank percentile of an ascending-sorted slice.
pub(crate) fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let n = sorted.len();
    let rank = ((p / 100.0) * n as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(n - 1);
    sorted[idx]
}

// ---------------------------------------------------------------------------
// Formatting.
// ---------------------------------------------------------------------------

/// Human-friendly nanosecond duration (`456ns`, `12.34us`, `1.235ms`).
pub(crate) fn fmt_ns(ns: u64) -> String {
    if ns < 1_000 {
        format!("{ns}ns")
    } else if ns < 1_000_000 {
        format!("{:.2}us", ns as f64 / 1_000.0)
    } else if ns < 1_000_000_000 {
        format!("{:.3}ms", ns as f64 / 1_000_000.0)
    } else {
        format!("{:.3}s", ns as f64 / 1_000_000_000.0)
    }
}

/// The `ALLOCS/it` cell: the steady value, or `min~max` when it drifts.
pub(crate) fn fmt_allocs(r: &Raw) -> String {
    if r.allocs_min == r.allocs_max {
        format!("{}", r.allocs_median)
    } else {
        format!("{}~{}", r.allocs_min, r.allocs_max)
    }
}

/// Renders an ASCII table from a header row + data rows.
pub(crate) fn render_table(cols: &[&str], rows: &[Vec<String>]) {
    let mut w: Vec<usize> = cols.iter().map(|c| c.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            w[i] = w[i].max(cell.len());
        }
    }
    let sep = {
        let mut s = String::from("+");
        for width in &w {
            s.push_str(&"-".repeat(width + 2));
            s.push('+');
        }
        s
    };
    let fmt_row = |row: &[String]| -> String {
        let mut s = String::from("|");
        for (i, cell) in row.iter().enumerate() {
            s.push_str(&format!(" {:<width$} |", cell, width = w[i]));
        }
        s
    };
    println!("{sep}");
    println!(
        "{}",
        fmt_row(&cols.iter().map(|c| c.to_string()).collect::<Vec<_>>())
    );
    println!("{sep}");
    for row in rows {
        println!("{}", fmt_row(row));
    }
    println!("{sep}");
}

pub(crate) fn print_budget_table(results: &[PathResult]) {
    // Header: state exactly what the numbers do and do not cover.
    println!("schnellui-bench — SOUL §4.1 allocation + time budgets");
    println!(
        "  measured: CPU-side App::frame() (pull -> layout-if-dirty -> paint-prep -> a11y-dirty + damage fold)."
    );
    println!(
        "  NOT measured: GPU submission (render_to_png owns that; wgpu internals allocate in a foreign crate)."
    );
    println!(
        "  ALLOCS = allocation-counter count_total (allocs INCLUDING reallocs; no separate realloc counter)."
    );
    println!(
        "  BYTES  = bytes_total. Zero rows also gate net frees (count_current == 0). TIME excludes the counting pass."
    );
    println!(
        "  mount-class rows run min(--iters, {MOUNT_ITERS_CAP}) iterations (each iteration is a full build)."
    );
    println!();

    let cols = [
        "PATH",
        "ITERS",
        "TIME/it (med)",
        "TIME/it (p99)",
        "ALLOCS/it",
        "BYTES/it",
        "BUDGET",
        "VERDICT",
    ];
    let rows: Vec<Vec<String>> = results
        .iter()
        .map(|r| {
            vec![
                r.name.to_string(),
                r.raw.iters.to_string(),
                fmt_ns(r.raw.time_median_ns),
                fmt_ns(r.raw.time_p99_ns),
                fmt_allocs(&r.raw),
                r.raw.bytes_median.to_string(),
                r.budget.label(),
                r.verdict.label().to_string(),
            ]
        })
        .collect();
    render_table(&cols, &rows);

    for r in results {
        if r.verdict == Verdict::Fail {
            println!("  FAIL {}: {}", r.name, r.reason);
        }
    }
    println!();
    println!("notes:");
    for r in results {
        println!("  - {:<22} {}", r.name, path_note(r.name));
    }
}

pub(crate) fn print_scale_table(results: &[ScaleResult]) {
    println!();
    println!("proportionality — work ∝ WHAT CHANGED, never document size (SOUL Directive #3)");
    println!("  cells: median time / allocs per iteration over an n-paragraph retained doc.");
    println!(
        "  verdict: SCALES-FLAT iff median-time ratio(n={} / n={}) < {FLAT_RATIO}x AND alloc counts identical across n.",
        SCALE_NS[SCALE_NS.len() - 1],
        SCALE_NS[0]
    );
    println!(
        "  ({FLAT_RATIO}x is a generous CI-noise bound: a linear-in-n walk would show ~{}x, a sqrt(n) walk ~{:.0}x.)",
        SCALE_NS[SCALE_NS.len() - 1] / SCALE_NS[0],
        ((SCALE_NS[SCALE_NS.len() - 1] as f64) / (SCALE_NS[0] as f64)).sqrt()
    );
    println!();

    let mut cols: Vec<String> = vec!["PATH".to_string()];
    for n in SCALE_NS {
        cols.push(format!("n={n}"));
    }
    cols.push(format!(
        "ratio({}/{})",
        SCALE_NS[SCALE_NS.len() - 1],
        SCALE_NS[0]
    ));
    cols.push("VERDICT".to_string());
    let col_refs: Vec<&str> = cols.iter().map(|s| s.as_str()).collect();

    let rows: Vec<Vec<String>> = results
        .iter()
        .map(|r| {
            let mut row = vec![r.name.to_string()];
            for (_, raw) in &r.series {
                row.push(format!(
                    "{} / {}a",
                    fmt_ns(raw.time_median_ns),
                    fmt_allocs(raw)
                ));
            }
            row.push(format!("{:.2}x", r.ratio));
            row.push(r.verdict_label().to_string());
            row
        })
        .collect();
    render_table(&col_refs, &rows);

    for r in results {
        if !r.is_flat() {
            println!("  FAIL {}: {}", r.name, r.reason);
        }
    }

    println!();
    println!("notes:");
    let registry = scale_paths();
    for r in results {
        if let Some(sp) = registry.iter().find(|s| s.name == r.name) {
            println!("  - {:<18} {}", sp.name, sp.note);
        }
    }
}

/// Look up a path's coverage note (kept alongside the registry, printed under table).
pub(crate) fn path_note(name: &str) -> &'static str {
    paths()
        .into_iter()
        .find(|p| p.name == name)
        .map(|p| p.note)
        .unwrap_or("")
}

/// Machine-readable output (hand-rolled to avoid a serde_json dep; SOUL §4.4 minimal):
/// `{"paths": [...], "scaling": [...]}` where each scaling entry carries its per-n
/// series.
pub(crate) fn print_json(results: &[PathResult], scales: &[ScaleResult]) {
    fn raw_fields(r: &Raw) -> String {
        format!(
            "\"iters\":{},\"time_ns_median\":{},\"time_ns_p99\":{},\"allocs_per_iter\":{},\"allocs_min\":{},\"allocs_max\":{},\"bytes_per_iter\":{},\"net_min\":{},\"net_max\":{}",
            r.iters,
            r.time_median_ns,
            r.time_p99_ns,
            r.allocs_median,
            r.allocs_min,
            r.allocs_max,
            r.bytes_median,
            r.net_min,
            r.net_max
        )
    }

    let mut out = String::from("{\n  \"paths\": [\n");
    for (i, r) in results.iter().enumerate() {
        let budget = match r.budget {
            Budget::Zero => "\"zero\"".to_string(),
            Budget::Bounded { allocs, bytes } => {
                format!("{{\"allocs\":{allocs},\"bytes\":{bytes}}}")
            }
            Budget::Report => "\"report\"".to_string(),
        };
        out.push_str(&format!(
            "    {{\"path\":\"{}\",{},\"budget\":{},\"verdict\":\"{}\"}}{}\n",
            r.name,
            raw_fields(&r.raw),
            budget,
            r.verdict.label().to_ascii_lowercase(),
            if i + 1 < results.len() { "," } else { "" },
        ));
    }
    out.push_str("  ],\n  \"scaling\": [\n");
    for (i, s) in scales.iter().enumerate() {
        let series: Vec<String> = s
            .series
            .iter()
            .map(|(n, raw)| format!("{{\"n\":{n},{}}}", raw_fields(raw)))
            .collect();
        out.push_str(&format!(
            "    {{\"path\":\"{}\",\"flat_threshold\":{FLAT_RATIO},\"series\":[{}],\"time_ratio\":{:.4},\"alloc_flat\":{},\"time_flat\":{},\"verdict\":\"{}\"}}{}\n",
            s.name,
            series.join(","),
            s.ratio,
            s.alloc_flat,
            s.time_flat,
            s.verdict_label().to_ascii_lowercase(),
            if i + 1 < scales.len() { "," } else { "" },
        ));
    }
    out.push_str("  ]\n}");
    println!("{out}");
}

// ---------------------------------------------------------------------------
// CLI.
// ---------------------------------------------------------------------------
