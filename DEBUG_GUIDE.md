# Debug Guide for ScreenAutomation

## Debugging Keyboard & OCR Issues

### Running with Debug Output

To see the debug messages, run the application from a terminal/command prompt:

#### Windows (PowerShell)
```powershell
cd 'E:\Software projects\Other projects\ScreenAutomation'
.\target\debug\screen_automation.exe
```

#### Windows (Command Prompt)
```cmd
cd E:\Software projects\Other projects\ScreenAutomation
target\debug\screen_automation.exe
```

The debug messages will appear in your terminal as `[OCR]`, `[KEYBOARD]`, and `[CLICK]` prefixed lines.

---

## Understanding Debug Output

### OCR Debug Messages

When an OCR node runs, you'll see:
```
[OCR] Region: (100, 100) 300x120, Pixels captured: 108000 bytes
[OCR] Recognition result: 'recognized text here'
```

**What to look for:**
- ✅ `Pixels captured: X bytes` - OCR is successfully capturing screen region
- ❌ `Pixels captured: 0 bytes` - Region is off-screen or invalid
- ✅ `Recognition result: '...'` - Text was successfully recognized
- ❌ `Recognition result: ''` - No text found in the region (check if region is correct)

**If OCR isn't working:**
1. Verify the region overlay is positioned over text you want to read
2. Check that the text is clearly visible and not too small
3. Ensure the region coordinates are on your active screen

### Keyboard Debug Messages

When a Keyboard node runs, you'll see:
```
[KEYBOARD] Typing text: 'hello world' (region: 300 x 120 at 100, 100)
```

**What to look for:**
- ✅ Message appears - Keyboard node is executing
- ❌ No message - Keyboard node didn't receive input (check input connections)
- Check that the text is what you expect

**If keyboard typing isn't working:**
1. Ensure the target application is in focus (has focus window)
2. Try using the `Function` node to transform text before passing to Keyboard
3. Ensure the Keyboard node has an input connected to it

### Click Debug Messages

When a Click node runs, you'll see:
```
[CLICK] Clicking at (250, 160) with button: left
```

**What to look for:**
- Coordinates should be where you want to click
- Button should be "left", "right", or "middle"

---

## Workflow Testing Examples

### Example 1: Read Text from Screen and Type It

1. **Create OCR Node:**
   - Select "ocr" from dropdown
   - Click "+ Add node"
   - An OCR node will be created

2. **Position OCR Node:**
   - A viewport window will appear on your screen
   - Drag it over text you want to read
   - Resize it using the bottom-right corner grip

3. **Create Keyboard Node:**
   - Select "keyboard" from dropdown
   - Click "+ Add node"

4. **Connect Nodes:**
   - Drag output circle from OCR node
   - Drop on input circle of Keyboard node
   - Connection line should appear

5. **Run Workflow:**
   - Adjust refresh rate (e.g., 500ms)
   - Click "Run"
   - Check terminal for:
     ```
     [OCR] Region: ... Pixels captured: ...
     [OCR] Recognition result: '...'
     [KEYBOARD] Typing text: '...'
     ```

### Example 2: Debug OCR Not Working

If OCR node shows `Pixels captured: X bytes` but `Recognition result: ''`:

1. Check the region overlay:
   - Make sure it covers the text area
   - Increase the region size
   - Ensure text is not too small

2. Verify OCR models are downloaded:
   - Check: `%LOCALAPPDATA%\ScreenAutomation\ScreenAutomation\cache\ocrs-models\`
   - Should have `text-detection.onnx` and `text-recognition.onnx`

3. Try with different text:
   - Move overlay to different text
   - Test with larger, clearer text

### Example 3: Debug Keyboard Not Typing

If you see `[KEYBOARD]` message but text isn't being typed:

1. Check application focus:
   - Make sure target app is focused
   - Text input should go to active window

2. Check text encoding:
   - Try simple ASCII text first
   - Then test with special characters

3. Create a test workflow:
   - Use Function node to create text: `"hello"`
   - Connect to Keyboard node
   - Run and check if it types

---

## Common Issues and Solutions

### Issue: "No pixels captured" for OCR

**Symptom:** `Pixels captured: 0 bytes`

**Solutions:**
1. Move overlay window completely on-screen
2. Ensure monitor is in same coordinate space
3. Check if overlay size is too small (minimum 80x60)

### Issue: OCR recognizes but Keyboard doesn't type

**Symptom:** 
```
[OCR] Recognition result: 'hello'
(but no [KEYBOARD] message)
```

**Solution:**
- Check connection between OCR and Keyboard nodes
- Verify Keyboard node has input flowing to it
- Click on Keyboard node to see if it shows input in settings

### Issue: Keyboard types in wrong window

**Symptom:** Text appears in wrong application

**Solutions:**
1. Ensure target window is focused before running
2. You may need to add a Click node before Keyboard to focus window:
   - Create Click node over target window
   - Connect to it before Keyboard node

### Issue: Overlay resize jumps or is jerky

**Symptom:** Resizing the region overlay feels unresponsive

**Solutions:**
1. This is now fixed - should be smooth
2. If still having issues:
   - Check system performance
   - Reduce refresh rate to lower CPU usage
   - Restart the application

---

## Checking Error Logs

All errors are displayed in the "status" area at the bottom of the left toolbar panel. 

**Common errors:**
- `"unknown node"` - Connection references non-existent node
- `"graph contains a cycle"` - Circular dependency in workflow
- `"duplicate node name"` - Two nodes with same name (shouldn't happen)

---

## Performance Tips

1. **Increase refresh rate** if OCR/Keyboard seem slow:
   - Default is 1000ms (1 second)
   - Reduce to 500ms for faster response
   - Increase if high CPU usage

2. **Minimize active nodes:**
   - Only enable nodes you're using
   - Extra nodes still run even if not in use

3. **OCR is resource-intensive:**
   - Running OCR on large region uses significant CPU
   - Minimize overlay region size to what you need
   - Increase refresh rate if needed

---

## Collecting Debug Info for Support

If something isn't working:

1. **Take a screenshot** of the workflow
2. **Copy the workflow** (Save to file, share contents)
3. **Capture console output:**
   - Run from terminal
   - Reproduce issue
   - Copy output with [OCR], [KEYBOARD], [CLICK] messages
4. **Note exact steps** to reproduce

This will help diagnose what's going wrong!
