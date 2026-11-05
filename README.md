# ScreenAutomation

ScreenAutomation is a desktop screen-automation prototype built in Rust. It combines a small, n8n-inspired node graph with screen-region capture, OCR, simple value transformation, and mouse/keyboard actions. You assemble a workflow from nodes, connect the nodes that should pass data to one another, and run the graph repeatedly at a configurable interval.

The project has two parts:

- A reusable graph engine that can be tested without a desktop display.
- An optional native desktop application with an interactive node-canvas editor, screen-region overlays, capture, OCR, and input.

> **Use with care.** A running workflow can click and type into other applications. Review the graph and target regions before pressing **Run**, and press **Stop** before editing or leaving an automation unattended.

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
- Internet access the first time OCR is used, so the application can download its OCR models. Models are cached in the operating system's application cache directory afterward.

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

The GUI initializes a desktop input backend when it starts. The first OCR operation downloads the OCRS detection and recognition models into the application cache, then loads them locally. A model download or OCR error is shown in the application status area. Screen capture and simulated input also depend on operating-system support, permissions, and the active desktop session.

## Using the application

1. Start the GUI with `cargo run --features gui`.
2. Choose a node type from the type selector and press **＋ Add node**. The node appears on the canvas with a generated name.
3. Drag nodes to arrange the workflow. Drag from an output pin to an input pin to create an edge; use the graph editor's pin controls to remove or reroute connections. Use the canvas zoom and pan controls to navigate larger workflows.
4. Select a node in the canvas to edit its settings in the side panel. For OCR, keyboard, and click nodes, position and resize the transparent on-screen region window. Drag inside the window to move it; drag its lower-right grip to resize it. The region is used both for screen capture and as the target location for actions.
5. Configure the refresh interval, then press **Run**. Press **Stop** to pause execution.
6. Enter a file path in the save/load field and use **Save** or **Load** to persist or restore the graph. The default path is `graph.json`.

Region-bearing nodes start with a region at screen position `(100, 100)` and size `300 × 120` pixels. Their overlay color and opacity can be adjusted in the selected node's settings. Opacity applies to the complete overlay (fill, outline, label, and resize grip), not only the fill color.

Node positions are saved with the workflow. Existing workflow files that do not contain positions are laid out automatically when loaded.

## Node types

| Type | Inputs and behavior | Output |
| --- | --- | --- |
| `ocr` | Captures its region and runs OCR only when the captured pixel bytes differ from the previous capture. | Recognized text, trimmed. No output when the pixels are unchanged. |
| `model` | Uses the first input and converts it to the selected `text`, `number`, `int`, or `bool` format. | The converted value, or no output if numeric parsing fails. |
| `function` | Evaluates Rhai code with an `inputs` array in scope. The last expression is the result. | A boolean, integer, number, or text value. Rhai's unit value `()` produces no output. |
| `keyboard` | Requires an input. Clicks the center of its region, then types its configured text; if that setting is empty, it types the first input converted to text. | The text it typed. |
| `click` | Requires at least one input. Clicks the center of its region using the configured mouse button (`left`, `right`, or `middle`; unknown values fall back to `left`). | `true` after the click. |

`model` and `function` nodes do not have screen regions. OCR, keyboard, and click nodes do.

### Example workflow

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
- The refresh interval is adjustable from 100 ms to 60,000 ms; the initial interval is 1,000 ms.

## Saving and loading

The application serializes graphs as readable JSON. A graph contains its nodes and directed connections. Node data includes its name, type-specific settings, canvas position, optional screen region, overlay color, and opacity. OCR's in-memory pixel-change cache is not persisted, so a loaded OCR node establishes its baseline when it next runs.

Use the **Save** and **Load** controls in the desktop app to write or read the JSON file at the path shown in the file field. Loading replaces the current graph. Keep a backup of workflows you want to preserve.

## Library and test backend

The core modules are available without enabling the GUI:

- `backend`: `Region` and the `Backend` trait for screen capture, OCR, text entry, and clicking.
- `node`: graph node types, formats, and values.
- `graph`: node management, connections, JSON serialization, and one-tick execution.

Implement `Backend` to connect the engine to another environment or to a test double. The integration tests use a fake backend to verify workflow execution and screen-change behavior without capturing a real display or sending real input.

## Project layout

```text
src/
  backend.rs   Screen-region type and OS-access abstraction
  desktop.rs   Optional real desktop capture, OCRS engine, and input backend
  graph.rs     Graph operations, ordering, persistence, and execution
  lib.rs       Library module exports
  main.rs      Optional eframe application and egui-snarl graph editor
  node.rs      Node kinds, values, conversions, and processing
tests/
  graph.rs     Graph execution, change-detection, and JSON/cycle tests
```

## Current scope

ScreenAutomation is an early, deliberately small automation tool rather than a full workflow platform. The editor provides a visual node canvas and direct screen-region overlays, but does not show a live graph execution trace or per-node output inspector. It does not currently provide workflow scheduling outside the running application, remote execution, or a built-in mechanism for selecting regions from inside another process. The graph engine and backend trait are intended to keep the core workflow logic testable and make alternate integrations possible.
