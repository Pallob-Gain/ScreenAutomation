# ScreenAutomation

An n8n-like node editor driven by screen regions.

- Pick a node type and press **＋ Add**: a node is created and (for OCR / Keyboard / Click nodes)
  a frameless, translucent overlay window appears with a border and the node name as title.
  Drag it to move, drag the red corner to resize; **Color** and the **Opacity** slider restyle it.
- Node types: `ocr` (reads the region when its pixels change), `model` (convert to number/int/text/bool/json),
  `function` (Python code: use `inputs`, set `result`; `None` stops the flow), `keyboard` (types into the region),
  `click` (clicks the region center).
- **Connect** links the selected node to a target; **Run** executes the graph every second.

```
pip install -r requirements.txt   # plus tkinter and the tesseract binary
python -m screenauto
python -m unittest discover tests
```
