//! Real backends: xcap (capture), OCRS (OCR), enigo (input).
use crate::backend::{Backend, KeyAction, Region};
use directories::ProjectDirs;
use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use rten::Model;
use rten_imageproc::{RectF, RotatedRect};
use std::path::{Path, PathBuf};

pub struct Desktop {
    enigo: Enigo,
    ocr: Option<OcrEngine>,
}

impl Desktop {
    pub fn new() -> Result<Desktop, String> {
        Ok(Desktop {
            enigo: Enigo::new(&Settings::default()).map_err(|e| e.to_string())?,
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
    log::info!(
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

fn recognize_single_line(
    engine: &OcrEngine,
    region: &Region,
    pixels: &[u8],
) -> Result<String, String> {
    if region.width == 0 || region.height == 0 {
        return Err("Single-line OCR requires a non-empty region".into());
    }
    let (pixels, width, height) = prepare_single_line(pixels, region.width, region.height)?;
    if pixels.is_empty() {
        return Ok(String::new());
    }
    let text = recognize_line(engine, &pixels, width, height)?;
    let numeric = text.chars().filter(char::is_ascii_digit).count() >= 2
        && text.matches('.').count() <= 1
        && text.chars().all(|c| c.is_ascii_digit() || c.is_ascii_whitespace()
            || matches!(c, '$' | '+' | '-' | '.' | '%'));
    if height >= 24 || !numeric {
        return Ok(text);
    }
    // Preserve tiny punctuation before the recognizer's interpolated resize.
    let mut enlarged = Vec::with_capacity(pixels.len() * 4);
    for y in 0..height * 2 {
        for x in 0..width * 2 {
            let start = ((y / 2 * width + x / 2) * 3) as usize;
            enlarged.extend_from_slice(&pixels[start..start + 3]);
        }
    }
    let enhanced = recognize_line(engine, &enlarged, width * 2, height * 2)?;
    validate_numeric_readings(&text, &enhanced)?;
    Ok(enhanced)
}

fn validate_numeric_readings(original: &str, enhanced: &str) -> Result<(), String> {
    let original: String = original.split_whitespace().collect();
    let enhanced: String = enhanced.split_whitespace().collect();
    let without_dots = |text: &str| text.chars().filter(|c| *c != '.').collect::<String>();
    if without_dots(&original) != without_dots(&enhanced)
        || (original.contains('.') && original != enhanced)
        || enhanced.matches('.').count() > 1
    {
        return Err(format!("Numeric OCR disagrees between scales: {original:?} versus {enhanced:?}"));
    }
    Ok(())
}

fn recognize_line(engine: &OcrEngine, pixels: &[u8], width: u32, height: u32) -> Result<String, String> {
    let image = ImageSource::from_bytes(pixels, (width, height))
        .map_err(|e| e.to_string())?;
    let input = engine.prepare_input(image).map_err(|e| e.to_string())?;
    // Keep punctuation inside the recognition crop instead of relying on word detection.
    let line = RotatedRect::from_rect(RectF::from_tlhw(0.0, 0.0, height as f32, width as f32));
    let lines = engine
        .recognize_text(&input, &[vec![line]])
        .map_err(|e| e.to_string())?;
    Ok(lines
        .into_iter()
        .flatten()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join(""))
}

fn prepare_single_line(pixels: &[u8], width: u32, height: u32) -> Result<(Vec<u8>, u32, u32), String> {
    if width == 0 || height == 0 {
        return Err("Single-line OCR requires a non-empty region".into());
    }
    let expected_len = (width as usize).checked_mul(height as usize)
        .and_then(|len| len.checked_mul(3))
        .ok_or_else(|| "Single-line OCR dimensions are too large".to_string())?;
    if pixels.len() != expected_len {
        return Err("Single-line OCR requires an RGB pixel buffer matching its region".into());
    }
    // Single-line regions have a plain background and a small margin around the text.
    let background = &pixels[..3];
    let invert = background.iter().map(|&v| u32::from(v)).sum::<u32>() < 3 * 128;
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    for (index, pixel) in pixels.chunks_exact(3).enumerate() {
        if pixel.iter().zip(background).any(|(&a, &b)| a.abs_diff(b) > 32) {
            let x = index as u32 % width;
            let y = index as u32 / width;
            left = left.min(x);
            right = right.max(x + 1);
            top = top.min(y);
            bottom = bottom.max(y + 1);
        }
    }
    if right == 0 {
        return Ok((Vec::new(), 0, 0));
    }
    left = left.saturating_sub(2);
    top = top.saturating_sub(2);
    right = (right + 2).min(width);
    bottom = (bottom + 2).min(height);
    let mut out = Vec::with_capacity(((right - left) * (bottom - top) * 3) as usize);
    for y in top..bottom {
        let start = ((y * width + left) * 3) as usize;
        let end = ((y * width + right) * 3) as usize;
        out.extend(pixels[start..end].iter().map(|&v| if invert { 255 - v } else { v }));
    }
    let (width, height) = (right - left, bottom - top);
    Ok((out, width, height))
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
        Ok(out)
    }

    fn ocr(&mut self, region: &Region, pixels: &[u8]) -> Result<String, String> {
        if region.width == 0 || region.height == 0 {
            return Err("OCR requires a non-empty screen region".into());
        }

        let image = ImageSource::from_bytes(pixels, (region.width, region.height))
            .map_err(|e| e.to_string())?;
        let engine = self.ocr_engine()?;
        let input = engine.prepare_input(image).map_err(|e| e.to_string())?;
        engine.get_text(&input).map_err(|e| e.to_string())
    }

    fn ocr_single_line(&mut self, region: &Region, pixels: &[u8]) -> Result<String, String> {
        recognize_single_line(self.ocr_engine()?, region, pixels)
    }

    fn type_text(&mut self, text: &str) -> Result<(), String> {
        self.enigo.text(text).map_err(|e| e.to_string())
    }

    fn press_key(&mut self, key: KeyAction) -> Result<(), String> {
        if key == KeyAction::SelectAll {
            let modifier = if cfg!(target_os = "macos") { Key::Meta } else { Key::Control };
            self.enigo.key(modifier, Direction::Press).map_err(|e| e.to_string())?;
            let selected = self.enigo.key(Key::Unicode('a'), Direction::Click);
            let released = self.enigo.key(modifier, Direction::Release);
            if let Err(error) = &released {
                log::error!("Could not release select-all modifier: {error}");
            }
            return selected.and(released).map_err(|e| e.to_string());
        }
        let key = match key {
            KeyAction::Tab => Key::Tab,
            KeyAction::Enter => Key::Return,
            KeyAction::Escape => Key::Escape,
            KeyAction::ArrowUp => Key::UpArrow,
            KeyAction::ArrowDown => Key::DownArrow,
            KeyAction::Backspace => Key::Backspace,
            KeyAction::SelectAll => unreachable!(),
        };
        self.enigo.key(key, Direction::Click).map_err(|e| e.to_string())
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
    #[ignore = "requires a local OCR recognition model"]
    fn small_price_ocr_preserves_decimals_and_text() {
        // Offline 100x28 RGB crops of Segoe UI Semibold at 10.5 and 11.725 pixels.
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests").join("fixtures").join("ocr");
        let model = find_model(&local_model_dirs(), "text-recognition.onnx")
            .expect("this test requires a local recognition model");
        let engine = OcrEngine::new(OcrEngineParams {
            recognition_model: Some(Model::load_file(model).unwrap()),
            ..Default::default()
        }).unwrap();
        for (file, expected) in [
            ("price-dark.rgb", "$164.21"), ("price-light.rgb", "$164.21"),
            ("tiny-price.rgb", "$164.21"), ("sell-text.rgb", "Sell 1 NVDA"), ("zero.rgb", "0"),
        ] {
            let pixels = std::fs::read(directory.join(file)).unwrap();
            let region = Region { x: 0, y: 0, width: 100, height: 28 };
            let text = recognize_single_line(&engine, &region, &pixels).unwrap();
            let actual = if expected.starts_with('$') {
                text.split_whitespace().collect::<String>()
            } else {
                text.trim().to_owned()
            };
            assert_eq!(actual, expected, "{file}");
        }
    }

    #[test]
    fn numeric_enhancement_rejects_changed_digits_signs_or_existing_decimals() {
        assert!(validate_numeric_readings("$16421", "$ 164.21").is_ok());
        for (original, enhanced) in [
            ("$16421", "$164.22"), ("-16421", "164.21"),
            ("$164.21", "$16.421"), ("$164.21", "$16421"), ("$16421", "$$164.21"),
        ] {
            let error = validate_numeric_readings(original, enhanced).unwrap_err();
            assert!(error.contains("Numeric OCR disagrees between scales"));
        }
    }

    #[test]
    fn single_line_preparation_keeps_isolated_decimal_and_normalizes_polarity() {
        let mut dark = vec![0; 10 * 8 * 3];
        for (x, y) in [(3, 3), (6, 3), (6, 5)] {
            dark[(y * 10 + x) * 3..(y * 10 + x) * 3 + 3].fill(255);
        }
        let light: Vec<u8> = dark.iter().map(|&v| 255 - v).collect();
        let (prepared, width, height) = prepare_single_line(&dark, 10, 8).unwrap();
        assert_eq!((width, height), (8, 7));
        let dot = (4 * width as usize + 5) * 3;
        assert_eq!(&prepared[dot..dot + 3], &[0, 0, 0]);
        assert_eq!(
            prepare_single_line(&light, 10, 8).unwrap(),
            (prepared, width, height)
        );
        assert_eq!(prepare_single_line(&[255; 12], 2, 2).unwrap(), (vec![], 0, 0));
        assert!(prepare_single_line(&[0; 2], 2, 2).is_err());
        assert!(prepare_single_line(&[], 0, 0).is_err());
        assert!(prepare_single_line(&[0], 1, 1).is_err());
        assert!(prepare_single_line(&[0; 4], 1, 1).is_err());
    }

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
