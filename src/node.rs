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
                let mut h = DefaultHasher::new();
                pixels.hash(&mut h);
                let hash = h.finish();
                if *last_hash == Some(hash) {
                    return Ok(None);
                }
                *last_hash = Some(hash);
                backend
                    .ocr(&region, &pixels)
                    .map(|t| Some(Value::Text(t.trim().into())))
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
            Kind::Keyboard { text } => {
                let Some(v) = inputs.first() else {
                    return Ok(None);
                };
                let t = if text.is_empty() {
                    v.to_text()
                } else {
                    text.clone()
                };
                let (x, y) = region.center();
                backend.click(x, y, "left")?;
                backend.type_text(&t)?;
                Ok(Some(Value::Text(t)))
            }
            Kind::Click { button } => {
                if inputs.is_empty() {
                    return Ok(None);
                }
                let (x, y) = region.center();
                backend.click(x, y, button)?;
                Ok(Some(Value::Bool(true)))
            }
        }
    }
}

fn number_in(s: &str) -> Option<String> {
    let s = s.replace(',', ".");
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit()
            || (b[i] == '-' && b.get(i + 1).is_some_and(|c| c.is_ascii_digit()))
        {
            let mut j = i + 1;
            let mut dot = false;
            while j < b.len()
                && (b[j].is_ascii_digit()
                    || (b[j] == '.' && !dot && b.get(j + 1).is_some_and(|c| c.is_ascii_digit())))
            {
                dot |= b[j] == '.';
                j += 1;
            }
            return Some(b[i..j].iter().collect());
        }
        i += 1;
    }
    None
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
