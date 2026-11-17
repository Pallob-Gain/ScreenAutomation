# ScreenAutomation

ScreenAutomation is a desktop screen-automation prototype built in Rust. It combines a small, n8n-inspired node graph with screen-region capture, OCR, simple value transformation, and mouse/keyboard actions. You assemble a workflow from nodes, connect the nodes that should pass data to one another, and run the graph repeatedly at a configurable interval.

The project has two parts:

- A reusable graph engine that can be tested without a desktop display.
- An optional native desktop application with an interactive node-canvas editor, screen-region overlays, capture, OCR, and input.

> **Use with care.** A running workflow can click and type into other applications. Review the graph and target regions before pressing **Run**, and press **Stop** (or **Esc**) before editing or leaving an automation unattended.

## What it can do

- Capture pixels from a screen region and run OCRS (a Rust OCR engine) when that region's pixels change.
- Convert text and other values to text, number, integer, or boolean values.
- Evaluate a Rhai expression or script to make decisions and transform values.
- Click the center of a configured screen region, or click and type text there.
- Run connected nodes in dependency order on a timer, with display refresh, or from native screen-change events on Windows.
- Save and load the graph, including node settings, connections, and screen regions, as JSON.
- Undo/redo edits, search and focus nodes, fit the graph, and organize nodes with groups and sticky notes.
- Preview workflows without real clicks or typing, inspect execution one step at a time, and validate graphs before running.
- Filter suspicious OCR/numeric readings and reuse embedded subflows.
- Trigger branches from global hotkeys, microphone levels, local HTTP webhooks, text-file changes, or timers.
- Keep separate crash-recovery copies and a recent-file list.
- Test workflow behavior using a fake backend without requiring a monitor or real input devices.

## Requirements

- Rust and Cargo (Rust 1.92 or newer; the package uses Rust edition 2021).
- The native system libraries required by `eframe`, `xcap`, `enigo`, and `cpal` on your operating system. On Linux, desktop builds may require X11-related libraries, ALSA development libraries for microphone input, and input/display access.
- The OCR models `text-detection.onnx` and `text-recognition.onnx`. They are loaded from a local `models/` folder when present; otherwise internet access is needed the first time OCR is used so they can be downloaded into the operating system's application cache.

The GUI and real desktop integrations are behind the `gui` Cargo feature. The default build has no desktop backend dependencies enabled. The desktop editor uses eframe for native windows and `egui-snarl` for its interactive graph canvas.

## Build and run

Run the graph-engine tests without launching a GUI:

```sh
cargo test
```

Build and launch the desktop application (release mode is recommended for OCR performance):

```sh
cargo run --release --features gui
```

The GUI initializes a desktop input backend when it starts. The first OCR operation loads the OCRS detection and recognition models, looking in this order:

1. The folder named by the `SCREEN_AUTOMATION_MODELS` environment variable.
2. `models/` next to the executable.
3. `models/` in the working directory, for example the repository root when using `cargo run`.
4. The application cache, downloading the models there from `https://ocrs-models.s3-accelerate.amazonaws.com/` if they are missing.

The model paths used are logged as `[OCR] Models: ...`. To run fully offline, place both `.onnx` files in one of the local folders. A model download or OCR error is shown in the application status area. Screen capture and simulated input also depend on operating-system support, permissions, and the active desktop session.

## Using the application

1. Start the GUI with `cargo run --release --features gui`. An unoptimized debug build can make OCR substantially slower.
2. Click the floating plus icon at the top right of the canvas and choose a node type. The node appears on the canvas with a generated name.
3. Drag nodes to arrange the workflow. Drag from an output pin to an input pin to create an edge; use the graph editor's pin controls to remove or reroute connections. Use the canvas zoom and pan controls to navigate larger workflows.
4. Single-click a node to select it; double-click to open its floating settings window. To delete a node and its connections, drag it onto the trash icon at the bottom right of the canvas, click the icon while a node is selected, or use **Delete node** in its settings. For OCR, keyboard, and click nodes, position and resize the on-screen region window. Drag inside the window to move it; drag its lower-right grip to resize it. The region is used both for screen capture and as the target location for actions. While running, the region interior is transparent, its label and outline are outside the region, and mouse input passes through; press **Stop** to move or resize it. Avoid placing another overlay's label or outline over an OCR region.
5. Open **Settings → Run timing...** to choose **Timed delay** (100–60,000 ms; default 1,000), **Display refresh**, or **HMI Event**, then press the menu bar's green play **Run** button. Screen-change sources require Windows; configured Event nodes also work in HMI Event mode. Invalid delay input leaves the last valid delay in effect. Press the red-square **Stop** button or the **Esc** key to stop execution. On Windows, **Esc** stops the workflow even when another application has focus; on other platforms the ScreenAutomation window must be focused. A run already in progress finishes before the workflow stops.
6. Use the **File** menu to create a **New** graph, **Load graph JSON...**, **Save**, **Save As...**, or **Exit**. New starts an empty, untitled graph with default timing settings. Before New, Load, Exit, or closing the application window, unsaved changes prompt you to **Save**, **Discard**, or **Cancel**. Save must succeed before continuing; canceling Save As or a save error keeps the current graph open. Load and Save As open native file dialogs. Save writes to the last successfully loaded or saved file; for a new graph it opens Save As, suggesting `graph.json`. Opening a file dialog or an unsaved-changes prompt stops automation (even if canceled), and loading leaves the workflow stopped so you can review it before running.

Region-bearing nodes start with a region at screen position `(100, 100)` and size `300 × 120` UI points. The capture area can be resized down to `80 × 1` UI points, allowing thin, single-line regions; the marker window also reserves space for its title and margins outside that area. Overlay coordinates are converted to physical screen pixels for capture and input, including on scaled displays. Keep each OCR region entirely within one monitor. Their overlay color and opacity can be adjusted in the selected node's settings. Opacity applies to the fill, outline, label, and resize grip; the fill and grip are hidden while running.

The window title shows the current filename (or **Untitled** for a new graph), with an **\*** when there are unsaved changes. Changes to nodes, connections, positions, screen regions, appearance, and run timing count as edits; execution state and logs do not. The bottom bar shows the latest session log by default; hover over it for the full text, file path, and last run duration. Click **^** at the bottom right to expand a resizable, terminal-style log panel, and **v** to collapse it. The expanded view supports vertical/horizontal scrolling, selectable text, **Copy logs**, and automatic following of new entries unless you scroll up. Execution timings and debug-build information appear above the log.

Logs include OCR results, comparisons, threshold decisions, clicks, keyboard actions, run/stop events, file operations, and errors. Timestamps show elapsed time since the app started; multiline OCR text uses visible `\n`/`\r` escapes. Logs stay in memory across stops and workflow loads until the app exits; they are not saved in the graph JSON or automatically written to disk. The newest 5,000 entries are kept to bound memory, with a discarded-entry count shown when older entries are removed. Copy logs before exiting if you need to keep them. The same application messages still appear in the terminal.

**Timed delay** measures the interval between the starts of workflow checks; slow capture/OCR does not add another full delay. **Display refresh** checks once per rendered application frame, paced by VSync on the app window's display, rather than a fixed timer. It is not a notification of changes in another application: capture/OCR can reduce the achieved rate, and minimizing the app may pause checks. Both modes capture monitored regions and run OCR only when the captured pixels change, as before. Other source nodes, such as Constant, still run on every workflow check.

**HMI Event** uses native Windows Desktop Duplication frame notifications for top-level OCR sources instead of taking screenshots on a timer. It compares pixels only in monitored OCR regions; the initial snapshot and subsequent changed regions trigger OCR and their connected branches. Unrelated action branches do not run. Shared descendants execute once per batch, using the latest values from other inputs; constants and their transformations supply context without independently triggering actions. Only the latest pending pixels per region are retained if OCR cannot keep up. There must be at least one top-level OCR or Event node. Each screen region must fit on one unrotated monitor, and the display/driver must support Desktop Duplication. Capture failures (including a lost display/session) stop automation with an error; there is no silent screenshot-polling fallback. Capture workers are stopped and joined on Stop/Exit. Mode changes into or out of HMI Event, or edits to node configuration, connections, or monitored regions, stop the current run. Review the workflow before pressing Run again; restarting processes fresh screen snapshots and restarts external producers.

Add an **event** node and choose its producer in node settings, then select **HMI Event** in Run timing. Event capture does not repair OCR mistakes, and GUI scheduling or slow OCR may delay execution even when notifications arrive promptly.

| Event source | Configuration and output |
| --- | --- |
| Global hotkey | Windows only. Examples: `F8`, `Ctrl+Alt+K`. Emits `true` once per press, not repeatedly while held. Escape is reserved for Stop. Avoid application shortcuts such as F5–F7 while the editor is focused. |
| Microphone level | Uses the default input device only while the live workflow is running. Emits RMS on reaching or crossing above the configured threshold; dropping below it re-arms detection. Device/permission errors stop the workflow with an error. |
| HTTP webhook | Accepts UTF-8 `POST` bodies at the configured path on `127.0.0.1`; emits the body as text. Maximum body size: 64 KiB. Use a distinct port per webhook. It is not exposed to the LAN or internet and has no authentication; other local processes can trigger it while running. |
| File change | Checks an existing UTF-8 text file every 100 ms and emits its new contents when they change. Files are limited to 1 MiB. Initial contents establish the baseline and do not trigger. Read failures stop the workflow explicitly. |
| Timer | Emits increasing integer ticks at the configured interval (minimum 100 ms). This is an additional event source, not a cooldown on actions. |

Only the latest pending value per source is retained. Stop closes listeners, stops microphone capture, and joins workers. An in-flight webhook can delay shutdown by roughly four seconds (bounded request-read/write waits); audio-driver teardown and OS I/O can also delay it. External Event nodes must be in the top-level graph, not embedded subflows. Native screen-change monitoring also applies to top-level OCR sources; OCR inside a subflow runs when that subflow is invoked, for example by a connected Timer event.

### Editing, validation, and safe testing

- **Edit → Undo / Redo** restores graph edits, including deletion and connections. Dragging or editing one field is grouped as one edit; up to 100 edit groups are retained. New/load starts a new edit history. Undoing back to the saved graph clears the unsaved marker.
- **Edit → Find node...** searches title, internal name, type, and group, selects the result, opens settings, and focuses the canvas on that node. **Fit graph** frames every node without changing saved positions.
- Assign the same **Group** name in node settings to draw a named section around those nodes. **note** nodes display editable sticky-note text and have no pins.
- **Tools → Validate graph** lists configuration/connection problems and lets you select affected nodes; invalid nodes have red outlines. Live Run performs validation first. This checks graph structure, required inputs, thresholds, numeric guard configuration, regions, event configuration, and subflows; device availability is checked when the device is actually opened.
- **Tools → Dry Run** executes one fresh snapshot without sending clicks or typing to the real backend. **Step** advances one top-level node at a time. The preview window shows inputs, outputs, and actions that would occur. Capture/OCR remains real; external event listeners are not started. A subflow is one outer step, including its recorded inner actions. Editing invalidates the preview session; live Run remains a separate, explicitly real execution path.

| Shortcut | Action |
| --- | --- |
| Ctrl+N / Ctrl+O / Ctrl+S | New / Open / Save |
| Ctrl+Shift+S | Save As |
| Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z | Undo / Redo / Redo (outside text fields, which retain normal text editing) |
| Ctrl+F / Ctrl+0 | Find node / Fit graph |
| Delete | Delete selected node (outside text fields) |
| F5 / F6 / F7 | Live Run/Stop / Dry Run / Step |

On macOS, use Command instead of Ctrl. Inside an embedded editor, use **Back to parent graph** to access file operations.

### Recovery and recent files

Unsaved work is copied approximately every two seconds to a **separate recovery file** in the operating system's local application-data directory. It never silently overwrites your manually saved graph. After an interrupted session, choose **Recover** or **Discard recovery** at startup. Recovered changes remain unsaved until you explicitly save them. Saving or deliberately discarding changes clears the recovery copy. The last ten successfully opened/saved paths appear under **File → Recent files**, with the same unsaved-changes guard as Open.

Recovery and recent-file storage is locked to one application instance at a time. Another instance can still edit graphs but reports that persistence is unavailable rather than overwriting the first instance's recovery. Storage errors are shown in the status/logs.

### Embedded reusable subflows

Add a **subflow** node, open its settings, and select **Edit embedded workflow**. The canvas switches to its internal graph; **Back to parent graph** returns to the containing workflow. Changes are included in the parent's JSON, not linked to another file. Shift-drag the subflow node to reuse a copy with fresh runtime state. **Import workflow JSON...** embeds a copy of an existing graph.

Use **subflow input** to receive the containing node's value and exactly one **subflow output** to return a value. An external input connection is required only if the internal input is actually connected. Source-only subflows can run without an incoming value. Add the output node before importing a workflow not originally built as a subflow. Embedded run settings do not schedule separate loops: the parent invokes the subflow. Nested subflows are supported up to 16 levels; external Event nodes belong in the top-level graph. Region overlays and physical-pixel conversion also cover nested nodes.

Node positions, the run mode, and the refresh delay are saved with the workflow. Settings are stored as `settings.run_mode` (`timed`, `display_refresh`, or `hmi_event`) and `settings.refresh_ms` and restored when loading. Older files without a run mode use timed execution; those without a delay use 1,000 ms. Invalid saved settings are reported as load errors without replacing the current graph. Existing workflow files that do not contain positions are laid out automatically when loaded.

Click outside the floating node settings window to dismiss it. Single-clicking its selected node keeps it open; single-clicking another node closes it and selects that node. Double-click a node to open its settings. Controls and dropdowns inside the settings remain interactive.

Edit **Title** in node settings to change the title on the canvas and any region overlay. Titles are saved with the graph; clearing the field restores the generated name. Changing a title does not affect connections or the node's internal name.

Clickable controls show a hand cursor on hover. Text-entry, drag, and resize controls retain their specialized cursors; disabled controls do not show a hand.

Hold **Shift** when starting to drag a node to duplicate it. The original stays in place while the new, selected copy follows the pointer, even if you release Shift before dropping it. The copy keeps the title, settings, and screen region, but gets a unique internal name, no connections, and fresh execution state. Shift-click alone does not create a copy.

## Node types

| Type | Inputs and behavior | Output |
| --- | --- | --- |
| `ocr` | Captures its region and runs OCR only when the captured pixel bytes differ from the previous capture. Enable **Single line (prices / numbers)** for a single price: it trims plain background margins, normalizes dark backgrounds, and bypasses word/line detection so fragments stay in left-to-right order and decimal points remain inside the recognition crop. Fit the region around the entire number with a small plain-background margin and exclude other text/icons. Multiline detection remains the default. | Recognized text, trimmed. No output when the pixels are unchanged. |
| `model` | Uses the first input and converts it to the selected `text`, `number`, `int`, or `bool` format. For `number` and `int`, line breaks, spaces, and symbols are removed and words such as `BTC` are skipped, so OCR text like `$ 64,777\n.49` becomes `64777.49`. Thousands separators are removed, and decimals are kept for `number`. | The converted value, or no output if no number is found. |
| `constant` | A source node with no inputs. Enter a fixed value and choose Text, Number, Integer, or Boolean. Outputs on every workflow tick; invalid numeric values report an error. | The configured, typed value. |
| `function` | Evaluates Rhai code with an `inputs` array in scope. The last expression is the result. | A boolean, integer, number, or text value. Rhai's unit value `()` produces no output. |
| `compare` | Compares the first input with its configured value using `>`, `≥`, `=`, `≠`, `≤`, or `<`. A number is extracted from text input, such as OCR output like `BTC 64777.49`. If the input or configured value is not numeric, only `=` and `≠` apply, using trimmed text. With **Only trigger when the condition becomes true** enabled, which is the default, it fires once each time the condition changes from false to true. | The unchanged input when it triggers. Otherwise, no output, so downstream nodes such as `click` do not run. |
| `compare_inputs` (menu: **compare inputs**) | Compares two connected values: **A** is the top input and **B** the bottom. Choose `>`, `≥`, `=`, `≠`, `≤`, or `<`, then select **True** or **False** as the trigger state. Numeric/OCR text uses the same comparison rules as `compare`; nonnumeric text supports equality/inequality. Waits until both inputs have supplied a value, then compares their latest values whenever either updates. With **Only trigger when the result changes** enabled (default), it fires once when the selected state is first reached, then waits for a different result before firing again. Disconnecting, reconnecting, or deleting a source clears cached inputs. | The boolean result (`true` or `false`) when it matches the selected trigger state; otherwise no output. Either boolean can trigger a connected Click node. |
| `hysteresis` | The former threshold node. Choose `> upper`, `>= upper`, `< lower`, `<= lower`, or both strict crossings. After a rise, the value must fall strictly below the lower threshold to re-arm; after a fall, it must rise strictly above the upper threshold. Values between thresholds keep the last state. The first reading never fires. Numbers are extracted from text in the same way as `compare`. | The unchanged input when it triggers. Otherwise, no output. |
| `threshold` | Choose `> upper`, `>= upper`, `< lower`, `<= lower`, both strict crossings, or **Enters range** with strict (`lower < value < upper`) or inclusive (`lower <= value <= upper`) boundaries. The first reading fires if the selected condition holds. Range entry fires once, then requires leaving and re-entering the range; strict mode treats either boundary as outside. Equal bounds define an empty strict range or a single-value inclusive range. Other crossing modes re-arm on returning to the non-triggering side of the same boundary or reaching the other threshold. For example, with `> 100`, `101 → 99 → 101` fires on both readings of `101` without needing to cross the lower threshold. Numbers are extracted as for `compare`. | The unchanged input when it triggers. Otherwise, no output. |
| `keyboard` | Requires an input. Clicks the center of its region, then types its configured text; if that setting is empty, it types the first input converted to text. | The text it typed. |
| `click` | Requires at least one input. Clicks the center of its region using the configured mouse button (`left`, `right`, or `middle`; unknown values fall back to `left`). | `true` after the click. |
| `validate_number` | Rejects unreadable/non-finite numbers, readings outside optional minimum/maximum bounds, changes larger than an optional absolute limit from the last accepted reading, or text not matching an optional regular expression. Use `^` and `$` to require a whole-value match, e.g. `^[0-9]+\.[0-9]{2}$`. Rejections are logged, never update the accepted baseline, and do not guess missing decimal points. | Original input if accepted; otherwise no output. |
| `event` | One of the configured hotkey, microphone, webhook, file, or timer producers. Active only in HMI Event mode. | Producer's value, routed to connected branches. |
| `note` | Editable annotation with optional group; no execution or pins. | No output. |
| `subflow` | Executes a self-contained embedded graph, using its input/output nodes. | Value returned by the internal Subflow Output, or no output. |
| `subflow_input` / `subflow_output` | Boundary nodes inside an embedded workflow. | Receive the caller's value / return the internal result. |

`model`, `constant`, `function`, `compare`, `compare_inputs`, `hysteresis`, and `threshold` nodes do not have screen regions. OCR, keyboard, and click nodes do.

Each Compare Inputs port accepts one connection. Input assignments and constants are saved in the graph JSON; cached runtime values are not. Existing `[source, target]` edges still load as input A (port 0). Edges to input B use `{"source":"node1","target":"compare_inputs3","input":1}`.

### Settings supplied by input pins

Open node settings and use the independent checkboxes under **Values from input pins** to replace fixed values with connected inputs:

- **Compare:** comparison value.
- **Hysteresis / Threshold:** lower and upper thresholds.
- **Numeric Validation:** minimum, maximum, and maximum absolute change.

Each enabled option adds a labeled input pin accepting one connection. The first pin remains the main reading; connect a Constant, OCR/model output, or another value-producing node to a setting pin. Fixed editors are disabled while their pins are enabled, but retain their values for when you uncheck the option. Keyboard text and Constant values remain fixed settings.

The node waits until the main reading and every enabled setting have supplied values. A changed setting re-evaluates the latest cached reading; sending the same setting again alone does not repeat the output. A fresh main reading is evaluated even if its value is unchanged. Existing comparison, crossing, hysteresis, and numeric-validation rules still control whether evaluation triggers an output.

There is no fallback to fixed values for enabled pins. Missing or invalid settings block output without advancing crossing/accepted-reading state. Numeric settings must be finite numbers (plain numeric text is accepted), lower/minimum must not exceed upper/maximum, and maximum absolute change must be nonnegative. Runtime validation errors are logged.

Changing the checkboxes stops automation. Other setting wires are preserved if pin positions shift; disabling a setting removes only its wire. Pin choices and connections are saved in JSON (`input_parameters`), support Undo/Redo, and are copied with node settings; cached readings are never saved. Older graphs keep their fixed-value behavior.

Older JSON nodes tagged `"type": "threshold"` still load as **hysteresis**, preserving their behavior, names, and connections. They save with `"type": "hysteresis"`. New **threshold** nodes use `"type": "threshold_crossing"` in JSON to distinguish them from the legacy format.

OCR can still misread a decimal point. Number conversion cannot recover a dot that OCR omitted and does not guess decimal placement. Check recognized text before connecting live actions.

Nodes are drawn as compact blocks, colored by type, with an icon strip, the node name, and input and output pins on the left and right edges. The selected node has an orange outline. Hover over a node to see its type, a short description, and its last output.

When a node produces output during a run, its outgoing wires and pins turn green and fade over about one second. The last output is shown under the node with a status dot. A wire that stays grey did not carry data in that run, for example because OCR saw no change or a compare condition was false.

### Example workflow

To compare a live OCR value against a constant (or another OCR source), connect them to separate inputs:

```text
ocr --------------------> A
                          compare inputs (A > B, trigger True) -> click
constant (Number: 100) --> B
```

Select **False** instead to click when the comparison is false. Unlike the original Compare node, this node outputs the boolean result, not the incoming text.

To click a button when a price rises above a threshold:

```text
ocr → compare (> 65000) → click
```

To click Button 1 when a price rises above an upper threshold and Button 2 when it falls below a lower one, use two `threshold` nodes:

```text
ocr → model (number) → threshold (rises above upper) → click (Button 1)
                    └→ threshold (falls below lower) → click (Button 2)
```

Use `hysteresis` instead if each direction should stay disarmed until the opposite threshold is crossed, and the first reading should never trigger.

A useful pattern is:

```text
ocr → model (number) → function → keyboard
                               └→ click
```

For example, the function code below forwards `"go"` when the first input is greater than `10.0`; otherwise, it returns `()` and stops propagation along that branch:

```rhai
if inputs[0] > 10.0 { "go" } else { () }
```

With an OCR node upstream, the function receives the numeric value produced by the model node. A keyboard node with an empty text setting can type the function's `"go"` result. A click node connected to the same function can click only when that function produces a value.

## Graph execution behavior

- The graph engine runs nodes in topological order, so upstream nodes are evaluated before their downstream dependents.
- Nodes with no incoming connections are evaluated on each run. Nodes with incoming connections receive the values produced by their connected parents.
- When a node produces no value, that missing value is not passed downstream. If a node has parents but none produced a value during the current run, that node is skipped.
- A function returning `()` and an OCR node whose pixels have not changed both produce no value. This is how a branch can stop and how unchanged screen content avoids repeating downstream actions.
- Cycles are rejected when executing a graph. Connect nodes to form a directed acyclic graph (DAG).
- Each node can return an error, for example if screen capture, OCR, script evaluation, or simulated input fails. The GUI displays the error in its status area.
- The refresh interval is adjustable from 100 ms to 60,000 ms; the initial interval is 1,000 ms. It is a target time between run starts, not a guarantee: screen capture, OCR, actions, and rendering may take longer. The status bar shows the last run duration and observed interval. Runs do not overlap or accumulate; when one takes longer than the target, the next starts as soon as possible. For the best OCR speed use the release build.

## Saving and loading

The application serializes graphs as readable JSON. A graph contains its nodes and directed connections. Node data includes its name, type-specific settings, canvas position, optional screen region, overlay color, and opacity. OCR's in-memory pixel-change cache is not persisted, so a loaded OCR node establishes its baseline when it next runs.

Use the **Save** and **Load** controls in the desktop app to write or read the JSON file at the path shown in the file field. Loading replaces the current graph. Keep a backup of workflows you want to preserve.

## Library and test backend

The core modules are available without enabling the GUI:

- `backend`: `Region` and the `Backend` trait for screen capture, OCR, text entry, and clicking.
- `node`: graph node types, formats, and values.
- `graph`: node management, connections, JSON serialization, and one-tick execution.

Implement `Backend` to connect the engine to another environment or to a test double. The integration tests use a fake backend to verify workflow execution and screen-change behavior without capturing a real display or sending real input.

## Manual test page

Open `tests/web/index.html` in a browser to test workflows on the real screen. You do not need a server. The page includes:

- **Price feed**: a large live crypto price. Choose a source:
  - **Real-time streams (WebSocket)**: these push new values many times per second, and the page redraws at the update interval (default 200 ms, minimum 100 ms). The Kraken order-book microprice changes fastest because it moves whenever the best bid or ask changes, not only on trades. It is shown with two extra decimals so small moves are visible. Coinbase trade streams change only when trades happen.
  - **Polled**: Coinbase or Binance REST, refreshed at most once per second. Binance blocks US connections (HTTP 451).
  - **Simulated**: a random walk that changes on every tick, down to every 100 ms. It works offline and gives guaranteed fast changes.

  Use this value as an OCR target. The status line shows the live update rate.
- **Button 1 / Button 2**: buttons that count their clicks. Use them as targets for Click nodes.
- **Upper / lower threshold**: inputs that are filled automatically around the first price. Use them as Keyboard targets. The **Crossed above upper** count increases when the price rises from at or below the upper threshold to above it. The **Crossed below lower** count increases when the price falls from at or above the lower threshold to below it.
- **Event log**: a timestamped record of clicks, crossings, and setting changes.

## Project layout

```text
models/        Local OCR models (text-detection.onnx, text-recognition.onnx)
src/
  backend.rs   Screen-region type and OS-access abstraction
  desktop.rs   Optional real desktop capture, OCRS engine, and input backend
  graph.rs     Graph operations, ordering, persistence, and execution
  lib.rs       Library module exports
  session_log.rs Bounded session log capture for the GUI and terminal
  main.rs      Optional eframe application and egui-snarl graph editor
  node.rs      Node kinds, values, conversions, and processing
tests/
  graph.rs     Graph execution, change-detection, and JSON/cycle tests
  web/
    index.html Manual test page (price feed, click counters, thresholds)
```

## Current scope

ScreenAutomation is an early, deliberately small automation tool rather than a full workflow platform. The editor provides a visual node canvas and direct screen-region overlays, but does not show a live graph execution trace or per-node output inspector. It does not currently provide workflow scheduling outside the running application, remote execution, or a built-in mechanism for selecting regions from inside another process. The graph engine and backend trait are intended to keep the core workflow logic testable and make alternate integrations possible.
