# Live debug instrumentation

Native SchnellUI windows built in debug mode start a local HTTP service. On Unix it
uses a private Unix-domain socket; on other platforms it asks the OS for a free
ephemeral loopback port. The service exposes the live semantic tree, screenshots,
pointer state, keyboard input, and AccessKit actions. Commands execute on the UI
thread and settle a frame before replying, so a CLI agent can inspect immediately
after an interaction without a desktop automation tool or timing sleeps.

At startup the application prints its URL:

```text
schnellui debug instrumentation listening on unix:///tmp/schnellui-debug-1234-….sock
```

Every process writes a unique `/tmp/schnellui-debug-<pid>-<nonce>.json` discovery
record. With one application running, the first-party CLI selects it automatically:

```sh
cargo run -p schnellui-debug -- status
cargo run -p schnellui-debug -- tree
cargo run -p schnellui-debug -- snapshot
cargo run -p schnellui-debug -- capture --duration 5s
cargo run -p schnellui-debug -- capture --role dialog --name Settings --duration 10s
cargo run -p schnellui-debug -- wait --role dialog --name Settings --timeout 5s
cargo run -p schnellui-debug -- wait --role document --name Editor --remounts 2
cargo run -p schnellui-debug -- wait --remounts 1 --remount-reason route_changed
cargo run -p schnellui-debug -- wait --remount-count 3 --remount-reason route_changed
cargo run -p schnellui-debug -- --jq '.remounts.total' status
cargo run -p schnellui-debug -- tree --jq '.. | objects | select(.role == "button")'
cargo run -p schnellui-debug -- click \
  --selector '.. | objects | select(.role == "button" and .name == "increment")'
cargo run -p schnellui-debug -- script <<'EOF'
wait --role button --name increment
click --role button --name increment
wait --selector '.tree | .. | objects | select(.role == "status" and .value == "1")'
EOF
cargo run -p schnellui-debug -- click --role button --name increment
cargo run -p schnellui-debug -- action set_value --role text_input --value 'hello'
cargo run -p schnellui-debug -- move 120 80
cargo run -p schnellui-debug -- click-at 120 80
cargo run -p schnellui-debug -- key tab
cargo run -p schnellui-debug -- type 'typed into the focused input'
cargo run -p schnellui-debug -- screenshot /tmp/live.png
cargo run -p schnellui-debug -- quit
```

Several applications can run side by side without endpoint collisions. List them,
then select one by process id or exact window title:

```sh
cargo run -p schnellui-debug -- list
cargo run -p schnellui-debug -- --pid 1234 tree
cargo run -p schnellui-debug -- --title counter click --role button --name increment
```

When more than one application matches, the CLI refuses to guess. `--info` and
`--socket` provide unambiguous alternatives. Standard HTTP tooling can use the
same Unix socket:

```sh
curl --unix-socket /tmp/schnellui-debug-1234-….sock http://localhost/v1/tree
curl --unix-socket /tmp/schnellui-debug-1234-….sock \
  -X POST http://localhost/v1/action \
  -H 'content-type: application/json' \
  -d '{"action":"click","target":{"role":"button","name":"increment"}}'
```

The endpoints are `GET /v1/status`, `GET /v1/tree`, `GET /v1/snapshot`,
`GET /v1/screenshot`, and
`POST /v1/action`, `/v1/pointer/move`, `/v1/pointer/click`, `/v1/key`, `/v1/quit`.
Tree nodes include their logical-pixel `rect`. Semantic targets accept either
`{"id": <tree id>}` or a stable role/name query.
Supported actions are `click`, `focus`, `blur`, `increment`, `decrement`,
`scroll_up`, `scroll_down`, `show_context_menu`, and `set_value`.

## Capturing tree changes

`capture --duration DURATION` records the initial accessibility tree and every
distinct tree value observed during that time. It polls every 50ms by default and
takes a final observation at the end of the duration. Use `--poll-interval` to
change the sampling frequency:

```sh
cargo run -p schnellui-debug -- capture --duration 2s --poll-interval 10ms
```

With no target options, `capture` observes the whole tree. `--id`, `--role` and
`--name` select exactly one subtree root; the options can be combined. A jq
`--selector` can select the root instead and must emit exactly one node object or
numeric node id:

```sh
cargo run -p schnellui-debug -- capture \
  --selector '.. | objects | select(.role == "list")' --duration 5s
```

The selected subtree is pinned by its initial node id, so it remains selected when
its properties change. If that node disappears, the next change has `"tree":
null`; if the same id reappears during the capture, it is recorded again.

The result uses the `schnellui-debug-capture-v1` schema. `initial` contains the
whole tree object or selected subtree. `changes` contains only distinct subsequent
values, each with an `elapsed_ms` timestamp and `tree` value. `duration_ms`,
`poll_interval_ms`, and `target` describe how the capture was made. The global
`--jq` option is applied once to the completed capture.

## Waiting for live state

`wait` polls atomic status/tree snapshots until all requested conditions hold. A
semantic target can be selected by id, role, accessible name, value, and repeated
`--state` flags. `--count N` waits for at least N matches; `--absent` waits for no
matches. `--remounts N` is relative to the snapshot taken when the command starts;
`--remount-count N` waits for an absolute session count, which is useful after a
script command synchronously triggered and settled a remount. Either can be
restricted to a stable host remount reason with `--remount-reason`.
Selectors and remount conditions compose, so the second example above waits for two
remounts and for the resulting component tree to contain the named document.

The default timeout is 10 seconds and the default polling interval is 50ms. Both
accept `ms`, `s`, or `m` suffixes. On success the CLI prints the matching snapshot;
on timeout it exits unsuccessfully with the last observed match/remount counts.

## jq selectors

The CLI embeds a jq-compatible evaluator. The global `--jq FILTER` option filters
the JSON result of `list`, `status`, `tree`, `snapshot`, `capture`, `wait`, and every
interaction command.
It can emit zero, one, or several JSON values just like jq. Screenshots are binary,
so combining `screenshot` with `--jq` is rejected.

`click` and `action` also accept `--selector FILTER`. The filter receives the live
semantic tree and must emit exactly one node object (with an `id`) or one numeric
node id. `wait --selector FILTER` receives each atomic `{status, tree}` snapshot
and succeeds when the filter emits any value other than `false` or `null`. This can
be combined with the semantic shorthands and remount conditions; all supplied
conditions must hold in the same snapshot.

## Command scripts

`script [FILE]` runs one ordinary debug command per line against the same selected
application. If `FILE` is omitted or is `-`, commands are read from standard input.
Blank lines and lines whose first non-whitespace character is `#` are ignored, and
arguments use shell-style quoting. `wait`, jq selectors, interactions, screenshots,
and `quit` can be mixed in one script. Nested `script` commands and `list` are
rejected. Execution stops at the first error and reports the source line number.

For example:

```text
# settings-flow.sui
wait --role button --name Settings --timeout 5s
click --role button --name Settings
wait --selector '.tree | .. | objects | select(.role == "dialog")'
action set_value --role text_input --name Username --value 'Ada Lovelace'
tree --jq '.. | objects | select(.state | index("focused"))'
```

Run it with `schnellui-debug --title MyApp script settings-flow.sui`. A global
`--jq` on the outer command filters every JSON result; a `--jq` on an individual
script line overrides it for that command.

## Configuration and safety

Unix socket files are unique per process and mode `0600`. Windows and explicit
Unix TCP mode bind `127.0.0.1:0`, which always asks the OS for a free, unprivileged
ephemeral port. There is no fixed-port fallback.

- `SCHNELLUI_DEBUG_SERVER=off` disables startup.
- `SCHNELLUI_DEBUG_SOCKET=/path/app.sock` selects a Unix socket path.
- `SCHNELLUI_DEBUG_TRANSPORT=tcp` requests ephemeral loopback TCP on Unix.
- `SCHNELLUI_DEBUG_INFO=/path/app.json` selects the discovery-file path.
- `SCHNELLUI_DEBUG_URL=http://127.0.0.1:<ephemeral-port>` selects TCP in the CLI.

Optimized builds omit automatic startup. Applications that deliberately want the
same facility in an optimized build can enable SchnellUI's
`debug-instrumentation` Cargo feature. Do not expose or forward the port: the API
is intentionally unauthenticated because it is restricted to the local machine
and debug workflows.
