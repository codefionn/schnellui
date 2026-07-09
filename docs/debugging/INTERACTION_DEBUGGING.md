# Interaction and remount tracing

SchnellUI's native host can write an eagerly flushed JSONL trace of pointer
routing, cursor selection, focus, pointer capture, and structural remounts. The
trace is opt-in and adds no event-path work when disabled.

## Enable a trace

The fastest route requires no application changes:

```sh
SCHNELLUI_INTERACTION_TRACE=/tmp/schnellui-interaction.jsonl your-app
```

Use `-` or `stderr` instead of a path to stream records to standard error. A
path is created or truncated when the native event loop starts. Records are
flushed after every event so the tail survives a crash or forced termination.

Applications can configure the same recorder explicitly and optionally omit
high-volume pointer moves:

```rust
use schnellui::{App, InteractionTrace};

let mut app = App::mount(root_view);
app.set_interaction_trace(
    InteractionTrace::file("/tmp/schnellui-interaction.jsonl")
        .include_pointer_moves(false),
);
app.run_windowed("Example")?;
```

Explicit configuration takes priority over `SCHNELLUI_INTERACTION_TRACE`.

## Give remounts reasons

The existing `run_windowed_with*` methods remain compatible and report their
reason as `unspecified`. New code should use a reasoned hook:

```rust
use schnellui::{App, Remount};

app.run_windowed_with_viewport_reasoned_remount("Example", move |viewport| {
    pending_route.take().map(|route| {
        Remount::new(build_app(route, viewport), "route_changed")
    })
})?;
```

Reasons should be stable identifiers, not dynamic descriptions. This keeps
traces easy to group and compare.

## Read the trace

Every record carries the schema name, a session-local sequence, and elapsed
microseconds. Useful event records include:

- `pointer_move` and `pointer_button`: physical and logical coordinates, the
  semantic leaf-to-root hit path, focused widget, resolved/applied cursor, and
  every pointer-capture owner.
- `cursor_changed`: previous and next native cursor plus the interaction state
  that selected it.
- `content_drag_release`: whether release became a click, accepted drop, or no
  drag action.
- `remount`: stable reason and triggering event, remount count, and complete
  before/after snapshots.
- `interaction_interrupted_by_remount` and
  `interaction_interrupted_by_window_blur`: warnings that an in-flight raw
  pointer, text selection, slider, content drag, or dialog move/resize was cut
  off. These records are the first place to look for ignored clicks.

For a live summary:

```sh
tail -f /tmp/schnellui-interaction.jsonl |
  jq -c 'select(.event == "remount" or (.severity // "") == "warning")'
```

To inspect cursor instability around one control:

```sh
jq -c 'select(.event == "pointer_move" or .event == "cursor_changed") |
  {sequence, event, cursor: (.interaction.cursor // .cursor), hit: (.hit_path[0] // .interaction.hit_path[0])}' \
  /tmp/schnellui-interaction.jsonl
```

Semantic names and editable values are included because they are needed to
identify routing mistakes. Treat traces as application data when sharing them.
