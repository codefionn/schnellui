use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::net::UnixStream;

use clap::{Args, Parser, Subcommand};
use jaq_core::load::{Arena, File as JaqFile, Loader};
use jaq_core::{data, unwrap_valr, Compiler, Ctx, Vars};
use jaq_json::Val as JaqValue;
use serde_json::{json, Value};

#[derive(Debug, Parser)]
#[command(
    name = "schnellui-debug",
    about = "Inspect and drive a live debug-build SchnellUI application"
)]
struct Cli {
    /// Explicit ephemeral TCP server URL (or SCHNELLUI_DEBUG_URL).
    #[arg(long, global = true)]
    url: Option<String>,
    /// Explicit Unix-domain socket (or SCHNELLUI_DEBUG_SOCKET).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Discovery JSON written by the application (or SCHNELLUI_DEBUG_INFO).
    #[arg(long, global = true)]
    info: Option<PathBuf>,
    /// Select a discovered application by process id.
    #[arg(long, global = true)]
    pid: Option<u32>,
    /// Select a discovered application by exact window title.
    #[arg(long, global = true)]
    title: Option<String>,
    /// Filter any JSON command result with a jq-compatible expression.
    #[arg(long, global = true, value_name = "FILTER")]
    jq: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List every discovered application and its unique endpoint.
    List,
    /// Print application/window state and the semantic hit path at the cursor.
    Status,
    /// Print the live accessibility tree as JSON.
    Tree,
    /// Print an atomic application status and semantic tree snapshot as JSON.
    Snapshot,
    /// Capture a tree or subtree and every observed change for a duration.
    Capture(CaptureArgs),
    /// Wait until live tree and/or remount conditions are satisfied.
    Wait(WaitArgs),
    /// Run several commands from a file or standard input, one command per line.
    Script {
        /// Script file; omit or use - to read standard input.
        file: Option<PathBuf>,
    },
    /// Save a PNG of the current live application.
    Screenshot { path: PathBuf },
    /// Dispatch an accessibility action to a semantic target.
    Action {
        action: String,
        /// Select exactly one target node with jq, evaluated against the live tree.
        #[arg(long, conflicts_with_all = ["id", "role", "name"])]
        selector: Option<String>,
        #[arg(long)]
        id: Option<u64>,
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        value: Option<String>,
    },
    /// Click a widget located by id or by role and accessible name.
    Click {
        /// Select exactly one target node with jq, evaluated against the live tree.
        #[arg(long, conflicts_with_all = ["id", "role", "name"])]
        selector: Option<String>,
        #[arg(long)]
        id: Option<u64>,
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        name: Option<String>,
    },
    /// Move the framework's logical pointer and update hover/cursor state.
    Move { x: f32, y: f32 },
    /// Click the widget hit at logical coordinates.
    ClickAt { x: f32, y: f32 },
    /// Send one framework-neutral key press.
    Key {
        key: String,
        #[arg(long)]
        shift: bool,
        #[arg(long)]
        ctrl: bool,
        #[arg(long)]
        text: Option<String>,
    },
    /// Type text into the focused editable widget.
    Type { text: String },
    /// Ask the application to close cleanly.
    Quit,
}

#[derive(Debug, Args)]
struct CaptureArgs {
    /// Capture exactly one subtree selected with jq; omit all selectors for the whole tree.
    #[arg(long, conflicts_with_all = ["id", "role", "name"])]
    selector: Option<String>,
    /// Capture the subtree rooted at this semantic tree id.
    #[arg(long)]
    id: Option<u64>,
    /// Capture the single subtree whose root has this role.
    #[arg(long)]
    role: Option<String>,
    /// Capture the single subtree whose root has this exact accessible name.
    #[arg(long)]
    name: Option<String>,
    /// How long to observe changes (plain numbers are seconds; suffixes: ms, s, m).
    #[arg(long, value_parser = parse_nonzero_duration)]
    duration: Duration,
    /// Delay between tree observations (suffixes: ms, s, m).
    #[arg(long, default_value = "50ms", value_parser = parse_nonzero_duration)]
    poll_interval: Duration,
}

#[derive(Debug, Args)]
struct WaitArgs {
    /// A jq condition evaluated against each {status, tree} snapshot.
    #[arg(long, value_name = "FILTER")]
    selector: Option<String>,
    /// Match a semantic node by its current tree id.
    #[arg(long)]
    id: Option<u64>,
    /// Match a semantic node by its role (for example button or dialog).
    #[arg(long)]
    role: Option<String>,
    /// Match a semantic node by its exact accessible name.
    #[arg(long)]
    name: Option<String>,
    /// Match a semantic node by its exact accessible value.
    #[arg(long)]
    value: Option<String>,
    /// Require a semantic state; repeat to require several states.
    #[arg(long)]
    state: Vec<String>,
    /// Wait for at least this many matching nodes (default: 1).
    #[arg(long, conflicts_with = "absent")]
    count: Option<usize>,
    /// Wait until no node matches the semantic selector.
    #[arg(long)]
    absent: bool,
    /// Wait for this many remounts after the initial snapshot.
    #[arg(long, conflicts_with = "remount_count")]
    remounts: Option<u64>,
    /// Wait until the session's monotonic remount count reaches this value.
    #[arg(long, conflicts_with = "remounts")]
    remount_count: Option<u64>,
    /// Count only remounts with this exact stable host-provided reason.
    #[arg(long)]
    remount_reason: Option<String>,
    /// Maximum time to wait (plain numbers are seconds; suffixes: ms, s, m).
    #[arg(long, default_value = "10s", value_parser = parse_duration)]
    timeout: Duration,
    /// Delay between snapshots (suffixes: ms, s, m).
    #[arg(long, default_value = "50ms", value_parser = parse_nonzero_duration)]
    poll_interval: Duration,
}

#[derive(Debug, Parser)]
#[command(no_binary_name = true)]
struct ScriptInvocation {
    /// Filter this command's JSON result, overriding the script-level --jq.
    #[arg(long, global = true, value_name = "FILTER")]
    jq: Option<String>,
    #[command(subcommand)]
    command: Command,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("schnellui-debug: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    if matches!(&cli.command, Command::List) {
        let applications = scan_discovery()?;
        let values = applications
            .iter()
            .map(Discovery::display_value)
            .collect::<Vec<_>>();
        print_json_value(&Value::Array(values), cli.jq.as_deref())?;
        return Ok(());
    }
    let endpoint = discover_endpoint(&cli)?;
    if let Command::Script { file } = &cli.command {
        return run_script(&endpoint, file.as_deref(), cli.jq.as_deref());
    }
    run_connected(&endpoint, cli.command, cli.jq.as_deref())
}

fn run_connected(
    endpoint: &Endpoint,
    command: Command,
    output_filter: Option<&str>,
) -> Result<(), String> {
    if let Command::Capture(args) = &command {
        return capture_tree(endpoint, args, output_filter);
    }
    if let Command::Wait(args) = &command {
        return wait_for(endpoint, args, output_filter);
    }
    let (method, path, body, output) = match command {
        Command::List => unreachable!("list handled above"),
        Command::Status => ("GET", "/v1/status", None, Output::Stdout),
        Command::Tree => ("GET", "/v1/tree", None, Output::Stdout),
        Command::Snapshot => ("GET", "/v1/snapshot", None, Output::Stdout),
        Command::Capture(_) => unreachable!("capture handled above"),
        Command::Wait(_) => unreachable!("wait handled above"),
        Command::Script { .. } => return Err("scripts cannot invoke another script".into()),
        Command::Screenshot { path } => ("GET", "/v1/screenshot", None, Output::File(path)),
        Command::Action {
            action,
            selector,
            id,
            role,
            name,
            value,
        } => (
            "POST",
            "/v1/action",
            Some(action_body(
                action,
                selector_target(endpoint, selector.as_deref(), id)?,
                role,
                name,
                value,
            )?),
            Output::Stdout,
        ),
        Command::Click {
            selector,
            id,
            role,
            name,
        } => (
            "POST",
            "/v1/action",
            Some(action_body(
                "click".into(),
                selector_target(endpoint, selector.as_deref(), id)?,
                role,
                name,
                None,
            )?),
            Output::Stdout,
        ),
        Command::Move { x, y } => (
            "POST",
            "/v1/pointer/move",
            Some(json!({ "x": x, "y": y }).to_string()),
            Output::Stdout,
        ),
        Command::ClickAt { x, y } => (
            "POST",
            "/v1/pointer/click",
            Some(json!({ "x": x, "y": y }).to_string()),
            Output::Stdout,
        ),
        Command::Key {
            key,
            shift,
            ctrl,
            text,
        } => (
            "POST",
            "/v1/key",
            Some(json!({ "key": key, "shift": shift, "ctrl": ctrl, "text": text }).to_string()),
            Output::Stdout,
        ),
        Command::Type { text } => (
            "POST",
            "/v1/key",
            Some(json!({ "key": "text", "text": text }).to_string()),
            Output::Stdout,
        ),
        Command::Quit => ("POST", "/v1/quit", Some("{}".into()), Output::Stdout),
    };
    if matches!(&output, Output::File(_)) && output_filter.is_some() {
        return Err("--jq cannot filter binary screenshot output".into());
    }
    let response = request(endpoint, method, path, body.as_deref())?;
    match output {
        Output::Stdout => {
            print_json_bytes(&response, output_filter)?;
        }
        Output::File(path) => {
            fs::write(&path, response).map_err(|error| format!("{}: {error}", path.display()))?;
            println!("{}", path.display());
        }
    }
    Ok(())
}

fn run_script(
    endpoint: &Endpoint,
    file: Option<&Path>,
    default_output_filter: Option<&str>,
) -> Result<(), String> {
    let (source_name, source) = match file {
        None => ("<stdin>".to_string(), read_stdin()?),
        Some(path) if path == Path::new("-") => ("<stdin>".to_string(), read_stdin()?),
        Some(path) => (
            path.display().to_string(),
            fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?,
        ),
    };

    for (index, line) in source.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let words = shell_words::split(trimmed)
            .map_err(|error| format!("{source_name}:{line_number}: {error}"))?;
        let invocation = ScriptInvocation::try_parse_from(words)
            .map_err(|error| format!("{source_name}:{line_number}: {error}"))?;
        if matches!(invocation.command, Command::List) {
            return Err(format!(
                "{source_name}:{line_number}: list is unavailable inside a connected script"
            ));
        }
        let filter = invocation.jq.as_deref().or(default_output_filter);
        run_connected(endpoint, invocation.command, filter)
            .map_err(|error| format!("{source_name}:{line_number}: {error}"))?;
    }
    Ok(())
}

fn read_stdin() -> Result<String, String> {
    let mut source = String::new();
    std::io::stdin()
        .lock()
        .read_to_string(&mut source)
        .map_err(|error| format!("cannot read script from stdin: {error}"))?;
    Ok(source)
}

fn parse_duration(input: &str) -> Result<Duration, String> {
    let (digits, multiplier) = if let Some(value) = input.strip_suffix("ms") {
        (value, 1_u64)
    } else if let Some(value) = input.strip_suffix('s') {
        (value, 1_000)
    } else if let Some(value) = input.strip_suffix('m') {
        (value, 60_000)
    } else {
        (input, 1_000)
    };
    let amount = digits
        .parse::<u64>()
        .map_err(|_| format!("invalid duration {input:?}; use an integer with ms, s, or m"))?;
    let millis = amount
        .checked_mul(multiplier)
        .ok_or_else(|| format!("duration {input:?} is too large"))?;
    Ok(Duration::from_millis(millis))
}

fn parse_nonzero_duration(input: &str) -> Result<Duration, String> {
    let duration = parse_duration(input)?;
    if duration.is_zero() {
        return Err("duration must be greater than zero".into());
    }
    Ok(duration)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptureScope {
    Tree,
    Subtree(u64),
}

fn capture_tree(
    endpoint: &Endpoint,
    args: &CaptureArgs,
    output_filter: Option<&str>,
) -> Result<(), String> {
    let tree = request_tree(endpoint, Duration::from_secs(20))?;
    let (scope, initial) = capture_initial_value(&tree, args)?;
    let started = Instant::now();
    let deadline = started
        .checked_add(args.duration)
        .ok_or_else(|| "capture duration is too large".to_string())?;
    let mut previous = initial.clone();
    let mut changes = Vec::new();
    let mut next_observation = started
        .checked_add(args.poll_interval.min(args.duration))
        .unwrap_or(deadline)
        .min(deadline);

    loop {
        let now = Instant::now();
        if now < next_observation {
            std::thread::sleep(next_observation.saturating_duration_since(now));
        }

        let tree = request_tree(endpoint, Duration::from_secs(2))?;
        let current = capture_value(&tree, scope);
        if current != previous {
            changes.push(json!({
                "elapsed_ms": duration_millis(started.elapsed()),
                "tree": current,
            }));
            previous = current;
        }

        if next_observation >= deadline {
            break;
        }
        next_observation = next_observation
            .checked_add(args.poll_interval)
            .unwrap_or(deadline)
            .min(deadline);
    }

    let target = match scope {
        CaptureScope::Tree => json!({ "scope": "tree" }),
        CaptureScope::Subtree(id) => json!({ "scope": "subtree", "id": id }),
    };
    let capture = json!({
        "schema": "schnellui-debug-capture-v1",
        "duration_ms": duration_millis(args.duration),
        "poll_interval_ms": duration_millis(args.poll_interval),
        "target": target,
        "initial": initial,
        "changes": changes,
    });
    print_json_value(&capture, output_filter)
}

fn request_tree(endpoint: &Endpoint, timeout: Duration) -> Result<Value, String> {
    let bytes = request_with_timeout(endpoint, "GET", "/v1/tree", None, timeout)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid tree JSON from debug server: {error}"))
}

fn capture_initial_value(
    tree: &Value,
    args: &CaptureArgs,
) -> Result<(CaptureScope, Value), String> {
    if args.selector.is_none() && args.id.is_none() && args.role.is_none() && args.name.is_none() {
        return Ok((CaptureScope::Tree, tree.clone()));
    }

    let id = if let Some(selector) = args.selector.as_deref() {
        selected_node_id(tree, selector)?
    } else {
        let matches = matching_nodes(tree, args.id, args.role.as_deref(), args.name.as_deref());
        match matches.as_slice() {
            [node] => node["id"].as_u64().ok_or_else(|| {
                "matching semantic node has no numeric id and cannot be captured".to_string()
            })?,
            [] => return Err("capture target did not match any semantic node".into()),
            _ => {
                return Err(format!(
                    "capture target matched {} semantic nodes; exactly one is required",
                    matches.len()
                ))
            }
        }
    };
    let initial = find_node_by_id(tree, id)
        .cloned()
        .ok_or_else(|| format!("capture target node {id} is not present in the tree"))?;
    Ok((CaptureScope::Subtree(id), initial))
}

fn capture_value(tree: &Value, scope: CaptureScope) -> Value {
    match scope {
        CaptureScope::Tree => tree.clone(),
        CaptureScope::Subtree(id) => find_node_by_id(tree, id).cloned().unwrap_or(Value::Null),
    }
}

fn selected_node_id(tree: &Value, selector: &str) -> Result<u64, String> {
    let selected = jq_values(tree, selector)?;
    match selected.as_slice() {
        [value] => value
            .as_u64()
            .or_else(|| value["id"].as_u64())
            .ok_or_else(|| {
                format!("selector {selector:?} did not produce a node object or numeric node id")
            }),
        [] => Err(format!(
            "selector {selector:?} did not produce a node object or numeric node id"
        )),
        _ => Err(format!(
            "selector {selector:?} produced {} values; exactly one node is required",
            selected.len()
        )),
    }
}

fn matching_nodes<'a>(
    tree: &'a Value,
    id: Option<u64>,
    role: Option<&str>,
    name: Option<&str>,
) -> Vec<&'a Value> {
    fn collect<'a>(
        node: &'a Value,
        id: Option<u64>,
        role: Option<&str>,
        name: Option<&str>,
        matches: &mut Vec<&'a Value>,
    ) {
        if id.is_none_or(|id| node["id"].as_u64() == Some(id))
            && role.is_none_or(|role| node["role"].as_str() == Some(role))
            && name.is_none_or(|name| node["name"].as_str() == Some(name))
        {
            matches.push(node);
        }
        if let Some(children) = node["children"].as_array() {
            for child in children {
                collect(child, id, role, name, matches);
            }
        }
    }

    let mut matches = Vec::new();
    if let Some(root) = tree.get("root").filter(|root| !root.is_null()) {
        collect(root, id, role, name, &mut matches);
    }
    matches
}

fn find_node_by_id(tree: &Value, id: u64) -> Option<&Value> {
    fn find(node: &Value, id: u64) -> Option<&Value> {
        if node["id"].as_u64() == Some(id) {
            return Some(node);
        }
        node["children"]
            .as_array()?
            .iter()
            .find_map(|child| find(child, id))
    }

    tree.get("root").and_then(|root| find(root, id))
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn wait_for(
    endpoint: &Endpoint,
    args: &WaitArgs,
    output_filter: Option<&str>,
) -> Result<(), String> {
    let has_selector = args.id.is_some()
        || args.role.is_some()
        || args.name.is_some()
        || args.value.is_some()
        || !args.state.is_empty();
    if !has_selector && (args.absent || args.count.is_some()) {
        return Err("--absent and --count require a semantic selector".into());
    }
    let has_remount_condition = args.remounts.is_some() || args.remount_count.is_some();
    if args.remount_reason.is_some() && !has_remount_condition {
        return Err("--remount-reason requires --remounts or --remount-count".into());
    }
    if !has_selector && args.selector.is_none() && !has_remount_condition {
        return Err(
            "wait requires a semantic selector and/or --remounts; see `schnellui-debug wait --help`"
                .into(),
        );
    }

    let started = Instant::now();
    let deadline = started.checked_add(args.timeout).unwrap_or(started);
    let mut baseline_remounts = None;

    loop {
        let now = Instant::now();
        let request_timeout = deadline
            .saturating_duration_since(now)
            .min(Duration::from_secs(2))
            .max(Duration::from_millis(1));
        let last_observation = match request_with_timeout(
            endpoint,
            "GET",
            "/v1/snapshot",
            None,
            request_timeout,
        ) {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(snapshot) => {
                    let current_remounts = if has_remount_condition {
                        Some(snapshot_remount_count(
                            &snapshot,
                            args.remount_reason.as_deref(),
                        )?)
                    } else {
                        None
                    };
                    let baseline = baseline_remounts.get_or_insert(current_remounts.unwrap_or(0));
                    let observed_remounts = current_remounts
                        .map(|current| current.saturating_sub(*baseline))
                        .unwrap_or(0);
                    let match_count = if has_selector {
                        matching_node_count(&snapshot["tree"], args)
                    } else {
                        0
                    };
                    let target_matches = if !has_selector {
                        true
                    } else if args.absent {
                        match_count == 0
                    } else {
                        match_count >= args.count.unwrap_or(1)
                    };
                    let jq_matches = args
                        .selector
                        .as_deref()
                        .map(|filter| jq_matches(&snapshot, filter))
                        .transpose()?
                        .unwrap_or(true);
                    let remounts_match = args
                        .remounts
                        .map(|required| observed_remounts >= required)
                        .unwrap_or(true)
                        && args
                            .remount_count
                            .map(|required| current_remounts.unwrap_or(0) >= required)
                            .unwrap_or(true);
                    let observation = format!(
                        "{match_count} matching node(s), jq condition {jq_matches}, {observed_remounts}/{} relative remount(s), {}/{} absolute remount(s)",
                        args.remounts.unwrap_or(0),
                        current_remounts.unwrap_or(0),
                        args.remount_count.unwrap_or(0),
                    );
                    if target_matches && jq_matches && remounts_match {
                        print_json_value(&snapshot, output_filter)?;
                        return Ok(());
                    }
                    observation
                }
                Err(error) => format!("invalid snapshot JSON: {error}"),
            },
            Err(error) => error,
        };

        let now = Instant::now();
        if now >= deadline {
            return Err(format!(
                "timed out after {} waiting for conditions ({last_observation})",
                display_duration(args.timeout)
            ));
        }
        std::thread::sleep(
            args.poll_interval
                .min(deadline.saturating_duration_since(now)),
        );
    }
}

fn selector_target(
    endpoint: &Endpoint,
    selector: Option<&str>,
    id: Option<u64>,
) -> Result<Option<u64>, String> {
    let Some(selector) = selector else {
        return Ok(id);
    };
    let bytes = request(endpoint, "GET", "/v1/tree", None)?;
    let tree: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid tree JSON from debug server: {error}"))?;
    let selected = jq_values(&tree, selector)?;
    let ids = selected
        .iter()
        .filter_map(|value| value.as_u64().or_else(|| value["id"].as_u64()))
        .collect::<Vec<_>>();
    match ids.as_slice() {
        [id] => Ok(Some(*id)),
        [] => Err(format!(
            "selector {selector:?} did not produce a node object or numeric node id"
        )),
        _ => Err(format!(
            "selector {selector:?} produced {} target nodes; exactly one is required",
            ids.len()
        )),
    }
}

fn jq_matches(input: &Value, filter: &str) -> Result<bool, String> {
    Ok(jq_values(input, filter)?
        .iter()
        .any(|value| !matches!(value, Value::Null | Value::Bool(false))))
}

fn jq_values(input: &Value, filter: &str) -> Result<Vec<Value>, String> {
    let arena = Arena::default();
    let definitions = jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs());
    let loader = Loader::new(definitions);
    let modules = loader
        .load(
            &arena,
            JaqFile {
                code: filter,
                path: (),
            },
        )
        .map_err(|error| format!("invalid jq selector {filter:?}: {error:?}"))?;
    let functions = jaq_core::funs()
        .chain(jaq_std::funs())
        .chain(jaq_json::funs());
    let compiled = Compiler::default()
        .with_funs(functions)
        .compile(modules)
        .map_err(|error| format!("invalid jq selector {filter:?}: {error:?}"))?;
    let input: JaqValue = serde_json::from_value(input.clone())
        .map_err(|error| format!("cannot convert JSON for jq: {error}"))?;
    let context = Ctx::<data::JustLut<JaqValue>>::new(&compiled.lut, Vars::new([]));
    compiled
        .id
        .run((context, input))
        .map(unwrap_valr)
        .map(|result| {
            let value = result.map_err(|error| format!("jq selector failed: {error}"))?;
            let mut bytes = Vec::new();
            jaq_json::write::write(&mut bytes, &jaq_json::write::Pp::default(), 0, &value)
                .map_err(|error| format!("cannot encode jq result: {error}"))?;
            serde_json::from_slice(&bytes)
                .map_err(|error| format!("jq produced a non-JSON result: {error}"))
        })
        .collect()
}

fn print_json_bytes(bytes: &[u8], filter: Option<&str>) -> Result<(), String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("debug server returned invalid JSON: {error}"))?;
    print_json_value(&value, filter)
}

fn print_json_value(value: &Value, filter: Option<&str>) -> Result<(), String> {
    let values = match filter {
        Some(filter) => jq_values(value, filter)?,
        None => vec![value.clone()],
    };
    let mut stdout = std::io::stdout().lock();
    for value in values {
        serde_json::to_writer_pretty(&mut stdout, &value).map_err(|error| error.to_string())?;
        stdout.write_all(b"\n").map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn snapshot_remount_count(snapshot: &Value, reason: Option<&str>) -> Result<u64, String> {
    let remounts = snapshot
        .pointer("/status/remounts")
        .ok_or_else(|| "debug server snapshot has no remount counters".to_string())?;
    match reason {
        Some(reason) => Ok(remounts
            .get("by_reason")
            .and_then(Value::as_object)
            .and_then(|counts| counts.get(reason))
            .and_then(Value::as_u64)
            .unwrap_or(0)),
        None => remounts
            .get("total")
            .and_then(Value::as_u64)
            .ok_or_else(|| "debug server snapshot has no total remount counter".into()),
    }
}

fn matching_node_count(tree: &Value, args: &WaitArgs) -> usize {
    fn count(node: &Value, args: &WaitArgs) -> usize {
        let matches = args.id.is_none_or(|id| node["id"].as_u64() == Some(id))
            && args
                .role
                .as_deref()
                .is_none_or(|role| node["role"].as_str() == Some(role))
            && args
                .name
                .as_deref()
                .is_none_or(|name| node["name"].as_str() == Some(name))
            && args
                .value
                .as_deref()
                .is_none_or(|value| node["value"].as_str() == Some(value))
            && args.state.iter().all(|required| {
                node["state"].as_array().is_some_and(|states| {
                    states.iter().any(|state| state.as_str() == Some(required))
                })
            });
        usize::from(matches)
            + node["children"]
                .as_array()
                .map(|children| children.iter().map(|child| count(child, args)).sum())
                .unwrap_or(0)
    }

    tree.get("root").map(|root| count(root, args)).unwrap_or(0)
}

fn display_duration(duration: Duration) -> String {
    if duration.subsec_millis() == 0 {
        format!("{}s", duration.as_secs())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

enum Output {
    Stdout,
    File(PathBuf),
}

fn action_body(
    action: String,
    id: Option<u64>,
    role: Option<String>,
    name: Option<String>,
    value: Option<String>,
) -> Result<String, String> {
    if id.is_none() && role.is_none() {
        return Err("action target requires --id or --role".into());
    }
    Ok(json!({
        "action": action,
        "target": { "id": id, "role": role, "name": name },
        "value": value,
    })
    .to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Endpoint {
    Tcp(String),
    Unix(PathBuf),
}

#[derive(Clone, Debug)]
struct Discovery {
    info_path: PathBuf,
    pid: u32,
    title: String,
    endpoint: Endpoint,
}

impl Discovery {
    fn display_value(&self) -> Value {
        match &self.endpoint {
            Endpoint::Tcp(url) => json!({
                "pid": self.pid,
                "title": self.title,
                "transport": "tcp",
                "url": url,
                "info": self.info_path,
            }),
            Endpoint::Unix(socket) => json!({
                "pid": self.pid,
                "title": self.title,
                "transport": "unix",
                "socket": socket,
                "info": self.info_path,
            }),
        }
    }
}

fn discover_endpoint(cli: &Cli) -> Result<Endpoint, String> {
    if let Some(socket) = cli
        .socket
        .clone()
        .or_else(|| std::env::var_os("SCHNELLUI_DEBUG_SOCKET").map(PathBuf::from))
    {
        return Ok(Endpoint::Unix(socket));
    }
    if let Some(url) = cli
        .url
        .as_deref()
        .map(str::to_string)
        .or_else(|| std::env::var("SCHNELLUI_DEBUG_URL").ok())
    {
        return Ok(Endpoint::Tcp(url.trim_end_matches('/').to_string()));
    }
    if let Some(path) = cli
        .info
        .clone()
        .or_else(|| std::env::var_os("SCHNELLUI_DEBUG_INFO").map(PathBuf::from))
    {
        return read_discovery(&path).map(|discovery| discovery.endpoint);
    }

    let mut applications = scan_discovery()?;
    if let Some(pid) = cli.pid {
        applications.retain(|application| application.pid == pid);
    }
    if let Some(title) = cli.title.as_deref() {
        applications.retain(|application| application.title == title);
    }
    match applications.as_slice() {
        [] => Err("no matching live app; run `schnellui-debug list` to discover apps".into()),
        [application] => Ok(application.endpoint.clone()),
        _ => Err(format!(
            "{} applications match; select one with --pid, --title, --info, or --socket",
            applications.len()
        )),
    }
}

fn scan_discovery() -> Result<Vec<Discovery>, String> {
    let mut candidates = fs::read_dir(std::env::temp_dir())
        .map_err(|error| format!("cannot scan temporary directory: {error}"))?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("schnellui-debug-") && name.ends_with(".json"))
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    Ok(candidates
        .into_iter()
        .filter_map(|(_, path)| read_discovery(&path).ok())
        .filter(|application| endpoint_is_reachable(&application.endpoint))
        .collect())
}

fn endpoint_is_reachable(endpoint: &Endpoint) -> bool {
    match endpoint {
        Endpoint::Tcp(url) => url
            .strip_prefix("http://")
            .and_then(|authority| authority.parse::<SocketAddr>().ok())
            .is_some_and(|address| {
                TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok()
            }),
        #[cfg(unix)]
        Endpoint::Unix(socket) => UnixStream::connect(socket).is_ok(),
        #[cfg(not(unix))]
        Endpoint::Unix(_) => false,
    }
}

fn read_discovery(path: &Path) -> Result<Discovery, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{}: invalid discovery JSON: {error}", path.display()))?;
    let pid = value["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .ok_or_else(|| format!("{}: discovery JSON has no valid pid", path.display()))?;
    let title = value["title"]
        .as_str()
        .ok_or_else(|| format!("{}: discovery JSON has no title", path.display()))?
        .to_string();
    let endpoint = match value["transport"].as_str() {
        Some("unix") => Endpoint::Unix(
            value["socket"]
                .as_str()
                .map(PathBuf::from)
                .ok_or_else(|| format!("{}: discovery JSON has no socket", path.display()))?,
        ),
        Some("tcp") | None => Endpoint::Tcp(
            value["url"]
                .as_str()
                .map(|url| url.trim_end_matches('/').to_string())
                .ok_or_else(|| format!("{}: discovery JSON has no url", path.display()))?,
        ),
        Some(transport) => {
            return Err(format!(
                "{}: unsupported debug transport {transport:?}",
                path.display()
            ))
        }
    };
    Ok(Discovery {
        info_path: path.to_path_buf(),
        pid,
        title,
        endpoint,
    })
}

fn request(
    endpoint: &Endpoint,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<Vec<u8>, String> {
    request_with_timeout(endpoint, method, path, body, Duration::from_secs(20))
}

fn request_with_timeout(
    endpoint: &Endpoint,
    method: &str,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    match endpoint {
        Endpoint::Tcp(url) => request_tcp(url, method, path, body, timeout),
        Endpoint::Unix(socket) => request_unix(socket, method, path, body, timeout),
    }
}

fn request_tcp(
    url: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let authority = url
        .strip_prefix("http://")
        .ok_or_else(|| "debug URL must begin with http://".to_string())?;
    if authority.contains('/') {
        return Err("debug URL must not contain a path".into());
    }
    let address: SocketAddr = authority
        .parse()
        .map_err(|error| format!("invalid debug URL {url:?}: {error}"))?;
    if !address.ip().is_loopback() {
        return Err("refusing to connect to a non-loopback debug server".into());
    }
    let mut stream = TcpStream::connect_timeout(&address, timeout.min(Duration::from_secs(2)))
        .map_err(|error| format!("cannot connect to {url}: {error}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    request_stream(&mut stream, authority, method, path, body)
}

#[cfg(unix)]
fn request_unix(
    socket: &Path,
    method: &str,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|error| format!("cannot connect to {}: {error}", socket.display()))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    request_stream(&mut stream, "localhost", method, path, body)
}

#[cfg(not(unix))]
fn request_unix(
    socket: &Path,
    _method: &str,
    _path: &str,
    _body: Option<&str>,
    _timeout: Duration,
) -> Result<Vec<u8>, String> {
    Err(format!(
        "Unix-domain sockets are unsupported on this platform: {}",
        socket.display()
    ))
}

fn request_stream(
    stream: &mut (impl Read + Write),
    authority: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<Vec<u8>, String> {
    let body = body.unwrap_or("");
    write!(
        &mut *stream,
        "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .map_err(|error| error.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| format!("response read failed: {error}"))?;
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
        .ok_or_else(|| "invalid HTTP response".to_string())?;
    let headers = std::str::from_utf8(&response[..header_end])
        .map_err(|_| "invalid HTTP response headers".to_string())?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| "invalid HTTP response status".to_string())?;
    let response_body = response[header_end..].to_vec();
    if !(200..300).contains(&status) {
        return Err(format!(
            "server returned HTTP {status}: {}",
            String::from_utf8_lossy(&response_body)
        ));
    }
    Ok(response_body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_action_requires_a_target() {
        assert!(action_body("click".into(), None, None, None, None).is_err());
        assert!(action_body("click".into(), None, Some("button".into()), None, None).is_ok());
    }

    #[test]
    fn explicit_url_wins_discovery() {
        let cli = Cli {
            url: Some("http://127.0.0.1:1234/".into()),
            socket: None,
            info: None,
            pid: None,
            title: None,
            jq: None,
            command: Command::Status,
        };
        assert_eq!(
            discover_endpoint(&cli).unwrap(),
            Endpoint::Tcp("http://127.0.0.1:1234".into())
        );
    }

    #[test]
    fn parses_human_durations() {
        assert_eq!(parse_duration("25ms").unwrap(), Duration::from_millis(25));
        assert_eq!(parse_duration("2s").unwrap(), Duration::from_secs(2));
        assert_eq!(parse_duration("3m").unwrap(), Duration::from_secs(180));
        assert_eq!(parse_duration("4").unwrap(), Duration::from_secs(4));
        assert!(parse_nonzero_duration("0ms").is_err());
    }

    #[test]
    fn jq_selects_nested_semantic_nodes() {
        let tree = json!({
            "root": {
                "id": 1,
                "role": "group",
                "children": [
                    { "id": 2, "role": "button", "name": "increment" },
                    { "id": 3, "role": "button", "name": "decrement" }
                ]
            }
        });
        let selected = jq_values(
            &tree,
            r#".. | objects | select(.role == "button" and .name == "increment")"#,
        )
        .unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0]["id"], 2);
    }

    #[test]
    fn jq_condition_uses_jq_truthiness() {
        let input = json!({ "ready": true });
        assert!(jq_matches(&input, ".ready").unwrap());
        assert!(!jq_matches(&input, ".missing").unwrap());
        assert!(!jq_matches(&input, "false").unwrap());
    }

    #[test]
    fn capture_defaults_to_the_whole_tree() {
        let tree = sample_tree();
        let args = capture_args();
        let (scope, initial) = capture_initial_value(&tree, &args).unwrap();
        assert_eq!(scope, CaptureScope::Tree);
        assert_eq!(initial, tree);
    }

    #[test]
    fn capture_selects_and_pins_one_subtree_by_id() {
        let tree = sample_tree();
        let mut args = capture_args();
        args.role = Some("button".into());
        args.name = Some("increment".into());

        let (scope, initial) = capture_initial_value(&tree, &args).unwrap();
        assert_eq!(scope, CaptureScope::Subtree(2));
        assert_eq!(initial["name"], "increment");

        let changed = json!({
            "root": {
                "id": 1,
                "role": "group",
                "children": [
                    { "id": 2, "role": "button", "name": "renamed", "value": "1" }
                ]
            }
        });
        assert_eq!(capture_value(&changed, scope)["name"], "renamed");
        assert!(capture_value(&json!({ "root": null }), scope).is_null());
    }

    #[test]
    fn capture_rejects_ambiguous_subtree_matches() {
        let tree = sample_tree();
        let mut args = capture_args();
        args.role = Some("button".into());
        let error = capture_initial_value(&tree, &args).unwrap_err();
        assert!(error.contains("matched 2 semantic nodes"), "{error}");
    }

    #[test]
    fn reads_total_and_reasoned_remount_counts() {
        let snapshot = json!({
            "status": {
                "remounts": {
                    "total": 7,
                    "by_reason": { "route_changed": 3 }
                }
            }
        });
        assert_eq!(snapshot_remount_count(&snapshot, None).unwrap(), 7);
        assert_eq!(
            snapshot_remount_count(&snapshot, Some("route_changed")).unwrap(),
            3
        );
        assert_eq!(
            snapshot_remount_count(&snapshot, Some("never_seen")).unwrap(),
            0
        );
    }

    #[test]
    fn script_lines_use_shell_quoting_and_parse_normal_commands() {
        let words = shell_words::split(
            r#"action set_value --role text_input --name "User name" --value 'Ada Lovelace'"#,
        )
        .unwrap();
        let invocation = ScriptInvocation::try_parse_from(words).unwrap();
        let Command::Action {
            action,
            role,
            name,
            value,
            ..
        } = invocation.command
        else {
            panic!("script line parsed as wrong command");
        };
        assert_eq!(action, "set_value");
        assert_eq!(role.as_deref(), Some("text_input"));
        assert_eq!(name.as_deref(), Some("User name"));
        assert_eq!(value.as_deref(), Some("Ada Lovelace"));
    }

    fn sample_tree() -> Value {
        json!({
            "root": {
                "id": 1,
                "role": "group",
                "children": [
                    { "id": 2, "role": "button", "name": "increment" },
                    { "id": 3, "role": "button", "name": "decrement" }
                ]
            }
        })
    }

    fn capture_args() -> CaptureArgs {
        CaptureArgs {
            selector: None,
            id: None,
            role: None,
            name: None,
            duration: Duration::from_secs(1),
            poll_interval: Duration::from_millis(50),
        }
    }
}
