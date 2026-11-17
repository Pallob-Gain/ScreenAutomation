use crate::backend::Backend;
use crate::node::{Kind, Node, Value};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const DEFAULT_REFRESH_MS: u64 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    Timed,
    DisplayRefresh,
    HmiEvent,
}

/// A source node's new value, independent of the mechanism that generated it.
pub struct NodeEvent {
    pub source: String,
    pub value: Value,
}

#[derive(Default)]
pub struct EventRuntime {
    values: HashMap<String, Value>,
    initialized: bool,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphSettings {
    pub run_mode: RunMode,
    #[serde(deserialize_with = "deserialize_refresh_ms")]
    pub refresh_ms: u64,
}

impl Default for GraphSettings {
    fn default() -> Self {
        Self { run_mode: RunMode::Timed, refresh_ms: DEFAULT_REFRESH_MS }
    }
}

fn deserialize_refresh_ms<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = u64::deserialize(deserializer)?;
    if !(100..=60_000).contains(&value) {
        return Err(serde::de::Error::custom("refresh_ms must be between 100 and 60000 ms"));
    }
    Ok(value)
}

#[derive(Default, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub settings: GraphSettings,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SavedEdge", into = "SavedEdge")]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub input: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum SavedEdge {
    Legacy((String, String)),
    Routed { source: String, target: String, input: usize },
}

impl From<SavedEdge> for Edge {
    fn from(edge: SavedEdge) -> Self {
        match edge {
            SavedEdge::Legacy((source, target)) => Self { source, target, input: 0 },
            SavedEdge::Routed { source, target, input } => Self { source, target, input },
        }
    }
}

impl From<Edge> for SavedEdge {
    fn from(edge: Edge) -> Self {
        if edge.input == 0 {
            Self::Legacy((edge.source, edge.target))
        } else {
            Self::Routed { source: edge.source, target: edge.target, input: edge.input }
        }
    }
}

impl Graph {
    pub fn add_node(&mut self, node: Node) -> Result<(), String> {
        if self.nodes.iter().any(|n| n.name == node.name) {
            return Err(format!("duplicate node name: {}", node.name));
        }
        self.nodes.push(node);
        Ok(())
    }

    pub fn remove_node(&mut self, name: &str) {
        let affected: Vec<String> = self.edges.iter()
            .filter(|edge| edge.source == name).map(|edge| edge.target.clone()).collect();
        self.nodes.retain(|n| n.name != name);
        self.edges.retain(|edge| edge.source != name && edge.target != name);
        for node in &mut self.nodes {
            if affected.contains(&node.name) {
                node.reset_input_state();
            }
        }
    }

    pub fn connect(&mut self, src: &str, dst: &str) -> Result<(), String> {
        self.connect_input(src, dst, 0)
    }

    pub fn connect_input(&mut self, src: &str, dst: &str, input: usize) -> Result<(), String> {
        if !self.nodes.iter().any(|n| n.name == src) {
            return Err("unknown node".into());
        }
        let destination = self.nodes.iter().position(|node| node.name == dst)
            .ok_or("unknown node")?;
        let pair = matches!(self.nodes[destination].kind, crate::node::Kind::CompareInputs { .. });
        if input >= if pair { 2 } else { 1 } {
            return Err("unknown input port".into());
        }
        let e = Edge { source: src.into(), target: dst.into(), input };
        if !self.edges.contains(&e) {
            if pair && self.edges.iter().any(|edge| edge.target == dst && edge.input == input) {
                return Err(format!("Input {} already has a connection", if input == 0 { "A" } else { "B" }));
            }
            self.edges.push(e);
            self.nodes[destination].reset_input_state();
        }
        Ok(())
    }

    pub fn disconnect_input(&mut self, src: &str, dst: &str, input: usize) {
        let before = self.edges.len();
        self.edges.retain(|edge| !(edge.source == src && edge.target == dst && edge.input == input));
        if self.edges.len() != before {
            if let Some(node) = self.nodes.iter_mut().find(|node| node.name == dst) {
                node.reset_input_state();
            }
        }
    }

    fn order(&self) -> Result<Vec<usize>, String> {
        let idx: HashMap<&str, usize> = self.nodes.iter().enumerate().map(|(i, n)| (n.name.as_str(), i)).collect();
        let mut indeg = vec![0; self.nodes.len()];
        for edge in &self.edges {
            let Some(&destination) = idx.get(edge.target.as_str()) else {
                return Err(format!("unknown node: {}", edge.target));
            };
            if !idx.contains_key(edge.source.as_str()) {
                return Err(format!("unknown node: {}", edge.source));
            }
            indeg[destination] += 1;
        }
        let mut queue: Vec<usize> = (0..self.nodes.len()).filter(|&i| indeg[i] == 0).collect();
        let mut order = vec![];
        while !queue.is_empty() {
            let n = queue.remove(0);
            order.push(n);
            for edge in &self.edges {
                if idx[edge.source.as_str()] == n {
                    let di = idx[edge.target.as_str()];
                    indeg[di] -= 1;
                    if indeg[di] == 0 {
                        queue.push(di);
                    }
                }
            }
        }
        if order.len() != self.nodes.len() {
            return Err("graph contains a cycle".into());
        }
        Ok(order)
    }

    /// One refresh tick. Nodes producing no output stop propagation.
    pub fn run_once(&mut self, backend: &mut dyn Backend) -> Result<HashMap<String, Option<Value>>, String> {
        let mut outputs: HashMap<String, Option<Value>> = HashMap::new();
        for i in self.order()? {
            let name = self.nodes[i].name.clone();
            let parents: Vec<&Edge> = self.edges.iter().filter(|edge| edge.target == name).collect();
            if matches!(self.nodes[i].kind, crate::node::Kind::CompareInputs { .. }) {
                let mut inputs = [None, None];
                for edge in &parents {
                    let slot = inputs.get_mut(edge.input).ok_or("unknown comparison input port")?;
                    *slot = outputs.get(&edge.source).cloned().flatten();
                }
                let out = if inputs.iter().any(Option::is_some) {
                    self.nodes[i].process_slots(&inputs, backend)?
                } else { None };
                outputs.insert(name, out);
                continue;
            }
            let ins: Vec<Value> = parents.iter().filter_map(|edge| outputs.get(&edge.source).cloned().flatten()).collect();
            if !parents.is_empty() && ins.is_empty() {
                outputs.insert(name, None);
                continue;
            }
            let out = self.nodes[i].process(&ins, backend)?;
            outputs.insert(name, out);
        }
        Ok(outputs)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap()
    }

    /// Executes event descendants, using latest values from other inputs as context.
    /// Use a fresh runtime after changing the graph or restarting event capture.
    pub fn run_events(
        &mut self,
        events: &[NodeEvent],
        runtime: &mut EventRuntime,
        backend: &mut dyn Backend,
    ) -> Result<HashMap<String, Option<Value>>, String> {
        if events.is_empty() {
            return Ok(HashMap::new());
        }
        let order = self.order()?;
        let mut outputs = HashMap::new();
        for event in events {
            if !self.nodes.iter().any(|node| node.name == event.source) {
                return Err(format!("Unknown event source: {}", event.source));
            }
            if outputs.insert(event.source.clone(), Some(event.value.clone())).is_some() {
                return Err(format!("Duplicate event source: {}", event.source));
            }
        }
        if !runtime.initialized {
            for node in &mut self.nodes {
                node.reset_input_state();
            }
            runtime.initialized = true;
        }
        let mut affected: HashSet<String> = outputs.keys().cloned().collect();
        for &index in &order {
            let name = &self.nodes[index].name;
            if self.edges.iter().any(|edge| edge.target == *name && affected.contains(&edge.source)) {
                affected.insert(name.clone());
            }
        }
        let mut needed = affected.clone();
        for &index in order.iter().rev() {
            let name = &self.nodes[index].name;
            if needed.contains(name) {
                needed.extend(self.edges.iter().filter(|edge| edge.target == *name)
                    .map(|edge| edge.source.clone()));
            }
        }
        for event in events {
            runtime.values.insert(event.source.clone(), event.value.clone());
        }
        for index in order {
            let name = self.nodes[index].name.clone();
            if outputs.contains_key(&name) || !needed.contains(&name) {
                continue;
            }
            let parents: Vec<&Edge> = self.edges.iter().filter(|edge| edge.target == name).collect();
            let triggered = parents.iter().any(|edge| {
                outputs.get(&edge.source).is_some_and(Option::is_some)
            });
            if !triggered && affected.contains(&name) {
                continue;
            }
            if !triggered && matches!(self.nodes[index].kind,
                Kind::Ocr { .. } | Kind::Keyboard { .. } | Kind::Click { .. })
            {
                continue;
            }
            let value_for = |edge: &Edge| {
                outputs.get(&edge.source).cloned()
                    .unwrap_or_else(|| runtime.values.get(&edge.source).cloned())
            };
            let out = if matches!(self.nodes[index].kind, Kind::CompareInputs { .. }) {
                let mut inputs = [None, None];
                for edge in &parents {
                    let slot = inputs.get_mut(edge.input).ok_or("Unknown comparison input port")?;
                    *slot = value_for(edge);
                }
                if inputs.iter().any(Option::is_some) {
                    self.nodes[index].process_slots(&inputs, backend)?
                } else {
                    None
                }
            } else {
                let inputs: Vec<_> = parents.iter().filter_map(|edge| value_for(edge)).collect();
                if !parents.is_empty() && inputs.is_empty() {
                    None
                } else {
                    self.nodes[index].process(&inputs, backend)?
                }
            };
            if let Some(value) = &out {
                runtime.values.insert(name.clone(), value.clone());
            }
            if triggered {
                outputs.insert(name, out);
            }
        }
        Ok(outputs)
    }

    pub fn from_json(s: &str) -> Result<Graph, String> {
        let graph: Graph = serde_json::from_str(s).map_err(|e| e.to_string())?;
        let mut occupied = HashSet::new();
        for edge in &graph.edges {
            if !graph.nodes.iter().any(|node| node.name == edge.source) {
                return Err(format!("unknown node: {}", edge.source));
            }
            let node = graph.nodes.iter().find(|node| node.name == edge.target)
                .ok_or_else(|| format!("unknown node: {}", edge.target))?;
            let pair = matches!(node.kind, crate::node::Kind::CompareInputs { .. });
            if edge.input >= if pair { 2 } else { 1 } {
                return Err("unknown input port".into());
            }
            if pair && !occupied.insert((&edge.target, edge.input)) {
                return Err("comparison input has more than one connection".into());
            }
        }
        Ok(graph)
    }
}
