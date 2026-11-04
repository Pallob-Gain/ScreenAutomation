# ScreenAutomation

An n8n-like node editor (Rust + egui) driven by screen regions.

- **＋ Add** creates a node of the chosen type. Nodes that act on the screen (ocr, keyboard, click) also open a
  title-bar-less, transparent overlay window with a red border and the node name as its title. Drag to move,
  drag the red corner to resize; color and opacity are set in the node's settings.
- Node types: `ocr` (runs tesseract on its region only when the pixels changed), `model` (convert to
  text/number/int/bool), `function` ([Rhai](https://rhai.rs) script; `inputs` array; last expression is the result,
  `()` stops the flow, e.g. `if inputs[0] > 10.0 { "go" } else { () }`), `keyboard` (clicks the region, types),
  `click` (clicks the region center).
- Connect nodes, then **Run**; the graph executes every refresh interval. Graphs save/load as JSON.

```
cargo test                      # core engine, no display needed
cargo run --features gui        # needs the tesseract binary and system libs (X11, dbus, xdo)
```
