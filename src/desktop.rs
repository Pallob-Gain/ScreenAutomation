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

/// Folders searched for bundled models, in order: `$SCREEN_AUTOMATION_MODELS`, `models/` next to
/// the executable, and `models/` in the working directory.
fn local_model_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("SCREEN_AUTOMATION_MODELS") {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        dirs.push(dir.join("models"));
    }
    if let Ok(dir) = std::env::current_dir() {
        dirs.push(dir.join("models"));
    }
    dirs
}

fn find_model(dirs: &[PathBuf], filename: &str) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(filename))
        .find(|path| path.is_file())
}

/// Uses a local copy of the model when present; otherwise downloads it once into the user cache.
fn local_or_cached_model(filename: &str) -> Result<PathBuf, String> {
    if let Some(path) = find_model(&local_model_dirs(), filename) {
        return Ok(path);
    }
    let project_dirs = ProjectDirs::from("org", "ScreenAutomation", "ScreenAutomation")
        .ok_or_else(|| "could not determine the OCR model cache directory".to_string())?;
    let model_dir = project_dirs.cache_dir().join("ocrs-models");
    std::fs::create_dir_all(&model_dir).map_err(|e| e.to_string())?;
    model_path(
        &model_dir,
        filename,
        &format!("https://ocrs-models.s3-accelerate.amazonaws.com/{filename}"),
    )
}

fn load_ocr_engine() -> Result<OcrEngine, String> {
    let detection_path = local_or_cached_model("text-detection.onnx")?;
    let recognition_path = local_or_cached_model("text-recognition.onnx")?;
    eprintln!(
        "[OCR] Models: {} and {}",
        detection_path.display(),
        recognition_path.display()
    );
    let detection_model = Model::load_file(detection_path).map_err(|e| e.to_string())?;
    let recognition_model = Model::load_file(recognition_path).map_err(|e| e.to_string())?;
    OcrEngine::new(OcrEngineParams {
        detection_model: Some(detection_model),
        recognition_model: Some(recognition_model),
        ..Default::default()
    })
    .map_err(|e| e.to_string())
}

fn crop_rgb(
    rgba: &[u8],
    image_width: u32,
    image_height: u32,
    monitor_x: i32,
    monitor_y: i32,
    region: &Region,
) -> Result<Vec<u8>, String> {
    let x = i64::from(region.x) - i64::from(monitor_x);
    let y = i64::from(region.y) - i64::from(monitor_y);
    if region.width == 0
        || region.height == 0
        || x < 0
        || y < 0
        || x + i64::from(region.width) > i64::from(image_width)
        || y + i64::from(region.height) > i64::from(image_height)
    {
        return Err("Screen region must fit entirely within one monitor".into());
    }
    let expected_len = image_width as usize * image_height as usize * 4;
    if rgba.len() != expected_len {
        return Err("Screen capture has an unexpected pixel buffer size".into());
    }

    let mut out = Vec::with_capacity(region.width as usize * region.height as usize * 3);
    for row in y as usize..y as usize + region.height as usize {
        let start = (row * image_width as usize + x as usize) * 4;
        let end = start + region.width as usize * 4;
        for pixel in rgba[start..end].chunks_exact(4) {
            out.extend_from_slice(&pixel[..3]);
        }
    }
    Ok(out)
}

impl Backend for Desktop {
    fn capture(&mut self, r: &Region) -> Result<Vec<u8>, String> {
        let mon = xcap::Monitor::from_point(r.x, r.y).map_err(|e| e.to_string())?;
        let img = mon.capture_image().map_err(|e| e.to_string())?;
        let out = crop_rgb(
            img.as_raw(),
            img.width(),
            img.height(),
            mon.x().map_err(|e| e.to_string())?,
            mon.y().map_err(|e| e.to_string())?,
            r,
        )?;
        self.width = r.width;
        self.height = r.height;
        Ok(out)
    }

    fn ocr(&mut self, _: &Region, pixels: &[u8]) -> Result<String, String> {
        if self.width == 0 || self.height == 0 {
            return Err("No screen region has been captured for OCR".into());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_model_in_first_folder_that_has_it() {
        let root = std::env::temp_dir().join(format!("sa-models-{}", std::process::id()));
        let (first, second) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join("m.onnx"), b"x").unwrap();
        let dirs = [first.clone(), second.clone()];
        assert_eq!(find_model(&dirs, "m.onnx"), Some(second.join("m.onnx")));
        std::fs::write(first.join("m.onnx"), b"x").unwrap();
        assert_eq!(find_model(&dirs, "m.onnx"), Some(first.join("m.onnx")));
        assert_eq!(find_model(&dirs, "missing.onnx"), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn crops_exact_region_on_offset_monitor() {
        let pixels: Vec<u8> = (0..12).flat_map(|i| [i, i + 20, i + 40, 255]).collect();
        let region = Region {
            x: -99,
            y: 51,
            width: 2,
            height: 2,
        };
        assert_eq!(
            crop_rgb(&pixels, 4, 3, -100, 50, &region).unwrap(),
            vec![5, 25, 45, 6, 26, 46, 9, 29, 49, 10, 30, 50]
        );
    }

    #[test]
    fn rejects_partial_or_invalid_regions() {
        let pixels = vec![0; 4 * 3 * 4];
        for region in [
            Region {
                x: -101,
                y: 50,
                width: 2,
                height: 2,
            },
            Region {
                x: -98,
                y: 52,
                width: 3,
                height: 1,
            },
            Region {
                x: -100,
                y: 50,
                width: 0,
                height: 1,
            },
        ] {
            assert!(crop_rgb(&pixels, 4, 3, -100, 50, &region).is_err());
        }
        assert!(crop_rgb(
            &pixels[..4],
            4,
            3,
            -100,
            50,
            &Region {
                x: -100,
                y: 50,
                width: 1,
                height: 1,
            }
        )
        .is_err());
    }
}
