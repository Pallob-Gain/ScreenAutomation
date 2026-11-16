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
- Run connected nodes in dependency order on a repeating timer.
- Save and load the graph, including node settings, connections, and screen regions, as JSON.
- Test workflow behavior using a fake backend without requiring a monitor or real input devices.

## Requirements

- Rust and Cargo (Rust 1.92 or newer; the package uses Rust edition 2021).
- The native system libraries required by `eframe`, `xcap`, and `enigo` on your operating system. On Linux, desktop builds may require X11-related libraries and input/display access.
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
4. Click a node to open its floating settings window. To delete a node and its connections, drag it onto the trash icon at the bottom right of the canvas, click the icon while a node is selected, or use **Delete node** in its settings. For OCR, keyboard, and click nodes, position and resize the on-screen region window. Drag inside the window to move it; drag its lower-right grip to resize it. The region is used both for screen capture and as the target location for actions. While running, the region interior is transparent, its label and outline are outside the region, and mouse input passes through; press **Stop** to move or resize it. Avoid placing another overlay's label or outline over an OCR region.
5. Enter a refresh delay in milliseconds (100–60,000; default 3,000), then press **Run**. Invalid input leaves the last valid delay in effect. Press **Stop** or the **Esc** key to stop execution. On Windows, **Esc** stops the workflow even when another application has focus; on other platforms the ScreenAutomation window must be focused. A run already in progress finishes before the workflow stops.
6. Enter a file path in the save/load field and use **Save** or **Load** to persist or restore the graph. The default path is `graph.json`.

Region-bearing nodes start with a region at screen position `(100, 100)` and size `300 × 120` UI points. Overlay coordinates are converted to physical screen pixels for capture and input, including on scaled displays. Keep each OCR region entirely within one monitor. Their overlay color and opacity can be adjusted in the selected node's settings. Opacity applies to the fill, outline, label, and resize grip; the fill and grip are hidden while running.

Node positions are saved with the workflow. Existing workflow files that do not contain positions are laid out automatically when loaded.

## Node types

| Type | Inputs and behavior | Output |
| --- | --- | --- |
| `ocr` | Captures its region and runs OCR only when the captured pixel bytes differ from the previous capture. | Recognized text, trimmed. No output when the pixels are unchanged. |
| `model` | Uses the first input and converts it to the selected `text`, `number`, `int`, or `bool` format. For `number` and `int`, line breaks, spaces, and symbols are removed and words such as `BTC` are skipped, so OCR text like `$ 64,777\n.49` becomes `64777.49`. Thousands separators are removed, and decimals are kept for `number`. | The converted value, or no output if no number is found. |
| `function` | Evaluates Rhai code with an `inputs` array in scope. The last expression is the result. | A boolean, integer, number, or text value. Rhai's unit value `()` produces no output. |
| `compare` | Compares the first input with its configured value using `>`, `≥`, `=`, `≠`, `≤`, or `<`. A number is extracted from text input, such as OCR output like `BTC 64777.49`. If the input or configured value is not numeric, only `=` and `≠` apply, using trimmed text. With **Only trigger when the condition becomes true** enabled, which is the default, it fires once each time the condition changes from false to true. | The unchanged input when it triggers. Otherwise, no output, so downstream nodes such as `click` do not run. |
| `threshold` | Hysteresis trigger with an upper and a lower threshold. It fires when the number rises from at or below the upper threshold to above it, or falls from at or above the lower threshold to below it. Choose **Rises above upper**, **Falls below lower**, or both. Values between the thresholds keep the last state, so after a rise the value must fall below the lower threshold before another rise can fire, and noise around one threshold does not fire repeatedly. The first reading never fires. Numbers are extracted from text in the same way as `compare`. | The unchanged input when it triggers. Otherwise, no output. |
| `keyboard` | Requires an input. Clicks the center of its region, then types its configured text; if that setting is empty, it types the first input converted to text. | The text it typed. |
| `click` | Requires at least one input. Clicks the center of its region using the configured mouse button (`left`, `right`, or `middle`; unknown values fall back to `left`). | `true` after the click. |

`model`, `function`, `compare`, and `threshold` nodes do not have screen regions. OCR, keyboard, and click nodes do.

Nodes are drawn as compact blocks, colored by type, with an icon strip, the node name, and input and output pins on the left and right edges. The selected node has an orange outline. Hover over a node to see its type, a short description, and its last output.

When a node produces output during a run, its outgoing wires and pins turn green and fade over about one second. The last output is shown under the node with a status dot. A wire that stays grey did not carry data in that run, for example because OCR saw no change or a compare condition was false.

### Example workflow

To click a button when a price rises above a threshold:

```text
ocr → compare (> 65000) → click
```

To click Button 1 when a price rises above an upper threshold and Button 2 when it falls below a lower one, use two `threshold` nodes:

```text
ocr → model (number) → threshold (rises above upper) → click (Button 1)
                    └→ threshold (falls below lower) → click (Button 2)
```

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
- The refresh interval is adjustable from 100 ms to 60,000 ms; the initial interval is 3,000 ms. It is a target time between run starts, not a guarantee: screen capture, OCR, actions, and rendering may take longer. The toolbar shows the last run duration and observed interval. Runs do not overlap or accumulate; when one takes longer than the target, the next starts as soon as possible. For the best OCR speed use the release build.

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
  main.rs      Optional eframe application and egui-snarl graph editor
  node.rs      Node kinds, values, conversions, and processing
tests/
  graph.rs     Graph execution, change-detection, and JSON/cycle tests
  web/
    index.html Manual test page (price feed, click counters, thresholds)
```

## Current scope

ScreenAutomation is an early, deliberately small automation tool rather than a full workflow platform. The editor provides a visual node canvas and direct screen-region overlays, but does not show a live graph execution trace or per-node output inspector. It does not currently provide workflow scheduling outside the running application, remote execution, or a built-in mechanism for selecting regions from inside another process. The graph engine and backend trait are intended to keep the core workflow logic testable and make alternate integrations possible.
