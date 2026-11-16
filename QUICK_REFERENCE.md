# ScreenAutomation - Quick Reference

## Running with Debug Output

```powershell
cd 'E:\Software projects\Other projects\ScreenAutomation'
.\target\debug\screen_automation.exe
```

## Debug Output Meanings

### OCR Node
```
[OCR] Region: (100, 100) 300x120, Pixels captured: 108000 bytes
[OCR] Recognition result: 'recognized text'
```
- ✅ Means: Region is valid, pixels captured, text recognized
- ❌ `Pixels captured: 0 bytes` = Region off-screen
- ❌ `Recognition result: ''` = No text found (adjust region)

### Keyboard Node
```
[KEYBOARD] Typing text: 'hello world' (region: 300 x 120 at 100, 100)
```
- ✅ Means: Text is being typed
- ❌ Missing message = No input connection
- **Important:** Mouse is NOT moved (fixed!)

### Click Node
```
[CLICK] Clicking at (250, 160) with button: left
```
- ✅ Means: Click executed at coordinates
- ❌ Missing message = No input connection

---

## Common Issues & Quick Fixes

| Issue | Symptom | Fix |
|-------|---------|-----|
| **OCR not recognizing text** | `Recognition result: ''` | Move overlay over clearer text, enlarge region |
| **Keyboard not typing** | No `[KEYBOARD]` message | Check input connection, ensure target window focused |
| **Overlay resize janky** | Resize jumps | ✅ Fixed in this update |
| **Mouse moves when typing** | Cursor moves to region | ✅ Fixed in this update |
| **Node won't delete** | Can't remove node | Select node, right panel "Delete node" button |
| **No debug output** | Can't see logs | Run from terminal/PowerShell, check stderr |

---

## Node Creation Quick Guide

### OCR Node (Read Text)
1. Select "ocr" from dropdown
2. Click "+ Add node"
3. Drag overlay window to text
4. Resize overlay to cover text area

### Keyboard Node (Type Text)
1. Select "keyboard" from dropdown
2. Click "+ Add node"
3. Connect input from another node
4. (Optional) Enter fixed text in settings

### Click Node (Click Area)
1. Select "click" from dropdown
2. Click "+ Add node"
3. Connect input from another node
4. (Optional) Change button (left/right/middle)

### Function Node (Transform Data)
1. Select "function" from dropdown
2. Click "+ Add node"
3. Edit Rhai code in settings
4. Example: `inputs[0]` returns first input

### Model Node (Format Data)
1. Select "model" from dropdown
2. Click "+ Add node"
3. Choose format (Text/Number/Int/Bool)

---

## Workflow Examples

### Example 1: Read and Type
```
OCR → Keyboard
```
OCR reads text, Keyboard types it

### Example 2: Click with Input
```
Function (create delay) → Click
```
Function calculates, Click acts on it

### Example 3: Read, Process, Type
```
OCR → Function (transform) → Keyboard
```
OCR reads, Function modifies text, Keyboard types result

### Example 4: Multi-Step
```
OCR → Model (format to number) → Function (calculate) → Keyboard (type result)
```

---

## Build Commands

```bash
# Build with debug info (faster compile)
cargo build --features gui

# Build optimized (faster runtime)
cargo build --release --features gui

# Run tests
cargo test
```

---

## File Locations

| File | Purpose |
|------|---------|
| `src/node.rs` | Node type definitions and processing |
| `src/main.rs` | GUI and overlay rendering |
| `src/graph.rs` | Workflow graph management |
| `src/desktop.rs` | OCR, keyboard, mouse implementation |
| `src/backend.rs` | Interface for OS operations |

---

## Keyboard Shortcuts (Future Enhancement)

Currently none, but could add:
- `Delete` - Delete selected node
- `Ctrl+S` - Save workflow
- `Ctrl+Z` - Undo (not yet implemented)

---

## Performance Tuning

| Setting | Effect | Default |
|---------|--------|---------|
| **Refresh Rate** | Lower = faster response, higher CPU | 1000ms |
| **OCR Region Size** | Smaller = faster, lower CPU | Variable |
| **Number of Nodes** | More nodes = more CPU | Variable |

**Recommendation:**
- Start with 1000ms refresh rate
- Reduce if response is too slow
- Increase if CPU usage is high

---

## Troubleshooting Workflow

1. **Run with debug output:** See all messages
2. **Check logs:** Look for `[OCR]`, `[KEYBOARD]`, `[CLICK]`
3. **Isolate nodes:** Test each node individually
4. **Check connections:** Ensure edges are connected
5. **Verify regions:** Overlay windows in correct positions
6. **Test manually:** Do actions manually to verify workflow idea

---

## Tips & Tricks

✨ **Pro Tips:**
- Save workflows frequently (button in toolbar)
- Start simple, build complexity
- Use Function nodes to debug data flow
- Keep OCR regions small for speed
- Test workflows on multiple runs

⚠️ **Important:**
- Ensure target window is focused before running
- Some apps require special permissions for input
- OCR quality depends on text clarity
- Refresh rate affects responsiveness vs. CPU usage

---

## Support

If something isn't working:
1. Check DEBUG_GUIDE.md for detailed troubleshooting
2. Review FIXES_SUMMARY.md for what changed
3. Check this QUICK_REFERENCE.md for common issues
4. Run with debug output to see what's happening

All three documents are in the project root directory.
