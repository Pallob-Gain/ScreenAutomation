"""Pluggable OS backends (screen capture, OCR, keyboard, mouse).

Third-party libraries are imported lazily so the core works without them.
"""
from dataclasses import dataclass


@dataclass
class Region:
    x: int = 100
    y: int = 100
    width: int = 300
    height: int = 120

    def bbox(self):
        return (self.x, self.y, self.x + self.width, self.y + self.height)

    def center(self):
        return (self.x + self.width // 2, self.y + self.height // 2)


class Backends:
    """Default implementations: Pillow (capture), pytesseract (OCR), pyautogui (input)."""

    def capture(self, region):
        from PIL import ImageGrab
        return ImageGrab.grab(bbox=region.bbox())

    def ocr(self, image):
        import pytesseract
        return pytesseract.image_to_string(image).strip()

    def type_text(self, text):
        import pyautogui
        pyautogui.write(str(text))

    def click(self, x, y, button="left"):
        import pyautogui
        pyautogui.click(x, y, button=button)
