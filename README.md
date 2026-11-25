# ScreenAutomation

ScreenAutomation is a desktop screen-automation prototype built in Rust. It combines a small, n8n-inspired node graph with screen-region capture, OCR, simple value transformation, and mouse/keyboard actions. You assemble a workflow from nodes, connect the nodes that should pass data to one another, and run the graph repeatedly at a configurable interval.

The project has two parts:

- A reusable graph engine that can be tested without a desktop display.
- An optional native desktop application with an interactive node-canvas editor, screen-region overlays, capture, OCR, and input.

> **Use with care.** A running workflow can click and type into other applications. Review the graph and target regions before pressing **Run**, and press **Stop** (or **Esc**) before editing or leaving an automation unattended.

## What it can do

- Capture pixels from a screen region and run OCRS (a Rust OCR engine) when that region's pixels change.
- Convert text and other values to text, number, integer, or boolean values.
- Evaluate a Rhai or JavaScript expression or script to make decisions and transform values.
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
4. Single-click a node to select it; double-click to open its floating settings window. To delete a node and its connections, drag it onto the trash icon at the bottom right of the canvas, click the icon while a node is selected, or use **Delete node** in its settings. For region-bearing nodes, position and resize the on-screen region window. Drag inside the window to move it; drag its lower-right grip to resize it. The region is used both for screen capture and as the target location for actions. Pressing **Run** completely hides all region windows, including labels, outlines, and resize grips, before capture begins. Automation continues using the calibrated coordinates and display scales. **Stop**, **Esc**, or a run error restores the region windows in place; the main app and its Stop control remain available.
5. Open **Settings → Run timing...** to choose **Timed delay** (100–60,000 ms; default 1,000), **Display refresh**, or **HMI Event**, then press the menu bar's green play **Run** button. Screen-change sources require Windows; configured Event nodes also work in HMI Event mode. Invalid delay input leaves the last valid delay in effect. Press the red-square **Stop** button or the **Esc** key to stop execution. On Windows, **Esc** stops the workflow even when another application has focus; on other platforms the ScreenAutomation window must be focused. A run already in progress finishes before the workflow stops.
6. Use the **File** menu to create a **New** graph, **Load graph JSON...**, **Save**, **Save As...**, or **Exit**. New starts an empty, untitled graph with default timing settings. Before New, Load, Exit, or closing the application window, unsaved changes prompt you to **Save**, **Discard**, or **Cancel**. Save must succeed before continuing; canceling Save As or a save error keeps the current graph open. Load and Save As open native file dialogs. Save writes to the last successfully loaded or saved file; for a new graph it opens Save As, suggesting `graph.json`. Opening a file dialog or an unsaved-changes prompt stops automation (even if canceled), and loading leaves the workflow stopped so you can review it before running.

Region-bearing nodes start with a region at screen position `(100, 100)` and size `300 × 120` UI points. The capture area can be resized down to `80 × 1` UI points, allowing thin, single-line regions; the marker window also reserves space for its title and margins outside that area. Overlay coordinates are converted to physical screen pixels for capture and input, including on scaled displays. Keep each OCR region entirely within one monitor. Their overlay color and opacity can be adjusted in the selected node's settings. Opacity applies to the fill, outline, label, and resize grip while stopped. Selecting a block gives its matching region window an opaque orange title bar with black text, even at low overlay opacity; other windows retain their configured appearance. Clicking a region window's title, body, or resize grip selects its corresponding block and brings a newly selected block into view, opening its embedded subflow if needed. Selection works in both directions inside subflows too. Dragging and resizing still work normally. The capture area and saved opacity do not change just from selection, and the entire region window is still hidden while running. Each region is its own always-on-top native window. Region windows repaint independently of the editor, so panning and zooming the canvas stays responsive even with dozens of regions. **Tools → Show region overlays** can still hide the windows while editing; the selected node's overlay stays visible even while the others are hidden, so regions can be positioned one at a time. Hiding them changes nothing else: saved regions, measured display scales, runs, and dry runs are unaffected, and re-ticking restores the windows in place.

Canvas nodes, Node settings titles, and region titles identify the node type with a consistent, bundled-font icon. Custom titles and serialized type names are unchanged. Region headers use two compact lines (type, then custom title) so calibrated capture coordinates do not move; long custom titles are elided. The creation menu shows matching colored icons and groups nodes as **Inputs → Data & validation → Logic → Actions → Subflows → Notes**.

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
- **Edit → Find node...** lists the current graph's nodes in a scrollable **Group → Nodes** tree, including **Ungrouped**, with type icons and node counts. Groups start expanded; search automatically expands matching groups and filters title, internal name, readable/internal type, and group. Nodes are sorted by title within each group. Selecting a result opens settings and focuses the canvas without moving saved positions. Subflow editors search their own graph. **Fit graph** frames every node without changing saved positions.
- Use **+ → Create group...** to create a named group. In node settings, the **Group** dropdown lists existing groups and **Ungrouped**; **Create new group...** creates a group and assigns that node immediately. Names are trimmed, and blank or duplicate names (ignoring case) are rejected with an explanation. Existing group assignments from older workflows appear automatically. Created groups are saved with the workflow even while empty; their canvas section appears once nodes are assigned. Groups are local to the current graph/subflow and support undo/redo; they do not change execution. **note** nodes display editable sticky-note text and have no pins.
- With **Regular expression** selected in Wait for text or Assert text, **Generate...** appears to the right of the expected-text input. It also appears beside Validate number's regex field. Type or paste a sample, review the generated pattern, then choose **Apply regex**; no screen capture, OCR, clicks, or typing into other apps occurs. Special characters are escaped; whole-text and flexible-whitespace matching default on. Optional digit generalization preserves digit counts and defaults off. Text-condition samples use the same whitespace normalization as execution. Applying is undoable, cancels pending waits, and leaves automation stopped; Cancel leaves the existing pattern unchanged.
- **Tools → Validate graph** lists configuration/connection problems and lets you select affected nodes; invalid nodes have red outlines. Live Run performs validation first. This checks graph structure, required inputs, thresholds, numeric guard configuration, regions, event configuration, and subflows; device availability is checked when the device is actually opened.
- **Tools → Dry Run** executes one fresh snapshot without sending clicks or typing to the real backend. **Step** advances one top-level node at a time. Both hide region overlays and wait for the same 150 ms settling period as live execution before capturing, then restore the overlays. Esc cancels a queued preview. The preview window shows inputs, outputs, and actions that would occur. Capture/OCR remains real; external event listeners are not started. A subflow is one outer step, including its recorded inner actions. Editing invalidates the preview session; live Run remains a separate, explicitly real execution path.

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
| `function` | Evaluates Rhai or JavaScript code (pick the language above the editor) with an `inputs` array, per-node runtime `state` (a Rhai map / JS object), `elapsed_ms` since first evaluation, and `workflow_busy` (a pending wait in the containing graph) in scope. Node settings can also declare **named inputs**: each name adds its own single-wire pin and is bound as a script variable, reading `()` in Rhai or `undefined` in JavaScript when its pin produced no value this evaluation (compare with `name == ()` or `name === undefined` to test). Named pin values also appear in `inputs`. Names must be valid identifiers, unique, and must not shadow built-in bindings; invalid names fail validation and execution. State values must be text, finite numbers, integers, or booleans; invalid state or script errors fail explicitly and do not commit state updates. State is not saved, resets on loading/duplication/runtime reset, script or language changes, input rewiring, or renaming/adding/removing named inputs, and is isolated in previews. JavaScript runs sandboxed in the embedded Boa engine with no host APIs (no `console`, timers, or network). The last expression is the result. | A boolean, integer, number, or text value. Rhai's unit value `()` and JavaScript's `undefined`/`null` produce no output; integral JavaScript numbers up to 2^53 become integers, other results are converted to text. |
| `compare` | Compares the first input with its configured value using `>`, `≥`, `=`, `≠`, `≤`, or `<`. A number is extracted from text input, such as OCR output like `BTC 64777.49`. If the input or configured value is not numeric, only `=` and `≠` apply, using trimmed text. With **Only trigger when the condition becomes true** enabled, which is the default, it fires once each time the condition changes from false to true. | The unchanged input when it triggers. Otherwise, no output, so downstream nodes such as `click` do not run. |
| `compare_inputs` (menu: **compare inputs**) | Compares two connected values: **A** is the top input and **B** the bottom. Choose `>`, `≥`, `=`, `≠`, `≤`, or `<`, then select **True** or **False** as the trigger state. Numeric/OCR text uses the same comparison rules as `compare`; nonnumeric text supports equality/inequality. Waits until both inputs have supplied a value, then compares their latest values whenever either updates. With **Only trigger when the result changes** enabled (default), it fires once when the selected state is first reached, then waits for a different result before firing again. Disconnecting, reconnecting, or deleting a source clears cached inputs. | The boolean result (`true` or `false`) when it matches the selected trigger state; otherwise no output. Either boolean can trigger a connected Click node. |
| `hysteresis` | The former threshold node. Choose `> upper`, `>= upper`, `< lower`, `<= lower`, or both strict crossings. After a rise, the value must fall strictly below the lower threshold to re-arm; after a fall, it must rise strictly above the upper threshold. Values between thresholds keep the last state. The first reading never fires. Numbers are extracted from text in the same way as `compare`. | The unchanged input when it triggers. Otherwise, no output. |
| `threshold` | Choose `> upper`, `>= upper`, `< lower`, `<= lower`, both strict crossings, or **Enters range** with strict (`lower < value < upper`) or inclusive (`lower <= value <= upper`) boundaries. The first reading fires if the selected condition holds. Range entry fires once, then requires leaving and re-entering the range; strict mode treats either boundary as outside. Equal bounds define an empty strict range or a single-value inclusive range. Other crossing modes re-arm on returning to the non-triggering side of the same boundary or reaching the other threshold. For example, with `> 100`, `101 → 99 → 101` fires on both readings of `101` without needing to cross the lower threshold. Numbers are extracted as for `compare`. | The unchanged input when it triggers. Otherwise, no output. |
| `keyboard` | Requires an input. Clicks the center of its region, then types its configured text; if that setting is empty, it types the first input converted to text. | The text it typed. |
| `click` | Requires at least one input. Clicks the center of its region using the configured mouse button (`left`, `right`, or `middle`; unknown values fall back to `left`). | `true` after the click. |
| `wait_for_text` | After a trigger, repeatedly captures its region until text matches, without blocking the GUI between checks. Choose exact, contains, or regex matching, case sensitivity, single-line OCR, timeout (1–120000 ms), and minimum poll interval. Whitespace is normalized. Continues on later checks even if the upstream trigger stops. Timeout/capture/OCR errors stop the GUI run. Top-level graphs only, not subflows. | Original trigger after a match; no output while waiting. |
| `assert_text` | Immediately captures and checks its region using the same text matching options. A mismatch or OCR/capture error stops the GUI run. Use a wait first for changing dialogs. | Original input after a match. |
| `once` | Starts disarmed. When **Armed**, passes one trigger and latches before downstream actions run. Stop/Run does not rearm it. Stop, inspect the outcome/order history, then use **Reset latch** explicitly. Loading, duplicating, and runtime reset clear the latch; it is not persistent across application restarts. | First input only, until explicitly reset. |
| `set_field` | Clicks the center of its region, presses Select All (Ctrl+A; Command+A on macOS), then types replacement text. Empty configured text uses the input. Requires a backend with key support. Follow with `wait_for_text` to verify the rendered value; this node does not assume the field accepted it. | Replacement text. |
| `keypress` | Sends Select All, Tab, Enter, Escape, Arrow Up, Arrow Down, or Backspace to the currently focused control. Requires a trigger and backend key support. No screen region. | Original input after the key action succeeds. |
| `validate_number` | Rejects unreadable/non-finite numbers, readings outside optional minimum/maximum bounds, changes larger than an optional absolute limit from the last accepted reading, or text not matching an optional regular expression. Use `^` and `$` to require a whole-value match, e.g. `^[0-9]+\.[0-9]{2}$`. Rejections are logged, never update the accepted baseline, and do not guess missing decimal points. | Original input if accepted; otherwise no output. |
| `event` | One of the configured hotkey, microphone, webhook, file, or timer producers. Active only in HMI Event mode. | Producer's value, routed to connected branches. |
| `note` | Editable annotation with optional group; no execution or pins. | No output. |
| `subflow` | Executes a self-contained embedded graph, using its input/output nodes. | Value returned by the internal Subflow Output, or no output. |
| `subflow_input` / `subflow_output` | Boundary nodes inside an embedded workflow. | Receive the caller's value / return the internal result. |

`model`, `constant`, `function`, `compare`, `compare_inputs`, `hysteresis`, `threshold`, `once`, and `keypress` nodes do not have screen regions. OCR, keyboard, click, wait/assert text, and set-field nodes do.

Waits are checked on workflow ticks in Timed/Display Refresh modes; use a short refresh interval for responsive multi-step workflows. HMI Event schedules idle continuation checks every 50 ms while a wait is pending, even without another external event. The configured poll interval is a minimum, not a timing guarantee: capture/OCR and app scheduling still take time. Stop/Escape cancels pending waits but leaves once gates latched. Dry Run stops advancing at an unmatched wait rather than pretending a simulated click changed the screen; **Step** rechecks that wait against the actual screen. Dry Run/Step key presses, clicks, and typing are recorded, not sent.

Editing wait/assert text, field replacement, or keypress settings during a live run stops automation and cancels pending waits. Review the workflow before restarting; once gates remain latched.

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

Single-line OCR now gives small, plain numeric lines a second recognition pass using pixel-preserving 2x enlargement. It accepts the enhanced image reading only if digits, signs, and other symbols agree with the original, and an existing decimal is not moved or lost. A disagreement returns an explicit OCR error and stops execution. Ordinary text, single-digit readings, and thousands-separated numbers retain their original recognition path; multiline OCR is unchanged. This does not relax workflow validators or insert decimal points into strings.

An offline comparison on 432 rendered small-text crops improved exact non-whitespace recognition from **399/432 (92.4%)** to **419/432 (97.0%)**, counting rejected readings as failures. Confusion between `O` and `0`, thousands separators, and other ambiguities remains possible; this is not a live-screen accuracy guarantee. Five retained regression crops cover previously missed `$164.21` decimals on dark/light backgrounds and ensure ordinary text and single-digit readings are preserved. With local OCR models, run them using `cargo test --release --features gui --lib desktop::tests::small_price_ocr_preserves_decimals_and_text -- --ignored`.

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
- Each node can return an error, for example if screen capture, OCR, script evaluation, or simulated input fails. The GUI stops automation, cancels pending waits, and displays the error in its status area without rearming once gates.
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

For a brokerage-style automation target, run `node tests/web/trading-server.js` (Node.js required), then open `http://127.0.0.1:8765/`. The page loads actual NVIDIA and Apple price history immediately from Yahoo Finance and refreshes every 15 seconds. Choose **1D**, **5D**, or **1M** charts (1-minute, 5-minute, or 30-minute bars), and hover over the graph to inspect prices and New York timestamps. Quotes may be delayed; polling does not provide an exchange real-time feed, and prices may not change when markets are closed. SpaceX and Anthropic are private, so their prices and trading are unavailable. Buy and sell orders are simulated, and the account resets when the page is refreshed. Internet access is required; quote errors disable trading in the affected instrument. Run `node --test tests/web/trading-server.test.js` to test history processing.

The trade ticket uses **Buy / Sell → Market / Limit → Review → Submit → Confirmation**. Orders use whole shares. Market orders fill at the latest available paper quote when submitted. Limit orders fill at the quoted price only when it meets the limit or better; otherwise they remain open until cancelled in **Order history** or the page is refreshed. Open buy orders reserve buying power, and open sell orders reserve shares. Later fills require a newer provider quote, not merely refetching the same quote. This simplified simulator does not model exchange liquidity, partial fills, trading-session restrictions, or fees; even good-until-cancelled orders exist only for the current page session.

### Example screen workflow

Load [`tests/trading-buy.json`](tests/trading-buy.json) in the desktop app. It demonstrates one paper purchase: validate a price, test an example ceiling, latch once, verify the paper badge and empty position, select Buy, replace/verify quantity, open Review, assert ticker/action/quantity/type/total format, submit once, verify the filled confirmation, and verify the updated holding.

- **It is a calibration template, not a ready-positioned desktop macro.** Every screen region is the same placeholder `(100, 100, 300, 120)`. Reposition and tightly size each region on your own display, including the review and confirmation dialog states. Keep window size, browser zoom, and scroll position fixed, with the ticket controls visible.
- Start the paper page fresh with **NVIDIA**, **Market order**, and zero shares. Crop the price region to digits and decimal only, excluding `$`. The decimal regex intentionally rejects missing decimal points. The `$200` ceiling is an arbitrary test setting, not a recommendation or a guaranteed fill-price cap.
- Keep `one_order` disarmed while positioning regions. Use Dry Run/Step to check readings and planned actions; preview cannot create real dialogs, so open them manually to calibrate their regions. Close the preview and dialogs, restore the starting screen, then arm the gate **before the first live Run** on this paper page.
- If submission or confirmation times out, Stop and inspect the page's Order history; never blindly resubmit. Stop/Run alone neither resumes a cancelled wait nor clears the gate. If you ran while disarmed, earlier OCR/comparison state can suppress an identical trigger; save/reload the calibrated graph before the intended run.
- This JSON is for the local paper page only. It does not configure a real brokerage account, model uncertain execution, or replace human approval for real-money orders. The estimated-total assertion checks formatting, not a spending limit.

#### Calibrated desktop example

[`tests/trading-buy.calibrated.json`](tests/trading-buy.calibrated.json) contains regions calibrated and exercised with the actual desktop app, capture, OCR, mouse clicks, field replacement, and Enter key. It is also saved **disarmed**. The tested layout is:

- Microsoft Edge maximized on the primary display, with a browser client (including chrome) of **1920 x 1140 physical pixels**, Windows scaling **125%**, and browser zoom **67%**.
- Open `http://127.0.0.1:8765/` at the very top, with NVIDIA selected, Market order, and a fresh zero-share paper account. Keep ScreenAutomation on the left monitor, clear of the browser.
- Do not move, resize, zoom, scroll, or cover the browser during execution. Stored regions use UI points; the app converts them using each overlay's measured display scale. Other layouts require recalibration.
- The paper badge uses multiline detection so its rounded border is not recognized as a parenthesis. Confirmation uses a tight, single-line crop of the first summary line, avoiding multiline reading-order errors. After verifying it, Enter activates the confirmation dialog's focused Done button.
- Currency assertions permit OCR whitespace immediately after `$` (for example, `$ 164.94`), but still require the expected stock, side, quantity, and two-decimal price format. The price-signal crop and numeric validator remain digits-only and strict.

The buy-only desktop smoke test completed price validation, review assertions, one simulated NVDA buy at **$164.72**, confirmation verification, automatic dialog dismissal, and the final OCR check that shares owned became **1**. Continued polling did not place another order. Region windows disappeared during execution and returned on Stop. This is a layout-specific smoke test, not a guarantee against future page/layout or OCR changes.

#### Buy and sell round trip

[`tests/trading-buy-sell.calibrated.json`](tests/trading-buy-sell.calibrated.json) uses the same display layout and starts **disarmed**. On a fresh NVIDIA Market ticket with zero shares and no orders, it buys one share, verifies/dismisses the confirmation, waits for one owned share, then selects Sell and repeats the quantity, review, and fill checks before verifying zero shares. One Once gate protects the entire sequence; it is a fixed paper test, not a trading strategy.

The corrected desktop test filled exactly two orders: buy **1 NVDA at $164.40**, then sell **1 NVDA at $164.44**. Both confirmations closed automatically, holdings returned to **0**, and buying power became **$100,000.04**. More than a minute of continued polling produced no additional orders. Trying to review another sell with zero shares displayed the insufficient-shares error and created no order. An earlier attempt correctly stopped after a filled buy because OCR inserted a space after `$`; the currency matching above fixes that false rejection without bypassing the confirmation checks. A failure after Submit does not undo a fill: inspect Order history before any retry, and do not reload/rearm blindly.

A repeat desktop test completed **five one-share buy/sell round trips**, producing exactly ten filled NVDA Market orders and zero remaining shares after every completed round. Final buying power was **$100,000.01**, with no duplicate orders during an additional minute of running. These were not five error-free starts: one of six starts stopped before submission when review-total OCR read `$16421` without its decimal. After closing the unsubmitted review and verifying the original six orders and zero shares, a deliberate retry completed the fourth round. Strict validation was unchanged; intermittent decimal recognition remains a reliability limitation. The workflow was stopped and reloaded disarmed afterward.

A subsequent rapid-repeat desktop test used a temporary, finite ten-round chain with 100 ms workflow refresh and wait polling, retaining the calibrated assertions and confirmations. It was stopped after **eight completed rounds**, not the planned ten. Order history verified exactly **16 additional filled one-share NVDA Market orders**, alternating Buy/Sell (#11 through #26), with **122 seconds between the first and last new fills**. Final holdings were zero and buying power remained **$100,000.01**; no account reset was used. This is measured screen/OCR throughput, not high-frequency trading or a completed ten-round certification. The temporary armed graph was removed; the user's subsequently reopened buy workflow was left stopped and saved disarmed.

#### IBKR Desktop one-cent paper workflow

[`tests/trading-ibkr-paper.json`](tests/trading-ibkr-paper.json) is a **disarmed reference workflow** for the actual IBKR Desktop simulated account, not the local browser simulator. It requires rebuilding the desktop app with the Function runtime-state support. Regenerate with `node tests\generate-ibkr-workflow.js`.

- Keep the NVDA **Buy ticket open** between orders, with **Instant Submit disabled**, exactly zero or one share, and **no open orders**. Use only the simulated account; never switch accounts or use manual order entry while running.
- The first valid price establishes a baseline. Subsequent changes of **at least $0.01** from the previous valid observed price buy **one share only when flat**, or sell **one share only when holding one**. Continued rises while holding one and falls while flat do nothing. There is no Once gate: eligible cycles repeat until stopped.
- The workflow replaces/verifies quantity, selects/verifies Market, opens Preview, checks NVDA/side/quantity/type/current position and the simulated banner, then Transmits once. It waits for **Your order has been filled**, verifies the fill summary, closes Done, reopens Buy, and verifies the new holding. Pending waits and a position-transition latch prevent new orders; a one-second cooldown follows recognition of that transition. A price jump over $0.50 during new signal evaluation stops for inspection.
- **Calibrate before arming.** Coordinates target the observed far-left display at 100% scaling (main window physical origin `-5760,0`, preview `-5240,206`). The main-window screenshot was displayed at 1536 pixels from a 1920-pixel capture; its coordinates must be multiplied by 1.25, unlike the full-size preview. Price and the Buy-ticket Position row have been checked in ScreenAutomation, but order-entry/review/fill regions remain reference estimates. Position OCR includes its label and must read exactly `Position -`, `Position 0`, or `Position 1`; blank or unlabeled readings are never treated as flat. Check all overlays, including the very small simulated banner, Buy/Sell ticket layout differences, and the separate Preview window. Keep Preview scrolled to the top. Use Dry Run/Step and manually opened previews before setting `workflow_armed` to `true`. A Dry Run cannot create the dialogs it records.
- Invalid positions and review/fill mismatches stop execution. Missing decimal points fail numeric validation instead of being guessed. Esc/Stop cannot undo a transmitted order. On any failure, inspect **Orders & Trades** and actual positions before restarting or reloading; runtime state is not persistent across loads or crashes. A pending order must never be retried automatically.

**Validation status:** fake-backend tests exercise three repeating one-cent buy/sell cycles, holding limits, pending-fill reentry blocking, timeout, failed reviews, and rejected blank/unlabeled positions. The rebuilt ScreenAutomation app passed graph validation, completed a disarmed Dry Run, and recognized actual prices (including `153.88`) and `Position -` in Step. A timed disarmed run emitted only WAIT and was stopped. Testing exposed incorrect monitor coordinates and overlay contamination in Dry Run; both were corrected and retested. Computer Use previously verified one explicitly approved manual paper BUY and its filled confirmation; a subsequent manual SELL fill was visible. Neither was submitted by the ScreenAutomation workflow. Order preparation, submission/fill verification, and continuous desktop trading remain **unverified**; the attempted non-submitting preparation test did not execute. The saved workflow remains disarmed. IBKR displayed **DELAYED DATA** and a missing-market-data warning during inspection. Screen changes do not establish exchange quote freshness. A one-cent change can be smaller than spreads and commissions; this is an automation exercise, not a profitable strategy or live-trading recommendation. Do not leave it unattended until the complete calibrated screen path has been exercised.

Later chart/history/news analysis should supply a structured signal to the decision stage only, leaving the one-share limit, simulated-account checks, order review, and fill checks independent of AI. This phase does not implement AI or send account/screen data to an external model.

#### Continuous historical breakout paper workflow

[`tests/trading-breakout.json`](tests/trading-breakout.json) is a **disarmed**, repeating screen workflow for NVDA. It uses a visible chart-study panel, like a trading platform indicator; the panel never submits orders. Order entry, review assertions, and fill verification remain in the screen workflow.

1. Restart `node tests\web\trading-server.js` after updating the server. Open `http://127.0.0.1:8765/?breakout=1` in a **new tab** if you want to preserve an existing paper account. Refreshing a tab resets its account.
2. Select **NVDA**, **Market**, and **1D** (one-minute candles), with 0-10 owned NVDA shares and no open orders. The study uses the highest high / lowest low of the **last 20 consecutive completed one-minute candles**, excluding the current candle and the appended last-price quote.
3. **Buy start:** price strictly above resistance + 0.1%. **Buy stop:** price strictly below resistance. **Sell start:** price strictly below support - 0.1%. **Sell stop:** price strictly above support. Equality does not start or stop a phase. Levels stay frozen during an active phase and are recalculated while idle. Invalid data clears the phase.
4. Check all regions before arming. Ticket and dialog regions reuse the calibrated layout above. The two new indicator crops are `signal` and the `indicator_armed` guards, positioned from measured desktop bounds at the layout above (125% Windows scaling, 67% Edge zoom); verify them on your display. The panel is fixed in the bottom-left gutter and does not move the existing ticket/dialog controls.
5. Enable **guarded paper signals** in the panel, then set the workflow's `workflow_armed` Boolean Constant from `false` to `true` only when ready. Both must be armed. The workflow starts with `false`; merely opening the indicator never places orders.
6. While a phase is active, submit **one share per order**, at most **10 held**, with **no short selling**. New signals are suppressed while either dialog is open and for a full second after confirmation closes. The changing position in each command allows the next order even if the price is unchanged. The page also checks quantity, symbol, Market type, phase, holdings, open orders, and cooldown again when Submit is clicked.
7. Stop with **Esc**; disarm the panel and workflow before editing/repositioning. Once guarded signals have been enabled in a tab, disarming blocks new paper orders there until re-enabled or the tab is reloaded. If any review/fill check fails, inspect Order history before retrying: a failure after Submit does not undo a fill.

Quotes older than **90 seconds**, insufficient/gapped history, a non-one-minute chart, or provider failures produce **WAIT**, never guessed prices or orders. Yahoo data may be delayed: closed markets and delayed feeds can legitimately leave this workflow waiting. This is a paper automation test, **not a recommended trading strategy or a live-trading certification**.

Offline validation exercises ten buys followed by ten sells using simulated price moves, checks the ten/zero holding limits, strict four-threshold transitions, frozen levels, 999 ms versus 1,000 ms cooldown, busy-order suppression, and pre-submission phase changes. These tests do not send desktop input or place orders. Run:

```powershell
node --test tests\web\trading-server.test.js
cargo test --test workflow_tools
```

An additional offline local-model OCR check recognized all 46 synthetic indicator crops exactly: every valid buy/sell position, WAIT, and both armed markers, with two text-rendering modes at the measured scale. This is not a desktop execution test; actual review/confirmation OCR still has the reliability limitations noted above.

Regenerate the fixture after deliberately changing its template with `node tests\generate-breakout-workflow.js`.

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
    trading.html Paper-trading test page
    trading-server.js Local quote proxy for the paper-trading page
    breakout-study.js Historical breakout indicator and guarded paper signals
  trading-breakout.json Disarmed repeating NVDA paper screen workflow
```

## Current scope

ScreenAutomation is an early, deliberately small automation tool rather than a full workflow platform. The editor provides a visual node canvas and direct screen-region overlays, but does not show a live graph execution trace or per-node output inspector. It does not currently provide workflow scheduling outside the running application, remote execution, or a built-in mechanism for selecting regions from inside another process. The graph engine and backend trait are intended to keep the core workflow logic testable and make alternate integrations possible.
