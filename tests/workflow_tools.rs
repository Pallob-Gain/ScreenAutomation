use screen_automation::backend::{Backend, KeyAction, Region};
use screen_automation::event_sources::EventSource;
use screen_automation::graph::{Edge, EventRuntime, Graph, NodeEvent};
use screen_automation::node::{CompareOp, Format, Kind, Node, ScriptLanguage, TextMatch, Value};
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

fn arm_breakout(graph: &mut Graph) {
    let node = graph.nodes.iter_mut().find(|node| node.name == "workflow_armed").unwrap();
    let Kind::Constant { value, .. } = &mut node.kind else { panic!("missing arm control") };
    *value = "true".into();
    graph.reset_runtime();
}

#[test]
fn function_state_is_transactional_ephemeral_and_reset_on_script_changes() {
    let mut node = Node::new("memory", Kind::Function {
        code: "if !state.contains(\"count\") { state.count = 0; } state.count += 1; state.count".into(),
        input_names: Vec::new(),
        language: ScriptLanguage::Rhai,
    });
    let mut screen = Screen::new("");
    assert_eq!(node.process(&[], &mut screen).unwrap(), Some(Value::Int(1)));
    assert_eq!(node.process(&[], &mut screen).unwrap(), Some(Value::Int(2)));
    let saved = serde_json::to_string(&node).unwrap();
    let mut loaded: Node = serde_json::from_str(&saved).unwrap();
    assert_eq!(loaded.process(&[], &mut screen).unwrap(), Some(Value::Int(1)));
    let mut duplicate = node.duplicate("copy");
    assert_eq!(duplicate.process(&[], &mut screen).unwrap(), Some(Value::Int(1)));
    let Kind::Function { code, .. } = &mut node.kind else { unreachable!() };
    *code = "if !state.contains(\"count\") { state.count = 0; } state.count += 1; if inputs.len() > 0 { throw \"failed\"; } state.count".into();
    assert!(node.process(&[Value::Bool(true)], &mut screen).unwrap_err().contains("failed"));
    assert_eq!(node.process(&[], &mut screen).unwrap(), Some(Value::Int(1)));
    let Kind::Function { code, .. } = &mut node.kind else { unreachable!() };
    *code = "state.invalid = [1]; true".into();
    assert!(node.process(&[], &mut screen).unwrap_err().contains("must be text"));
}

fn named_function(code: &str, names: &[&str]) -> Kind {
    Kind::Function {
        code: code.into(),
        input_names: names.iter().map(|&name| name.to_owned()).collect(),
        language: ScriptLanguage::Rhai,
    }
}

#[test]
fn function_named_inputs_bind_variables_and_missing_pins_read_unit() {
    let mut graph = Graph::default();
    graph.add_node(Node::new("left", Kind::Constant { value: "2".into(), format: Format::Number })).unwrap();
    graph.add_node(Node::new("right", Kind::Constant { value: "3".into(), format: Format::Number })).unwrap();
    graph.add_node(Node::new("sum", named_function(
        r#"if right == () { "left only: " + left } else { left + right + inputs.len() }"#,
        &["left", "right"],
    ))).unwrap();
    graph.connect_input("left", "sum", 0).unwrap();
    let mut screen = Screen::new("");
    let outputs = graph.run_once(&mut screen).unwrap();
    assert_eq!(outputs["sum"], Some(Value::Text("left only: 2.0".into())));
    graph.connect_input("right", "sum", 1).unwrap();
    let outputs = graph.run_once(&mut screen).unwrap();
    assert_eq!(outputs["sum"], Some(Value::Num(7.0)));
    assert!(
        graph.connect_input("left", "sum", 1).unwrap_err().contains("right already has a connection"),
        "named pins accept one wire each",
    );
}

#[test]
fn function_named_input_changes_reset_state_and_invalid_names_fail_closed() {
    let mut screen = Screen::new("");
    let mut node = Node::new("counter", named_function(
        "if !state.contains(\"count\") { state.count = 0; } state.count += 1; state.count",
        &["value"],
    ));
    assert_eq!(node.process(&[Value::Int(5)], &mut screen).unwrap(), Some(Value::Int(1)));
    assert_eq!(node.process(&[Value::Int(5)], &mut screen).unwrap(), Some(Value::Int(2)));
    let Kind::Function { input_names, .. } = &mut node.kind else { unreachable!() };
    input_names[0] = "reading".into();
    assert_eq!(node.process(&[Value::Int(5)], &mut screen).unwrap(), Some(Value::Int(1)),
        "renaming an input must clear saved state");
    for (bad, message) in [
        ("1st", "must start with a letter"),
        ("two words", "must start with a letter"),
        ("state", "is reserved"),
        ("reading", "duplicate input name"),
    ] {
        let Kind::Function { input_names, .. } = &mut node.kind else { unreachable!() };
        input_names.truncate(1);
        input_names.push(bad.into());
        assert!(node.process(&[Value::Int(5)], &mut screen).unwrap_err().contains(message), "{bad}");
        let mut graph = Graph::default();
        graph.add_node(node.duplicate("checked")).unwrap();
        assert!(graph.validate().iter().any(|issue| issue.message.contains(message)), "{bad}");
    }
}

#[test]
fn function_input_pins_can_be_added_and_removed_with_wires_remapped() {
    let mut graph = Graph::default();
    for name in ["a", "b", "c"] {
        graph.add_node(Node::new(name, Kind::Constant { value: "1".into(), format: Format::Number })).unwrap();
    }
    graph.add_node(Node::new("fn", named_function("inputs.len()", &["first", "second", "third"]))).unwrap();
    graph.connect_input("a", "fn", 0).unwrap();
    graph.connect_input("b", "fn", 1).unwrap();
    graph.connect_input("c", "fn", 2).unwrap();
    graph.remove_function_input("fn", 1).unwrap();
    assert_eq!(graph.nodes[3].named_function_inputs(), ["first", "third"]);
    let mut wires: Vec<(String, usize)> = graph.edges.iter()
        .map(|edge| (edge.source.clone(), edge.input)).collect();
    wires.sort();
    assert_eq!(wires, [("a".into(), 0), ("c".into(), 1)], "the removed pin's wire is dropped and later wires move up");
    assert!(graph.validate().is_empty(), "{:?}", graph.validate());
    graph.add_function_input("fn", "third").unwrap_err();
    graph.add_function_input("fn", "fourth").unwrap();
    assert_eq!(graph.nodes[3].input_count(), 3);
    assert_eq!(graph.nodes[3].input_label(2), "fourth");

    let mut legacy = Graph::default();
    legacy.add_node(Node::new("x", Kind::Constant { value: "1".into(), format: Format::Number })).unwrap();
    legacy.add_node(Node::new("y", Kind::Constant { value: "2".into(), format: Format::Number })).unwrap();
    legacy.add_node(Node::new("fn", named_function("inputs.len()", &[]))).unwrap();
    legacy.connect("x", "fn").unwrap();
    legacy.connect("y", "fn").unwrap();
    assert!(legacy.add_function_input("fn", "value").unwrap_err().contains("multiple connections"),
        "naming inputs must not silently drop one of several wires");
    legacy.disconnect_input("y", "fn", 0);
    legacy.add_function_input("fn", "value").unwrap();
    let mut screen = Screen::new("");
    let outputs = legacy.run_once(&mut screen).unwrap();
    assert_eq!(outputs["fn"], Some(Value::Int(1)));
}

fn js_function(code: &str, names: &[&str]) -> Kind {
    Kind::Function {
        code: code.into(),
        input_names: names.iter().map(|&name| name.to_owned()).collect(),
        language: ScriptLanguage::Javascript,
    }
}

#[test]
fn javascript_named_inputs_bind_variables_and_missing_pins_read_undefined() {
    let mut graph = Graph::default();
    graph.add_node(Node::new("left", Kind::Constant { value: "2".into(), format: Format::Number })).unwrap();
    graph.add_node(Node::new("right", Kind::Constant { value: "3".into(), format: Format::Number })).unwrap();
    graph.add_node(Node::new("sum", js_function(
        "right === undefined ? 'left only: ' + left : left + right + inputs.length",
        &["left", "right"],
    ))).unwrap();
    graph.connect_input("left", "sum", 0).unwrap();
    let mut screen = Screen::new("");
    let outputs = graph.run_once(&mut screen).unwrap();
    assert_eq!(outputs["sum"], Some(Value::Text("left only: 2".into())));
    graph.connect_input("right", "sum", 1).unwrap();
    let outputs = graph.run_once(&mut screen).unwrap();
    assert_eq!(outputs["sum"], Some(Value::Int(7)), "integral JS numbers become Int");
}

#[test]
fn javascript_state_is_transactional_and_language_changes_reset_it() {
    let mut screen = Screen::new("");
    let mut node = Node::new("conv", js_function("[1, 2]", &[]));
    assert_eq!(node.process(&[], &mut screen).unwrap(), Some(Value::Text("1,2".into())),
        "unsupported results fall back to their text form");
    let set_code = |node: &mut Node, next: &str| {
        let Kind::Function { code, .. } = &mut node.kind else { unreachable!() };
        *code = next.into();
    };
    set_code(&mut node, "null");
    assert_eq!(node.process(&[], &mut screen).unwrap(), None, "null and undefined stop the flow");
    set_code(&mut node, "1.5 + inputs[0]");
    assert_eq!(node.process(&[Value::Int(1)], &mut screen).unwrap(), Some(Value::Num(2.5)));
    set_code(&mut node, "if (state.count === undefined) { state.count = 0 } state.count += 1; \
        if (inputs.length > 0) { throw 'failed' } state.count");
    assert_eq!(node.process(&[], &mut screen).unwrap(), Some(Value::Int(1)));
    assert!(node.process(&[Value::Bool(true)], &mut screen).unwrap_err().contains("failed"));
    assert_eq!(node.process(&[], &mut screen).unwrap(), Some(Value::Int(2)),
        "a failed run must not commit state changes");
    let set_language = |node: &mut Node, next: ScriptLanguage| {
        let Kind::Function { language, .. } = &mut node.kind else { unreachable!() };
        *language = next;
    };
    set_language(&mut node, ScriptLanguage::Rhai);
    assert!(node.process(&[], &mut screen).is_err(), "the JavaScript code is not valid Rhai");
    set_language(&mut node, ScriptLanguage::Javascript);
    assert_eq!(node.process(&[], &mut screen).unwrap(), Some(Value::Int(1)),
        "switching the language clears saved state");
    set_code(&mut node, "state.bad = [1]; true");
    assert!(node.process(&[], &mut screen).unwrap_err().contains("must be text"));
}

fn ibkr_readings(price: &str, side: &str) -> VecDeque<String> {
    [
        price.into(), if side == "Buy" { "Position -" } else { "Position 1" }.into(),
        "SIMULATED TRADING".into(), "NVDA".into(),
        if side == "Buy" { "Position -" } else { "Position 1" }.into(), "Quantity".into(), "1".into(),
        "Market".into(), format!("{} 1 shares NVDA", side.to_uppercase()),
        side.into(), "1".into(), "Market".into(),
        if side == "Buy" { "0" } else { "1" }.into(),
        "SIMULATED TRADING".into(), "Your order has been filled".into(),
        format!("{} 1 NVDA Market", if side == "Buy" { "Bought" } else { "Sold" }),
        if side == "Buy" { "Position 1" } else { "Position -" }.into(),
    ].into_iter().collect()
}

#[test]
fn ibkr_fixture_repeats_exact_one_cent_round_trips_without_averaging_or_shorting() {
    let mut graph = Graph::from_json(include_str!("trading-ibkr-paper.json")).unwrap();
    assert!(graph.validate().is_empty(), "{:?}", graph.validate());
    let mut screen = Screen::new("");
    screen.script = ["155.00", "Position -"].map(String::from).into();
    graph.run_once(&mut screen).unwrap();
    assert!(screen.actions.is_empty());
    arm_breakout(&mut graph);
    screen.script = ["155.00", "Position -"].map(String::from).into();
    graph.run_once(&mut screen).unwrap();
    let buy = ["click", "key SelectAll", "type 1", "key Tab", "click", "click", "click", "click", "click"];
    let sell = ["click", "click", "key SelectAll", "type 1", "key Tab", "click", "click", "click", "click", "click"];
    let mut expected = Vec::new();
    for _ in 0..3 {
        screen.advance(1000);
        screen.script = ibkr_readings("155.01", "Buy");
        graph.run_once(&mut screen).unwrap();
        assert!(screen.script.is_empty());
        expected.extend(buy.map(String::from));
        assert_eq!(screen.actions, expected);
        screen.script = ["155.01", "Position 1"].map(String::from).into();
        graph.run_once(&mut screen).unwrap();
        screen.advance(1000);
        screen.script = ["155.02", "Position 1"].map(String::from).into();
        graph.run_once(&mut screen).unwrap();
        assert_eq!(screen.actions, expected, "continued rise must not buy while holding");
        screen.advance(1000);
        screen.script = ibkr_readings("155.01", "Sell");
        graph.run_once(&mut screen).unwrap();
        assert!(screen.script.is_empty());
        expected.extend(sell.map(String::from));
        assert_eq!(screen.actions, expected);
        screen.script = ["155.01", "Position -"].map(String::from).into();
        graph.run_once(&mut screen).unwrap();
        screen.advance(1000);
        screen.script = ["155.00", "Position -"].map(String::from).into();
        graph.run_once(&mut screen).unwrap();
        assert_eq!(screen.actions, expected, "continued fall must not sell while flat");
    }
}

#[test]
fn ibkr_fixture_blocks_reentry_during_waits_and_stops_on_bad_review_or_position() {
    for bad in ["position", "blank", "unlabeled", "review", "busy"] {
        let mut graph = Graph::from_json(include_str!("trading-ibkr-paper.json")).unwrap();
        arm_breakout(&mut graph);
        let mut screen = Screen::new("");
        screen.script = ["155.00", "Position -"].map(String::from).into();
        graph.run_once(&mut screen).unwrap();
        screen.script = ibkr_readings("155.01", "Buy");
        if matches!(bad, "position" | "blank" | "unlabeled") {
            screen.script[1] = match bad {
                "position" => "Position 2",
                "blank" => "",
                _ => "-",
            }.into();
            assert!(graph.run_once(&mut screen).unwrap_err().contains("position OCR"));
            assert!(screen.actions.is_empty());
        } else if bad == "review" {
            screen.script[11] = "Limit".into();
            assert!(graph.run_once(&mut screen).unwrap_err().contains("buy_review_type"));
            assert_eq!(screen.actions.len(), 6, "must not transmit");
        } else {
            screen.script.truncate(15);
            screen.script[14] = "Submitted".into();
            graph.run_once(&mut screen).unwrap();
            assert!(graph.has_pending_waits());
            let count = screen.actions.len();
            screen.advance(1000);
            screen.script = ["155.00", "Position 1", "Submitted"].map(String::from).into();
            graph.run_once(&mut screen).unwrap();
            assert_eq!(screen.actions.len(), count, "price reversal during a wait must not start a sell");
            screen.advance(60000);
            screen.script = ["155.00", "Position 1"].map(String::from).into();
            assert!(graph.run_once(&mut screen).unwrap_err().contains("timed out"));
            assert_eq!(screen.actions.len(), count);
        }
    }
}

fn breakout_readings(side: &str, held: usize) -> VecDeque<String> {
    [
        format!("{} NVDA 1 POSITION {held}", side.to_uppercase()),
        "PAPER BREAKOUT ARMED".into(), "PAPER TRADING".into(), held.to_string(),
        format!("Review {} order", side.to_lowercase()), "1".into(),
        format!("Review {} order", side.to_lowercase()), "NVIDIA (NVDA)".into(), side.into(),
        "1 shares".into(), "Market".into(), "$166.00".into(), "Order filled".into(),
        format!("Paper order #{}. {side} 1 NVDA filled at $166.00 per share.", held + 1),
    ].into_iter().collect()
}

#[test]
fn paper_breakout_fixture_is_disarmed_and_repeats_verified_buy_and_sell_signals() {
    let mut graph = Graph::from_json(include_str!("trading-breakout.json")).unwrap();
    assert!(graph.validate().is_empty(), "{:?}", graph.validate());
    let mut screen = Screen::new("BUY NVDA 1 POSITION 0");
    graph.run_once(&mut screen).unwrap();
    assert!(screen.actions.is_empty());
    arm_breakout(&mut graph);
    let leg = ["click", "click", "key SelectAll", "type 1", "click", "click", "key Enter"];
    let mut completed = 0;
    for (side, held) in (0..10).map(|held| ("Buy", held))
        .chain((1..=10).rev().map(|held| ("Sell", held))) {
        screen.advance(1000);
        screen.script = breakout_readings(side, held);
        let output = graph.run_once(&mut screen).unwrap();
        let done = if side == "Buy" { "done" } else { "sell_done" };
        assert!(output[done].is_some(), "{output:?}");
        assert!(screen.script.is_empty());
        assert!(!graph.has_pending_waits());
        completed += 1;
        assert_eq!(screen.actions, leg.repeat(completed));
        screen.text = "WAIT".into();
        graph.run_once(&mut screen).unwrap();
        assert_eq!(screen.actions.len(), completed * 7, "busy/cooldown must not create another order");
    }
}

#[test]
fn paper_breakout_ignores_invalid_signals_and_fails_before_submit_on_wrong_holdings_or_side() {
    for signal in ["WAIT", "BUY AAPL 1 POSITION 0", "BUY NVDA 2 POSITION 0",
        "BUY NVDA 1 POSITION 10", "SELL NVDA 1 POSITION 0", "unreadable"] {
        let mut graph = Graph::from_json(include_str!("trading-breakout.json")).unwrap();
        arm_breakout(&mut graph);
        let mut screen = Screen::new(signal);
        graph.run_once(&mut screen).unwrap();
        assert!(screen.actions.is_empty(), "{signal}");
    }
    for wrong_holding in [false, true] {
        let mut graph = Graph::from_json(include_str!("trading-breakout.json")).unwrap();
        arm_breakout(&mut graph);
        let mut screen = Screen::new("WAIT");
        screen.script = breakout_readings("Sell", 10);
        if wrong_holding { screen.script[3] = "0".into(); }
        else { screen.script[8] = "Buy".into(); }
        let error = graph.run_once(&mut screen).unwrap_err();
        assert!(error.contains(if wrong_holding { "sell_holding_limit" } else { "sell_check_side" }));
        assert_eq!(screen.actions.len(), if wrong_holding { 0 } else { 5 });
    }
}

#[test]
fn paper_breakout_busy_signal_does_not_restart_an_order_while_a_wait_continues() {
    let mut graph = Graph::from_json(include_str!("trading-breakout.json")).unwrap();
    arm_breakout(&mut graph);
    let mut screen = Screen::new("WAIT");
    let readings = breakout_readings("Buy", 0);
    screen.script = readings.iter().take(4).cloned().chain(["Loading".into()]).collect();
    graph.run_once(&mut screen).unwrap();
    assert!(graph.has_pending_waits());
    assert_eq!(screen.actions, ["click"]);
    screen.advance(1000);
    screen.script = ["WAIT".into()].into_iter().chain(readings.into_iter().skip(4)).collect();
    graph.run_once(&mut screen).unwrap();
    assert!(!graph.has_pending_waits());
    assert!(screen.script.is_empty());
    assert_eq!(screen.actions, ["click", "click", "key SelectAll", "type 1", "click", "click", "key Enter"]);
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
    graph.add_node(Node::new("function", Kind::Function { code: "throw \"bad\"".into(), input_names: Vec::new(), language: ScriptLanguage::Rhai })).unwrap();
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
