use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};
use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Instant;

const MAX_ENTRIES: usize = 5000;

#[derive(Clone)]
pub struct LogEntry {
    pub level: Level,
    pub text: Arc<str>,
}

#[derive(Default)]
struct Buffer {
    entries: VecDeque<LogEntry>,
    dropped: usize,
}

#[derive(Clone)]
pub struct SessionLog {
    started: Instant,
    buffer: Arc<Mutex<Buffer>>,
}

impl Default for SessionLog {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            buffer: Arc::new(Mutex::new(Buffer::default())),
        }
    }
}

impl SessionLog {
    pub fn install(&self) -> Result<(), SetLoggerError> {
        log::set_boxed_logger(Box::new(self.clone()))?;
        log::set_max_level(LevelFilter::Info);
        Ok(())
    }

    pub fn record(&self, level: Level, message: fmt::Arguments<'_>) {
        let message = message.to_string();
        eprintln!("{message}");
        let mut buffer = self.buffer.lock().expect("Session log lock poisoned");
        let elapsed = self.started.elapsed();
        let seconds = elapsed.as_secs();
        let text = format!(
            "[{:02}:{:02}:{:02}.{:03}] {level:5} {}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60,
            elapsed.subsec_millis(),
            message.replace('\r', "\\r").replace('\n', "\\n"),
        );
        if buffer.entries.len() == MAX_ENTRIES {
            buffer.entries.pop_front();
            buffer.dropped += 1;
        }
        buffer.entries.push_back(LogEntry {
            level,
            text: text.into(),
        });
    }

    pub fn snapshot(&self) -> (Vec<LogEntry>, usize) {
        let buffer = self.buffer.lock().expect("Session log lock poisoned");
        (buffer.entries.iter().cloned().collect(), buffer.dropped)
    }
}

impl Log for SessionLog {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= Level::Info
            && (metadata.target() == "screen_automation"
                || metadata.target().starts_with("screen_automation::"))
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            self.record(record.level(), *record.args());
        }
    }

    fn flush(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_app_records_with_level_and_readable_multiline_ocr() {
        let logs = SessionLog::default();
        logs.log(
            &Record::builder()
                .target("screen_automation::node")
                .level(Level::Warn)
                .args(format_args!("[OCR] '1822.91\n04'"))
                .build(),
        );
        logs.log(
            &Record::builder()
                .target("other_crate")
                .args(format_args!("ignored"))
                .build(),
        );
        let (entries, dropped) = logs.snapshot();
        assert_eq!(entries.len(), 1);
        assert_eq!(dropped, 0);
        assert_eq!(entries[0].level, Level::Warn);
        assert!(entries[0].text.contains("[OCR] '1822.91\\n04'"));
        assert!(!entries[0].text.contains('\n'));
    }

    #[test]
    fn captures_real_workflow_ocr_threshold_and_click_records() {
        use crate::backend::{Backend, Region};
        use crate::graph::Graph;
        use crate::node::{Crossing, Kind, Node};

        struct Fake {
            text: &'static str,
            clicks: usize,
        }
        impl Backend for Fake {
            fn capture(&mut self, _: &Region) -> Result<Vec<u8>, String> {
                Ok(self.text.as_bytes().to_vec())
            }
            fn ocr(&mut self, _: &Region, _: &[u8]) -> Result<String, String> {
                Ok(self.text.into())
            }
            fn type_text(&mut self, _: &str) -> Result<(), String> {
                Err("Unexpected keyboard action".into())
            }
            fn click(&mut self, _: i32, _: i32, _: &str) -> Result<(), String> {
                self.clicks += 1;
                Ok(())
            }
        }
        let logs = SessionLog::default();
        logs.install().unwrap();
        let mut graph = Graph::default();
        graph
            .add_node(Node::new(
                "source",
                Kind::Ocr {
                    single_line: false,
                    last_hash: None,
                },
            ))
            .unwrap();
        graph
            .add_node(Node::new(
                "threshold",
                Kind::Threshold {
                    upper: 100.0,
                    lower: 90.0,
                    trigger: Crossing::RisingOrEqual,
                    band: None,
                },
            ))
            .unwrap();
        graph
            .add_node(Node::new(
                "action",
                Kind::Click {
                    button: "left".into(),
                },
            ))
            .unwrap();
        graph.connect("source", "threshold").unwrap();
        graph.connect("threshold", "action").unwrap();
        let mut backend = Fake {
            text: "99.5",
            clicks: 0,
        };
        graph.run_once(&mut backend).unwrap();
        backend.text = "100.0";
        graph.run_once(&mut backend).unwrap();
        assert_eq!(backend.clicks, 1);
        let (entries, _) = logs.snapshot();
        for expected in [
            "[OCR] Recognition result: '100.0'",
            "Rising trigger",
            "[CLICK] Clicking at",
        ] {
            assert!(
                entries.iter().any(|entry| entry.text.contains(expected)),
                "missing {expected}"
            );
        }
    }

    #[test]
    fn retains_recent_entries_and_reports_evictions() {
        let logs = SessionLog::default();
        for index in 0..=MAX_ENTRIES {
            logs.record(Level::Info, format_args!("event {index}"));
        }
        let (entries, dropped) = logs.snapshot();
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(dropped, 1);
        assert!(entries.first().unwrap().text.ends_with("event 1"));
        assert!(entries
            .last()
            .unwrap()
            .text
            .ends_with(&format!("event {MAX_ENTRIES}")));
    }
}
