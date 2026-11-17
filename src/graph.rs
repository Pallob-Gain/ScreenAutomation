use crate::backend::Backend;
use crate::node::{InputParameter, Kind, Node, Value};
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub settings: GraphSettings,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphIssue {
    pub node: Option<String>,
    pub message: String,
}

pub(crate) const MAX_SUBFLOW_DEPTH: usize = 16;

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

fn input_slots(
    node: &Node,
    parents: &[&Edge],
    value_for: impl Fn(&Edge) -> Option<Value>,
) -> Result<Vec<Option<Value>>, String> {
    let mut slots = vec![None; node.input_count()];
    let mut occupied = vec![false; slots.len()];
    for edge in parents {
        let slot = slots.get_mut(edge.input)
            .ok_or_else(|| format!("{}: unknown input port", node.name))?;
        if std::mem::replace(&mut occupied[edge.input], true) {
            return Err(format!("{}: input '{}' has multiple connections",
                node.name, node.input_label(edge.input)));
        }
        *slot = value_for(edge);
    }
    Ok(slots)
}

impl Graph {
    pub fn validate(&self) -> Vec<GraphIssue> {
        let mut issues = Vec::new();
        self.validate_inner(0, false, &mut issues);
        issues
    }

    fn validate_inner(&self, depth: usize, subflow: bool, issues: &mut Vec<GraphIssue>) {
        if depth > MAX_SUBFLOW_DEPTH {
            issues.push(GraphIssue { node: None, message: "maximum subflow depth exceeded".into() });
            return;
        }
        let mut names = HashSet::new();
        for node in &self.nodes {
            let mut add = |message: String| issues.push(GraphIssue { node: Some(node.name.clone()), message });
            if node.name.trim().is_empty() {
                add("node name is empty".into());
            } else if !names.insert(&node.name) {
                add("duplicate node name".into());
            }
            if matches!(node.kind, Kind::Ocr { .. } | Kind::Keyboard { .. } | Kind::Click { .. })
                && node.region.map_or(true, |r| r.width == 0 || r.height == 0) {
                add("region is missing or has zero size".into());
            }
            if let Err(error) = node.validate_input_parameters() {
                add(error);
            }
            let fixed = |parameter| node.parameter_port(parameter).is_none();
            match &node.kind {
                Kind::Threshold { lower, upper, trigger, .. } | Kind::Hysteresis { lower, upper, trigger, .. } => {
                    use InputParameter::{Lower, Upper};
                    if (fixed(Lower) && !lower.is_finite()) || (fixed(Upper) && !upper.is_finite())
                        || (fixed(Lower) && fixed(Upper) && (lower > upper
                            || (lower == upper && matches!(trigger, crate::node::Crossing::EnteringRange)))) {
                        add("invalid or impossible threshold range".into());
                    }
                    if matches!(node.kind, Kind::Hysteresis { .. }) && trigger.is_range() {
                        add("range-entry trigger is not supported by hysteresis".into());
                    }
                }
                Kind::ValidateNumber { minimum, maximum, max_change, pattern, .. } => {
                    use InputParameter::{Minimum, Maximum, MaxChange};
                    if (fixed(Minimum) && minimum.is_some_and(|v| !v.is_finite()))
                        || (fixed(Maximum) && maximum.is_some_and(|v| !v.is_finite()))
                        || (fixed(MaxChange) && max_change.is_some_and(|v| !v.is_finite() || v < 0.0))
                        || (fixed(Minimum) && fixed(Maximum)
                            && matches!((minimum, maximum), (Some(min), Some(max)) if min > max)) {
                        add("invalid numeric range or maximum change".into());
                    }
                    if !pattern.is_empty() {
                        if let Err(error) = regex::Regex::new(pattern) {
                            add(format!("invalid regex: {error}"));
                        }
                    }
                }
                Kind::Event { event } => {
                    if let Err(error) = event.validate() {
                        add(format!("invalid event: {error}"));
                    }
                    if subflow {
                        add("event sources are not supported inside subflows".into());
                    }
                }
                Kind::Subflow { graph } => {
                    let mut nested = Vec::new();
                    graph.validate_inner(depth + 1, true, &mut nested);
                    for issue in nested {
                        add(match issue.node {
                            Some(name) => format!("subflow '{name}': {}", issue.message),
                            None => format!("subflow: {}", issue.message),
                        });
                    }
                }
                _ => {}
            }
            let optional_input = matches!(node.kind, Kind::Function { .. })
                || matches!(&node.kind, Kind::Subflow { graph } if !graph.uses_subflow_input());
            if !optional_input {
                for port in 0..node.input_count() {
                    if !self.edges.iter().any(|edge| edge.target == node.name && edge.input == port) {
                        add(format!("required input port {port} is not connected"));
                    }
                }
            }
        }
        if subflow {
            if self.nodes.iter().filter(|n| matches!(n.kind, Kind::SubflowInput)).count() > 1 {
                issues.push(GraphIssue { node: None, message: "subflow may have at most one input node".into() });
            }
            if self.nodes.iter().filter(|n| matches!(n.kind, Kind::SubflowOutput)).count() != 1 {
                issues.push(GraphIssue { node: None, message: "subflow must have exactly one output node".into() });
            }
        }
        let mut occupied = HashSet::new();
        for edge in &self.edges {
            let source = self.nodes.iter().find(|n| n.name == edge.source);
            let target = self.nodes.iter().find(|n| n.name == edge.target);
            if source.is_none() {
                issues.push(GraphIssue { node: Some(edge.source.clone()), message: "edge source is missing".into() });
            }
            if target.is_none() {
                issues.push(GraphIssue { node: Some(edge.target.clone()), message: "edge target is missing".into() });
            }
            if source.is_some_and(|n| n.output_count() == 0) {
                issues.push(GraphIssue { node: Some(edge.source.clone()), message: "node has no output port".into() });
            }
            if target.is_some_and(|n| edge.input >= n.input_count()) {
                issues.push(GraphIssue { node: Some(edge.target.clone()), message: "unknown input port".into() });
            }
            if target.is_some_and(Node::has_single_connection_ports)
                && !occupied.insert((&edge.target, edge.input)) {
                issues.push(GraphIssue { node: Some(edge.target.clone()), message: format!("input port {} has multiple connections", edge.input) });
            }
        }
        if let Err(error) = self.order() {
            if error.contains("cycle") {
                issues.push(GraphIssue { node: None, message: error });
            }
        }
    }

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

    pub fn set_parameter_input(
        &mut self, node: &str, parameter: InputParameter, enabled: bool,
    ) -> Result<(), String> {
        let target = self.nodes.iter_mut().find(|n| n.name == node).ok_or("unknown node")?;
        let available = target.available_input_parameters();
        if !available.contains(&parameter) {
            return Err(format!("Unsupported input parameter '{}'", parameter.label()));
        }
        target.validate_input_parameters()?;
        let previous = &target.input_parameters;
        let next: Vec<_> = available.iter().copied()
            .filter(|&p| if p == parameter { enabled } else { previous.contains(&p) })
            .collect();
        if *previous == next {
            return Ok(());
        }
        if !next.is_empty() && self.edges.iter()
            .filter(|edge| edge.target == node && edge.input == 0).take(2).count() > 1 {
            return Err("Main input has multiple connections".into());
        }
        self.edges.retain_mut(|edge| {
            if edge.target != node || edge.input == 0 {
                return true;
            }
            let Some(port) = previous.get(edge.input - 1)
                .and_then(|p| next.iter().position(|q| q == p)) else { return false };
            edge.input = port + 1;
            true
        });
        target.input_parameters = next;
        target.reset_runtime();
        Ok(())
    }

    pub fn connect_input(&mut self, src: &str, dst: &str, input: usize) -> Result<(), String> {
        if !self.nodes.iter().any(|n| n.name == src) {
            return Err("unknown node".into());
        }
        let destination = self.nodes.iter().position(|node| node.name == dst)
            .ok_or("unknown node")?;
        let target = &self.nodes[destination];
        // Keep accepting legacy port-zero edges to source nodes; validation flags them.
        if input >= target.input_count().max(1) {
            return Err("unknown input port".into());
        }
        if target.has_single_connection_ports()
            && self.edges.iter().any(|edge| edge.target == dst && edge.input == input) {
            return Err(format!("Input {} already has a connection", target.input_label(input)));
        }
        let e = Edge { source: src.into(), target: dst.into(), input };
        if !self.edges.contains(&e) {
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

    pub(crate) fn order(&self) -> Result<Vec<usize>, String> {
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
        self.run_with_input(backend, None, 0)
    }

    pub fn reset_runtime(&mut self) {
        for node in &mut self.nodes {
            node.reset_runtime();
        }
    }

    pub fn uses_subflow_input(&self) -> bool {
        self.nodes.iter().filter(|node| matches!(node.kind, Kind::SubflowInput))
            .any(|node| self.edges.iter().any(|edge| edge.source == node.name))
    }

    pub(crate) fn run_subflow(&mut self, input: Option<Value>, backend: &mut dyn Backend, depth: usize) -> Result<Option<Value>, String> {
        if depth > MAX_SUBFLOW_DEPTH {
            return Err("maximum subflow depth exceeded".into());
        }
        let output = self.nodes.iter().find(|node| matches!(node.kind, Kind::SubflowOutput))
            .map(|node| node.name.clone()).ok_or("subflow has no output node")?;
        if self.nodes.iter().filter(|node| matches!(node.kind, Kind::SubflowInput)).count() > 1
            || self.nodes.iter().filter(|node| matches!(node.kind, Kind::SubflowOutput)).count() != 1 {
            return Err("subflow must have at most one input and exactly one output node".into());
        }
        let outputs = self.run_with_input(backend, input, depth)?;
        Ok(outputs.get(&output).cloned().flatten())
    }

    fn run_with_input(&mut self, backend: &mut dyn Backend, injected: Option<Value>, depth: usize) -> Result<HashMap<String, Option<Value>>, String> {
        let mut outputs: HashMap<String, Option<Value>> = HashMap::new();
        for i in self.order()? {
            self.execute_node(i, &mut outputs, backend, injected.as_ref(), depth)?;
        }
        Ok(outputs)
    }

    pub(crate) fn execute_node(
        &mut self, i: usize, outputs: &mut HashMap<String, Option<Value>>,
        backend: &mut dyn Backend, injected: Option<&Value>, depth: usize,
    ) -> Result<(Vec<Value>, Option<Value>), String> {
        let name = self.nodes[i].name.clone();
        let parents: Vec<&Edge> = self.edges.iter().filter(|edge| edge.target == name).collect();
        let value_for = |edge: &Edge| outputs.get(&edge.source).cloned().flatten();
        let slots = if self.nodes[i].has_single_connection_ports() {
            Some(input_slots(&self.nodes[i], &parents, value_for)?)
        } else {
            None
        };
        let inputs: Vec<Value> = match &slots {
            Some(slots) => slots.iter().flatten().cloned().collect(),
            None => parents.iter().filter_map(|edge| value_for(edge)).collect(),
        };
        let result = if matches!(self.nodes[i].kind, Kind::SubflowInput) {
            injected.cloned()
        } else if let Some(slots) = slots {
            if !inputs.is_empty() {
                self.nodes[i].process_slots(&slots, true, backend, depth).map_err(|e| format!("{name}: {e}"))?
            } else { None }
        } else if !parents.is_empty() && inputs.is_empty() {
            None
        } else {
            self.nodes[i].process_with_depth(&inputs, backend, depth).map_err(|e| format!("{name}: {e}"))?
        };
        outputs.insert(name, result.clone());
        Ok((inputs, result))
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
        let mut context_outputs = HashMap::new();
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
                Kind::Ocr { .. } | Kind::Keyboard { .. } | Kind::Click { .. } | Kind::Subflow { .. })
            {
                continue;
            }
            let value_for = |edge: &Edge| {
                outputs.get(&edge.source).cloned()
                    .unwrap_or_else(|| runtime.values.get(&edge.source).cloned())
            };
            let out = if self.nodes[index].has_single_connection_ports() {
                let node = &self.nodes[index];
                let dynamic = !node.input_parameters.is_empty();
                // Only events/propagated outputs are fresh main samples. Context can
                // seed a main input and supply settings, but cannot replay a reading.
                let main_is_fresh = parents.iter().any(|edge| edge.input == 0
                    && outputs.get(&edge.source).is_some_and(Option::is_some));
                let inputs = input_slots(node, &parents, |edge| {
                    if dynamic {
                        outputs.get(&edge.source).or_else(|| context_outputs.get(&edge.source))
                            .cloned().flatten()
                    } else {
                        value_for(edge)
                    }
                })?;
                if inputs.iter().any(Option::is_some) {
                    self.nodes[index].process_slots(&inputs, main_is_fresh, backend, 0)?
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
            } else {
                context_outputs.insert(name, out);
            }
        }
        Ok(outputs)
    }

    pub fn from_json(s: &str) -> Result<Graph, String> {
        let graph: Graph = serde_json::from_str(s).map_err(|e| e.to_string())?;
        graph.validate_saved_routing()?;
        Ok(graph)
    }

    fn validate_saved_routing(&self) -> Result<(), String> {
        for node in &self.nodes {
            node.validate_input_parameters().map_err(|error| format!("{}: {error}", node.name))?;
            if let Kind::Subflow { graph } = &node.kind {
                graph.validate_saved_routing()?;
            }
        }
        let mut occupied = HashSet::new();
        for edge in &self.edges {
            if !self.nodes.iter().any(|node| node.name == edge.source) {
                return Err(format!("unknown node: {}", edge.source));
            }
            let node = self.nodes.iter().find(|node| node.name == edge.target)
                .ok_or_else(|| format!("unknown node: {}", edge.target))?;
            if edge.input >= node.input_count().max(1) {
                return Err("unknown input port".into());
            }
            if node.has_single_connection_ports() && !occupied.insert((&edge.target, edge.input)) {
                return Err(format!("{}: input '{}' has more than one connection",
                    node.name, node.input_label(edge.input)));
            }
        }
        Ok(())
    }
}
