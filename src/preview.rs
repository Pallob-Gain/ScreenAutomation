use crate::backend::{Backend, KeyAction, Region};
use crate::graph::Graph;
use crate::node::Value;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct StepResult {
    pub node: String,
    pub inputs: Vec<Value>,
    pub output: Option<Value>,
    pub actions: Vec<String>,
}

pub struct PreviewSession {
    graph: Graph,
    order: Vec<usize>,
    next: usize,
    outputs: HashMap<String, Option<Value>>,
}

impl PreviewSession {
    pub fn new(graph: &Graph) -> Result<Self, String> {
        let issues = graph.validate();
        if !issues.is_empty() {
            return Err(issues.into_iter().map(|issue| match issue.node {
                Some(node) => format!("{node}: {}", issue.message),
                None => issue.message,
            }).collect::<Vec<_>>().join("; "));
        }
        let mut graph = graph.clone();
        graph.reset_runtime();
        let order = graph.order()?;
        Ok(Self { graph, order, next: 0, outputs: HashMap::new() })
    }

    pub fn step(&mut self, backend: &mut dyn Backend) -> Result<Option<StepResult>, String> {
        let Some(&index) = self.order.get(self.next) else { return Ok(None) };
        let node = self.graph.nodes[index].name.clone();
        let mut preview = PreviewBackend { backend, actions: Vec::new() };
        let (inputs, output) = self.graph.execute_node(index, &mut self.outputs, &mut preview, None, 0)?;
        if !self.graph.nodes[index].is_waiting() {
            self.next += 1;
        }
        Ok(Some(StepResult { node, inputs, output, actions: preview.actions }))
    }

    pub fn is_finished(&self) -> bool {
        self.next == self.order.len()
    }

    pub fn is_waiting(&self) -> bool {
        self.graph.has_pending_waits()
    }
}

struct PreviewBackend<'a> {
    backend: &'a mut dyn Backend,
    actions: Vec<String>,
}

impl Backend for PreviewBackend<'_> {
    fn capture(&mut self, region: &Region) -> Result<Vec<u8>, String> {
        self.backend.capture(region)
    }

    fn ocr(&mut self, region: &Region, pixels: &[u8]) -> Result<String, String> {
        self.backend.ocr(region, pixels)
    }

    fn ocr_single_line(&mut self, region: &Region, pixels: &[u8]) -> Result<String, String> {
        self.backend.ocr_single_line(region, pixels)
    }

    fn type_text(&mut self, text: &str) -> Result<(), String> {
        self.actions.push(format!("type {text}"));
        Ok(())
    }

    fn click(&mut self, x: i32, y: i32, button: &str) -> Result<(), String> {
        self.actions.push(format!("click {button} at ({x}, {y})"));
        Ok(())
    }

    fn press_key(&mut self, key: KeyAction) -> Result<(), String> {
        self.actions.push(format!("keypress {}", key.label()));
        Ok(())
    }

    fn now(&self) -> std::time::Instant {
        self.backend.now()
    }
}
