use screen_automation::backend::{Backend, KeyAction, Region};
use screen_automation::event_sources::EventSource;
use screen_automation::graph::{Edge, EventRuntime, Graph, NodeEvent};
use screen_automation::node::{CompareOp, Format, Kind, Node, TextMatch, Value};
use screen_automation::preview::PreviewSession;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

struct Screen {
    text: String,
    script: VecDeque<String>,
    now: Instant,
    ocr_ms: u64,
    captures: usize,
    actions: Vec<String>,
}

impl Screen {
    fn new(text: &str) -> Self {
        Self { text: text.into(), script: VecDeque::new(), now: Instant::now(), ocr_ms: 0, captures: 0, actions: Vec::new() }
    }

    fn advance(&mut self, ms: u64) {
        self.now += Duration::from_millis(ms);
    }
}

impl Backend for Screen {
    fn capture(&mut self, _: &Region) -> Result<Vec<u8>, String> {
        self.captures += 1;
        Ok(self.script.pop_front().unwrap_or_else(|| self.text.clone()).into_bytes())
    }
    fn ocr(&mut self, _: &Region, pixels: &[u8]) -> Result<String, String> {
        self.advance(self.ocr_ms);
        String::from_utf8(pixels.to_vec()).map_err(|error| error.to_string())
    }
    fn ocr_single_line(&mut self, region: &Region, pixels: &[u8]) -> Result<String, String> {
        self.ocr(region, pixels)
    }
    fn click(&mut self, _: i32, _: i32, _: &str) -> Result<(), String> {
        self.actions.push("click".into());
        Ok(())
    }
    fn type_text(&mut self, text: &str) -> Result<(), String> {
        self.actions.push(format!("type {text}"));
        Ok(())
    }
    fn press_key(&mut self, key: KeyAction) -> Result<(), String> {
        self.actions.push(format!("key {key:?}"));
        Ok(())
    }
    fn now(&self) -> Instant { self.now }
}

fn wait_node(name: &str) -> Node {
    Node::new(name, Kind::WaitForText {
        expected: "Ready".into(), matching: TextMatch::Exact, case_sensitive: false,
        single_line: false, timeout_ms: 1000, poll_ms: 100, pending: None,
    })
}

fn guarded_wait_graph() -> Graph {
    let mut graph = Graph::default();
    for node in [
        constant("source", "start"),
        Node::new("once", Kind::Once { armed: true, fired: false }),
        Node::new("open", Kind::Click { button: "left".into() }),
        wait_node("wait"),
        Node::new("submit", Kind::Click { button: "left".into() }),
    ] { graph.add_node(node).unwrap(); }
    for (source, target) in [("source", "once"), ("once", "open"), ("open", "wait"), ("wait", "submit")] {
        graph.connect(source, target).unwrap();
    }
    graph
}

#[test]
fn wait_polls_without_blocking_and_keeps_the_original_trigger() {
    let mut node = wait_node("wait");
    let mut screen = Screen::new("Loading");
    assert_eq!(node.process(&[Value::Int(7)], &mut screen).unwrap(), None);
    assert!(node.is_waiting());
    screen.advance(99);
    assert_eq!(node.process(&[Value::Int(99)], &mut screen).unwrap(), None);
    assert_eq!(screen.captures, 1);
    screen.advance(1);
    screen.text = "  READY\n".into();
    assert_eq!(node.process(&[], &mut screen).unwrap(), Some(Value::Int(7)));
    assert!(!node.is_waiting());
    assert_eq!(screen.captures, 2);
}

#[test]
fn wait_continues_after_upstream_stops_and_never_replays_previous_actions() {
    let mut graph = guarded_wait_graph();
    let mut screen = Screen::new("Loading");
    graph.run_once(&mut screen).unwrap();
    assert_eq!(screen.actions, ["click"]);
    screen.advance(100);
    screen.text = "Ready".into();
    assert_eq!(graph.run_once(&mut screen).unwrap()["submit"], Some(Value::Bool(true)));
    graph.run_once(&mut screen).unwrap();
    assert_eq!(screen.actions, ["click", "click"]);
    assert!(!graph.has_pending_waits());
}

#[test]
fn event_wait_resumes_without_another_source_event_or_unrelated_actions() {
    let mut graph = guarded_wait_graph();
    graph.add_node(constant("other_source", "unrelated")).unwrap();
    graph.add_node(Node::new("unrelated", Kind::Click { button: "left".into() })).unwrap();
    graph.connect("other_source", "unrelated").unwrap();
    let mut screen = Screen::new("Loading");
    let mut runtime = EventRuntime::default();
    graph.run_events(&[NodeEvent { source: "source".into(), value: Value::Text("go".into()) }],
        &mut runtime, &mut screen).unwrap();
    screen.advance(100);
    screen.text = "Ready".into();
    let result = graph.run_events(&[], &mut runtime, &mut screen).unwrap();
    assert_eq!(result["submit"], Some(Value::Bool(true)));
    assert!(!result.contains_key("unrelated"));
    assert_eq!(screen.actions, ["click", "click"]);
}

#[test]
fn timeout_and_stop_cancel_waits_without_rearming_once_gate() {
    let mut graph = guarded_wait_graph();
    let mut screen = Screen::new("Loading");
    graph.run_once(&mut screen).unwrap();
    screen.advance(1000);
    let error = graph.run_once(&mut screen).unwrap_err();
    assert!(error.contains("wait: timed out"), "{error}");
    assert!(error.contains("order history"));
    assert!(!graph.has_pending_waits());
    graph.cancel_waits();
    screen.text = "Ready".into();
    graph.run_once(&mut screen).unwrap();
    assert_eq!(screen.actions, ["click"]);

    graph.reset_runtime();
    screen.text = "Loading".into();
    graph.run_once(&mut screen).unwrap();
    assert!(graph.has_pending_waits());
    graph.cancel_waits();
    screen.text = "Ready".into();
    graph.run_once(&mut screen).unwrap();
    assert!(!graph.has_pending_waits());
    assert_eq!(screen.actions, ["click", "click"]);
}

#[test]
fn slow_ocr_cannot_submit_after_the_wait_deadline() {
    for ocr_ms in [999, 1000, 1001] {
        let mut graph = guarded_wait_graph();
        let mut screen = Screen::new("Ready");
        screen.ocr_ms = ocr_ms;
        let result = graph.run_once(&mut screen);
        if ocr_ms < 1000 {
            result.unwrap();
            assert_eq!(screen.actions, ["click", "click"]);
        } else {
            assert!(result.unwrap_err().contains("timed out"));
            assert_eq!(screen.actions, ["click"]);
            assert!(!graph.has_pending_waits());
            screen.ocr_ms = 0;
            graph.run_once(&mut screen).unwrap();
            assert_eq!(screen.actions, ["click"]);
        }
    }
}

#[test]
fn screen_assertions_fail_closed_and_validate_patterns() {
    let mut graph = Graph::default();
    graph.add_node(constant("source", "go")).unwrap();
    graph.add_node(Node::new("check", Kind::AssertText {
        expected: "^Buy 1 NVDA$".into(), matching: TextMatch::Regex, case_sensitive: true, single_line: false,
    })).unwrap();
    graph.add_node(Node::new("submit", Kind::Click { button: "left".into() })).unwrap();
    graph.connect("source", "check").unwrap();
    graph.connect("check", "submit").unwrap();
    let mut screen = Screen::new("Sell 1 NVDA");
    assert!(graph.run_once(&mut screen).unwrap_err().contains("screen assertion failed"));
    assert!(screen.actions.is_empty());
    screen.text = "Buy\n1  NVDA".into();
    graph.run_once(&mut screen).unwrap();
    assert_eq!(screen.actions, ["click"]);
    if let Kind::AssertText { expected, .. } = &mut graph.nodes[1].kind { *expected = "[".into(); }
    assert!(graph.validate().iter().any(|issue| issue.message.contains("regex")));
}

#[test]
fn field_replacement_and_keypress_are_ordered_and_dry_run_never_sends_input() {
    let mut graph = Graph::default();
    graph.add_node(constant("source", "1")).unwrap();
    graph.add_node(Node::new("field", Kind::SetField { text: String::new() })).unwrap();
    graph.add_node(Node::new("tab", Kind::Keypress { key: KeyAction::Tab })).unwrap();
    graph.connect("source", "field").unwrap();
    graph.connect("field", "tab").unwrap();
    let mut screen = Screen::new("");
    let mut preview = PreviewSession::new(&graph).unwrap();
    let mut actions = Vec::new();
    while let Some(step) = preview.step(&mut screen).unwrap() { actions.extend(step.actions); }
    assert_eq!(actions, ["click left at (250, 160)", "keypress Select all", "type 1", "keypress Tab"]);
    assert!(screen.actions.is_empty());
    graph.run_once(&mut screen).unwrap();
    assert_eq!(screen.actions, ["click", "key SelectAll", "type 1", "key Tab"]);

    let mut unsupported = Fake::default();
    assert!(graph.run_once(&mut unsupported).unwrap_err().contains("not supported"));
    assert_eq!(unsupported.typed, 0);
}

#[test]
fn preview_pauses_at_wait_instead_of_skipping_a_screen_transition() {
    let graph = guarded_wait_graph();
    let mut preview = PreviewSession::new(&graph).unwrap();
    let mut screen = Screen::new("Loading");
    for _ in 0..4 { preview.step(&mut screen).unwrap().unwrap(); }
    assert!(preview.is_waiting());
    screen.advance(100);
    screen.text = "Ready".into();
    assert_eq!(preview.step(&mut screen).unwrap().unwrap().node, "wait");
    assert_eq!(preview.step(&mut screen).unwrap().unwrap().node, "submit");
    assert!(preview.is_finished());
    assert!(screen.actions.is_empty());
}

#[test]
fn waits_reject_invalid_timing_and_nested_use_and_latches_do_not_serialize() {
    let mut graph = guarded_wait_graph();
    let mut screen = Screen::new("Ready");
    graph.run_once(&mut screen).unwrap();
    assert!(!graph.to_json().contains("\"fired\""));
    assert!(!graph.to_json().contains("\"pending\""));
    let mut loaded = Graph::from_json(&graph.to_json()).unwrap();
    loaded.run_once(&mut screen).unwrap();
    assert_eq!(screen.actions.len(), 4);
    if let Kind::WaitForText { poll_ms, .. } = &mut graph.nodes[3].kind { *poll_ms = 0; }
    assert!(graph.validate().iter().any(|issue| issue.message.contains("poll interval")));
    let mut outer = Graph::default();
    graph.add_node(Node::new("out", Kind::SubflowOutput)).unwrap();
    graph.connect("submit", "out").unwrap();
    outer.add_node(Node::new("nested", Kind::Subflow { graph: Box::new(graph) })).unwrap();
    assert!(outer.validate().iter().any(|issue| issue.message.contains("top-level")));
}

#[test]
fn paper_trading_fixture_is_disarmed_and_runs_one_verified_order_when_configured() {
    for (json, summary, final_action) in [
        (include_str!("trading-buy.json"),
            "Paper order #1. Buy 1 NVDA filled at $166.00 per share. Total cost: $166.00.", "click"),
        (include_str!("trading-buy.calibrated.json"),
            "Paper order #1. Buy 1 NVDA filled at $ 166.00 per share.", "key Enter"),
    ] {
        let mut graph = Graph::from_json(json).unwrap();
        assert!(graph.validate().is_empty(), "{:?}", graph.validate());
        let mut screen = Screen::new("166.00");
        graph.run_once(&mut screen).unwrap();
        assert!(screen.actions.is_empty());
        assert!(matches!(graph.nodes.iter().find(|node| node.name == "one_order").unwrap().kind,
            Kind::Once { armed: false, fired: false }));
        graph.reset_runtime();
        if let Kind::Once { armed, .. } = &mut graph.nodes.iter_mut().find(|node| node.name == "one_order").unwrap().kind {
            *armed = true;
        }
        screen.script = [
            "166.00", "PAPER TRADING", "0", "Review buy order", "1",
            "Review buy order", "NVIDIA (NVDA)", "Buy", "1 shares", "Market", "$166.00",
            "Order filled", summary, "1",
        ].into_iter().map(String::from).collect();
        let output = graph.run_once(&mut screen).unwrap();
        assert!(output["position_updated"].is_some(), "{output:?}; actions: {:?}", screen.actions);
        assert_eq!(screen.actions, ["click", "click", "key SelectAll", "type 1", "click", "click", final_action]);
        assert!(screen.script.is_empty());
        graph.run_once(&mut screen).unwrap();
        assert_eq!(screen.actions.len(), 7);
    }
}

#[test]
fn paper_round_trip_fixture_verifies_both_sides_and_cannot_repeat() {
    for sell_side in ["Sell", "Buy"] {
        let mut graph = Graph::from_json(include_str!("trading-buy-sell.calibrated.json")).unwrap();
        assert!(graph.validate().is_empty(), "{:?}", graph.validate());
        let mut screen = Screen::new("166.00");
        graph.run_once(&mut screen).unwrap();
        assert!(screen.actions.is_empty());
        graph.reset_runtime();
        if let Kind::Once { armed, .. } = &mut graph.nodes.iter_mut().find(|node| node.name == "one_order").unwrap().kind {
            *armed = true;
        }
        screen.script = [
            "166.00", "PAPER TRADING", "0", "Review buy order", "1",
            "Review buy order", "NVIDIA (NVDA)", "Buy", "1 shares", "Market", "$ 166.00",
            "Order filled", "Paper order #1. Buy 1 NVDA filled at $ 166.00 per share.", "1",
            "Review sell order", "1", "Review sell order", "NVIDIA (NVDA)", sell_side,
            "1 shares", "Market", "$166.00", "Order filled",
            "Paper order #2. Sell 1 NVDA filled at $ 166.00 per share.", "0",
        ].into_iter().map(String::from).collect();
        let result = graph.run_once(&mut screen);
        let leg = ["click", "click", "key SelectAll", "type 1", "click", "click", "key Enter"];
        if sell_side == "Sell" {
            let output = result.unwrap();
            assert!(output["position_updated"].is_some() && output["flat_position"].is_some(), "{output:?}");
            assert_eq!(screen.actions, leg.repeat(2));
            assert!(screen.script.is_empty());
            graph.run_once(&mut screen).unwrap();
            assert_eq!(screen.actions.len(), 14);
        } else {
            assert!(result.unwrap_err().contains("sell_check_side"));
            assert_eq!(screen.actions, [leg.as_slice(), &leg[..5]].concat());
        }
    }
}

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
