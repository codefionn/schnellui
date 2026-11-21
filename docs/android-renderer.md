# Android renderer

`schnellui::render_android` turns a template into real Android views. A `Button` template becomes an `android.widget.Button`, and Android does the layout. The retained scene, taffy and wgpu are not involved.

Rust renders the template into a node tree, diffs it against the previous tree and encodes the difference as an op buffer. A Java host library applies the buffer to views in one JNI call per update. View events travel back to Rust as (node id, event, payload).

The feature is `render-android`. The op stream and the diff are plain Rust and build on every target, so they are tested on the development machine. Only the JNI glue is `cfg(target_os = "android")`.

## Units

Sizes are dp. Text sizes are sp. Rust logical pixels equal dp.

## Op buffer

Integers and floats are little-endian. A string is a `u32` byte length followed by UTF-8.

A buffer starts with two bytes, magic `0x53` and version `0x01`. Ops follow until the buffer ends.

| Code | Op | Fields | Effect |
|------|----|--------|--------|
| 0x01 | CREATE | id u32, kind u8 | Create a detached view. |
| 0x02 | SET_STR | id u32, prop u8, string | Set a string property. |
| 0x03 | SET_F32 | id u32, prop u8, f32 | Set a float property. |
| 0x04 | SET_I32 | id u32, prop u8, i32 | Set an integer, enum or bool (0 or 1) property. |
| 0x05 | CLEAR | id u32, prop u8 | Put a property back to its default. |
| 0x06 | INSERT | parent u32, child u32, index u32 | Attach the child, or move it, so it sits at `index` among the parent's children. |
| 0x07 | REMOVE | id u32 | Detach the view and forget it and all its descendants. |
| 0x08 | SET_ROOT | id u32 | Make the view the content of the host container. |

Ids are non-zero and never reused within one mount. A CREATE comes before any other op that names its id. The host must not assume any other ordering. An unknown op code is a decode error. An unknown prop or kind is skipped with a log line, so an older host survives a newer app.

## View kinds

| Code | Kind | Android view |
|------|------|--------------|
| 1 | Row | horizontal `LinearLayout` |
| 2 | Column | vertical `LinearLayout` |
| 3 | Stack | `FrameLayout` |
| 4 | Scroll | `ScrollView` around a vertical `LinearLayout`. Children go into the inner layout. |
| 5 | Text | `TextView` |
| 6 | Button | `Button` |
| 7 | Checkbox | `CheckBox` |
| 8 | Slider | `SeekBar` |
| 9 | TextInput | `EditText` |
| 10 | Spacer | `Space` |

## Properties

| Code | Prop | Type | Applies to | Default |
|------|------|------|-----------|---------|
| 1 | WIDTH | f32 | all | wrap content |
| 2 | HEIGHT | f32 | all | wrap content |
| 3 | MIN_WIDTH | f32 | all | 0 |
| 4 | MIN_HEIGHT | f32 | all | 0 |
| 5 | MAX_WIDTH | f32 | all | none |
| 6 | MAX_HEIGHT | f32 | all | none |
| 7 | FILL | bool | all | false. True means match parent on both axes. WIDTH or HEIGHT still wins on its axis. |
| 8 | GROW | f32 | all | 0. `LinearLayout` weight on the parent's main axis. |
| 9 | BASIS | f32 | all | none. Main-axis size before GROW. With GROW above 0 and no BASIS the main-axis size is 0. |
| 10 | PAD_LEFT | f32 | all | view default |
| 11 | PAD_TOP | f32 | all | view default |
| 12 | PAD_RIGHT | f32 | all | view default |
| 13 | PAD_BOTTOM | f32 | all | view default |
| 14 | VISIBLE | bool | all | true. False is `View.GONE`. |
| 15 | ENABLED | bool | all | true |
| 20 | JUSTIFY | i32 | Row, Column | 0 start, 1 center, 2 end, 3 space-between, 4 space-around, 5 space-evenly |
| 21 | ALIGN | i32 | Row, Column, Stack | 0 start, 1 center, 2 end, 3 stretch |
| 22 | GAP | f32 | Row, Column | 0. Space between adjacent children. |
| 23 | WRAP | bool | Row, Column | false. Hosts may ignore it for now. |
| 24 | SCROLLBAR | bool | Scroll | false |
| 30 | TEXT | string | Text, Button, TextInput | empty |
| 31 | TEXT_SIZE | f32 | Text | view default |
| 32 | TEXT_WRAP | i32 | Text | 0 no wrap, 1 word, 2 anywhere |
| 33 | TEXT_ALIGN | i32 | Text | 0 start, 1 center, 2 end, 3 justify |
| 34 | ELLIPSIS | bool | Text | false |
| 35 | ROLE | i32 | all | none. The `schnellui::a11y::Role` tag. Heading roles set `setAccessibilityHeading(true)`. |
| 36 | A11Y_LABEL | string | all | none. `contentDescription`. On TextInput it is also the hint. |
| 40 | CHECKED | bool | Checkbox | false |
| 41 | VALUE | f32 | Slider | 0 |
| 42 | MIN | f32 | Slider | 0 |
| 43 | MAX | f32 | Slider | 100 |
| 44 | STEP | f32 | Slider | 0, which means continuous. The host then uses 1000 positions. |
| 45 | PASSWORD | bool | TextInput | false |
| 46 | APPEARANCE | i32 | Button | 0 solid, 1 ghost (borderless button style) |

## Events

| Code | Event | Sent by | Payload |
|------|-------|---------|---------|
| 1 | CLICK | Button | none |
| 2 | TOGGLE | Checkbox | `i` is 0 or 1 |
| 3 | CHANGE | Slider | `f` is the value between MIN and MAX |
| 4 | INPUT | TextInput | `s` is the whole text |

The host attaches listeners to every view of these kinds and reports every user change. Rust ignores events for nodes without a handler.

The host sends no event for a change it made itself while applying ops. A SET that matches what the view already shows is a no-op, so the cursor and IME composition of an `EditText` survive a round trip.

Rust records the reported value in its previous tree before it runs the handler. If the handler rejects the change, the diff then contains a SET that puts the view back.

## JNI

The host class is `io.github.codefionn.schnellui.SchnellBridge`. All calls happen on the Android main thread.

```java
static native long nativeMount(float widthDp, float heightDp, float density, boolean dark);
static native byte[] nativeRender(long app);
static native byte[] nativeDispatch(long app, int node, int event, int i, float f, String s);
static native byte[] nativeResize(long app, float widthDp, float heightDp);
static native void nativeUnmount(long app);
```

- `nativeMount` builds the app and returns its handle.
- `nativeRender` returns a buffer that builds the whole tree from nothing and ends with SET_ROOT. A host calls it once after mount, and again whenever it needs to rebuild its views.
- `nativeDispatch` and `nativeResize` return the diff, or `null` when nothing changed.
- `s` is `null` for events without a string payload.

An app exports these symbols with `schnellui::android_app!(|| view)` from a `cdylib`. The host loads the library named by the `schnellui.lib` meta-data entry of the activity.

## Template mapping

- `container` creates Row, Column, Stack or Scroll with the container props.
- `pad` with a child creates a Stack that carries the PAD props. Without a child it creates a Spacer with the PAD props.
- `spacer` creates a Spacer with GROW 1.
- `flex` with a child sets WIDTH, HEIGHT, the MIN and MAX props, GROW and BASIS on the child's own node. If the `FlexChild` has padding, the child is first wrapped in a Stack with the PAD props and the layout props go on the wrapper. Without a child it creates a Spacer with those props. `shrink` has no Android equivalent and is dropped.
- `responsive` with a viewport target sets VISIBLE from the current viewport size, and is re-evaluated on resize. Parent and component targets are always visible for now.
- `keyed` gives the node its key for the diff. `component_ref` returns the child unchanged for now.
- `text`, `button`, `checkbox`, `slider` and `text_input` create the matching kind. Dynamic text is evaluated at each render.
- `component` maps a `ComponentKind` that has a kind above onto it. Every other kind renders as a Column of its children, led by a Text of its label when the label is not empty. Later stages replace these fallbacks with native widgets.

## Diff

Children are matched by key first, then by position and kind among unkeyed siblings. A matched node keeps its id and gets SET or CLEAR ops for changed props only. A keyed node that changed position gets one INSERT and keeps its view. An unmatched old node gets REMOVE. An unmatched new node gets CREATE, its props and an INSERT. A node whose kind changed is removed and created again.

## Not yet

Signal changes that happen outside an event or resize (timers, threads) do not reach the host. Theme colors, dark-mode palettes from `Theme`, and every `ComponentKind` without a base-set view are later stages.
