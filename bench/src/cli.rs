use super::*;

#[derive(Parser, Debug)]
#[command(
    name = "schnellui-bench",
    about = "Measure time + heap allocations per hot path and gate them against SOUL §4.1 budgets + Directive #3 proportionality (nonzero exit on any breach)."
)]
pub(crate) struct Cli {
    /// print the registered path names (budget + scaling), one per line, and exit.
    #[arg(long)]
    list: bool,
    /// only run paths whose name contains this substring.
    #[arg(long)]
    filter: Option<String>,
    /// iterations measured per path (after warmup; mount-class rows cap at 25).
    #[arg(long, default_value_t = 1000)]
    iters: u64,
    /// emit machine-readable JSON (paths + per-n scaling series) instead of tables.
    #[arg(long)]
    json: bool,
}

pub fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let all = paths();
    let all_scales = scale_paths();

    if cli.list {
        for p in &all {
            println!("{}", p.name);
        }
        for s in &all_scales {
            println!("{}", s.name);
        }
        return std::process::ExitCode::SUCCESS;
    }

    let matches = |name: &str| {
        cli.filter
            .as_deref()
            .map(|s| name.contains(s))
            .unwrap_or(true)
    };
    let selected: Vec<&BenchPath> = all.iter().filter(|p| matches(p.name)).collect();
    let selected_scales: Vec<&ScalePath> = all_scales.iter().filter(|s| matches(s.name)).collect();

    if selected.is_empty() && selected_scales.is_empty() {
        eprintln!(
            "no path matches --filter {:?}; try --list",
            cli.filter.unwrap_or_default()
        );
        return std::process::ExitCode::FAILURE;
    }

    let results: Vec<PathResult> = selected.iter().map(|p| measure(p, cli.iters)).collect();
    let scale_results: Vec<ScaleResult> = selected_scales
        .iter()
        .map(|s| measure_scale(s, cli.iters))
        .collect();

    if cli.json {
        print_json(&results, &scale_results);
    } else {
        if !results.is_empty() {
            print_budget_table(&results);
        }
        if !scale_results.is_empty() {
            print_scale_table(&scale_results);
        }
    }

    // Exit nonzero iff any gated path breached its budget or any proportionality
    // path scales with document size — this binary IS a CI gate.
    let breached = results.iter().any(|r| r.verdict == Verdict::Fail)
        || scale_results.iter().any(|s| !s.is_flat());
    if breached {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}
