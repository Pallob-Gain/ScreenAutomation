use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Default for Region {
    fn default() -> Self {
        Region { x: 100, y: 100, width: 300, height: 120 }
    }
}

impl Region {
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.width as i32 / 2, self.y + self.height as i32 / 2)
    }
}

/// OS access abstraction so the engine can be tested without a display.
pub trait Backend {
    /// Raw pixels of the region (used for change detection).
    fn capture(&mut self, region: &Region) -> Result<Vec<u8>, String>;
    /// Text recognised in the pixels returned by `capture`.
    fn ocr(&mut self, region: &Region, pixels: &[u8]) -> Result<String, String>;
    /// Recognizes the region as one line, without automatic layout detection.
    fn ocr_single_line(&mut self, _: &Region, _: &[u8]) -> Result<String, String> {
        Err("Single-line OCR is not supported by this backend".into())
    }
    fn type_text(&mut self, text: &str) -> Result<(), String>;
    fn click(&mut self, x: i32, y: i32, button: &str) -> Result<(), String>;
}
