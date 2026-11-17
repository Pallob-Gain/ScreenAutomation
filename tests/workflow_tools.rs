use screen_automation::backend::{Backend, Region};
use screen_automation::event_sources::EventSource;
use screen_automation::graph::{Edge, EventRuntime, Graph, NodeEvent};
use screen_automation::node::{CompareOp, Format, Kind, Node, Value};
use screen_automation::preview::PreviewSession;

#[derive(Default)]
struct Fake {
    clicks: usize,
    typed: usize,
}

impl Backend for Fake {
    fn capture(&mut self, _: &Region) -> Result<Vec<u8>, String> { Ok(b"12".to_vec()) }
    fn ocr(&mut self, _: &Region, pixels: &[u8]) -> Result<String, String> {
        Ok(String::from_utf8_lossy(pixels).into())
    }
    fn click(&mut self, _: i32, _: i32, _: &str) -> Result<(), String> {
        self.clicks += 1;
        Ok(())
    }
    fn type_text(&mut self, _: &str) -> Result<(), String> {
        self.typed += 1;
        Ok(())
    }
}

fn constant(name: &str, value: &str) -> Node {
    Node::new(name, Kind::Constant { value: value.into(), format: Format::Text })
}

fn validator() -> Node {
    Node::new("validator", Kind::ValidateNumber {
        minimum: Some(0.0), maximum: Some(100.0), max_change: Some(2.0),
        pattern: r"^\d+(\.\d+)?$".into(), last: None,
    })
}

#[test]
fn validate_number_is_strict_and_tracks_only_accepted_samples() {
    let mut node = validator();
    let mut backend = Fake::default();
    for (input, expected) in [
        ("12.5", true), ("12,5", false), ("NaN", false), ("inf", false),
        ("13.0", true), ("19", false), ("14.5", true), ("101", false),
        ("-1", false), ("abc", false),
    ] {
        let value = Value::Text(input.into());
        assert_eq!(node.process(&[value.clone()], &mut backend).unwrap(),
            expected.then_some(value), "{input}");
    }
    assert_eq!(node.process(&[Value::Num(f64::INFINITY)], &mut backend).unwrap(), None);
    assert_eq!(node.process(&[Value::Bool(true)], &mut backend).unwrap(), None);
    let mut copy = node.duplicate("copy");
    assert_eq!(copy.process(&[Value::Text("30".into())], &mut backend).unwrap(), Some(Value::Text("30".into())));
}

#[test]
fn legacy_group_and_validation_issues() {
    let mut graph = Graph::default();
    graph.add_node(constant("source", "1")).unwrap();
    graph.add_node(validator()).unwrap();
    graph.connect("source", "validator").unwrap();
    let json = graph.to_json();
    assert!(!json.contains("\"group\""));
    let mut loaded = Graph::from_json(&json).unwrap();
    assert!(loaded.validate().is_empty());
    loaded.nodes[0].group = "prices".into();
    assert_eq!(Graph::from_json(&loaded.to_json()).unwrap().nodes[0].group, "prices");
    let Kind::ValidateNumber { minimum, maximum, pattern, .. } = &mut loaded.nodes[1].kind else { unreachable!() };
    *minimum = Some(10.0);
    *maximum = Some(1.0);
    *pattern = "[".into();
    let issues = loaded.validate();
    assert!(issues.iter().any(|issue| issue.node.as_deref() == Some("validator") && issue.message.contains("range")));
    assert!(issues.iter().any(|issue| issue.message.contains("regex")));
}

fn flow_with_keyboard() -> Graph {
    let mut graph = Graph::default();
    graph.add_node(Node::new("in", Kind::SubflowInput)).unwrap();
    graph.add_node(Node::new("keyboard", Kind::Keyboard { text: String::new() })).unwrap();
    graph.add_node(Node::new("out", Kind::SubflowOutput)).unwrap();
    graph.connect("in", "keyboard").unwrap();
    graph.connect("keyboard", "out").unwrap();
    graph
}

#[test]
fn nested_subflows_preview_records_actions_without_touching_backend() {
    let mut inner = Graph::default();
    inner.add_node(Node::new("in", Kind::SubflowInput)).unwrap();
    inner.add_node(Node::new("nested", Kind::Subflow { graph: Box::new(flow_with_keyboard()) })).unwrap();
    inner.add_node(Node::new("out", Kind::SubflowOutput)).unwrap();
    inner.connect("in", "nested").unwrap();
    inner.connect("nested", "out").unwrap();
    let mut graph = Graph::default();
    graph.add_node(constant("source", "hello")).unwrap();
    graph.add_node(Node::new("flow", Kind::Subflow { graph: Box::new(inner) })).unwrap();
    graph.connect("source", "flow").unwrap();
    assert!(graph.validate().is_empty(), "{:?}", graph.validate());
    let mut session = PreviewSession::new(&graph).unwrap();
    let mut backend = Fake::default();
    assert_eq!(session.step(&mut backend).unwrap().unwrap().node, "source");
    let step = session.step(&mut backend).unwrap().unwrap();
    assert_eq!(step.node, "flow");
    assert_eq!(step.output, Some(Value::Text("hello".into())));
    assert_eq!(step.actions.len(), 2);
    assert_eq!((backend.clicks, backend.typed), (0, 0));
    assert!(session.is_finished());
    assert!(session.step(&mut backend).unwrap().is_none());
    assert_eq!(graph.run_once(&mut backend).unwrap()["flow"], Some(Value::Text("hello".into())));
    assert_eq!((backend.clicks, backend.typed), (1, 1));
}

#[test]
fn event_context_does_not_run_untriggered_subflow_actions() {
    let mut graph = Graph::default();
    graph.add_node(constant("event", "1")).unwrap();
    graph.add_node(constant("side", "2")).unwrap();
    graph.add_node(Node::new("flow", Kind::Subflow { graph: Box::new(flow_with_keyboard()) })).unwrap();
    graph.add_node(Node::new("pair", Kind::CompareInputs {
        op: CompareOp::Equal, trigger_when: true, on_change: false, last: None, values: [None, None],
    })).unwrap();
    graph.connect("side", "flow").unwrap();
    graph.connect_input("event", "pair", 0).unwrap();
    graph.connect_input("flow", "pair", 1).unwrap();
    let mut backend = Fake::default();
    graph.run_events(&[NodeEvent { source: "event".into(), value: Value::Text("1".into()) }],
        &mut EventRuntime::default(), &mut backend).unwrap();
    assert_eq!((backend.clicks, backend.typed), (0, 0));
}

#[test]
fn preview_surfaces_node_errors_and_blocks_downstream() {
    let mut graph = Graph::default();
    graph.add_node(constant("source", "12")).unwrap();
    graph.add_node(Node::new("function", Kind::Function { code: "throw \"bad\"".into() })).unwrap();
    graph.connect("source", "function").unwrap();
    let mut preview = PreviewSession::new(&graph).unwrap();
    let mut backend = Fake::default();
    preview.step(&mut backend).unwrap();
    assert!(preview.step(&mut backend).unwrap_err().contains("function"));
}

#[test]
fn event_sources_are_external_and_validate_configuration() {
    let mut graph = Graph::default();
    graph.add_node(Node::new("timer", Kind::Event { event: EventSource::Timer { interval_ms: 100 } })).unwrap();
    graph.add_node(Node::new("model", Kind::Model { format: Format::Text })).unwrap();
    graph.connect("timer", "model").unwrap();
    let mut backend = Fake::default();
    assert_eq!(graph.run_once(&mut backend).unwrap()["timer"], None);
    let result = graph.run_events(&[NodeEvent { source: "timer".into(), value: Value::Text("tick".into()) }],
        &mut EventRuntime::default(), &mut backend).unwrap();
    assert_eq!(result["model"], Some(Value::Text("tick".into())));
    let Kind::Event { event } = &mut graph.nodes[0].kind else { unreachable!() };
    *event = EventSource::Timer { interval_ms: 0 };
    assert!(graph.validate().iter().any(|issue| issue.node.as_deref() == Some("timer") && issue.message.contains("event")));
}

#[test]
fn validation_reports_graph_and_subflow_errors() {
    let mut graph = Graph::default();
    graph.add_node(Node::new("note", Kind::Note { text: "comment".into() })).unwrap();
    graph.add_node(Node::new("click", Kind::Click { button: "left".into() })).unwrap();
    graph.nodes[1].region = None;
    graph.add_node(Node::new("flow", Kind::Subflow { graph: Box::new(Graph::default()) })).unwrap();
    graph.connect("note", "click").unwrap();
    graph.connect("click", "flow").unwrap();
    graph.connect("flow", "click").unwrap();
    graph.nodes.push(constant("note", "duplicate"));
    let issues = graph.validate();
    for text in ["duplicate", "region", "output port", "subflow", "cycle"] {
        assert!(issues.iter().any(|issue| issue.message.contains(text)), "{text}: {issues:?}");
    }
}

#[test]
fn validation_accepts_empty_graph_and_reports_missing_sources_and_inputs() {
    let mut graph = Graph::default();
    assert!(graph.validate().is_empty());
    graph.add_node(constant("source", "1")).unwrap();
    graph.add_node(Node::new("sink", Kind::Model { format: Format::Text })).unwrap();
    let issues = graph.validate();
    assert!(issues.iter().any(|issue| issue.node.as_deref() == Some("sink") && issue.message.contains("required input")));
    assert!(!issues.iter().any(|issue| issue.node.as_deref() == Some("source")));
    graph.edges.push(Edge { source: "missing".into(), target: "sink".into(), input: 0 });
    assert!(graph.validate().iter().any(|issue| issue.node.as_deref() == Some("missing") && issue.message.contains("source is missing")));
}

#[test]
fn preview_routes_comparison_ports_and_resets_copied_runtime() {
    let mut graph = Graph::default();
    graph.add_node(constant("a", "10")).unwrap();
    graph.add_node(constant("b", "5")).unwrap();
    graph.add_node(Node::new("pair", Kind::CompareInputs {
        op: CompareOp::Greater, trigger_when: true, on_change: true, last: None, values: [None, None],
    })).unwrap();
    graph.connect_input("b", "pair", 1).unwrap();
    graph.connect_input("a", "pair", 0).unwrap();
    let mut backend = Fake::default();
    graph.run_once(&mut backend).unwrap();
    assert_eq!(graph.run_once(&mut backend).unwrap()["pair"], None);
    let mut preview = PreviewSession::new(&graph).unwrap();
    preview.step(&mut backend).unwrap();
    preview.step(&mut backend).unwrap();
    let result = preview.step(&mut backend).unwrap().unwrap();
    assert_eq!(result.inputs, vec![Value::Text("10".into()), Value::Text("5".into())]);
    assert_eq!(result.output, Some(Value::Bool(true)));
}

#[test]
fn source_only_subflows_need_no_outer_input_but_nested_events_are_invalid() {
    let mut inner = Graph::default();
    inner.add_node(constant("source", "hello")).unwrap();
    inner.add_node(Node::new("out", Kind::SubflowOutput)).unwrap();
    inner.connect("source", "out").unwrap();
    let mut graph = Graph::default();
    graph.add_node(Node::new("flow", Kind::Subflow { graph: Box::new(inner) })).unwrap();
    assert!(graph.validate().is_empty(), "{:?}", graph.validate());
    let mut backend = Fake::default();
    assert_eq!(graph.run_once(&mut backend).unwrap()["flow"], Some(Value::Text("hello".into())));
    assert_eq!(PreviewSession::new(&graph).unwrap().step(&mut backend).unwrap().unwrap().output,
        Some(Value::Text("hello".into())));
    let Kind::Subflow { graph: inner } = &mut graph.nodes[0].kind else { unreachable!() };
    inner.add_node(Node::new("event", Kind::Event { event: EventSource::Timer { interval_ms: 100 } })).unwrap();
    assert!(graph.validate().iter().any(|issue| issue.message.contains("not supported inside subflows")));
}

#[test]
fn subflow_runtime_resets_recursively_and_depth_is_bounded() {
    let mut inner = Graph::default();
    inner.add_node(Node::new("in", Kind::SubflowInput)).unwrap();
    inner.add_node(validator()).unwrap();
    inner.add_node(Node::new("out", Kind::SubflowOutput)).unwrap();
    inner.connect("in", "validator").unwrap();
    inner.connect("validator", "out").unwrap();
    let mut flow = Node::new("flow", Kind::Subflow { graph: Box::new(inner) });
    let mut backend = Fake::default();
    assert!(flow.process(&[Value::Int(10)], &mut backend).unwrap().is_some());
    assert_eq!(flow.process(&[Value::Int(20)], &mut backend).unwrap(), None);
    let mut duplicate = flow.duplicate("copy");
    assert_eq!(duplicate.process(&[Value::Int(20)], &mut backend).unwrap(), Some(Value::Int(20)));
    let Kind::Subflow { graph } = &mut flow.kind else { unreachable!() };
    graph.reset_runtime();
    assert_eq!(flow.process(&[Value::Int(20)], &mut backend).unwrap(), Some(Value::Int(20)));

    let mut graph = flow_with_keyboard();
    for _ in 0..18 {
        let mut outer = Graph::default();
        outer.add_node(Node::new("in", Kind::SubflowInput)).unwrap();
        outer.add_node(Node::new("nested", Kind::Subflow { graph: Box::new(graph) })).unwrap();
        outer.add_node(Node::new("out", Kind::SubflowOutput)).unwrap();
        outer.connect("in", "nested").unwrap();
        outer.connect("nested", "out").unwrap();
        graph = outer;
    }
    assert!(graph.validate().iter().any(|issue| issue.message.contains("depth")));
    let mut node = Node::new("deep", Kind::Subflow { graph: Box::new(graph) });
    assert!(node.process(&[Value::Int(1)], &mut backend).unwrap_err().contains("depth"));
}
