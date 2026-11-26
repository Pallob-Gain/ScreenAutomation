use crate::backend::{Backend, KeyAction, Region};
use crate::graph::Graph;
use rhai::{Dynamic, Engine, Scope};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

/// The scripting language of a Function node. Both languages share the same interface:
/// `inputs`, named input variables, `state`, `workflow_busy`, and `elapsed_ms`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptLanguage {
    #[default]
    Rhai,
    Javascript,
}

impl ScriptLanguage {
    pub const ALL: [Self; 2] = [Self::Rhai, Self::Javascript];

    pub fn label(self) -> &'static str {
        match self {
            Self::Rhai => "Rhai",
            Self::Javascript => "JavaScript",
        }
    }

    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextMatch {
    Exact,
    #[default]
    Contains,
    Regex,
}

impl TextMatch {
    pub const ALL: [Self; 3] = [Self::Exact, Self::Contains, Self::Regex];

    pub fn label(self) -> &'static str {
        match self {
            Self::Exact => "Exact text",
            Self::Contains => "Contains text",
            Self::Regex => "Regular expression",
        }
    }

    pub fn validate(self, expected: &str) -> Result<(), String> {
        if expected.trim().is_empty() {
            return Err("expected text must not be empty".into());
        }
        if self == Self::Regex {
            regex::Regex::new(expected).map_err(|error| format!("invalid text regex: {error}"))?;
        }
        Ok(())
    }

    fn matches(self, actual: &str, expected: &str, case_sensitive: bool) -> Result<bool, String> {
        self.validate(expected)?;
        let actual = actual.split_whitespace().collect::<Vec<_>>().join(" ");
        if self == Self::Regex {
            return regex::RegexBuilder::new(expected).case_insensitive(!case_sensitive)
                .build().map(|pattern| pattern.is_match(&actual)).map_err(|error| error.to_string());
        }
        let expected = expected.split_whitespace().collect::<Vec<_>>().join(" ");
        let (actual, expected) = if case_sensitive { (actual, expected) }
            else { (actual.to_lowercase(), expected.to_lowercase()) };
        Ok(if self == Self::Exact { actual == expected } else { actual.contains(&expected) })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PendingWait {
    input: Value,
    started: Instant,
    next_poll: Instant,
}

fn default_timeout_ms() -> u64 { 10_000 }
fn default_poll_ms() -> u64 { 100 }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldVerification {
    #[serde(default)]
    pub numeric: bool,
    #[serde(default = "default_single_line")]
    pub single_line: bool,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_poll_ms")]
    pub poll_ms: u64,
}

fn default_single_line() -> bool { true }

impl Default for FieldVerification {
    fn default() -> Self {
        Self { numeric: false, single_line: true, timeout_ms: default_timeout_ms(), poll_ms: default_poll_ms() }
    }
}

impl FieldVerification {
    pub(crate) fn validate(&self, expected: &str) -> Result<(), String> {
        validate_wait_timing(self.timeout_ms, self.poll_ms)?;
        if self.numeric && !expected.is_empty() && finite_number(expected).is_none() {
            return Err("numeric field verification requires a finite numeric replacement".into());
        }
        Ok(())
    }

    fn matches(&self, actual: &str, expected: &str) -> bool {
        if self.numeric {
            matches!((finite_number(actual), finite_number(expected)), (Some(a), Some(b)) if a == b)
        } else {
            actual.trim() == expected.trim()
        }
    }
}

fn finite_number(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|value| value.is_finite())
}

pub(crate) fn validate_wait_timing(timeout_ms: u64, poll_ms: u64) -> Result<(), String> {
    if !(1..=120_000).contains(&timeout_ms) || poll_ms == 0 || poll_ms > timeout_ms {
        return Err("wait requires timeout 1..120000 ms and poll interval 1..timeout ms".into());
    }
    Ok(())
}

fn poll_screen_text(
    backend: &mut dyn Backend, region: &Region, name: &str, expected: &str,
    single_line: bool, timeout_ms: u64, poll_ms: u64, pending: &mut Option<PendingWait>,
    matches: impl FnOnce(&str) -> Result<bool, String>,
) -> Result<Option<Value>, String> {
    let now = backend.now();
    let state = pending.take().expect("wait initialized");
    let timeout = || format!("timed out after {timeout_ms} ms waiting for {expected:?}; check screen/order history before retrying");
    if now.duration_since(state.started) >= Duration::from_millis(timeout_ms) {
        return Err(timeout());
    }
    if now < state.next_poll {
        *pending = Some(state);
        return Ok(None);
    }
    let reading = (|| {
        let pixels = backend.capture(region)?;
        if single_line { backend.ocr_single_line(region, &pixels) }
            else { backend.ocr(region, &pixels) }
    })();
    let actual = match reading {
        Ok(actual) => actual,
        Err(error) => {
            *pending = Some(state);
            return Err(error);
        }
    };
    if backend.now().duration_since(state.started) >= Duration::from_millis(timeout_ms) {
        return Err(timeout());
    }
    if matches(&actual)? {
        log::info!("[WAIT] '{name}' matched {expected:?}");
        Ok(Some(state.input))
    } else {
        log::debug!("[WAIT] '{name}' read {actual:?}, expected {expected:?}");
        *pending = Some(PendingWait { next_poll: now + Duration::from_millis(poll_ms), ..state });
        Ok(None)
    }
}

/// Value flowing between nodes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Num(f64),
    Text(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputParameter {
    CompareValue,
    Lower,
    Upper,
    Minimum,
    Maximum,
    MaxChange,
}

impl InputParameter {
    pub fn label(self) -> &'static str {
        match self {
            Self::CompareValue => "Value",
            Self::Lower => "Lower",
            Self::Upper => "Upper",
            Self::Minimum => "Minimum",
            Self::Maximum => "Maximum",
            Self::MaxChange => "Max change",
        }
    }
}

impl Value {
    pub fn to_text(&self) -> String {
        match self {
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Num(n) => n.to_string(),
            Value::Text(s) => s.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Text,
    Number,
    Int,
    Bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompareOp {
    #[serde(rename = ">")]
    Greater,
    #[serde(rename = ">=")]
    GreaterOrEqual,
    #[serde(rename = "==")]
    Equal,
    #[serde(rename = "!=")]
    NotEqual,
    #[serde(rename = "<=")]
    LessOrEqual,
    #[serde(rename = "<")]
    Less,
}

impl CompareOp {
    pub const ALL: [CompareOp; 6] = [
        CompareOp::Greater,
        CompareOp::GreaterOrEqual,
        CompareOp::Equal,
        CompareOp::NotEqual,
        CompareOp::LessOrEqual,
        CompareOp::Less,
    ];

    pub fn symbol(self) -> &'static str {
        match self {
            CompareOp::Greater => ">",
            CompareOp::GreaterOrEqual => "≥",
            CompareOp::Equal => "=",
            CompareOp::NotEqual => "≠",
            CompareOp::LessOrEqual => "≤",
            CompareOp::Less => "<",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CompareOp::Greater => "Greater than",
            CompareOp::GreaterOrEqual => "Greater than or equal to",
            CompareOp::Equal => "Equal to",
            CompareOp::NotEqual => "Not equal to",
            CompareOp::LessOrEqual => "Less than or equal to",
            CompareOp::Less => "Less than",
        }
    }

    /// Numbers are compared numerically (extracted from text, e.g. OCR output); otherwise
    /// trimmed text is compared, where only `=` and `≠` can match. `None` means not comparable.
    pub fn test(self, input: &Value, target: &str) -> Option<bool> {
        let parse = |s: &str| number_in(s).and_then(|n| n.parse::<f64>().ok());
        let ordering = match (input, parse(target)) {
            (Value::Int(i), Some(t)) => (*i as f64).partial_cmp(&t)?,
            (Value::Num(n), Some(t)) => n.partial_cmp(&t)?,
            (Value::Text(s), Some(t)) => parse(s)?.partial_cmp(&t)?,
            _ => {
                let equal = input.to_text().trim() == target.trim();
                return match self {
                    CompareOp::Equal => Some(equal),
                    CompareOp::NotEqual => Some(!equal),
                    _ => None,
                };
            }
        };
        use std::cmp::Ordering::*;
        Some(match self {
            CompareOp::Greater => ordering == Greater,
            CompareOp::GreaterOrEqual => ordering != Less,
            CompareOp::Equal => ordering == Equal,
            CompareOp::NotEqual => ordering != Equal,
            CompareOp::LessOrEqual => ordering != Greater,
            CompareOp::Less => ordering == Less,
        })
    }
}

/// Which threshold crossings make a threshold or hysteresis node trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Crossing {
    Rising,
    RisingOrEqual,
    Falling,
    FallingOrEqual,
    Both,
    EnteringRange,
    EnteringRangeInclusive,
}

impl Crossing {
    pub const ALL: [Crossing; 7] = [
        Crossing::Rising,
        Crossing::RisingOrEqual,
        Crossing::Falling,
        Crossing::FallingOrEqual,
        Crossing::Both,
        Crossing::EnteringRange,
        Crossing::EnteringRangeInclusive,
    ];

    pub fn is_range(self) -> bool {
        matches!(self, Self::EnteringRange | Self::EnteringRangeInclusive)
    }

    pub fn label(self) -> &'static str {
        match self {
            Crossing::Rising => "Rises above upper",
            Crossing::RisingOrEqual => "Rises to or above upper (>=)",
            Crossing::Falling => "Falls below lower",
            Crossing::FallingOrEqual => "Falls to or below lower (<=)",
            Crossing::Both => "Rises above upper or falls below lower",
            Crossing::EnteringRange => "Enters range (lower < value < upper)",
            Crossing::EnteringRangeInclusive => "Enters range (lower <= value <= upper)",
        }
    }
}

/// Last band a threshold or hysteresis node saw the input in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    Below,
    Between,
    Above,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Kind {
    /// Reads its region when the pixels change.
    Ocr {
        #[serde(default)]
        single_line: bool,
        #[serde(skip)]
        last_hash: Option<u64>,
    },
    /// Converts a value to a format.
    Model {
        format: Format,
    },
    Constant {
        value: String,
        format: Format,
    },
    /// Rhai or JavaScript script: `inputs` array in scope; the last expression is the result.
    /// `()` (Rhai) or `undefined`/`null` (JavaScript) stops the flow. Optional named inputs add
    /// one single-connection pin per name, bound as a script variable.
    Function {
        code: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        input_names: Vec<String>,
        #[serde(default, skip_serializing_if = "ScriptLanguage::is_default")]
        language: ScriptLanguage,
    },
    /// Passes the input on when `input <op> value` holds; otherwise stops the flow.
    Compare {
        op: CompareOp,
        value: String,
        /// Only trigger when the condition changes from false to true.
        #[serde(default = "default_true")]
        on_change: bool,
        #[serde(skip)]
        last: Option<bool>,
    },
    #[serde(rename = "compare_inputs")]
    CompareInputs {
        op: CompareOp,
        #[serde(default = "default_true")]
        trigger_when: bool,
        #[serde(default = "default_true")]
        on_change: bool,
        #[serde(skip)]
        last: Option<bool>,
        #[serde(skip)]
        values: [Option<Value>; 2],
    },
    /// Hysteresis trigger: passes the input on when it crosses above `upper` or below `lower`.
    /// After crossing one threshold it must cross the other before that direction can trigger
    /// again, so noise around a single threshold does not trigger repeatedly.
    #[serde(alias = "threshold")]
    Hysteresis {
        upper: f64,
        lower: f64,
        trigger: Crossing,
        #[serde(skip)]
        band: Option<Band>,
    },
    /// Triggers on the first qualifying reading and each new crossing of either boundary.
    /// The distinct JSON tag preserves the meaning of legacy `type: threshold` nodes.
    #[serde(rename = "threshold_crossing")]
    Threshold {
        upper: f64,
        lower: f64,
        trigger: Crossing,
        #[serde(skip)]
        band: Option<Band>,
    },
    /// Clicks the region then types `text` (or the input if empty).
    Keyboard {
        text: String,
    },
    Click {
        button: String,
    },
    #[serde(rename = "wait_for_text")]
    WaitForText {
        expected: String,
        #[serde(default)]
        matching: TextMatch,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        single_line: bool,
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
        #[serde(default = "default_poll_ms")]
        poll_ms: u64,
        #[serde(skip)]
        pending: Option<PendingWait>,
    },
    #[serde(rename = "assert_text")]
    AssertText {
        expected: String,
        #[serde(default)]
        matching: TextMatch,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        single_line: bool,
    },
    Once {
        #[serde(default)]
        armed: bool,
        #[serde(skip)]
        fired: bool,
    },
    #[serde(rename = "set_field")]
    SetField {
        #[serde(default)]
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verification: Option<FieldVerification>,
        #[serde(skip)]
        pending: Option<PendingWait>,
    },
    Keypress {
        key: KeyAction,
    },
    #[serde(rename = "validate_number")]
    ValidateNumber {
        minimum: Option<f64>,
        maximum: Option<f64>,
        max_change: Option<f64>,
        #[serde(default)]
        pattern: String,
        #[serde(skip)]
        last: Option<f64>,
    },
    Event {
        event: crate::event_sources::EventSource,
    },
    Note {
        text: String,
    },
    Subflow {
        graph: Box<Graph>,
    },
    #[serde(rename = "subflow_input")]
    SubflowInput,
    #[serde(rename = "subflow_output")]
    SubflowOutput,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    #[serde(flatten)]
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_parameters: Vec<InputParameter>,
    #[serde(skip)]
    parameter_values: Vec<Option<Value>>,
    #[serde(skip)]
    function_state: HashMap<String, Value>,
    #[serde(skip)]
    function_started: Option<Instant>,
    #[serde(skip)]
    function_code: String,
    #[serde(skip)]
    pub(crate) workflow_busy: bool,
    #[serde(default)]
    pub position: [f32; 2],
    #[serde(default)]
    pub region: Option<Region>,
    pub color: [u8; 3],
    pub opacity: f32,
}

impl Node {
    pub fn new(name: &str, kind: Kind) -> Node {
        let has_region = matches!(
            kind,
            Kind::Ocr { .. } | Kind::Keyboard { .. } | Kind::Click { .. }
                | Kind::WaitForText { .. } | Kind::AssertText { .. } | Kind::SetField { .. }
        );
        Node {
            name: name.into(),
            title: String::new(),
            group: String::new(),
            kind,
            input_parameters: Vec::new(),
            parameter_values: Vec::new(),
            function_state: HashMap::new(),
            function_started: None,
            function_code: String::new(),
            workflow_busy: false,
            position: [0.0, 0.0],
            region: has_region.then(Region::default),
            color: [0, 170, 255],
            opacity: 0.3,
        }
    }

    /// Copies configuration without carrying over execution state.
    pub fn duplicate(&self, name: &str) -> Self {
        let mut node = self.clone();
        node.name = name.into();
        node.reset_runtime();
        node
    }

    pub(crate) fn reset_runtime(&mut self) {
        self.parameter_values.clear();
        self.function_state.clear();
        self.function_started = None;
        self.function_code.clear();
        self.workflow_busy = false;
        match &mut self.kind {
            Kind::Ocr { last_hash, .. } => *last_hash = None,
            Kind::Compare { last, .. } => *last = None,
            Kind::CompareInputs { last, values, .. } => {
                *last = None;
                *values = [None, None];
            }
            Kind::Hysteresis { band, .. } | Kind::Threshold { band, .. } => *band = None,
            Kind::ValidateNumber { last, .. } => *last = None,
            Kind::WaitForText { pending, .. } | Kind::SetField { pending, .. } => *pending = None,
            Kind::Once { fired, .. } => *fired = false,
            Kind::Subflow { graph } => graph.reset_runtime(),
            _ => {}
        }
    }

    pub fn input_count(&self) -> usize {
        match &self.kind {
            Kind::Ocr { .. } | Kind::Constant { .. } | Kind::Event { .. }
            | Kind::Note { .. } | Kind::SubflowInput => 0,
            Kind::CompareInputs { .. } => 2,
            Kind::Function { input_names, .. } if !input_names.is_empty() => input_names.len(),
            _ if !self.available_input_parameters().is_empty() => 1 + self.input_parameters.len(),
            _ => 1,
        }
    }

    /// Names of this Function node's named input pins; empty for every other configuration.
    pub fn named_function_inputs(&self) -> &[String] {
        match &self.kind {
            Kind::Function { input_names, .. } => input_names,
            _ => &[],
        }
    }

    pub fn is_waiting(&self) -> bool {
        matches!(&self.kind, Kind::WaitForText { pending: Some(_), .. } | Kind::SetField { pending: Some(_), .. })
    }

    pub(crate) fn cancel_wait(&mut self) {
        match &mut self.kind {
            Kind::WaitForText { pending, .. } | Kind::SetField { pending, .. } => *pending = None,
            Kind::Subflow { graph } => graph.cancel_waits(),
            _ => {}
        }
    }

    pub fn available_input_parameters(&self) -> &'static [InputParameter] {
        use InputParameter::*;
        match self.kind {
            Kind::Compare { .. } => &[CompareValue],
            Kind::Hysteresis { .. } | Kind::Threshold { .. } => &[Lower, Upper],
            Kind::ValidateNumber { .. } => &[Minimum, Maximum, MaxChange],
            _ => &[],
        }
    }

    pub fn parameter_port(&self, parameter: InputParameter) -> Option<usize> {
        self.input_parameters.iter().position(|&p| p == parameter).map(|p| p + 1)
    }

    pub fn input_label(&self, port: usize) -> &str {
        if let Some(name) = self.named_function_inputs().get(port) {
            return name;
        }
        match (port, &self.kind) {
            (0, Kind::CompareInputs { .. }) => "A",
            (1, Kind::CompareInputs { .. }) => "B",
            (0, _) => "Input",
            (port, _) => self.input_parameters.get(port - 1).map_or("Input", |p| p.label()),
        }
    }

    pub fn validate_function_inputs(&self) -> Result<(), String> {
        validate_input_names(self.named_function_inputs())
    }

    pub(crate) fn validate_input_parameters(&self) -> Result<(), String> {
        for (i, parameter) in self.input_parameters.iter().enumerate() {
            if !self.available_input_parameters().contains(parameter) {
                return Err(format!("unsupported input parameter '{}'", parameter.label()));
            }
            if self.input_parameters[..i].contains(parameter) {
                return Err(format!("duplicate input parameter '{}'", parameter.label()));
            }
        }
        Ok(())
    }

    pub(crate) fn has_single_connection_ports(&self) -> bool {
        !self.input_parameters.is_empty()
            || !self.named_function_inputs().is_empty()
            || matches!(self.kind, Kind::CompareInputs { .. })
    }

    pub fn output_count(&self) -> usize {
        match self.kind {
            Kind::Note { .. } | Kind::SubflowOutput => 0,
            _ => 1,
        }
    }

    pub fn display_title(&self) -> &str {
        if self.title.trim().is_empty() {
            &self.name
        } else {
            &self.title
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self.kind {
            Kind::Ocr { .. } => "ocr",
            Kind::Model { .. } => "model",
            Kind::Constant { .. } => "constant",
            Kind::Function { .. } => "function",
            Kind::Compare { .. } => "compare",
            Kind::CompareInputs { .. } => "compare_inputs",
            Kind::Hysteresis { .. } => "hysteresis",
            Kind::Threshold { .. } => "threshold",
            Kind::Keyboard { .. } => "keyboard",
            Kind::Click { .. } => "click",
            Kind::WaitForText { .. } => "wait_for_text",
            Kind::AssertText { .. } => "assert_text",
            Kind::Once { .. } => "once",
            Kind::SetField { .. } => "set_field",
            Kind::Keypress { .. } => "keypress",
            Kind::ValidateNumber { .. } => "validate_number",
            Kind::Event { .. } => "event",
            Kind::Note { .. } => "note",
            Kind::Subflow { .. } => "subflow",
            Kind::SubflowInput => "subflow_input",
            Kind::SubflowOutput => "subflow_output",
        }
    }

    pub(crate) fn reset_input_state(&mut self) {
        if matches!(self.kind, Kind::Function { .. }) {
            self.reset_runtime();
            return;
        }
        if !self.input_parameters.is_empty() {
            self.reset_runtime();
            return;
        }
        match &mut self.kind {
            Kind::CompareInputs { values, last, .. } => {
                *values = [None, None];
                *last = None;
            }
            Kind::Subflow { graph } => graph.reset_runtime(),
            Kind::WaitForText { pending, .. } | Kind::SetField { pending, .. } => *pending = None,
            _ => {}
        }
    }

    pub(crate) fn process_slots(
        &mut self,
        inputs: &[Option<Value>],
        main_is_fresh: bool,
        backend: &mut dyn Backend,
        depth: usize,
    ) -> Result<Option<Value>, String> {
        if !self.named_function_inputs().is_empty() {
            return self.evaluate_function(inputs, backend);
        }
        if let Kind::CompareInputs { values, .. } = &mut self.kind {
            for (cached, incoming) in values.iter_mut().zip(inputs) {
                if let Some(value) = incoming {
                    *cached = Some(value.clone());
                }
            }
            return self.evaluate(&[], backend, depth);
        }

        self.validate_input_parameters()?;
        self.parameter_values.resize(self.input_count(), None);
        let mut changed = false;
        for (port, (cached, incoming)) in self.parameter_values.iter_mut().zip(inputs).enumerate() {
            if let Some(value) = incoming {
                if port == 0 && !main_is_fresh && cached.is_some() {
                    continue;
                }
                if (port == 0 && main_is_fresh) || cached.as_ref() != Some(value) {
                    *cached = Some(value.clone());
                    changed = true;
                }
            }
        }
        if !changed {
            return Ok(None);
        }
        let Some(main) = self.parameter_values[0].clone() else { return Ok(None) };
        if let Some(port) = self.parameter_values.iter().position(Option::is_none) {
            log::warn!("[{}] Waiting for input '{}'", self.name, self.input_label(port));
            return Ok(None);
        }
        self.evaluate(&[main], backend, depth)
    }

    /// Evaluates a Function node's script. `slots` holds one entry per input pin; named inputs
    /// are bound as variables, reading `()` (Rhai) or `undefined` (JavaScript) where a pin
    /// produced no value this evaluation.
    fn evaluate_function(
        &mut self,
        slots: &[Option<Value>],
        backend: &mut dyn Backend,
    ) -> Result<Option<Value>, String> {
        self.validate_function_inputs()?;
        let (code, input_names, language) = match &self.kind {
            Kind::Function { code, input_names, language } => {
                (code.clone(), input_names.clone(), *language)
            }
            _ => return Err("not a Function node".into()),
        };
        // The language and named inputs are part of the script's interface: changing them
        // invalidates persistent state exactly like editing the script itself.
        let mut signature = format!("{language:?}\u{1f}{}\u{1f}", input_names.join("\u{1f}"));
        signature.push_str(&code);
        if self.function_code != signature {
            self.function_state.clear();
            self.function_started = None;
            self.function_code = signature;
        }
        let now = backend.now();
        let started = *self.function_started.get_or_insert(now);
        let elapsed_ms = i64::try_from(now.duration_since(started).as_millis())
            .map_err(|_| "Function runtime exceeds the supported elapsed time")?;
        let script = ScriptRun {
            code: &code,
            input_names: &input_names,
            slots,
            state: &self.function_state,
            workflow_busy: self.workflow_busy,
            elapsed_ms,
        };
        let (output, next_state) = match language {
            ScriptLanguage::Rhai => evaluate_rhai(&script)?,
            ScriptLanguage::Javascript => evaluate_javascript(&script)?,
        };
        self.function_state = next_state;
        Ok(output)
    }

    /// Recognizes an already captured event frame without taking another screenshot.
    pub fn process_frame(
        &mut self,
        region: &Region,
        pixels: &[u8],
        backend: &mut dyn Backend,
    ) -> Result<Option<Value>, String> {
        match &mut self.kind {
            Kind::Ocr { single_line, last_hash } =>
                recognize_changed_frame(region, pixels, *single_line, last_hash, backend),
            _ => Err(format!("Node '{}' is not an OCR event source", self.name)),
        }
    }

    /// Returns `Ok(None)` when the node produced nothing (stops downstream).
    pub fn process(
        &mut self,
        inputs: &[Value],
        backend: &mut dyn Backend,
    ) -> Result<Option<Value>, String> {
        self.process_with_depth(inputs, backend, 0)
    }

    pub(crate) fn process_with_depth(
        &mut self,
        inputs: &[Value],
        backend: &mut dyn Backend,
        depth: usize,
    ) -> Result<Option<Value>, String> {
        if !self.input_parameters.is_empty() {
            let slots: Vec<_> = inputs.iter().cloned().map(Some).collect();
            return self.process_slots(&slots, true, backend, depth);
        }
        self.evaluate(inputs, backend, depth)
    }

    fn evaluate(
        &mut self,
        inputs: &[Value],
        backend: &mut dyn Backend,
        depth: usize,
    ) -> Result<Option<Value>, String> {
        use InputParameter::*;
        let parameter = |p| {
            self.input_parameters.iter().position(|&enabled| enabled == p)
                .map(|index| self.parameter_values[index + 1].as_ref().expect("settings are ready"))
        };
        let limit = |p: InputParameter, fixed| match parameter(p) {
            None => Ok(fixed),
            Some(value) => strict_number(value).filter(|n| n.is_finite()).map(Some).ok_or_else(|| {
                log::warn!("[{}] '{}' must be a finite number", self.name, p.label());
            }),
        };
        let region = self.region.unwrap_or_default();
        let hysteresis = matches!(self.kind, Kind::Hysteresis { .. });
        if matches!(self.kind, Kind::Function { .. }) {
            let slots: Vec<Option<Value>> = inputs.iter().cloned().map(Some).collect();
            return self.evaluate_function(&slots, backend);
        }
        match &mut self.kind {
            Kind::Ocr { single_line, last_hash } => {
                let pixels = backend.capture(&region)?;
                recognize_changed_frame(&region, &pixels, *single_line, last_hash, backend)
            }
            Kind::Model { format } => Ok(inputs.first().and_then(|v| convert(v, *format))),
            Kind::Constant { value, format } => convert(&Value::Text(value.clone()), *format)
                .map(Some).ok_or_else(|| format!("Constant '{}' cannot convert '{}' to {format:?}", self.name, value)),
            Kind::Function { .. } => unreachable!("Function nodes are evaluated above"),
            Kind::Compare {
                op,
                value,
                on_change,
                last,
            } => {
                let Some(input) = inputs.first() else {
                    return Ok(None);
                };
                let target = parameter(CompareValue);
                if matches!(target, Some(Value::Num(n)) if !n.is_finite()) {
                    log::warn!("[{}] Comparison value must be finite", self.name);
                    return Ok(None);
                }
                let dynamic_value = target.map(Value::to_text);
                let value = dynamic_value.as_deref().unwrap_or(value);
                let Some(result) = op.test(input, value) else {
                    log::warn!(
                        "[COMPARE] Cannot compare '{}' {} '{}'",
                        input.to_text(),
                        op.symbol(),
                        value
                    );
                    return Ok(None);
                };
                let was = last.replace(result);
                let fire = result && !(*on_change && was == Some(true));
                log::info!(
                    "[COMPARE] '{}' {} '{}' = {}{}",
                    input.to_text(),
                    op.symbol(),
                    value,
                    result,
                    if fire { " -> trigger" } else { "" }
                );
                Ok(fire.then(|| input.clone()))
            }
            Kind::CompareInputs { op, trigger_when, on_change, last, values } => {
                for (cached, incoming) in values.iter_mut().zip(inputs) {
                    *cached = Some(incoming.clone());
                }
                let [Some(left), Some(right)] = values else {
                    return Ok(None);
                };
                let Some(result) = op.test(left, &right.to_text()) else {
                    log::warn!("[COMPARE INPUTS] Cannot compare '{}' {} '{}'",
                        left.to_text(), op.symbol(), right.to_text());
                    return Ok(None);
                };
                let previous = last.replace(result);
                let fire = result == *trigger_when && (!*on_change || previous != Some(result));
                log::info!("[COMPARE INPUTS] '{}' {} '{}' = {result}{}",
                    left.to_text(), op.symbol(), right.to_text(), if fire { " -> trigger" } else { "" });
                Ok(fire.then_some(Value::Bool(result)))
            }
            Kind::Hysteresis {
                upper,
                lower,
                trigger,
                band,
            } | Kind::Threshold {
                upper,
                lower,
                trigger,
                band,
            } => {
                let label = if hysteresis { "HYSTERESIS" } else { "THRESHOLD" };
                let Some(input) = inputs.first() else {
                    return Ok(None);
                };
                let (Ok(Some(lower)), Ok(Some(upper))) =
                    (limit(Lower, Some(*lower)), limit(Upper, Some(*upper))) else {
                    return Ok(None);
                };
                if !lower.is_finite() || !upper.is_finite() || lower > upper
                    || (lower == upper && *trigger == Crossing::EnteringRange) {
                    log::warn!("[{label}] Invalid or impossible threshold range");
                    return Ok(None);
                }
                let Some(v) = as_number(input) else {
                    log::warn!("[{label}] No number in '{}'", input.to_text());
                    return Ok(None);
                };
                if !v.is_finite() {
                    log::warn!("[{label}] Input must be a finite number");
                    return Ok(None);
                }
                if hysteresis && trigger.is_range() {
                    return Err("Range-entry triggers are only supported by Threshold nodes".into());
                }
                let previous = *band;
                let current = if v > upper
                    || (matches!(*trigger, Crossing::RisingOrEqual | Crossing::EnteringRange) && v == upper)
                {
                    Band::Above
                } else if v < lower
                    || (matches!(*trigger, Crossing::FallingOrEqual | Crossing::EnteringRange) && v == lower)
                {
                    Band::Below
                } else if hysteresis {
                    previous.unwrap_or(Band::Between)
                } else {
                    Band::Between
                };
                *band = Some(current);
                let crossed = match (previous, current) {
                    (Some(p), Band::Above) if p != Band::Above => Some(Crossing::Rising),
                    (Some(p), Band::Below) if p != Band::Below => Some(Crossing::Falling),
                    (None, Band::Above) if !hysteresis => Some(Crossing::Rising),
                    (None, Band::Below) if !hysteresis => Some(Crossing::Falling),
                    (Some(p), Band::Between) if p != Band::Between && trigger.is_range() => Some(*trigger),
                    (None, Band::Between) if !hysteresis && trigger.is_range() => Some(*trigger),
                    _ => None,
                };
                let fire = crossed.is_some_and(|c| {
                    *trigger == Crossing::Both
                        || *trigger == c
                        || matches!(
                            (*trigger, c),
                            (Crossing::RisingOrEqual, Crossing::Rising)
                                | (Crossing::FallingOrEqual, Crossing::Falling)
                        )
                });
                log::info!(
                    "[{label}] {v} (lower {lower}, upper {upper}) {current:?}{}",
                    match (crossed, fire) {
                        (Some(c), true) => format!(" -> {c:?} trigger"),
                        (Some(c), false) => format!(" ({c:?}, ignored)"),
                        _ => String::new(),
                    }
                );
                Ok(fire.then(|| input.clone()))
            }
            Kind::Keyboard { text } => {
                let Some(v) = inputs.first() else {
                    log::warn!("[KEYBOARD] No input provided");
                    return Ok(None);
                };
                let t = if text.is_empty() {
                    v.to_text()
                } else {
                    text.clone()
                };
                let (x, y) = region.center();
                backend.click(x, y, "left")?;
                log::info!(
                    "[KEYBOARD] Typing text: '{}' (region: {} x {} at {}, {})",
                    t, region.width, region.height, region.x, region.y
                );
                backend.type_text(&t)?;
                Ok(Some(Value::Text(t)))
            }
            Kind::Click { button } => {
                if inputs.is_empty() {
                    log::warn!("[CLICK] No input provided");
                    return Ok(None);
                }
                let (x, y) = region.center();
                log::info!("[CLICK] Clicking at ({}, {}) with button: {}", x, y, button);
                backend.click(x, y, button)?;
                Ok(Some(Value::Bool(true)))
            }
            Kind::Once { armed, fired } => {
                if !*armed || *fired { return Ok(None); }
                let Some(input) = inputs.first() else { return Ok(None) };
                *fired = true;
                log::info!("[ONCE] '{}' latched; reset explicitly before another attempt", self.name);
                Ok(Some(input.clone()))
            }
            Kind::SetField { text, verification, pending } => {
                if pending.is_none() {
                    let Some(input) = inputs.first() else { return Ok(None) };
                    let text = if text.is_empty() { input.to_text() } else { text.clone() };
                    if let Some(verification) = verification {
                        verification.validate(&text)?;
                        if verification.numeric && finite_number(&text).is_none() {
                            return Err("numeric field verification requires a finite numeric replacement".into());
                        }
                    }
                    let (x, y) = region.center();
                    backend.click(x, y, "left")?;
                    backend.press_key(KeyAction::SelectAll)?;
                    backend.press_key(KeyAction::Backspace)?;
                    backend.type_text(&text)?;
                    log::info!("[SET FIELD] '{}' typed {text:?}", self.name);
                    let Some(verification) = verification else { return Ok(Some(Value::Text(text))) };
                    let now = backend.now();
                    *pending = Some(PendingWait {
                        input: Value::Text(text), started: now,
                        next_poll: now + Duration::from_millis(verification.poll_ms),
                    });
                    log::info!("[SET FIELD] '{}' verifying rendered value with OCR", self.name);
                    return Ok(None);
                }
                let verification = verification.as_ref().expect("pending field verification");
                let expected = pending.as_ref().expect("pending field value").input.to_text();
                poll_screen_text(backend, &region, &self.name, &expected,
                    verification.single_line, verification.timeout_ms, verification.poll_ms, pending,
                    |actual| Ok(verification.matches(actual, &expected)))
            }
            Kind::Keypress { key } => {
                let Some(input) = inputs.first() else { return Ok(None) };
                backend.press_key(*key)?;
                log::info!("[KEYPRESS] '{}' pressed {}", self.name, key.label());
                Ok(Some(input.clone()))
            }
            Kind::AssertText { expected, matching, case_sensitive, single_line } => {
                let Some(input) = inputs.first() else { return Ok(None) };
                matching.validate(expected)?;
                let pixels = backend.capture(&region)?;
                let actual = if *single_line { backend.ocr_single_line(&region, &pixels)? }
                    else { backend.ocr(&region, &pixels)? };
                if !matching.matches(&actual, expected, *case_sensitive)? {
                    return Err(format!("screen assertion failed: expected {matching:?} {expected:?}, read {actual:?}"));
                }
                log::info!("[ASSERT] '{}' matched {expected:?}", self.name);
                Ok(Some(input.clone()))
            }
            Kind::WaitForText { expected, matching, case_sensitive, single_line, timeout_ms, poll_ms, pending } => {
                matching.validate(expected)?;
                validate_wait_timing(*timeout_ms, *poll_ms)?;
                let now = backend.now();
                if pending.is_none() {
                    let Some(input) = inputs.first() else { return Ok(None) };
                    *pending = Some(PendingWait { input: input.clone(), started: now, next_poll: now });
                    log::info!("[WAIT] '{}' waiting for {expected:?}, timeout {timeout_ms} ms", self.name);
                }
                poll_screen_text(backend, &region, &self.name, expected,
                    *single_line, *timeout_ms, *poll_ms, pending,
                    |actual| matching.matches(actual, expected, *case_sensitive))
            }
            Kind::ValidateNumber { minimum, maximum, max_change, pattern, last } => {
                let Some(input) = inputs.first() else { return Ok(None) };
                let (Ok(minimum), Ok(maximum), Ok(max_change)) =
                    (limit(Minimum, *minimum), limit(Maximum, *maximum), limit(MaxChange, *max_change)) else {
                    return Ok(None);
                };
                if minimum.is_some_and(|v| !v.is_finite()) || maximum.is_some_and(|v| !v.is_finite())
                    || max_change.is_some_and(|v| !v.is_finite() || v < 0.0)
                    || matches!((minimum, maximum), (Some(min), Some(max)) if min > max)
                {
                    log::warn!("[VALIDATE NUMBER] Invalid limits at node '{}'", self.name);
                    return Ok(None);
                }
                let text = input.to_text();
                let number = strict_number(input);
                let regex_ok = pattern.is_empty() || regex::Regex::new(pattern)
                    .map(|regex| regex.is_match(&text)).unwrap_or(false);
                let accepted = number.filter(|n| n.is_finite()
                    && regex_ok
                    && minimum.map_or(true, |v| *n >= v)
                    && maximum.map_or(true, |v| *n <= v)
                    && max_change.map_or(true, |v| last.map_or(true, |previous| (*n - previous).abs() <= v)));
                if let Some(number) = accepted {
                    *last = Some(number);
                    Ok(Some(input.clone()))
                } else {
                    log::warn!("[VALIDATE NUMBER] Rejected '{}' at node '{}'", text, self.name);
                    Ok(None)
                }
            }
            Kind::Event { .. } | Kind::Note { .. } | Kind::SubflowInput => Ok(None),
            Kind::SubflowOutput => Ok(inputs.first().cloned()),
            Kind::Subflow { graph } => {
                if inputs.is_empty() && graph.uses_subflow_input() {
                    return Ok(None);
                }
                graph.run_subflow(inputs.first().cloned(), backend, depth + 1)
            }
        }
    }
}

fn strict_number(input: &Value) -> Option<f64> {
    match input {
        Value::Int(n) => Some(*n as f64),
        Value::Num(n) => Some(*n),
        Value::Text(s) => s.trim().parse().ok(),
        Value::Bool(_) => None,
    }
}

fn recognize_changed_frame(
    region: &Region,
    pixels: &[u8],
    single_line: bool,
    last_hash: &mut Option<u64>,
    backend: &mut dyn Backend,
) -> Result<Option<Value>, String> {
    log::info!(
        "[OCR] Region: ({}, {}) {}x{}, Pixels captured: {} bytes",
        region.x, region.y, region.width, region.height, pixels.len()
    );
    let mut h = DefaultHasher::new();
    pixels.hash(&mut h);
    let hash = h.finish();
    if *last_hash == Some(hash) {
        log::info!("[OCR] No change detected (hash match)");
        return Ok(None);
    }
    *last_hash = None;
    let result = if single_line {
        backend.ocr_single_line(region, pixels)?
    } else {
        backend.ocr(region, pixels)?
    };
    *last_hash = Some(hash);
    log::info!("[OCR] Recognition result: '{}'", result);
    Ok(Some(Value::Text(result.trim().into())))
}

/// Extracts the first number from text such as OCR output.
///
/// Line breaks, spaces, and symbols (`$`, `%`, `=`, ...) are removed, so `"$ 64,777\n.49"` reads
/// as `64777.49`. Words containing letters (`BTC`, `ocr1`) separate numbers instead of joining them.
fn number_in(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut cleaned = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_alphanumeric() {
            let start = i;
            while i < chars.len() && chars[i].is_alphanumeric() {
                i += 1;
            }
            let word = &chars[start..i];
            if word.iter().all(char::is_ascii_digit) {
                cleaned.extend(word);
            } else {
                cleaned.push(' ');
            }
            continue;
        }
        let minus = c == '-' && chars.get(i + 1).is_some_and(char::is_ascii_digit);
        if c == '.' || c == ',' || minus {
            cleaned.push(c);
        }
        i += 1;
    }
    cleaned.split(' ').find_map(number_token)
}

/// Normalizes one cleaned token, treating the last `.` or `,` as the decimal point when both
/// appear and repeated or three-digit-grouped separators as thousands separators.
fn number_token(token: &str) -> Option<String> {
    let start = token.find(|c: char| c.is_ascii_digit())?;
    let negative = token[..start].ends_with('-');
    let body: String = token[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .collect();
    let body = body.trim_end_matches(['.', ',']);
    let decimal = match (body.rfind('.'), body.rfind(',')) {
        (Some(dot), Some(comma)) => Some(dot.max(comma)),
        (Some(dot), None) => (body.matches('.').count() == 1).then_some(dot),
        (None, Some(comma)) => {
            (body.matches(',').count() == 1 && body.len() - comma - 1 != 3).then_some(comma)
        }
        (None, None) => None,
    };
    let mut number = String::from(if negative { "-" } else { "" });
    for (index, c) in body.char_indices() {
        if c.is_ascii_digit() {
            number.push(c);
        } else if Some(index) == decimal {
            number.push('.');
        }
    }
    Some(number)
}

fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Int(i) => Some(*i as f64),
        Value::Num(n) => Some(*n),
        Value::Text(s) => number_in(s)?.parse().ok(),
        Value::Bool(_) => None,
    }
}

fn convert(v: &Value, f: Format) -> Option<Value> {
    let s = v.to_text();
    match f {
        Format::Text => Some(Value::Text(s.trim().into())),
        Format::Number => number_in(&s)?.parse().ok().map(Value::Num),
        Format::Int => number_in(&s)?
            .split('.')
            .next()?
            .parse()
            .ok()
            .map(Value::Int),
        Format::Bool => Some(Value::Bool(matches!(
            s.trim().to_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ))),
    }
}

fn to_dynamic(v: &Value) -> Dynamic {
    match v {
        Value::Bool(b) => (*b).into(),
        Value::Int(i) => (*i).into(),
        Value::Num(n) => (*n).into(),
        Value::Text(s) => s.clone().into(),
    }
}

/// One Function-node script evaluation: the source plus every binding the script can read.
/// Evaluators return the output and the next persistent state; errors leave the node's
/// previous state untouched.
struct ScriptRun<'a> {
    code: &'a str,
    input_names: &'a [String],
    slots: &'a [Option<Value>],
    state: &'a HashMap<String, Value>,
    workflow_busy: bool,
    elapsed_ms: i64,
}

type ScriptOutcome = Result<(Option<Value>, HashMap<String, Value>), String>;

fn state_error(key: &str) -> String {
    format!("state value '{key}' must be text, a finite number, or a boolean")
}

fn evaluate_rhai(run: &ScriptRun) -> ScriptOutcome {
    let engine = Engine::new();
    let mut scope = Scope::new();
    let inputs: rhai::Array = run.slots.iter()
        .map(|slot| slot.as_ref().map_or(Dynamic::UNIT, to_dynamic))
        .collect();
    scope.push("inputs", inputs);
    for (name, slot) in run.input_names.iter().zip(run.slots) {
        scope.push_dynamic(name.as_str(), slot.as_ref().map_or(Dynamic::UNIT, to_dynamic));
    }
    let state: rhai::Map = run.state.iter()
        .map(|(key, value)| (key.as_str().into(), to_dynamic(value)))
        .collect();
    scope.push("state", state);
    scope.push_constant("workflow_busy", run.workflow_busy);
    scope.push_constant("elapsed_ms", run.elapsed_ms);
    let out: Dynamic = engine
        .eval_with_scope(&mut scope, run.code)
        .map_err(|e| e.to_string())?;
    let state: rhai::Map = scope
        .get_value("state")
        .ok_or("the 'state' variable must remain a map")?;
    let mut next_state = HashMap::with_capacity(state.len());
    for (key, value) in state {
        let value = rhai_state_value(value).ok_or_else(|| state_error(&key))?;
        next_state.insert(key.to_string(), value);
    }
    Ok((from_dynamic(out), next_state))
}

/// Persistent state only keeps plain values; collections cannot round-trip through saves.
fn rhai_state_value(d: Dynamic) -> Option<Value> {
    if let Some(b) = d.clone().try_cast::<bool>() {
        return Some(Value::Bool(b));
    }
    if let Some(i) = d.clone().try_cast::<i64>() {
        return Some(Value::Int(i));
    }
    if let Some(f) = d.clone().try_cast::<f64>() {
        return f.is_finite().then(|| Value::Num(f));
    }
    d.try_cast::<rhai::ImmutableString>().map(|s| Value::Text(s.to_string()))
}

fn evaluate_javascript(run: &ScriptRun) -> ScriptOutcome {
    use boa_engine::object::builtins::JsArray;
    use boa_engine::object::ObjectInitializer;
    use boa_engine::property::Attribute;
    use boa_engine::{Context, JsString, JsValue, Source};

    fn slot_js(slot: &Option<Value>) -> JsValue {
        slot.as_ref().map_or_else(JsValue::undefined, value_to_js)
    }
    let js_err = |e: boa_engine::JsError| e.to_string();

    let mut context = Context::default();
    let inputs = JsArray::from_iter(run.slots.iter().map(slot_js), &mut context);
    context
        .register_global_property(JsString::from("inputs"), inputs, Attribute::all())
        .map_err(js_err)?;
    for (name, slot) in run.input_names.iter().zip(run.slots) {
        context
            .register_global_property(JsString::from(name.as_str()), slot_js(slot), Attribute::all())
            .map_err(js_err)?;
    }
    let mut state = ObjectInitializer::new(&mut context);
    for (key, value) in run.state {
        state.property(JsString::from(key.as_str()), value_to_js(value), Attribute::all());
    }
    let state = state.build();
    context
        .register_global_property(JsString::from("state"), state, Attribute::all())
        .map_err(js_err)?;
    context
        .register_global_property(JsString::from("workflow_busy"), run.workflow_busy, Attribute::all())
        .map_err(js_err)?;
    // JavaScript numbers are doubles; elapsed times stay exact for ~285,000 years.
    context
        .register_global_property(JsString::from("elapsed_ms"), run.elapsed_ms as f64, Attribute::all())
        .map_err(js_err)?;

    let out = context
        .eval(Source::from_bytes(run.code.as_bytes()))
        .map_err(js_err)?;

    // Read `state` back through the script scope so a `let state = {...}` rebinding counts.
    let state = context.eval(Source::from_bytes(b"state")).map_err(js_err)?;
    let Some(state) = state.as_object() else {
        return Err("the 'state' variable must remain an object".into());
    };
    let keys = context
        .eval(Source::from_bytes(b"JSON.stringify(Object.keys(state))"))
        .map_err(js_err)?;
    let keys = keys
        .as_string()
        .map(|s| s.to_std_string_escaped())
        .ok_or("cannot list the state keys")?;
    let keys: Vec<String> =
        serde_json::from_str(&keys).map_err(|e| format!("cannot list the state keys: {e}"))?;
    let mut next_state = HashMap::with_capacity(keys.len());
    for key in keys {
        let value = state.get(JsString::from(key.as_str()), &mut context).map_err(js_err)?;
        let value = js_state_value(&value).ok_or_else(|| state_error(&key))?;
        next_state.insert(key, value);
    }
    Ok((js_output(out, &mut context)?, next_state))
}

fn value_to_js(v: &Value) -> boa_engine::JsValue {
    use boa_engine::{JsString, JsValue};
    match v {
        Value::Bool(b) => JsValue::from(*b),
        // JavaScript numbers are doubles; huge Int values round exactly like JS literals would.
        Value::Int(i) => JsValue::from(*i as f64),
        Value::Num(n) => JsValue::from(*n),
        Value::Text(s) => JsValue::from(JsString::from(s.as_str())),
    }
}

/// Largest integer a JavaScript double represents exactly (2^53).
const JS_SAFE_INTEGER: f64 = 9_007_199_254_740_992.0;

/// Converts the script's completion value to a node output. `undefined`/`null` stop the flow;
/// integral numbers in the safe range become Int so downstream formatting matches Rhai.
fn js_output(
    value: boa_engine::JsValue,
    context: &mut boa_engine::Context,
) -> Result<Option<Value>, String> {
    if value.is_null_or_undefined() {
        return Ok(None);
    }
    if let Some(b) = value.as_boolean() {
        return Ok(Some(Value::Bool(b)));
    }
    if let Some(n) = value.as_number() {
        let integral = n.fract() == 0.0 && n.abs() <= JS_SAFE_INTEGER;
        return Ok(Some(if integral { Value::Int(n as i64) } else { Value::Num(n) }));
    }
    if let Some(s) = value.as_string() {
        return Ok(Some(Value::Text(s.to_std_string_escaped())));
    }
    let text = value.to_string(context).map_err(|e| e.to_string())?;
    Ok(Some(Value::Text(text.to_std_string_escaped())))
}

fn js_state_value(value: &boa_engine::JsValue) -> Option<Value> {
    if let Some(b) = value.as_boolean() {
        return Some(Value::Bool(b));
    }
    if let Some(n) = value.as_number() {
        if !n.is_finite() {
            return None;
        }
        let integral = n.fract() == 0.0 && n.abs() <= JS_SAFE_INTEGER;
        return Some(if integral { Value::Int(n as i64) } else { Value::Num(n) });
    }
    value.as_string().map(|s| Value::Text(s.to_std_string_escaped()))
}

/// Checks Function input pin names: valid Rhai identifiers, unique, and not shadowing the
/// built-in `inputs`, `state`, `workflow_busy`, or `elapsed_ms` bindings.
pub fn validate_input_names(names: &[String]) -> Result<(), String> {
    for (i, name) in names.iter().enumerate() {
        let mut chars = name.chars();
        let identifier = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !identifier {
            return Err(format!(
                "input name '{name}' must start with a letter or '_' and contain only letters, digits, and '_'"
            ));
        }
        if matches!(name.as_str(), "inputs" | "state" | "workflow_busy" | "elapsed_ms") {
            return Err(format!("input name '{name}' is reserved"));
        }
        if names[..i].contains(name) {
            return Err(format!("duplicate input name '{name}'"));
        }
    }
    Ok(())
}

fn from_dynamic(d: Dynamic) -> Option<Value> {
    if d.is_unit() {
        None
    } else if let Some(b) = d.clone().try_cast::<bool>() {
        Some(Value::Bool(b))
    } else if let Some(i) = d.clone().try_cast::<i64>() {
        Some(Value::Int(i))
    } else if let Some(f) = d.clone().try_cast::<f64>() {
        Some(Value::Num(f))
    } else {
        Some(Value::Text(d.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_conversion_cleans_ocr_text() {
        let num = |s: &str| convert(&Value::Text(s.into()), Format::Number);
        assert_eq!(num("64,777.49"), Some(Value::Num(64777.49)));
        assert_eq!(num("$ 64,777\n.49"), Some(Value::Num(64777.49)));
        assert_eq!(num("BTC\r\n64 777.49 USD"), Some(Value::Num(64777.49)));
        assert_eq!(num("ocr1\n5.1="), Some(Value::Num(5.1)));
        assert_eq!(num("1.234.567,8"), Some(Value::Num(1234567.8)));
        assert_eq!(num("-12,5 %"), Some(Value::Num(-12.5)));
        assert_eq!(num("no digits"), None);
        let int = convert(&Value::Text("Price:\n1,299.99".into()), Format::Int);
        assert_eq!(int, Some(Value::Int(1299)));
    }
}