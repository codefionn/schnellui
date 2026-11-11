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
- `key_input`: one key, typed text or IME commit, from the window
  (`"origin": "window"`) or from `schnellui-debug key` and `type`
  (`"origin": "debug"`). Fields: `key`, `state` (`pressed` or `released`),
  `repeat`, `shift`, `ctrl`, `alt`, `super`, and `text_len` when text came with
  it. Window keys are recorded before any handler can consume them. Debug
  `type` shows as `"key": "text"`; an IME commit as `"key": "ime_commit"`.
- `a11y_action`: an accessibility action, from `schnellui-debug action`
  (`"origin": "debug"`) or the platform adapter (`"origin": "accesskit"`).
  Fields: `action` (snake_case), `target` with `id`, `role` and `name`, and
  `value_len` for `set_value`.
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

Keys, actions and pointer records share one sequence, so the timing of a key
during a slider drag reads straight off the file:

```sh
jq -c 'select(.event == "pointer_button" or .event == "key_input" or .event == "a11y_action")
  | {sequence, elapsed_us, event, key, action, origin}' /tmp/schnellui-interaction.jsonl
```

Semantic names and editable values are included because they are needed to
identify routing mistakes. Treat traces as application data when sharing them.

Typed text is the exception. `key_input` and `a11y_action` record only the
length of typed, IME and `set_value` text (`text_len`, `value_len`), because it
can be a password or a message. To record it verbatim, set
`SCHNELLUI_INTERACTION_TRACE_TEXT=1` or call
`InteractionTrace::include_text(true)`; the record then also carries `text` or
`value`.

## Drive the pointer from a debug session

`schnellui-debug` can press, release, drag and leave through the window's own
`CursorMoved`, `MouseInput` and `CursorLeft` handlers, so a headless session
sees what a real mouse produces (and the trace above records it):

```sh
schnellui-debug move 120 80                  # hover
schnellui-debug press 120 80                 # move there, press left
schnellui-debug press --button right         # press where the pointer is
schnellui-debug release 200 80               # move, then release left
schnellui-debug drag 10 80 300 80 --steps 12 # press, 12 moves, release
schnellui-debug leave                        # pointer leaves the window
schnellui-debug wheel 120 80 0 -3             # 3 notches down at 120,80
schnellui-debug resize 800 600               # headless-windowed surface, logical px
schnellui-debug ime commit '日本'             # also: ime preedit TEXT --cursor A,B, ime cancel
```

`press` and `release` take an optional `X Y` (both or neither) and
`--button left|right|middle` (default left). `drag` takes `X1 Y1 X2 Y2`,
`--steps N` (default 8, at most 1000) and `--button`. Each command settles a
frame and prints the same status JSON as `move`, with `cursor`,
`cursor_inside`, `hit_path`, `focused` and a `pointer` object
(`left_down`, `raw_captured`, `text_drag`, `slider_drag`, `scrollbar_drag`).
Presses close together in app time at one spot count as double and triple
clicks, as they do with a mouse. A press on a plain button clicks at once, as
the window does; only drag-source widgets wait for the release.
