use crate::backend::Backend;
use crate::node::{Node, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Default, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<(String, String)>,
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
        self.nodes.retain(|n| n.name != name);
        self.edges.retain(|(a, b)| a != name && b != name);
    }

    pub fn connect(&mut self, src: &str, dst: &str) -> Result<(), String> {
        if !self.nodes.iter().any(|n| n.name == src) || !self.nodes.iter().any(|n| n.name == dst) {
            return Err("unknown node".into());
        }
        let e = (src.to_string(), dst.to_string());
        if !self.edges.contains(&e) {
            self.edges.push(e);
        }
        Ok(())
    }

    fn order(&self) -> Result<Vec<usize>, String> {
        let idx: HashMap<&str, usize> = self.nodes.iter().enumerate().map(|(i, n)| (n.name.as_str(), i)).collect();
        let mut indeg = vec![0; self.nodes.len()];
        for (_, d) in &self.edges {
            indeg[idx[d.as_str()]] += 1;
        }
        let mut queue: Vec<usize> = (0..self.nodes.len()).filter(|&i| indeg[i] == 0).collect();
        let mut order = vec![];
        while !queue.is_empty() {
            let n = queue.remove(0);
            order.push(n);
            for (s, d) in &self.edges {
                if idx[s.as_str()] == n {
                    let di = idx[d.as_str()];
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
            let parents: Vec<&String> = self.edges.iter().filter(|(_, d)| *d == name).map(|(s, _)| s).collect();
            let ins: Vec<Value> = parents.iter().filter_map(|p| outputs.get(*p).cloned().flatten()).collect();
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

    pub fn from_json(s: &str) -> Result<Graph, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
}
