//! Real backends: xcap (capture), tesseract CLI (OCR), enigo (input).
use crate::backend::{Backend, Region};
use enigo::{Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
use std::process::Command;

pub struct Desktop {
    enigo: Enigo,
    width: u32,
    height: u32,
}

impl Desktop {
    pub fn new() -> Result<Desktop, String> {
        Ok(Desktop { enigo: Enigo::new(&Settings::default()).map_err(|e| e.to_string())?, width: 0, height: 0 })
    }
}

impl Backend for Desktop {
    fn capture(&mut self, r: &Region) -> Result<Vec<u8>, String> {
        let mon = xcap::Monitor::from_point(r.x, r.y).map_err(|e| e.to_string())?;
        let img = mon.capture_image().map_err(|e| e.to_string())?;
        let (iw, ih) = (img.width() as i32, img.height() as i32);
        let (mx, my) = (r.x - mon.x().map_err(|e| e.to_string())?, r.y - mon.y().map_err(|e| e.to_string())?);
        let x0 = mx.clamp(0, iw);
        let y0 = my.clamp(0, ih);
        let x1 = (mx + r.width as i32).clamp(0, iw);
        let y1 = (my + r.height as i32).clamp(0, ih);
        let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
        self.width = w;
        self.height = h;
        let raw = img.as_raw();
        let mut out = Vec::with_capacity((w * h * 3) as usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * iw + x) * 4) as usize;
                out.extend_from_slice(&raw[i..i + 3]);
            }
        }
        Ok(out)
    }

    fn ocr(&mut self, _: &Region, pixels: &[u8]) -> Result<String, String> {
        if self.width == 0 || self.height == 0 {
            return Ok(String::new());
        }
        let path = std::env::temp_dir().join(format!("screen_automation_{}.ppm", std::process::id()));
        let mut data = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        data.extend_from_slice(pixels);
        std::fs::write(&path, data).map_err(|e| e.to_string())?;
        let out = Command::new("tesseract").arg(&path).arg("stdout").output();
        let _ = std::fs::remove_file(&path);
        let out = out.map_err(|e| format!("tesseract not available: {e}"))?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn type_text(&mut self, text: &str) -> Result<(), String> {
        self.enigo.text(text).map_err(|e| e.to_string())
    }

    fn click(&mut self, x: i32, y: i32, button: &str) -> Result<(), String> {
        self.enigo.move_mouse(x, y, Coordinate::Abs).map_err(|e| e.to_string())?;
        let b = match button {
            "right" => Button::Right,
            "middle" => Button::Middle,
            _ => Button::Left,
        };
        self.enigo.button(b, Direction::Click).map_err(|e| e.to_string())
    }
}
