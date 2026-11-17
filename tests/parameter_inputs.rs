use screen_automation::backend::{Backend, Region};
use screen_automation::graph::{Edge, EventRuntime, Graph, NodeEvent};
use screen_automation::node::{CompareOp, Crossing, Format, InputParameter::*, Kind, Node, Value};
use screen_automation::preview::PreviewSession;

#[derive(Default)]
struct Fake {
    screen: String,
    clicks: usize,
}

impl Backend for Fake {
    fn capture(&mut self, _: &Region) -> Result<Vec<u8>, String> {
        Ok(self.screen.as_bytes().to_vec())
    }

    fn ocr(&mut self, _: &Region, pixels: &[u8]) -> Result<String, String> {
        Ok(String::from_utf8_lossy(pixels).into())
    }

    fn type_text(&mut self, _: &str) -> Result<(), String> {
        panic!("unexpected keyboard action")
    }

    fn click(&mut self, _: i32, _: i32, _: &str) -> Result<(), String> {
        self.clicks += 1;
        Ok(())
    }
}

fn compare(on_change: bool) -> Node {
    Node::new("compare", Kind::Compare {
        op: CompareOp::Greater, value: "0".into(), on_change, last: None,
    })
}

fn ocr(name: &str) -> Node {
    Node::new(name, Kind::Ocr { single_line: false, last_hash: None })
}

fn constant(name: &str, value: &str) -> Node {
    Node::new(name, Kind::Constant { value: value.into(), format: Format::Text })
}

fn event(source: &str, value: Value) -> NodeEvent {
    NodeEvent { source: source.into(), value }
}

fn graph(nodes: Vec<Node>) -> Graph {
    Graph { nodes, ..Graph::default() }
}

#[test]
fn direct_compare_waits_for_rhs_and_keeps_fixed_value_and_change_memory() {
    let mut node = compare(true);
    node.input_parameters = vec![CompareValue];
    let mut backend = Fake::default();
    assert_eq!(node.process(&[Value::Int(12)], &mut backend).unwrap(), None);
    assert_eq!(node.process(&[Value::Int(12), Value::Int(10)], &mut backend).unwrap(),
        Some(Value::Int(12)));
    assert_eq!(node.process(&[Value::Int(13), Value::Text("bad".into())], &mut backend).unwrap(), None);
    assert_eq!(node.process(&[Value::Int(13)], &mut backend).unwrap(), None);
    assert_eq!(node.process(&[Value::Int(13), Value::Int(11)], &mut backend).unwrap(), None,
        "invalid parameter must not clear the previous true condition");
    assert_eq!(node.process(&[Value::Int(13), Value::Int(15)], &mut backend).unwrap(), None);
    assert_eq!(node.process(&[Value::Int(13), Value::Int(11)], &mut backend).unwrap(),
        Some(Value::Int(13)));
    assert!(matches!(&node.kind, Kind::Compare { value, .. } if value == "0"));
    let mut duplicate = node.duplicate("copy");
    assert_eq!(duplicate.input_parameters, vec![CompareValue]);
    assert_eq!(duplicate.process(&[Value::Int(13)], &mut backend).unwrap(), None);

    let mut fixed = compare(true);
    assert_eq!(fixed.process(&[Value::Int(12)], &mut backend).unwrap(), Some(Value::Int(12)));
    assert_eq!(fixed.process(&[Value::Int(12)], &mut backend).unwrap(), None);
    assert!(!serde_json::to_string(&fixed).unwrap().contains("input_parameters"));
}

#[test]
fn timed_settings_route_to_nonzero_port_and_only_changes_reuse_cached_main() {
    let mut graph = graph(vec![ocr("reading"), constant("limit", "3"), compare(false)]);
    graph.set_parameter_input("compare", CompareValue, true).unwrap();
    graph.connect_input("limit", "compare", 1).unwrap();
    let mut backend = Fake { screen: "12".into(), ..Fake::default() };
    assert_eq!(graph.run_once(&mut backend).unwrap()["compare"], None,
        "an isolated nonzero-port value is not a main reading");
    graph.connect("reading", "compare").unwrap();
    graph.reset_runtime();
    assert_eq!(graph.run_once(&mut backend).unwrap()["compare"], Some(Value::Text("12".into())));
    assert_eq!(graph.run_once(&mut backend).unwrap()["compare"], None);
    graph.nodes[1].kind = constant("limit", "invalid").kind;
    assert_eq!(graph.run_once(&mut backend).unwrap()["compare"], None);
    backend.screen = "13".into();
    assert_eq!(graph.run_once(&mut backend).unwrap()["compare"], None,
        "new main cannot use a formerly valid RHS");
    graph.nodes[1].kind = constant("limit", "4").kind;
    assert_eq!(graph.run_once(&mut backend).unwrap()["compare"], Some(Value::Text("13".into())));
    assert_eq!(graph.run_once(&mut backend).unwrap()["compare"], None);

    graph.disconnect_input("limit", "compare", 1);
    graph.connect_input("limit", "compare", 1).unwrap();
    assert_eq!(graph.run_once(&mut backend).unwrap()["compare"], None,
        "a reconnected input cannot replay a reading already used");
    graph.reset_runtime();
    assert_eq!(graph.run_once(&mut backend).unwrap()["compare"], Some(Value::Text("13".into())));
}

#[test]
fn bounds_changes_follow_threshold_and_hysteresis_rules_without_resetting_the_band() {
    for (kind, first) in [
        (Kind::Threshold { lower: 99.0, upper: 10.0, trigger: Crossing::Both, band: None },
            Some(Value::Int(12))),
        (Kind::Hysteresis { lower: 99.0, upper: 10.0, trigger: Crossing::Both, band: None },
            None),
    ] {
        let mut gate = Node::new("gate", kind);
        gate.input_parameters = vec![Lower, Upper];
        let mut backend = Fake::default();
        assert_eq!(gate.process(&[Value::Int(12), Value::Int(0)], &mut backend).unwrap(), None);
        assert_eq!(gate.process(&[Value::Int(12), Value::Int(0), Value::Text(" 1e1 ".into())],
            &mut backend).unwrap(), first);
        for invalid in [Value::Bool(true), Value::Text("$20".into()), Value::Num(f64::INFINITY), Value::Int(-1)] {
            assert_eq!(gate.process(&[Value::Int(-2), Value::Int(0), invalid], &mut backend).unwrap(), None);
        }
        assert_eq!(gate.process(&[Value::Int(12), Value::Int(0), Value::Int(9)],
            &mut backend).unwrap(), None, "invalid samples must not change the prior Above band");
        assert_eq!(gate.process(&[Value::Int(12), Value::Int(13), Value::Int(14)],
            &mut backend).unwrap(), Some(Value::Int(12)));
        assert_eq!(gate.process(&[Value::Int(12), Value::Int(0), Value::Int(11)],
            &mut backend).unwrap(), Some(Value::Int(12)));
    }
}

#[test]
fn dynamic_number_limits_validate_before_updating_change_memory() {
    let mut guard = Node::new("guard", Kind::ValidateNumber {
        minimum: Some(90.0), maximum: Some(10.0), max_change: Some(-1.0),
        pattern: String::new(), last: None,
    });
    guard.input_parameters = vec![Minimum, Maximum, MaxChange];
    let mut backend = Fake::default();
    let mut check = |reading, min, max, change| guard.process(
        &[Value::Int(reading), Value::Int(min), Value::Int(max), Value::Int(change)],
        &mut backend).unwrap();
    assert_eq!(check(5, 0, 10, 2), Some(Value::Int(5)));
    assert_eq!(check(8, 0, 10, 2), None);
    assert_eq!(check(8, 9, 8, 4), None);
    assert_eq!(check(8, 0, 10, -1), None);
    assert_eq!(check(8, 0, 10, 2), None);
    assert_eq!(check(7, 0, 7, 2), Some(Value::Int(7)));
    assert!(matches!(guard.kind, Kind::ValidateNumber {
        minimum: Some(90.0), maximum: Some(10.0), max_change: Some(-1.0), ..
    }));
}

#[test]
fn toggle_remaps_by_parameter_and_checks_graph_metadata_and_occupancy() {
    let mut graph = graph(vec![constant("reading", "12"), constant("hi", "11"),
        constant("lo", "0"), Node::new("gate", Kind::Threshold {
            lower: 100.0, upper: 10.0, trigger: Crossing::Rising, band: None,
        })]);
    graph.set_parameter_input("gate", Upper, true).unwrap();
    graph.connect_input("hi", "gate", 1).unwrap();
    graph.connect("reading", "gate").unwrap();
    assert!(graph.connect("lo", "gate").is_err());
    assert!(graph.connect_input("hi", "gate", 1).is_err());
    graph.set_parameter_input("gate", Lower, true).unwrap();
    assert_eq!(graph.nodes[3].parameter_port(Upper), Some(2));
    assert_eq!(graph.edges[0].input, 2);
    assert!(graph.validate().iter().any(|issue| issue.message.contains("port 1")));
    graph.connect_input("lo", "gate", 1).unwrap();
    assert!(graph.validate().is_empty(), "inactive fixed limits are ignored");

    let saved = graph.to_json();
    assert!(saved.contains("\"lower\"") && saved.contains("\"upper\""));
    let mut restored = Graph::from_json(&saved).unwrap();
    assert_eq!(restored, graph);
    assert_eq!(restored.nodes[3].input_label(2), "Upper");
    let mut backend = Fake::default();
    assert_eq!(restored.run_once(&mut backend).unwrap()["gate"], Some(Value::Text("12".into())));
    assert_eq!(restored.to_json(), saved, "evaluation must not persist input values");

    restored.nodes[3].input_parameters.swap(0, 1);
    for edge in &mut restored.edges {
        if edge.input != 0 {
            edge.input = 3 - edge.input;
        }
    }
    let mut restored = Graph::from_json(&restored.to_json()).unwrap();
    assert_eq!(restored.nodes[3].input_parameters, vec![Upper, Lower]);
    restored.set_parameter_input("gate", Lower, true).unwrap();
    assert_eq!(restored.nodes[3].input_parameters, vec![Lower, Upper]);
    assert_eq!(restored.edges, graph.edges);
    restored.set_parameter_input("gate", Upper, false).unwrap();
    assert_eq!(restored.edges.len(), 2);
    assert!(restored.edges.iter().any(|e| e.source == "lo" && e.input == 1));
    assert!(restored.edges.iter().any(|e| e.source == "reading" && e.input == 0));
    assert!(restored.set_parameter_input("gate", CompareValue, true).is_err());
    for invalid in [vec![Lower, Lower], vec![MaxChange]] {
        restored.nodes[3].input_parameters = invalid;
        assert!(!restored.validate().is_empty());
        assert!(Graph::from_json(&restored.to_json()).is_err());
    }

    graph.edges.push(Edge { source: "hi".into(), target: "gate".into(), input: 0 });
    assert!(graph.validate().iter().any(|issue| issue.message.contains("multiple connections")));
    assert!(Graph::from_json(&graph.to_json()).is_err());
}

#[test]
fn event_batches_use_fresh_main_and_do_not_replay_identical_settings() {
    let mut graph = graph(vec![ocr("reading"), ocr("limit"), compare(false)]);
    graph.set_parameter_input("compare", CompareValue, true).unwrap();
    graph.connect_input("limit", "compare", 1).unwrap();
    graph.connect("reading", "compare").unwrap();
    let mut backend = Fake::default();
    let mut runtime = EventRuntime::default();
    let reading = || event("reading", Value::Int(12));
    let setting = |n| event("limit", Value::Int(n));
    let mut run = |events: &[NodeEvent]| graph.run_events(events, &mut runtime, &mut backend)
        .unwrap()["compare"].clone();
    assert_eq!(run(&[setting(11), reading()]), Some(Value::Int(12)));
    assert_eq!(run(&[setting(11)]), None);
    assert_eq!(run(&[reading()]), Some(Value::Int(12)),
        "a genuinely new reading can emit even if its value is identical");
    assert_eq!(run(&[setting(10)]), Some(Value::Int(12)));
    assert_eq!(run(&[setting(13), reading()]), None, "batch must use its new RHS");
    let mut restarted = EventRuntime::default();
    assert_eq!(graph.run_events(&[setting(1)], &mut restarted, &mut backend).unwrap()["compare"], None);
}

#[test]
fn event_partial_bound_updates_preserve_threshold_and_hysteresis_bands() {
    let mut graph = graph(vec![
        ocr("reading"), ocr("lower"), ocr("upper"),
        Node::new("threshold", Kind::Threshold {
            lower: 100.0, upper: 0.0, trigger: Crossing::Both, band: None,
        }),
        Node::new("hysteresis", Kind::Hysteresis {
            lower: 100.0, upper: 0.0, trigger: Crossing::Both, band: None,
        }),
    ]);
    for name in ["threshold", "hysteresis"] {
        graph.set_parameter_input(name, Lower, true).unwrap();
        graph.set_parameter_input(name, Upper, true).unwrap();
        graph.connect("reading", name).unwrap();
        graph.connect_input("lower", name, 1).unwrap();
        graph.connect_input("upper", name, 2).unwrap();
    }
    let mut backend = Fake::default();
    let mut runtime = EventRuntime::default();
    let mut run = |batch: &[NodeEvent]| {
        let out = graph.run_events(batch, &mut runtime, &mut backend).unwrap();
        (out["threshold"].clone(), out["hysteresis"].clone())
    };
    let int = |source, n| event(source, Value::Int(n));
    let reading = Some(Value::Int(5));
    assert_eq!(run(&[int("reading", 5), int("upper", 10)]), (None, None));
    assert_eq!(run(&[int("lower", 0)]), (None, None));
    assert_eq!(run(&[int("upper", 4)]), (reading.clone(), reading.clone()));
    assert_eq!(run(&[int("lower", 6)]), (None, None)); // Inverted pair.
    assert_eq!(run(&[int("lower", 0)]), (None, None)); // Both still Above.
    assert_eq!(run(&[int("upper", 6)]), (None, None)); // Only Threshold rearms.
    assert_eq!(run(&[int("upper", 4)]), (reading, None));
}

#[test]
fn event_settings_seed_constant_main_and_preview_suppresses_actions_inside_subflow() {
    let mut inner = graph(vec![Node::new("in", Kind::SubflowInput), constant("limit", "10"),
        compare(false), Node::new("click", Kind::Click { button: "left".into() }),
        Node::new("out", Kind::SubflowOutput)]);
    inner.set_parameter_input("compare", CompareValue, true).unwrap();
    inner.connect_input("limit", "compare", 1).unwrap();
    inner.connect("in", "compare").unwrap();
    inner.connect("compare", "click").unwrap();
    inner.connect("click", "out").unwrap();
    let mut outer = graph(vec![constant("reading", "12"), Node::new("flow",
        Kind::Subflow { graph: Box::new(inner) })]);
    outer.connect("reading", "flow").unwrap();
    let mut preview = PreviewSession::new(&outer).unwrap();
    let mut backend = Fake::default();
    preview.step(&mut backend).unwrap();
    let step = preview.step(&mut backend).unwrap().unwrap();
    assert_eq!(step.output, Some(Value::Bool(true)));
    assert_eq!(step.actions.len(), 1);
    assert_eq!(backend.clicks, 0);

    let mut hmi = graph(vec![constant("reading", "12"), ocr("limit"), compare(false)]);
    hmi.set_parameter_input("compare", CompareValue, true).unwrap();
    hmi.connect_input("limit", "compare", 1).unwrap();
    hmi.connect("reading", "compare").unwrap();
    let mut runtime = EventRuntime::default();
    let mut run = || hmi.run_events(&[event("limit", Value::Int(10))], &mut runtime, &mut backend)
        .unwrap()["compare"].clone();
    assert_eq!(run(), Some(Value::Text("12".into())));
    assert_eq!(run(), None);
}
