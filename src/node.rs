use crate::backend::{Backend, Region};
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

/// Which threshold crossings make a Threshold node trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Crossing {
    Rising,
    Falling,
    Both,
}

impl Crossing {
    pub const ALL: [Crossing; 3] = [Crossing::Rising, Crossing::Falling, Crossing::Both];

    pub fn label(self) -> &'static str {
        match self {
            Crossing::Rising => "Rises above upper",
            Crossing::Falling => "Falls below lower",
            Crossing::Both => "Rises above upper or falls below lower",
        }
    }
}

/// Last band a Threshold node saw the input in.
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
        #[serde(skip)]
        last_hash: Option<u64>,
    },
    /// Converts a value to a format.
    Model {
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
    /// Hysteresis trigger: passes the input on when it crosses above `upper` or below `lower`.
    /// After crossing one threshold it must cross the other before that direction can trigger
    /// again, so noise around a single threshold does not trigger repeatedly.
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    #[serde(flatten)]
    pub kind: Kind,
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
            kind,
            position: [0.0, 0.0],
            region: has_region.then(Region::default),
            color: [0, 170, 255],
            opacity: 0.3,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self.kind {
            Kind::Ocr { .. } => "ocr",
            Kind::Model { .. } => "model",
            Kind::Function { .. } => "function",
            Kind::Compare { .. } => "compare",
            Kind::Threshold { .. } => "threshold",
            Kind::Keyboard { .. } => "keyboard",
            Kind::Click { .. } => "click",
        }
    }

    /// Returns `Ok(None)` when the node produced nothing (stops downstream).
    pub fn process(
        &mut self,
        inputs: &[Value],
        backend: &mut dyn Backend,
    ) -> Result<Option<Value>, String> {
        let region = self.region.unwrap_or_default();
        match &mut self.kind {
            Kind::Ocr { last_hash } => {
                let pixels = backend.capture(&region)?;
                eprintln!(
                    "[OCR] Region: ({}, {}) {}x{}, Pixels captured: {} bytes",
                    region.x,
                    region.y,
                    region.width,
                    region.height,
                    pixels.len()
                );
                let mut h = DefaultHasher::new();
                pixels.hash(&mut h);
                let hash = h.finish();
                if *last_hash == Some(hash) {
                    eprintln!("[OCR] No change detected (hash match)");
                    return Ok(None);
                }
                let result = backend.ocr(&region, &pixels)?;
                *last_hash = Some(hash);
                eprintln!("[OCR] Recognition result: '{}'", result);
                Ok(Some(Value::Text(result.trim().into())))
            }
            Kind::Model { format } => Ok(inputs.first().and_then(|v| convert(v, *format))),
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
                let Some(result) = op.test(input, value) else {
                    eprintln!(
                        "[COMPARE] Cannot compare '{}' {} '{}'",
                        input.to_text(),
                        op.symbol(),
                        value
                    );
                    return Ok(None);
                };
                let was = last.replace(result);
                let fire = result && !(*on_change && was == Some(true));
                eprintln!(
                    "[COMPARE] '{}' {} '{}' = {}{}",
                    input.to_text(),
                    op.symbol(),
                    value,
                    result,
                    if fire { " -> trigger" } else { "" }
                );
                Ok(fire.then(|| input.clone()))
            }
            Kind::Threshold {
                upper,
                lower,
                trigger,
                band,
            } => {
                let Some(input) = inputs.first() else {
                    return Ok(None);
                };
                if lower > upper {
                    eprintln!("[THRESHOLD] Lower {lower} is above upper {upper}");
                    return Ok(None);
                }
                let Some(v) = as_number(input) else {
                    eprintln!("[THRESHOLD] No number in '{}'", input.to_text());
                    return Ok(None);
                };
                let previous = *band;
                // Between the thresholds the last band is kept; that is the hysteresis.
                let current = if v > *upper {
                    Band::Above
                } else if v < *lower {
                    Band::Below
                } else {
                    previous.unwrap_or(Band::Between)
                };
                *band = Some(current);
                let crossed = match (previous, current) {
                    (Some(p), Band::Above) if p != Band::Above => Some(Crossing::Rising),
                    (Some(p), Band::Below) if p != Band::Below => Some(Crossing::Falling),
                    _ => None,
                };
                let fire = crossed.is_some_and(|c| *trigger == Crossing::Both || *trigger == c);
                eprintln!(
                    "[THRESHOLD] {v} (lower {lower}, upper {upper}) {current:?}{}",
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
                    eprintln!("[KEYBOARD] No input provided");
                    return Ok(None);
                };
                let t = if text.is_empty() {
                    v.to_text()
                } else {
                    text.clone()
                };
                let (x, y) = region.center();
                backend.click(x, y, "left")?;
                eprintln!(
                    "[KEYBOARD] Typing text: '{}' (region: {} x {} at {}, {})",
                    t, region.width, region.height, region.x, region.y
                );
                backend.type_text(&t)?;
                Ok(Some(Value::Text(t)))
            }
            Kind::Click { button } => {
                if inputs.is_empty() {
                    eprintln!("[CLICK] No input provided");
                    return Ok(None);
                }
                let (x, y) = region.center();
                eprintln!("[CLICK] Clicking at ({}, {}) with button: {}", x, y, button);
                backend.click(x, y, button)?;
                Ok(Some(Value::Bool(true)))
            }
        }
    }
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