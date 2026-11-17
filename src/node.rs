use crate::backend::{Backend, Region};
use crate::graph::Graph;
use rhai::{Dynamic, Engine, Scope};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

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
    /// Rhai script: `inputs` array in scope; the last expression is the result. `()` stops the flow.
    Function {
        code: String,
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
        );
        Node {
            name: name.into(),
            title: String::new(),
            group: String::new(),
            kind,
            input_parameters: Vec::new(),
            parameter_values: Vec::new(),
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
        match &mut self.kind {
            Kind::Ocr { last_hash, .. } => *last_hash = None,
            Kind::Compare { last, .. } => *last = None,
            Kind::CompareInputs { last, values, .. } => {
                *last = None;
                *values = [None, None];
            }
            Kind::Hysteresis { band, .. } | Kind::Threshold { band, .. } => *band = None,
            Kind::ValidateNumber { last, .. } => *last = None,
            Kind::Subflow { graph } => graph.reset_runtime(),
            _ => {}
        }
    }

    pub fn input_count(&self) -> usize {
        match self.kind {
            Kind::Ocr { .. } | Kind::Constant { .. } | Kind::Event { .. }
            | Kind::Note { .. } | Kind::SubflowInput => 0,
            Kind::CompareInputs { .. } => 2,
            _ if !self.available_input_parameters().is_empty() => 1 + self.input_parameters.len(),
            _ => 1,
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

    pub fn input_label(&self, port: usize) -> &'static str {
        match (port, &self.kind) {
            (0, Kind::CompareInputs { .. }) => "A",
            (1, Kind::CompareInputs { .. }) => "B",
            (0, _) => "Input",
            (port, _) => self.input_parameters.get(port - 1).map_or("Input", |p| p.label()),
        }
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
        !self.input_parameters.is_empty() || matches!(self.kind, Kind::CompareInputs { .. })
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
            Kind::ValidateNumber { .. } => "validate_number",
            Kind::Event { .. } => "event",
            Kind::Note { .. } => "note",
            Kind::Subflow { .. } => "subflow",
            Kind::SubflowInput => "subflow_input",
            Kind::SubflowOutput => "subflow_output",
        }
    }

    pub(crate) fn reset_input_state(&mut self) {
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
        match &mut self.kind {
            Kind::Ocr { single_line, last_hash } => {
                let pixels = backend.capture(&region)?;
                recognize_changed_frame(&region, &pixels, *single_line, last_hash, backend)
            }
            Kind::Model { format } => Ok(inputs.first().and_then(|v| convert(v, *format))),
            Kind::Constant { value, format } => convert(&Value::Text(value.clone()), *format)
                .map(Some).ok_or_else(|| format!("Constant '{}' cannot convert '{}' to {format:?}", self.name, value)),
            Kind::Function { code } => {
                let engine = Engine::new();
                let arr: rhai::Array = inputs.iter().map(to_dynamic).collect();
                let mut scope = Scope::new();
                scope.push("inputs", arr);
                let out: Dynamic = engine
                    .eval_with_scope(&mut scope, code)
                    .map_err(|e| e.to_string())?;
                Ok(from_dynamic(out))
            }
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