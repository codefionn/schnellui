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
| 0x09 | CREATE_THEMED | id u32, kind u8, theme u8 | CREATE with a fixed theme. Theme 1 is light and 2 is dark. The host builds the view with a `ContextThemeWrapper` over `Theme.DeviceDefault.Light` or `Theme.DeviceDefault`, so the view ignores the system dark mode. |

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
| 11 | Switch | `Switch`. TEXT is the visible label. |
| 12 | Radio | `RadioButton`. TEXT is the visible label. |
| 13 | Dropdown | `Spinner` in dropdown mode |
| 14 | Progress | horizontal determinate `ProgressBar` |
| 15 | Loading | indeterminate circular `ProgressBar` |
| 16 | TextArea | multi-line `EditText`, at least 3 lines tall, text starts at the top |
| 17 | Image | `ImageView`, scaled to fit inside its box |
| 18 | Divider | 1 dp high `View` drawn with the theme's `listDivider`, as wide as its parent |
| 19 | Badge | `TextView` with 12 sp text on a pill-shaped background in the theme accent color |
| 20 | Link | `TextView` in the theme link color, underlined, clickable |
| 21 | Dialog | a zero-size placeholder `View` in its parent. Its children live in an `android.app.Dialog` window. See below. |
| 22 | TabBar | `HorizontalScrollView` around a horizontal `LinearLayout`. Children go into the inner layout. |
| 23 | Tab | borderless `Button`. SELECTED sets the view's selected state, bold text and an accent underline. |
| 25 | Table | `TableLayout`. Columns share their width across rows. |
| 26 | TableRow | `android.widget.TableRow`, at least 48 dp tall, with the `selectableItemBackground` ripple when enabled. It starts with one host-owned `TextView` cell per entry of ITEMS, each with 12 dp horizontal padding. Children follow the cells. SELECTED sets the activated state. |
| 27 | ThemeProvider | vertical `LinearLayout` with 10 dp padding and a background in its theme's `colorBackground` with 8 dp rounded corners |
| 24 | ListItem | horizontal `LinearLayout`, at least 48 dp tall, with the `selectableItemBackground` ripple when enabled. Its first view is a `TextView` showing TEXT, which the host owns. Children follow it. SELECTED sets the activated state. |

## Properties

A Slider, TextInput, TextArea or Progress with no WIDTH, MIN_WIDTH, FILL, GROW or stretch is 160 dp wide. A wrap-content `SeekBar` would be as wide as its thumb.

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
| 30 | TEXT | string | Text, Button, TextInput, Switch, Radio, TextArea, Badge, Link, Tab, ListItem, Dialog (window title) | empty |
| 31 | TEXT_SIZE | f32 | Text | view default |
| 32 | TEXT_WRAP | i32 | Text | 0 no wrap, 1 word, 2 anywhere |
| 33 | TEXT_ALIGN | i32 | Text | 0 start, 1 center, 2 end, 3 justify |
| 34 | ELLIPSIS | bool | Text | false |
| 35 | ROLE | i32 | all | none. The `schnellui::a11y::Role` tag. Heading roles set `setAccessibilityHeading(true)`. |
| 36 | A11Y_LABEL | string | all | none. `contentDescription`. On TextInput and TextArea it is also the hint. |
| 40 | CHECKED | bool | Checkbox, Switch, Radio | false |
| 41 | VALUE | f32 | Slider, Progress | 0 |
| 42 | MIN | f32 | Slider, Progress | 0 |
| 43 | MAX | f32 | Slider, Progress | 100 |
| 44 | STEP | f32 | Slider | 0, which means continuous. The host then uses 1000 positions. |
| 45 | PASSWORD | bool | TextInput | false |
| 46 | APPEARANCE | i32 | Button | 0 solid, 1 ghost (borderless button style) |
| 50 | ITEMS | string | Dropdown, TableRow | empty. The option labels, or the cell texts, joined with U+001F. An empty string means none. |
| 52 | SELECTED_INDEX | i32 | Dropdown | -1, which means no selection. The host then shows the first option. |
| 53 | SRC | string | Image | none. A `data:` URL with base64 PNG, JPEG or WebP, an absolute file path, or `asset:///name` for an APK asset. Other schemes are logged and show nothing. |
| 54 | HREF | string | Link | none. On a click the host opens an `http`, `https` or `mailto` HREF with an `ACTION_VIEW` intent, after it sends CLICK. |
| 55 | SELECTED | bool | Tab, ListItem, TableRow | false |
| 56 | SHOW_TARGET | i32 | all | none. Turns on a size query, see below. 0 is the node's current parent. Any other value is the id of an ancestor node. |
| 57 | SHOW_MIN_W | f32 | all | none |
| 58 | SHOW_MAX_W | f32 | all | none |
| 59 | SHOW_MIN_H | f32 | all | none |
| 60 | SHOW_MAX_H | f32 | all | none |

Codes 47 to 49 and 51 are unused.

## Events

| Code | Event | Sent by | Payload |
|------|-------|---------|---------|
| 1 | CLICK | Button, Radio, Link, Tab, ListItem, TableRow | none |
| 2 | TOGGLE | Checkbox, Switch | `i` is 0 or 1 |
| 3 | CHANGE | Slider | `f` is the value between MIN and MAX |
| 4 | INPUT | TextInput, TextArea | `s` is the whole text |
| 5 | SELECT | Dropdown | `i` is the index of the chosen option |
| 6 | DISMISS | Dialog | none. The user pressed back or tapped outside. |

A `RadioButton` checks itself when tapped. Rust records CHECKED as true for a Radio before it runs the CLICK handler, so a handler that does not select it gets the view unchecked again. For SELECT Rust records SELECTED_INDEX the same way.

The host attaches listeners to every view of these kinds and reports every user change. Rust ignores events for nodes without a handler.

The host sends no event for a change it made itself while applying ops. A SET that matches what the view already shows is a no-op, so the cursor and IME composition of an `EditText` survive a round trip.

Rust records the reported value in its previous tree before it runs the handler. If the handler rejects the change, the diff then contains a SET that puts the view back.

## Size queries

A node with SHOW_TARGET is shown only while the target's content box, in dp, is inside every bound that is set. The content box is the view's size minus its padding. Bounds are inclusive. The host checks after each layout of the target and applies a change outside the layout pass. The node is visible only if the query matches and VISIBLE is not false. Until the target has been laid out once the query counts as matching. CLEAR of SHOW_TARGET turns the query off.

A hidden node can change its target's size, as with a CSS container query. Give the target a definite size.

## Themes

A view made by CREATE_THEMED keeps its theme for its lifetime. Rust stamps the theme on a ThemeProvider node and on every node below it, down to the next ThemeProvider. A node whose theme changes is removed and created again, like a kind change. A Dialog made themed uses that theme for its window.

## Dialog

A Dialog node is a window. The host shows it while the node is attached to the tree under the root, and dismisses it when the node is removed or detached. INSERT with a Dialog as parent puts the child into the dialog's content, a vertical `LinearLayout` with 24 dp padding inside a `ScrollView`. TEXT is the window title. The dialog uses the activity's theme.

The host never closes a dialog on its own. Back and outside taps send DISMISS and leave the window open. The app closes it by rendering without the node. When the activity goes away the host dismisses every dialog window without sending DISMISS, and the next full render shows them again.

## JNI

The host class is `io.github.codefionn.schnellui.SchnellBridge`. All calls happen on the Android main thread.

```java
static native long nativeMount(float widthDp, float heightDp, float density, boolean dark);
static native byte[] nativeRender(long app);
static native byte[] nativeDispatch(long app, int node, int event, int i, float f, String s);
static native byte[] nativeResize(long app, float widthDp, float heightDp);
static native void nativeUnmount(long app);
static native byte[] nativeUpdate(long app);

static void requestUpdate(long app);   // called by Rust, from any thread
```

- `nativeMount` builds the app and returns its handle.
- `nativeRender` returns a buffer that builds the whole tree from nothing and ends with SET_ROOT. A host calls it once after mount, and again whenever it needs to rebuild its views.
- `nativeDispatch` and `nativeResize` return the diff, or `null` when nothing changed.
- `s` is `null` for events without a string payload.
- `nativeUpdate` renders again and returns the diff, or `null`.
- The signal runtime has no hook that fires on a set. A Rust thread compares the runtime's global version every 32 ms while the app is mounted.
- Rust calls `requestUpdate` when a signal changes outside `nativeDispatch`, `nativeResize` and `nativeUpdate`, for example from a timer thread. The host posts one main-thread task per app, however many requests arrive before it runs. The task calls `nativeUpdate` and applies the result. A request for an app that is unmounted, or that has no activity attached, is dropped. The next `nativeRender` shows the current state anyway.

An app exports these symbols with `schnellui::android_app!(|| view)` from a `cdylib`. The host loads the library named by the `schnellui.lib` meta-data entry of the activity.

## Template mapping

- `container` creates Row, Column, Stack or Scroll with the container props.
- `pad` with a child creates a Stack that carries the PAD props. Without a child it creates a Spacer with the PAD props.
- `spacer` creates a Spacer with GROW 1.
- `flex` with a child sets WIDTH, HEIGHT, the MIN and MAX props, GROW and BASIS on the child's own node. If the `FlexChild` has padding, the child is first wrapped in a Stack with the PAD props and the layout props go on the wrapper. Without a child it creates a Spacer with those props. `shrink` has no Android equivalent and is dropped.
- `responsive` with a viewport target sets VISIBLE from the current viewport size, and is re-evaluated on resize. A parent target sets SHOW_TARGET 0 and the SHOW bounds, with 16 dp per em. A component target sets SHOW_TARGET to the id of the nearest ancestor that carries that reference. Without such an ancestor the node is always shown.
- `keyed` gives the node its key for the diff. `component_ref` records the reference on the child's node.
- `text`, `button`, `checkbox`, `slider` and `text_input` create the matching kind. Dynamic text is evaluated at each render.
- `component` maps a `ComponentKind` that has a kind above onto it, as the next section lists. Every other kind renders as a Column of its children, led by a Text of its label when the label is not empty. A later stage replaces these fallbacks.

## Component mapping

`ComponentProps` fields map as follows. `disabled` becomes ENABLED false everywhere. `width` and `height` become WIDTH and HEIGHT.

- Switch: TEXT from `label`, CHECKED from `checked`, TOGGLE runs `on_toggle`.
- Radio: TEXT from `label`, CHECKED from `selected || checked`, CLICK runs `on_click`.
- Dropdown: options must be direct children. Its options are `items` followed by its DropdownOption children. An option's label is its `label`, and its value is its `value`, or its label when the value is empty. An entry of `items` is both label and value. Options are not nodes. They become ITEMS, and SELECTED_INDEX is the first option with `selected`, or the first whose value equals the Dropdown's `value`. SELECT runs `on_input` with the option's value. A11Y_LABEL from `label`.
- ProgressBar: Progress with VALUE from `number`, MIN, MAX. A11Y_LABEL from `label`.
- LoadingSpinner: Loading with A11Y_LABEL from `label`.
- TextArea: TEXT from `value`, A11Y_LABEL from `label`, INPUT runs `on_input`.
- Image: SRC from `value`, A11Y_LABEL from `label`.
- Divider: Divider.
- Badge: TEXT from `label`.
- Link: TEXT from `label`, HREF from `value`, CLICK runs `on_click`.
- Dialog: TEXT from `label`, children as content. DISMISS runs `on_click`, because `ComponentProps` has no dismiss handler.
- TabBar: children as content, A11Y_LABEL from `label`. Tab: TEXT from `label`, SELECTED, CLICK runs `on_click`.
- List: a Column with ROLE list and A11Y_LABEL from `label`. ListItem: TEXT from `label`, SELECTED, children as content, CLICK runs `on_click`.

- Table: Table with A11Y_LABEL from `label`, children as rows. TableRow: ITEMS from `items`, SELECTED, children as content, CLICK runs `on_click`, A11Y_LABEL from `label`.
- RichText: a Column of a 20 sp Text of `label` with the `ColumnHeader` role when the label is not empty, which the host treats as a heading because `Role` has no heading, a word-wrapping Text of `value`, then the children.
- Icon: a 20 sp Text of `detail`, or of U+25C6 when the detail is empty, with an image role and A11Y_LABEL from `label`.
- DragHandle: a ghost Button with TEXT U+22EE U+22EE and A11Y_LABEL from `label`. CLICK runs `on_click`. Dragging is not supported.
- Svg: an Image whose SRC is a PNG `data:` URL. Rust rasterises `markup`, or `value` when there is no markup, at `width` by `height` times the display density, and caches the result by source and size. A11Y_LABEL from `label`.
- ThemeProvider: `value` "dark" is theme 2 and "light" is theme 1. Any other value is a plain Column.

A Dropdown or ProgressBar with a non-empty label is wrapped in a Column that holds a 14 sp Text of the label and then the widget. A key goes on the wrapper.

## Diff

Children are matched by key first, then by position and kind among unkeyed siblings. A matched node keeps its id and gets SET or CLEAR ops for changed props only. A keyed node that changed position gets one INSERT and keeps its view. An unmatched old node gets REMOVE. An unmatched new node gets CREATE, its props and an INSERT. A node whose kind changed is removed and created again.

## Not supported

Colors from `schnellui::widgets::Theme` are not applied. Views use the device theme. DockArea, GroupedTabList, TabGroup and TabNode render as a Column of their children, led by a Text of their label. Dragging a DragHandle does nothing. WRAP is ignored.
