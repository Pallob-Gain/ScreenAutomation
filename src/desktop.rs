//! Real backends: xcap (capture), OCRS (OCR), enigo (input).
use crate::backend::{Backend, Region};
use directories::ProjectDirs;
use enigo::{Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use rten::Model;
use std::path::{Path, PathBuf};

pub struct Desktop {
    enigo: Enigo,
    width: u32,
    height: u32,
    ocr: Option<OcrEngine>,
}

impl Desktop {
    pub fn new() -> Result<Desktop, String> {
        Ok(Desktop {
            enigo: Enigo::new(&Settings::default()).map_err(|e| e.to_string())?,
            width: 0,
            height: 0,
            ocr: None,
        })
    }

    fn ocr_engine(&mut self) -> Result<&OcrEngine, String> {
        if self.ocr.is_none() {
            self.ocr = Some(load_ocr_engine()?);
        }
        Ok(self.ocr.as_ref().expect("OCR engine was initialized"))
    }
}

fn model_path(directory: &Path, filename: &str, url: &str) -> Result<PathBuf, String> {
    let path = directory.join(filename);
    if path.is_file() {
        return Ok(path);
    }

    let temporary_path = directory.join(format!("{filename}.{}.download", std::process::id()));
    let response = ureq::get(url)
        .call()
        .map_err(|e| format!("could not download OCR model {filename}: {e}"))?;
    let download_result = (|| {
        let mut file = std::fs::File::create(&temporary_path).map_err(|e| e.to_string())?;
        let mut reader = response.into_reader();
        let downloaded = std::io::copy(&mut reader, &mut file).map_err(|e| e.to_string())?;
        if downloaded == 0 {
            return Err(format!("downloaded OCR model {filename} was empty"));
        }
        file.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&temporary_path, &path).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(error) = download_result {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error);
    }
    Ok(path)
}

fn load_ocr_engine() -> Result<OcrEngine, String> {
    let project_dirs = ProjectDirs::from("org", "ScreenAutomation", "ScreenAutomation")
        .ok_or_else(|| "could not determine the OCR model cache directory".to_string())?;
    let model_dir = project_dirs.cache_dir().join("ocrs-models");
    std::fs::create_dir_all(&model_dir).map_err(|e| e.to_string())?;

    let detection_path = model_path(
        &model_dir,
        "text-detection.onnx",
        "https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.onnx",
    )?;
    let recognition_path = model_path(
        &model_dir,
        "text-recognition.onnx",
        "https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.onnx",
    )?;
    let detection_model = Model::load_file(detection_path).map_err(|e| e.to_string())?;
    let recognition_model = Model::load_file(recognition_path).map_err(|e| e.to_string())?;
    OcrEngine::new(OcrEngineParams {
        detection_model: Some(detection_model),
        recognition_model: Some(recognition_model),
        ..Default::default()
    })
    .map_err(|e| e.to_string())
}

impl Backend for Desktop {
    fn capture(&mut self, r: &Region) -> Result<Vec<u8>, String> {
        let mon = xcap::Monitor::from_point(r.x, r.y).map_err(|e| e.to_string())?;
        let img = mon.capture_image().map_err(|e| e.to_string())?;
        let (iw, ih) = (img.width() as i32, img.height() as i32);
        let (mx, my) = (
            r.x - mon.x().map_err(|e| e.to_string())?,
            r.y - mon.y().map_err(|e| e.to_string())?,
        );
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
        let image = ImageSource::from_bytes(pixels, (self.width, self.height))
            .map_err(|e| e.to_string())?;
        let engine = self.ocr_engine()?;
        let input = engine.prepare_input(image).map_err(|e| e.to_string())?;
        engine.get_text(&input).map_err(|e| e.to_string())
    }

    fn type_text(&mut self, text: &str) -> Result<(), String> {
        self.enigo.text(text).map_err(|e| e.to_string())
    }

    fn click(&mut self, x: i32, y: i32, button: &str) -> Result<(), String> {
        self.enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|e| e.to_string())?;
        let b = match button {
            "right" => Button::Right,
            "middle" => Button::Middle,
            _ => Button::Left,
        };
        self.enigo
            .button(b, Direction::Click)
            .map_err(|e| e.to_string())
    }
}
