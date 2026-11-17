use crate::graph::NodeEvent;
use crate::node::Value;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::Path;
#[cfg(feature = "gui")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_WEBHOOK_BODY: usize = 64 * 1024;
const MAX_WEBHOOK_HEADERS: usize = 16 * 1024;
const WEBHOOK_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const FILE_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum EventSource {
    Hotkey { key: String },
    AudioLevel { threshold: f32 },
    Webhook { port: u16, path: String },
    FileChange { path: String },
    Timer { interval_ms: u64 },
}

impl EventSource {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Hotkey { key } => {
                if parse_hotkey(key)?.virtual_key == 0x1B {
                    return Err("Escape is reserved for Stop".into());
                }
            }
            Self::AudioLevel { threshold } => {
                if !threshold.is_finite() || !(0.0..=1.0).contains(threshold) || *threshold == 0.0 {
                    return Err("audio threshold must be greater than 0 and at most 1".into());
                }
            }
            Self::Webhook { port, path } => {
                if *port == 0 {
                    return Err("webhook port must be between 1 and 65535".into());
                }
                if !path.starts_with('/')
                    || path.starts_with("//")
                    || path.contains('?')
                    || path.contains('#')
                    || path.chars().any(|c| c.is_control() || c.is_whitespace())
                {
                    return Err("webhook path must be a slash-prefixed path without query, fragment, or whitespace".into());
                }
            }
            Self::FileChange { path } => {
                if path.trim().is_empty() {
                    return Err("file path must not be empty".into());
                }
                read_text_file(path)?;
            }
            Self::Timer { interval_ms } => {
                if *interval_ms < 100 {
                    return Err("timer interval must be at least 100 ms".into());
                }
            }
        }
        Ok(())
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Hotkey { .. } => "Hotkey",
            Self::AudioLevel { .. } => "Audio Level",
            Self::Webhook { .. } => "Webhook",
            Self::FileChange { .. } => "File Change",
            Self::Timer { .. } => "Timer",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventRegistration {
    pub name: String,
    pub source: EventSource,
}

struct Shared {
    slots: Mutex<Vec<Option<NodeEvent>>>,
    error: Mutex<Option<String>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn emit(&self, index: usize, source: &str, value: Value) {
        let emitted = {
            let mut slots = Self::lock(&self.slots);
            if let Some(slot) = slots.get_mut(index) {
                *slot = Some(NodeEvent {
                    source: source.to_owned(),
                    value,
                });
                true
            } else {
                false
            }
        };
        if emitted {
            (self.wake)();
        }
    }

    fn fail(&self, source: &str, error: impl std::fmt::Display) {
        let mut stored = Self::lock(&self.error);
        if stored.is_none() {
            *stored = Some(format!("{source}: {error}"));
            drop(stored);
            (self.wake)();
        }
    }
}

struct Cancellation {
    stopped: Mutex<bool>,
    changed: Condvar,
}

impl Cancellation {
    fn new() -> Self {
        Self {
            stopped: Mutex::new(false),
            changed: Condvar::new(),
        }
    }

    fn stop(&self) {
        *Shared::lock(&self.stopped) = true;
        self.changed.notify_all();
    }

    fn is_stopped(&self) -> bool {
        *Shared::lock(&self.stopped)
    }

    fn wait(&self, duration: Duration) -> bool {
        let stopped = Shared::lock(&self.stopped);
        if *stopped {
            return true;
        }
        let (stopped, _) = self
            .changed
            .wait_timeout(stopped, duration)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *stopped
    }
}

enum PreparedSource {
    Timer {
        interval: Duration,
    },
    Webhook {
        listener: TcpListener,
        path: String,
    },
    File {
        path: String,
        snapshot: String,
    },
    #[cfg(all(windows, feature = "gui"))]
    Hotkey {
        key: ParsedHotkey,
    },
    #[cfg(feature = "gui")]
    Audio {
        threshold: f32,
    },
}

pub struct EventSources {
    shared: Arc<Shared>,
    cancellation: Arc<Cancellation>,
    workers: Vec<JoinHandle<()>>,
    stopped: bool,
}

impl EventSources {
    pub fn start(
        registrations: Vec<EventRegistration>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let mut names = HashSet::new();
        let mut ports = HashSet::new();
        let mut prepared = Vec::with_capacity(registrations.len());

        for registration in &registrations {
            if registration.name.trim().is_empty() {
                return Err("event source name must not be empty".into());
            }
            if !names.insert(registration.name.clone()) {
                return Err(format!(
                    "duplicate event source name: {}",
                    registration.name
                ));
            }
            registration.source.validate()?;
            let source = match &registration.source {
                EventSource::Timer { interval_ms } => PreparedSource::Timer {
                    interval: Duration::from_millis(*interval_ms),
                },
                EventSource::Webhook { port, path } => {
                    if !ports.insert(*port) {
                        return Err(format!(
                            "webhook port {port} is already assigned; webhooks must use distinct ports"
                        ));
                    }
                    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, *port))
                        .map_err(|error| {
                            format!("cannot bind webhook to 127.0.0.1:{port}: {error}")
                        })?;
                    listener
                        .set_nonblocking(true)
                        .map_err(|error| format!("cannot configure webhook listener: {error}"))?;
                    PreparedSource::Webhook {
                        listener,
                        path: path.clone(),
                    }
                }
                EventSource::FileChange { path } => PreparedSource::File {
                    path: path.clone(),
                    snapshot: read_text_file(path)?,
                },
                EventSource::Hotkey { key } => {
                    let parsed = parse_hotkey(key)?;
                    #[cfg(not(all(windows, feature = "gui")))]
                    {
                        let _ = parsed;
                        return Err("global hotkeys require Windows and the gui feature".into());
                    }
                    #[cfg(all(windows, feature = "gui"))]
                    PreparedSource::Hotkey { key: parsed }
                }
                EventSource::AudioLevel { threshold } => {
                    #[cfg(not(feature = "gui"))]
                    {
                        let _ = threshold;
                        return Err("microphone audio events require the gui feature".into());
                    }
                    #[cfg(feature = "gui")]
                    PreparedSource::Audio {
                        threshold: *threshold,
                    }
                }
            };
            prepared.push((registration.clone(), source));
        }

        let shared = Arc::new(Shared {
            slots: Mutex::new((0..registrations.len()).map(|_| None).collect()),
            error: Mutex::new(None),
            wake,
        });
        let cancellation = Arc::new(Cancellation::new());
        let mut workers = Vec::with_capacity(prepared.len());
        for (index, (registration, source)) in prepared.into_iter().enumerate() {
            let worker_shared = Arc::clone(&shared);
            let worker_cancel = Arc::clone(&cancellation);
            let name = registration.name;
            let result = thread::Builder::new()
                .name(format!("event-{name}"))
                .spawn(move || {
                    run_source(index, name, source, worker_shared, worker_cancel);
                });
            match result {
                Ok(worker) => workers.push(worker),
                Err(error) => {
                    cancellation.stop();
                    for worker in workers {
                        let _ = worker.join();
                    }
                    return Err(format!("cannot start event source worker: {error}"));
                }
            }
        }
        Ok(Self {
            shared,
            cancellation,
            workers,
            stopped: false,
        })
    }

    pub fn take_pending(&self) -> Result<Vec<NodeEvent>, String> {
        if let Some(error) = Shared::lock(&self.shared.error).clone() {
            return Err(error);
        }
        Ok(Shared::lock(&self.shared.slots)
            .iter_mut()
            .filter_map(Option::take)
            .collect())
    }

    pub fn stop(&mut self) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        self.cancellation.stop();
        let mut errors = Vec::new();
        for worker in self.workers.drain(..) {
            if worker.join().is_err() {
                errors.push("an event source worker panicked".to_owned());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            let error = errors.join("; ");
            self.shared.fail("event sources", &error);
            Err(error)
        }
    }
}

impl Drop for EventSources {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn run_source(
    index: usize,
    name: String,
    source: PreparedSource,
    shared: Arc<Shared>,
    cancellation: Arc<Cancellation>,
) {
    match source {
        PreparedSource::Timer { interval } => {
            let Some(mut deadline) = Instant::now().checked_add(interval) else {
                shared.fail(&name, "timer interval exceeds the monotonic clock range");
                return;
            };
            let mut tick = 0i64;
            while !cancellation.is_stopped() {
                let now = Instant::now();
                if now < deadline {
                    if cancellation.wait((deadline - now).min(Duration::from_millis(100))) {
                        break;
                    }
                    continue;
                }
                tick = tick.saturating_add(1);
                shared.emit(index, &name, Value::Int(tick));
                let Some(next_deadline) = deadline.checked_add(interval) else {
                    shared.fail(&name, "timer interval exceeds the monotonic clock range");
                    return;
                };
                deadline = next_deadline;
                if deadline <= Instant::now() {
                    let Some(next_deadline) = Instant::now().checked_add(interval) else {
                        shared.fail(&name, "timer interval exceeds the monotonic clock range");
                        return;
                    };
                    deadline = next_deadline;
                }
            }
        }
        PreparedSource::Webhook { listener, path } => {
            while !cancellation.is_stopped() {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if let Err(error) = stream.set_nonblocking(false) {
                            shared.fail(
                                &name,
                                format!("cannot configure webhook connection: {error}"),
                            );
                            continue;
                        }
                        if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(2))) {
                            shared.fail(&name, format!("cannot set webhook read timeout: {error}"));
                            continue;
                        }
                        if let Err(error) = stream.set_write_timeout(Some(Duration::from_secs(2))) {
                            shared
                                .fail(&name, format!("cannot set webhook write timeout: {error}"));
                            continue;
                        }
                        match handle_webhook(&mut stream, &path) {
                            Ok(Some(body)) => shared.emit(index, &name, Value::Text(body)),
                            Ok(None) => {}
                            Err(error) => shared.fail(&name, error),
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        cancellation.wait(Duration::from_millis(10));
                    }
                    Err(_error) if cancellation.is_stopped() => break,
                    Err(error) => {
                        shared.fail(&name, format!("webhook listener failed: {error}"));
                        cancellation.wait(Duration::from_millis(100));
                    }
                }
            }
        }
        PreparedSource::File { path, mut snapshot } => {
            while !cancellation.wait(FILE_POLL_INTERVAL) {
                match read_text_file(&path) {
                    Ok(contents) if contents != snapshot => {
                        snapshot = contents.clone();
                        shared.emit(index, &name, Value::Text(contents));
                    }
                    Ok(_) => {}
                    Err(error) => shared.fail(&name, error),
                }
            }
        }
        #[cfg(all(windows, feature = "gui"))]
        PreparedSource::Hotkey { key } => run_hotkey(index, &name, key, shared, cancellation),
        #[cfg(feature = "gui")]
        PreparedSource::Audio { threshold } => {
            run_audio(index, &name, threshold, shared, cancellation);
        }
    }
}

fn read_text_file(path: &str) -> Result<String, String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("cannot read file {path}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("file source is not a regular file: {path}"));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!(
            "file source exceeds the {MAX_FILE_BYTES}-byte limit: {path}"
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(Path::new(path))
        .and_then(|file| file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("cannot read file {path}: {error}"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "file source exceeds the {MAX_FILE_BYTES}-byte limit: {path}"
        ));
    }
    String::from_utf8(bytes).map_err(|_| format!("file source must contain UTF-8 text: {path}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ParsedHotkey {
    virtual_key: u16,
    control: bool,
    alt: bool,
    shift: bool,
}

fn parse_hotkey(input: &str) -> Result<ParsedHotkey, String> {
    let mut virtual_key = None;
    let mut control = false;
    let mut alt = false;
    let mut shift = false;
    for raw in input.split('+') {
        let part = raw.trim();
        if part.is_empty() {
            return Err("hotkey contains an empty key or modifier".into());
        }
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => {
                if control {
                    return Err("hotkey repeats the Ctrl modifier".into());
                }
                control = true;
            }
            "alt" => {
                if alt {
                    return Err("hotkey repeats the Alt modifier".into());
                }
                alt = true;
            }
            "shift" => {
                if shift {
                    return Err("hotkey repeats the Shift modifier".into());
                }
                shift = true;
            }
            _ => {
                if virtual_key.is_some() {
                    return Err("hotkey must contain exactly one non-modifier key".into());
                }
                virtual_key = Some(
                    parse_virtual_key(part)
                        .ok_or_else(|| format!("unsupported hotkey key: {part}"))?,
                );
            }
        }
    }
    let virtual_key = virtual_key.ok_or("hotkey must include a key")?;
    Ok(ParsedHotkey {
        virtual_key,
        control,
        alt,
        shift,
    })
}

fn parse_virtual_key(key: &str) -> Option<u16> {
    let upper = key.to_ascii_uppercase();
    if upper.len() == 1 {
        let byte = upper.as_bytes()[0];
        if byte.is_ascii_alphanumeric() {
            return Some(u16::from(byte));
        }
    }
    if let Some(number) = upper.strip_prefix('F') {
        if let Ok(number) = number.parse::<u8>() {
            if (1..=24).contains(&number) {
                return Some(0x70 + u16::from(number - 1));
            }
        }
    }
    match upper.as_str() {
        "ESC" | "ESCAPE" => Some(0x1B),
        "ENTER" | "RETURN" => Some(0x0D),
        "SPACE" => Some(0x20),
        "TAB" => Some(0x09),
        "BACKSPACE" => Some(0x08),
        "DELETE" | "DEL" => Some(0x2E),
        "INSERT" | "INS" => Some(0x2D),
        "HOME" => Some(0x24),
        "END" => Some(0x23),
        "PAGEUP" | "PGUP" => Some(0x21),
        "PAGEDOWN" | "PGDN" => Some(0x22),
        "LEFT" => Some(0x25),
        "UP" => Some(0x26),
        "RIGHT" => Some(0x27),
        "DOWN" => Some(0x28),
        "MINUS" => Some(0xBD),
        "EQUAL" | "EQUALS" => Some(0xBB),
        "LEFTBRACKET" => Some(0xDB),
        "RIGHTBRACKET" => Some(0xDD),
        "BACKSLASH" => Some(0xDC),
        "SEMICOLON" => Some(0xBA),
        "QUOTE" | "APOSTROPHE" => Some(0xDE),
        "COMMA" => Some(0xBC),
        "PERIOD" | "DOT" => Some(0xBE),
        "SLASH" => Some(0xBF),
        "BACKTICK" | "GRAVE" => Some(0xC0),
        _ => None,
    }
}

fn handle_webhook(stream: &mut TcpStream, expected_path: &str) -> Result<Option<String>, String> {
    let deadline = Instant::now() + WEBHOOK_REQUEST_TIMEOUT;
    let mut request = Vec::new();
    let header_end = loop {
        if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        if request.len() >= MAX_WEBHOOK_HEADERS {
            write_response(stream, 431, "Request Header Fields Too Large")?;
            return Ok(None);
        }
        if !set_webhook_read_deadline(stream, deadline)? {
            write_response(stream, 408, "Request Timeout")?;
            return Ok(None);
        }
        let mut buffer = [0u8; 1024];
        match stream.read(&mut buffer) {
            Ok(0) => {
                write_response(stream, 400, "Bad Request")?;
                return Ok(None);
            }
            Ok(count) => request.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                write_response(stream, 408, "Request Timeout")?;
                return Ok(None);
            }
            Err(error) => return Err(format!("webhook request read failed: {error}")),
        }
    };
    if header_end > MAX_WEBHOOK_HEADERS {
        write_response(stream, 431, "Request Header Fields Too Large")?;
        return Ok(None);
    }
    let headers = match std::str::from_utf8(&request[..header_end - 4]) {
        Ok(headers) => headers,
        Err(_) => {
            write_response(stream, 400, "Bad Request")?;
            return Ok(None);
        }
    };
    let mut lines = headers.split("\r\n");
    let request_line = match lines.next().and_then(parse_request_line) {
        Some((method, target, version)) if version == "HTTP/1.1" || version == "HTTP/1.0" => {
            (method, target)
        }
        _ => {
            write_response(stream, 400, "Bad Request")?;
            return Ok(None);
        }
    };
    if request_line.0 != "POST" {
        write_response(stream, 405, "Method Not Allowed")?;
        return Ok(None);
    }
    if request_line.1 != expected_path {
        write_response(stream, 404, "Not Found")?;
        return Ok(None);
    }

    let mut content_length = None;
    let mut transfer_encoding = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            write_response(stream, 400, "Bad Request")?;
            return Ok(None);
        };
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                write_response(stream, 400, "Bad Request")?;
                return Ok(None);
            }
            content_length = value.trim().parse::<usize>().ok();
            if content_length.is_none() {
                write_response(stream, 400, "Bad Request")?;
                return Ok(None);
            }
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            transfer_encoding = true;
        }
    }
    if transfer_encoding {
        write_response(stream, 501, "Transfer Encoding Not Supported")?;
        return Ok(None);
    }
    let Some(content_length) = content_length else {
        write_response(stream, 411, "Length Required")?;
        return Ok(None);
    };
    if content_length > MAX_WEBHOOK_BODY {
        discard_webhook_body(stream, &request, header_end, content_length, deadline)?;
        write_response(stream, 413, "Payload Too Large")?;
        return Ok(None);
    }

    let body_start = header_end;
    while request.len() - body_start < content_length {
        if !set_webhook_read_deadline(stream, deadline)? {
            write_response(stream, 408, "Request Timeout")?;
            return Ok(None);
        }
        let remaining = content_length - (request.len() - body_start);
        let mut buffer = vec![0u8; remaining.min(4096)];
        match stream.read(&mut buffer) {
            Ok(0) => {
                write_response(stream, 400, "Bad Request")?;
                return Ok(None);
            }
            Ok(count) => request.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                write_response(stream, 408, "Request Timeout")?;
                return Ok(None);
            }
            Err(error) => return Err(format!("webhook body read failed: {error}")),
        }
    }
    let body = match String::from_utf8(request[body_start..body_start + content_length].to_vec()) {
        Ok(body) => body,
        Err(_) => {
            write_response(stream, 400, "Bad Request")?;
            return Ok(None);
        }
    };
    write_response(stream, 200, "OK")?;
    Ok(Some(body))
}

fn discard_webhook_body(
    stream: &mut TcpStream,
    request: &[u8],
    body_start: usize,
    content_length: usize,
    deadline: Instant,
) -> Result<bool, String> {
    let mut consumed = request.len().saturating_sub(body_start).min(content_length);
    let mut buffer = [0u8; 4096];
    while consumed < content_length {
        if !set_webhook_read_deadline(stream, deadline)? {
            return Ok(false);
        }
        let remaining = (content_length - consumed).min(buffer.len());
        match stream.read(&mut buffer[..remaining]) {
            Ok(0) => return Ok(false),
            Ok(count) => consumed += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                return Ok(false)
            }
            Err(error) => return Err(format!("webhook rejected body read failed: {error}")),
        }
    }
    Ok(true)
}

fn set_webhook_read_deadline(stream: &TcpStream, deadline: Instant) -> Result<bool, String> {
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        return Ok(false);
    };
    if remaining.is_zero() {
        return Ok(false);
    }
    stream
        .set_read_timeout(Some(remaining))
        .map_err(|error| format!("cannot set webhook request timeout: {error}"))?;
    Ok(true)
}

fn parse_request_line(line: &str) -> Option<(&str, &str, &str)> {
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    let version = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some((method, target, version))
}

fn write_response(stream: &mut TcpStream, status: u16, reason: &str) -> Result<(), String> {
    let body = format!("{reason}\n");
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .map_err(|error| format!("webhook response write failed: {error}"))
}

#[cfg(all(windows, feature = "gui"))]
#[cfg(all(windows, feature = "gui"))]
fn run_hotkey(
    index: usize,
    name: &str,
    key: ParsedHotkey,
    shared: Arc<Shared>,
    cancellation: Arc<Cancellation>,
) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT,
    };
    let mut was_down = false;
    while !cancellation.wait(Duration::from_millis(15)) {
        // SAFETY: GetAsyncKeyState only reads the global state of the selected virtual key.
        let key_down = unsafe { GetAsyncKeyState(i32::from(key.virtual_key)) } as u16 & 0x8000 != 0;
        let control_down = unsafe { GetAsyncKeyState(i32::from(VK_CONTROL)) } as u16 & 0x8000 != 0;
        let alt_down = unsafe { GetAsyncKeyState(i32::from(VK_MENU)) } as u16 & 0x8000 != 0;
        let shift_down = unsafe { GetAsyncKeyState(i32::from(VK_SHIFT)) } as u16 & 0x8000 != 0;
        let down = key_down
            && (!key.control || control_down)
            && (!key.alt || alt_down)
            && (!key.shift || shift_down);
        if down && !was_down {
            shared.emit(index, name, Value::Bool(true));
        }
        was_down = down;
    }
}

#[cfg(feature = "gui")]
fn run_audio(
    index: usize,
    name: &str,
    threshold: f32,
    shared: Arc<Shared>,
    cancellation: Arc<Cancellation>,
) {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let Some(device) = host.default_input_device() else {
        shared.fail(name, "no default microphone input device is available");
        return;
    };
    let supported = match device.default_input_config() {
        Ok(config) => config,
        Err(error) => {
            shared.fail(
                name,
                format!("cannot query microphone input format: {error}"),
            );
            return;
        }
    };
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let stream = match format {
        cpal::SampleFormat::F32 => {
            create_audio_stream::<f32>(&device, &config, index, name, threshold, &shared)
        }
        cpal::SampleFormat::F64 => {
            create_audio_stream::<f64>(&device, &config, index, name, threshold, &shared)
        }
        cpal::SampleFormat::I8 => {
            create_audio_stream::<i8>(&device, &config, index, name, threshold, &shared)
        }
        cpal::SampleFormat::I16 => {
            create_audio_stream::<i16>(&device, &config, index, name, threshold, &shared)
        }
        cpal::SampleFormat::I32 => {
            create_audio_stream::<i32>(&device, &config, index, name, threshold, &shared)
        }
        cpal::SampleFormat::I64 => {
            create_audio_stream::<i64>(&device, &config, index, name, threshold, &shared)
        }
        cpal::SampleFormat::U8 => {
            create_audio_stream::<u8>(&device, &config, index, name, threshold, &shared)
        }
        cpal::SampleFormat::U16 => {
            create_audio_stream::<u16>(&device, &config, index, name, threshold, &shared)
        }
        cpal::SampleFormat::U32 => {
            create_audio_stream::<u32>(&device, &config, index, name, threshold, &shared)
        }
        cpal::SampleFormat::U64 => {
            create_audio_stream::<u64>(&device, &config, index, name, threshold, &shared)
        }
        _ => {
            shared.fail(
                name,
                format!("unsupported microphone sample format: {format:?}"),
            );
            return;
        }
    };
    let stream = match stream {
        Ok(stream) => stream,
        Err(error) => {
            shared.fail(
                name,
                format!("cannot create microphone input stream: {error}"),
            );
            return;
        }
    };
    if let Err(error) = stream.play() {
        shared.fail(
            name,
            format!("cannot start microphone input stream: {error}"),
        );
        return;
    }
    while !cancellation.wait(Duration::from_millis(50)) {}
    drop(stream);
}

#[cfg(feature = "gui")]
trait NormalizedSample: cpal::SizedSample {
    fn normalized(self) -> f32;
}

#[cfg(feature = "gui")]
impl NormalizedSample for f32 {
    fn normalized(self) -> f32 {
        self
    }
}

#[cfg(feature = "gui")]
impl NormalizedSample for f64 {
    fn normalized(self) -> f32 {
        self as f32
    }
}

#[cfg(feature = "gui")]
macro_rules! signed_audio_sample {
    ($($type:ty),* $(,)?) => {$(
        impl NormalizedSample for $type {
            fn normalized(self) -> f32 {
                (self as f64 / ((<$type>::MAX as f64) + 1.0)) as f32
            }
        }
    )*};
}

#[cfg(feature = "gui")]
macro_rules! unsigned_audio_sample {
    ($($type:ty),* $(,)?) => {$(
        impl NormalizedSample for $type {
            fn normalized(self) -> f32 {
                let midpoint = ((<$type>::MAX as f64) + 1.0) / 2.0;
                ((self as f64 - midpoint) / midpoint) as f32
            }
        }
    )*};
}

#[cfg(feature = "gui")]
signed_audio_sample!(i8, i16, i32, i64);
#[cfg(feature = "gui")]
unsigned_audio_sample!(u8, u16, u32, u64);

#[cfg(feature = "gui")]
fn create_audio_stream<T: NormalizedSample>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    index: usize,
    name: &str,
    threshold: f32,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    use cpal::traits::DeviceTrait;

    let armed = Arc::new(AtomicBool::new(true));
    let callback_armed = Arc::clone(&armed);
    let callback_shared = Arc::clone(shared);
    let callback_name = name.to_owned();
    let error_shared = Arc::clone(shared);
    let error_name = name.to_owned();
    device.build_input_stream(
        config,
        move |samples: &[T], _: &cpal::InputCallbackInfo| {
            let rms = if samples.is_empty() {
                0.0
            } else {
                (samples
                    .iter()
                    .map(|sample| f64::from(sample.normalized()).powi(2))
                    .sum::<f64>()
                    / samples.len() as f64)
                    .sqrt() as f32
            };
            if rms < threshold {
                callback_armed.store(true, Ordering::Relaxed);
            } else if callback_armed.swap(false, Ordering::Relaxed) {
                callback_shared.emit(index, &callback_name, Value::Num(f64::from(rms)));
            }
        },
        move |error| error_shared.fail(&error_name, format!("microphone stream failed: {error}")),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn registration(name: &str, source: EventSource) -> EventRegistration {
        EventRegistration {
            name: name.into(),
            source,
        }
    }

    #[test]
    fn validates_sources_and_parses_hotkeys() {
        assert_eq!(
            parse_hotkey("Ctrl+Alt+F24").unwrap(),
            ParsedHotkey {
                virtual_key: 0x87,
                control: true,
                alt: true,
                shift: false
            }
        );
        assert_eq!(
            parse_hotkey("shift+7").unwrap().virtual_key,
            u16::from(b'7')
        );
        assert!(parse_hotkey("Ctrl+").is_err());
        assert!(parse_hotkey("F25").is_err());
        assert!(parse_hotkey("A+B").is_err());
        let escape_combo = EventSource::Hotkey {
            key: "Ctrl+Escape".into(),
        };
        assert_eq!(
            escape_combo.validate().unwrap_err(),
            "Escape is reserved for Stop"
        );
        assert!(EventSource::Timer { interval_ms: 99 }.validate().is_err());
        assert!(EventSource::AudioLevel {
            threshold: f32::NAN
        }
        .validate()
        .is_err());
        assert!(EventSource::Webhook {
            port: 0,
            path: "/hook".into()
        }
        .validate()
        .is_err());
        assert_eq!(EventSource::Timer { interval_ms: 100 }.label(), "Timer");
        let timer = EventSource::Timer { interval_ms: 250 };
        let encoded = serde_json::to_string(&timer).unwrap();
        assert!(encoded.contains("\"source\":\"timer\""));
        assert_eq!(
            serde_json::from_str::<EventSource>(&encoded).unwrap(),
            timer
        );
    }

    #[test]
    fn timer_delivers_latest_tick_and_stops_cleanly() {
        let wake_count = Arc::new(AtomicUsize::new(0));
        let callback_count = Arc::clone(&wake_count);
        let mut sources = EventSources::start(
            vec![registration(
                "timer",
                EventSource::Timer { interval_ms: 100 },
            )],
            Arc::new(move || {
                callback_count.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(360));
        let events = sources.take_pending().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].value, Value::Int(tick) if tick >= 2));
        assert!(wake_count.load(Ordering::Relaxed) >= 2);
        let start = Instant::now();
        sources.stop().unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(sources.take_pending().unwrap().is_empty());
    }

    fn reserve_local_port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn post(port: u16, path: &str, body: &[u8], extra_headers: &str) -> String {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        write!(
            stream,
            "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n{extra_headers}\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn webhook_returns_status_and_delivers_only_valid_utf8_body() {
        let port = reserve_local_port();
        let mut sources = EventSources::start(
            vec![registration(
                "hook",
                EventSource::Webhook {
                    port,
                    path: "/in".into(),
                },
            )],
            Arc::new(|| {}),
        )
        .unwrap();
        let response = post(port, "/in", b"hello", "");
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        let events = sources.take_pending().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source, "hook");
        assert_eq!(events[0].value, Value::Text("hello".into()));

        let response = post(port, "/in", &[0xff], "");
        assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
        assert!(sources.take_pending().unwrap().is_empty());

        let response = post(port, "/in", &[], "Transfer-Encoding: chunked\r\n");
        assert!(response.starts_with("HTTP/1.1 501 Transfer Encoding Not Supported"));

        let response = post(port, "/in", &[], "Content-Length: 0\r\n");
        assert!(response.starts_with("HTTP/1.1 400 Bad Request"));

        let oversized = vec![b'x'; MAX_WEBHOOK_BODY + 1];
        let response = post(port, "/in", &oversized, "");
        assert!(response.starts_with("HTTP/1.1 413 Payload Too Large"));
        assert!(sources.take_pending().unwrap().is_empty());
        sources.stop().unwrap();
    }

    #[test]
    fn file_change_emits_updated_utf8_contents_and_cleans_up() {
        struct TestFile(String);
        impl Drop for TestFile {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let path = format!(
            ".event-source-test-{}-{}.txt",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::fs::write(&path, "initial").unwrap();
        let _cleanup = TestFile(path.clone());
        let wake_count = Arc::new(AtomicUsize::new(0));
        let callback_count = Arc::clone(&wake_count);
        let result = (|| {
            let mut sources = EventSources::start(
                vec![registration(
                    "file",
                    EventSource::FileChange { path: path.clone() },
                )],
                Arc::new(move || {
                    callback_count.fetch_add(1, Ordering::Relaxed);
                }),
            )
            .unwrap();
            assert!(sources.take_pending().unwrap().is_empty());
            std::fs::write(&path, "updated").unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            let event = loop {
                let mut events = sources.take_pending().unwrap();
                if !events.is_empty() {
                    break events.remove(0);
                }
                assert!(Instant::now() < deadline, "file change was not observed");
                thread::sleep(Duration::from_millis(20));
            };
            assert_eq!(event.value, Value::Text("updated".into()));
            std::fs::remove_file(&path).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if sources.take_pending().is_err() {
                    break;
                }
                assert!(Instant::now() < deadline, "file error was not surfaced");
                thread::sleep(Duration::from_millis(20));
            }
            assert!(
                sources.take_pending().is_err(),
                "source errors must remain sticky"
            );
            assert!(wake_count.load(Ordering::Relaxed) >= 2);
            sources.stop().unwrap();
        })();
        result
    }

    #[test]
    fn webhook_rejects_shared_ports_explicitly() {
        let port = reserve_local_port();
        let result = EventSources::start(
            vec![
                registration(
                    "first",
                    EventSource::Webhook {
                        port,
                        path: "/first".into(),
                    },
                ),
                registration(
                    "second",
                    EventSource::Webhook {
                        port,
                        path: "/second".into(),
                    },
                ),
            ],
            Arc::new(|| {}),
        );
        match result {
            Err(error) => assert!(error.contains("distinct ports")),
            Ok(mut sources) => {
                sources.stop().unwrap();
                panic!("webhook port reuse must be rejected");
            }
        }
    }

    #[test]
    fn webhook_partial_body_has_a_total_read_deadline() {
        let port = reserve_local_port();
        let mut sources = EventSources::start(
            vec![registration(
                "slow-hook",
                EventSource::Webhook {
                    port,
                    path: "/in".into(),
                },
            )],
            Arc::new(|| {}),
        )
        .unwrap();
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        let start = Instant::now();
        stream
            .write_all(b"POST /in HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\na")
            .unwrap();
        let mut trickle = stream.try_clone().unwrap();
        let writer = thread::spawn(move || {
            for _ in 0..4 {
                thread::sleep(Duration::from_millis(600));
                if trickle.write_all(b"a").is_err() {
                    break;
                }
            }
        });
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        writer.join().unwrap();
        assert!(response.starts_with("HTTP/1.1 408 Request Timeout"));
        assert!(start.elapsed() >= Duration::from_millis(1_500));
        assert!(start.elapsed() < Duration::from_secs(4));
        sources.stop().unwrap();
    }
}
