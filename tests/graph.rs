use screen_automation::backend::{Backend, Region};
use screen_automation::graph::{EventRuntime, Graph, NodeEvent, RunMode};
use screen_automation::node::{CompareOp, Crossing, Format, Kind, Node, Value};

#[derive(Default)]
struct Fake {
    screen: String,
    typed: Vec<String>,
    clicks: usize,
    fail_ocr: bool,
    fail_capture: bool,
}

impl Backend for Fake {
    fn capture(&mut self, _: &Region) -> Result<Vec<u8>, String> {
        if self.fail_capture {
            return Err("Unexpected screenshot capture".into());
        }
        Ok(self.screen.clone().into_bytes())
    }
    fn ocr(&mut self, _: &Region, p: &[u8]) -> Result<String, String> {
        if self.fail_ocr {
            return Err("OCR failed".into());
        }
        Ok(String::from_utf8_lossy(p).into())
    }
    fn type_text(&mut self, t: &str) -> Result<(), String> {
        self.typed.push(t.into());
        Ok(())
    }
    fn click(&mut self, _: i32, _: i32, _: &str) -> Result<(), String> {
        self.clicks += 1;
        Ok(())
    }
}

#[test]
fn event_frames_use_supplied_pixels_and_retry_failed_recognition() {
    let mut node = Node::new("ocr", Kind::Ocr { single_line: false, last_hash: None });
    let mut backend = Fake { fail_capture: true, ..Default::default() };
    let region = Region::default();
    assert_eq!(node.process_frame(&region, b"12.5", &mut backend).unwrap(),
        Some(Value::Text("12.5".into())));
    assert_eq!(node.process_frame(&region, b"12.5", &mut backend).unwrap(), None);
    backend.fail_ocr = true;
    assert!(node.process_frame(&region, b"13.5", &mut backend).is_err());
    backend.fail_ocr = false;
    assert_eq!(node.process_frame(&region, b"13.5", &mut backend).unwrap(),
        Some(Value::Text("13.5".into())));
}

fn event(source: &str, value: f64) -> NodeEvent {
    NodeEvent { source: source.into(), value: Value::Num(value) }
}

#[test]
fn event_execution_uses_constant_context_without_triggering_unrelated_branches() {
    let mut graph = Graph::default();
    graph.add_node(Node::new("a", Kind::Ocr { single_line: false, last_hash: None })).unwrap();
    graph.add_node(Node::new("b", Kind::Ocr { single_line: false, last_hash: None })).unwrap();
    graph.add_node(Node::new("limit", Kind::Constant { value: "5".into(), format: Format::Number })).unwrap();
    graph.add_node(Node::new("converted", Kind::Model { format: Format::Number })).unwrap();
    graph.add_node(input_comparison(true)).unwrap();
    graph.add_node(Node::new("click", Kind::Click { button: "left".into() })).unwrap();
    graph.add_node(Node::new("other-click", Kind::Click { button: "left".into() })).unwrap();
    graph.add_node(Node::new("standalone-click", Kind::Click { button: "left".into() })).unwrap();
    graph.add_node(Node::new("bad-unused-constant", Kind::Constant {
        value: "invalid".into(), format: Format::Number,
    })).unwrap();
    graph.connect("limit", "converted").unwrap();
    graph.connect_input("converted", "pair", 1).unwrap();
    graph.connect_input("a", "pair", 0).unwrap();
    graph.connect("pair", "click").unwrap();
    graph.connect("b", "other-click").unwrap();
    let mut backend = Fake { fail_capture: true, ..Default::default() };
    let mut runtime = EventRuntime::default();
    assert!(graph.run_events(&[], &mut runtime, &mut backend).unwrap().is_empty());
    let outputs = graph.run_events(&[event("a", 12.0)], &mut runtime, &mut backend).unwrap();
    assert_eq!(backend.clicks, 1);
    assert!(!outputs.contains_key("other-click"));
    assert!(!outputs.contains_key("standalone-click"));
    assert!(!outputs.contains_key("converted"));
    graph.run_events(&[event("a", 1.0)], &mut runtime, &mut backend).unwrap();
    assert_eq!(backend.clicks, 1);
    let Kind::Constant { value, .. } = &mut graph.nodes[2].kind else { unreachable!() };
    *value = "20".into();
    graph.run_events(&[event("a", 15.0)], &mut runtime, &mut backend).unwrap();
    assert_eq!(backend.clicks, 1);
    graph.run_events(&[event("b", 10.0)], &mut runtime, &mut backend).unwrap();
    assert_eq!(backend.clicks, 2);
}

#[test]
fn event_comparisons_cache_other_sources_and_batch_shared_descendants_once() {
    let mut graph = Graph::default();
    for name in ["a", "b"] {
        graph.add_node(Node::new(name, Kind::Ocr { single_line: false, last_hash: None })).unwrap();
    }
    let mut pair = input_comparison(true);
    let Kind::CompareInputs { on_change, .. } = &mut pair.kind else { unreachable!() };
    *on_change = false;
    graph.add_node(pair).unwrap();
    graph.add_node(Node::new("click", Kind::Click { button: "left".into() })).unwrap();
    graph.connect_input("a", "pair", 0).unwrap();
    graph.connect_input("b", "pair", 1).unwrap();
    graph.connect("pair", "click").unwrap();
    let mut backend = Fake { fail_capture: true, ..Default::default() };
    let mut runtime = EventRuntime::default();
    graph.run_events(&[event("a", 12.0)], &mut runtime, &mut backend).unwrap();
    assert_eq!(backend.clicks, 0);
    graph.run_events(&[event("b", 10.0)], &mut runtime, &mut backend).unwrap();
    assert_eq!(backend.clicks, 1);
    graph.run_events(&[event("a", 5.0)], &mut runtime, &mut backend).unwrap();
    assert_eq!(backend.clicks, 1);
    graph.run_events(&[event("b", 1.0)], &mut runtime, &mut backend).unwrap();
    assert_eq!(backend.clicks, 2);
    graph.run_events(&[event("a", 20.0), event("b", 2.0)], &mut runtime, &mut backend).unwrap();
    assert_eq!(backend.clicks, 3);
    let Kind::CompareInputs { trigger_when, .. } = &mut graph.nodes[2].kind else { unreachable!() };
    *trigger_when = false;
    let outputs = graph.run_events(&[event("a", 1.0)], &mut runtime, &mut backend).unwrap();
    assert_eq!(outputs["pair"], Some(Value::Bool(false)));
    assert_eq!(backend.clicks, 4, "false comparison events still activate connected actions");
    let mut restarted = EventRuntime::default();
    graph.run_events(&[event("b", 1.0)], &mut restarted, &mut backend).unwrap();
    assert_eq!(backend.clicks, 4, "new event session must not reuse old comparison inputs");
    assert!(graph.run_events(&[event("unknown", 1.0)], &mut restarted, &mut backend).is_err());
    assert!(graph.run_events(&[event("a", 1.0), event("a", 2.0)], &mut restarted, &mut backend).is_err());
}

#[test]
fn blocked_events_do_not_replay_cached_actions_or_advance_downstream_state() {
    let mut graph = Graph::default();
    graph.add_node(Node::new("a", Kind::Ocr { single_line: false, last_hash: None })).unwrap();
    graph.add_node(Node::new("gate", Kind::Compare {
        op: CompareOp::Greater, value: "10".into(), on_change: false, last: None,
    })).unwrap();
    graph.add_node(Node::new("model", Kind::Model { format: Format::Number })).unwrap();
    graph.add_node(Node::new("click", Kind::Click { button: "left".into() })).unwrap();
    graph.connect("a", "gate").unwrap();
    graph.connect("gate", "model").unwrap();
    graph.connect("model", "click").unwrap();
    let mut backend = Fake { fail_capture: true, ..Default::default() };
    let mut runtime = EventRuntime::default();
    graph.run_events(&[event("a", 12.0)], &mut runtime, &mut backend).unwrap();
    let outputs = graph.run_events(&[event("a", 5.0)], &mut runtime, &mut backend).unwrap();
    assert_eq!(backend.clicks, 1);
    assert!(!outputs.contains_key("model"));
    assert!(!outputs.contains_key("click"));
    graph.run_events(&[event("a", 13.0)], &mut runtime, &mut backend).unwrap();
    assert_eq!(backend.clicks, 2);
}

#[test]
fn custom_titles_preserve_connections_and_legacy_graphs() {
    let mut graph = Graph::default();
    graph.add_node(Node::new("source", Kind::Constant {
        value: "12.5".into(), format: Format::Number,
    })).unwrap();
    graph.add_node(Node::new("target", Kind::Model { format: Format::Number })).unwrap();
    graph.connect("source", "target").unwrap();
    for node in &mut graph.nodes {
        node.title = "Price".into();
    }
    let mut restored = Graph::from_json(&graph.to_json()).unwrap();
    assert_eq!(restored.nodes[0].display_title(), "Price");
    assert_eq!(restored.nodes[1].display_title(), "Price");
    assert_eq!(restored.edges, graph.edges);
    assert_eq!(restored.run_once(&mut Fake::default()).unwrap()["target"], Some(Value::Num(12.5)));

    restored.nodes[0].title.clear();
    restored.nodes[1].title = "   ".into();
    assert_eq!(restored.nodes[1].display_title(), "target");
    let legacy = restored.to_json();
    let restored = Graph::from_json(&legacy).unwrap();
    assert_eq!(restored.nodes[0].display_title(), "source");
    assert_eq!(restored.edges, graph.edges);
}

#[test]
fn duplicated_nodes_keep_configuration_but_start_with_fresh_execution_state() {
    let mut backend = Fake { screen: "12.5".into(), ..Default::default() };
    for kind in [
        Kind::Ocr { single_line: false, last_hash: None },
        Kind::Compare { op: CompareOp::Greater, value: "10".into(), on_change: true, last: None },
        Kind::CompareInputs {
            op: CompareOp::Greater, trigger_when: true, on_change: true,
            last: None, values: [None, None],
        },
        Kind::Hysteresis { upper: 10.0, lower: 5.0, trigger: Crossing::Both, band: None },
        Kind::Threshold { upper: 10.0, lower: 5.0, trigger: Crossing::Both, band: None },
    ] {
        let mut original = Node::new("original", kind.clone());
        let inputs = [Value::Num(12.5), Value::Num(10.0)];
        let first = original.process(&inputs, &mut backend).unwrap();
        let before = original.clone();
        let mut copy = original.duplicate("copy");
        assert_eq!(copy.kind, kind, "cached runtime state must not be copied");
        if matches!(kind, Kind::CompareInputs { .. }) {
            assert_eq!(copy.process(&[], &mut backend).unwrap(), None);
        }
        assert_eq!(copy.process(&inputs, &mut backend).unwrap(), first);
        assert_eq!(original, before);
    }
}

#[test]
fn constants_supply_typed_values_and_report_invalid_numbers() {
    let mut backend = Fake::default();
    for (format, value, expected) in [
        (Format::Number, "12.5", Value::Num(12.5)),
        (Format::Int, "12", Value::Int(12)),
        (Format::Text, "hello", Value::Text("hello".into())),
        (Format::Bool, "true", Value::Bool(true)),
    ] {
        let mut node = Node::new("constant", Kind::Constant { value: value.into(), format });
        assert_eq!(node.process(&[], &mut backend).unwrap(), Some(expected));
    }
    let mut node = Node::new("bad", Kind::Constant { value: "not a number".into(), format: Format::Number });
    assert!(node.process(&[], &mut backend).unwrap_err().contains("cannot convert"));
}

fn input_comparison(trigger_when: bool) -> Node {
    Node::new("pair", Kind::CompareInputs {
        op: CompareOp::Greater, trigger_when, on_change: true, last: None, values: [None, None],
    })
}

#[test]
fn compare_inputs_routes_a_and_b_caches_values_and_can_trigger_false() {
    let mut graph = Graph::default();
    graph.add_node(Node::new("price", Kind::Ocr { single_line: false, last_hash: None })).unwrap();
    graph.add_node(Node::new("limit", Kind::Constant { value: "100".into(), format: Format::Number })).unwrap();
    graph.add_node(input_comparison(false)).unwrap();
    graph.add_node(Node::new("click", Kind::Click { button: "left".into() })).unwrap();
    // Connect B first to verify that port assignment, not connection order, determines A and B.
    graph.connect_input("limit", "pair", 1).unwrap();
    graph.connect_input("price", "pair", 0).unwrap();
    graph.connect("pair", "click").unwrap();
    let mut backend = Fake { screen: "BTC 90".into(), ..Default::default() };
    assert_eq!(graph.run_once(&mut backend).unwrap()["pair"], Some(Value::Bool(false)));
    assert_eq!(backend.clicks, 1, "false output still triggers the Click node");
    assert_eq!(graph.run_once(&mut backend).unwrap()["pair"], None);
    backend.screen = "BTC 110".into();
    assert_eq!(graph.run_once(&mut backend).unwrap()["pair"], None);
    let Kind::Constant { value, .. } = &mut graph.nodes[1].kind else { unreachable!() };
    *value = "120".into();
    assert_eq!(graph.run_once(&mut backend).unwrap()["pair"], Some(Value::Bool(false)),
        "B-only update must compare against cached A, not move B into A");
    assert_eq!(backend.clicks, 2);
    let Kind::CompareInputs { on_change, .. } = &mut graph.nodes[2].kind else { unreachable!() };
    *on_change = false;
    assert_eq!(graph.run_once(&mut backend).unwrap()["pair"], Some(Value::Bool(false)));
    assert_eq!(backend.clicks, 3);
    let mut restored = Graph::from_json(&graph.to_json()).unwrap();
    assert_eq!(restored.edges, graph.edges);
    assert_eq!(restored.run_once(&mut backend).unwrap()["pair"], Some(Value::Bool(false)));
}

#[test]
fn compare_inputs_waits_for_both_and_clears_cached_data_on_rewiring() {
    let mut graph = Graph::default();
    graph.add_node(Node::new("left", Kind::Constant { value: "10".into(), format: Format::Int })).unwrap();
    graph.add_node(Node::new("right", Kind::Function { code: "()".into() })).unwrap();
    graph.add_node(input_comparison(true)).unwrap();
    graph.connect_input("left", "pair", 0).unwrap();
    graph.connect_input("right", "pair", 1).unwrap();
    assert!(graph.connect_input("right", "pair", 0).is_err());
    assert!(graph.connect_input("left", "pair", 2).is_err());
    let mut backend = Fake::default();
    assert_eq!(graph.run_once(&mut backend).unwrap()["pair"], None);
    let Kind::Function { code } = &mut graph.nodes[1].kind else { unreachable!() };
    *code = "5".into();
    assert_eq!(graph.run_once(&mut backend).unwrap()["pair"], Some(Value::Bool(true)));
    graph.disconnect_input("right", "pair", 1);
    assert_eq!(graph.run_once(&mut backend).unwrap()["pair"], None);
    let Kind::Function { code } = &mut graph.nodes[1].kind else { unreachable!() };
    *code = "()".into();
    graph.connect_input("right", "pair", 1).unwrap();
    assert_eq!(graph.run_once(&mut backend).unwrap()["pair"], None);
    let Kind::Function { code } = &mut graph.nodes[1].kind else { unreachable!() };
    *code = "5".into();
    assert_eq!(graph.run_once(&mut backend).unwrap()["pair"], Some(Value::Bool(true)));
    graph.remove_node("right");
    assert_eq!(graph.run_once(&mut backend).unwrap()["pair"], None);
}

#[test]
fn compare_inputs_rejects_invalid_saved_ports_and_duplicate_connections() {
    let mut graph = Graph::default();
    graph.add_node(Node::new("value", Kind::Constant { value: "1".into(), format: Format::Int })).unwrap();
    graph.add_node(input_comparison(true)).unwrap();
    graph.connect_input("value", "pair", 1).unwrap();
    let saved = serde_json::to_value(&graph).unwrap();
    let mut invalid_port = saved.clone();
    invalid_port["edges"][0]["input"] = 2.into();
    assert!(Graph::from_json(&invalid_port.to_string()).is_err());
    let mut duplicate = saved;
    let edge = duplicate["edges"][0].clone();
    duplicate["edges"].as_array_mut().unwrap().push(edge);
    assert!(Graph::from_json(&duplicate.to_string()).is_err());
}

#[test]
fn inclusive_hysteresis_triggers_at_equality_and_keeps_state() {
    let mut backend = Fake::default();
    for (trigger, values) in [
        (Crossing::RisingOrEqual, [95.0, 100.0, 100.0, 95.0, 100.0, 89.0, 100.0]),
        (Crossing::FallingOrEqual, [95.0, 90.0, 90.0, 95.0, 90.0, 101.0, 90.0]),
    ] {
        let mut node = Node::new("hysteresis", Kind::Hysteresis {
            upper: 100.0, lower: 90.0, trigger, band: None,
        });
        assert_eq!(node.process(&[Value::Num(f64::NAN)], &mut backend).unwrap(), None);
        for (value, expected) in values.into_iter().zip([false, true, false, false, false, false, true]) {
            let result = node.process(&[Value::Num(value)], &mut backend).unwrap();
            assert_eq!(result, expected.then_some(Value::Num(value)), "{trigger:?} at {value}");
        }
        let json = serde_json::to_string(&node).unwrap();
        let mut restored: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.process(&[Value::Num(values[1])], &mut backend).unwrap(), None,
            "first reading at equality must not trigger, including after reload");
    }
}

#[test]
fn threshold_triggers_first_reading_and_rearms_at_the_same_boundary() {
    let cases: &[(Crossing, &[f64], &[bool])] = &[
        (Crossing::Rising, &[101.0, 102.0, 100.0, 101.0, 95.0, 101.0, 89.0, 101.0],
            &[true, false, false, true, false, true, false, true]),
        (Crossing::Falling, &[89.0, 88.0, 90.0, 89.0, 95.0, 89.0, 101.0, 89.0],
            &[true, false, false, true, false, true, false, true]),
        (Crossing::RisingOrEqual, &[100.0, 100.0, 99.9, 100.0, 95.0, 101.0, 89.0, 100.0],
            &[true, false, false, true, false, true, false, true]),
        (Crossing::FallingOrEqual, &[90.0, 90.0, 90.1, 90.0, 95.0, 89.0, 101.0, 90.0],
            &[true, false, false, true, false, true, false, true]),
        (Crossing::Both, &[101.0, 102.0, 100.0, 101.0, 95.0, 89.0, 88.0, 90.0, 89.0, 101.0],
            &[true, false, false, true, false, true, false, false, true, true]),
    ];
    let mut backend = Fake::default();
    for &(trigger, values, expected) in cases {
        let mut node = Node::new("threshold", Kind::Threshold {
            upper: 100.0, lower: 90.0, trigger, band: None,
        });
        for (&value, &fire) in values.iter().zip(expected) {
            let input = Value::Text(format!("BTC {value}"));
            assert_eq!(node.process(std::slice::from_ref(&input), &mut backend).unwrap(),
                fire.then_some(input), "{trigger:?} at {value}");
            assert_eq!(node.process(&[Value::Text("unreadable".into())], &mut backend).unwrap(), None);
        }
        let mut restored: Node = serde_json::from_str(&serde_json::to_string(&node).unwrap()).unwrap();
        assert!(restored.process(&[Value::Num(values[0])], &mut backend).unwrap().is_some());
        let mut between = Node::new("threshold", Kind::Threshold {
            upper: 100.0, lower: 90.0, trigger, band: None,
        });
        assert_eq!(between.process(&[Value::Num(95.0)], &mut backend).unwrap(), None,
            "first reading only triggers when the selected condition holds");
    }
}

#[test]
fn threshold_range_entry_handles_boundaries_reentry_and_persistence() {
    let cases: &[(Crossing, &[f64], &[bool])] = &[
        (Crossing::EnteringRange,
            &[95.0, 96.0, 90.0, 90.0, 95.0, 100.0, 95.0, 101.0, 89.0, 95.0],
            &[true, false, false, false, true, false, true, false, false, true]),
        (Crossing::EnteringRangeInclusive,
            &[95.0, 90.0, 100.0, 95.0, 89.0, 90.0, 90.0, 101.0, 100.0, 95.0],
            &[true, false, false, false, false, true, false, false, true, false]),
    ];
    let mut backend = Fake::default();
    for &(trigger, values, expected) in cases {
        let mut node = Node::new("range", Kind::Threshold {
            upper: 100.0, lower: 90.0, trigger, band: None,
        });
        for (&value, &fire) in values.iter().zip(expected) {
            let input = Value::Text(format!("BTC {value}"));
            assert_eq!(node.process(std::slice::from_ref(&input), &mut backend).unwrap(),
                fire.then_some(input), "{trigger:?} at {value}");
            for invalid in [Value::Text("unreadable".into()), Value::Num(f64::NAN)] {
                assert_eq!(node.process(&[invalid], &mut backend).unwrap(), None);
            }
        }
        let mut graph = Graph::default();
        graph.add_node(node).unwrap();
        let mut restored = Graph::from_json(&graph.to_json()).unwrap();
        assert!(matches!(restored.nodes[0].kind, Kind::Threshold { trigger: saved, .. } if saved == trigger));
        assert_eq!(restored.nodes[0].process(&[Value::Int(95)], &mut backend).unwrap(), Some(Value::Int(95)));
        for boundary in [90.0, 100.0] {
            let mut first = Node::new("range", Kind::Threshold {
                upper: 100.0, lower: 90.0, trigger, band: None,
            });
            let input = Value::Num(boundary);
            assert_eq!(first.process(std::slice::from_ref(&input), &mut backend).unwrap(),
                (trigger == Crossing::EnteringRangeInclusive).then_some(input));
        }
    }
}

#[test]
fn threshold_equal_bounds_define_empty_strict_or_single_value_inclusive_range() {
    let mut backend = Fake::default();
    for trigger in [Crossing::EnteringRange, Crossing::EnteringRangeInclusive] {
        let mut node = Node::new("range", Kind::Threshold {
            upper: 100.0, lower: 100.0, trigger, band: None,
        });
        for (value, entry) in [(100.0, true), (100.0, false), (99.0, false),
            (100.0, true), (101.0, false), (100.0, true)]
        {
            assert_eq!(node.process(&[Value::Num(value)], &mut backend).unwrap(),
                (entry && trigger == Crossing::EnteringRangeInclusive).then_some(Value::Num(value)));
        }
        let mut unsupported = Node::new("hysteresis", Kind::Hysteresis {
            upper: 100.0, lower: 90.0, trigger, band: None,
        });
        assert!(unsupported.process(&[Value::Int(95)], &mut backend).unwrap_err()
            .contains("only supported by Threshold"));
    }
}

#[test]
fn legacy_threshold_json_remains_hysteresis_and_new_threshold_roundtrips() {
    let json = r#"{"nodes":[{
        "name":"threshold3","type":"threshold","upper":100.0,"lower":90.0,
        "trigger":"both","color":[215,215,160],"opacity":0.3
    }],"edges":[]}"#;
    let mut graph = Graph::from_json(json).unwrap();
    assert!(matches!(graph.nodes[0].kind, Kind::Hysteresis { .. }));
    let mut backend = Fake::default();
    assert_eq!(graph.nodes[0].process(&[Value::Num(101.0)], &mut backend).unwrap(), None);
    let saved: serde_json::Value = serde_json::from_str(&graph.to_json()).unwrap();
    assert_eq!(saved["nodes"][0]["type"], "hysteresis");
    graph.nodes[0].kind = Kind::Threshold {
        upper: 100.0, lower: 90.0, trigger: Crossing::Both, band: None,
    };
    let mut restored = Graph::from_json(&graph.to_json()).unwrap();
    assert!(matches!(restored.nodes[0].kind, Kind::Threshold { .. }));
    assert_eq!(restored.nodes[0].process(&[Value::Num(101.0)], &mut backend).unwrap(),
        Some(Value::Num(101.0)));
}

#[test]
fn single_line_ocr_errors_are_retried_and_old_workflows_default_to_multiline() {
    let mut node: Node = serde_json::from_str(
        r#"{"name":"ocr","type":"ocr","color":[0,170,255],"opacity":0.3}"#
    ).unwrap();
    assert!(matches!(node.kind, Kind::Ocr { single_line: false, .. }));
    node.kind = Kind::Ocr { single_line: true, last_hash: None };
    let mut backend = Fake::default();
    for _ in 0..2 {
        assert_eq!(node.process(&[], &mut backend).unwrap_err(),
            "Single-line OCR is not supported by this backend");
    }
}

#[test]
fn failed_ocr_is_retried_for_unchanged_pixels() {
    let mut node = Node::new("ocr", Kind::Ocr { single_line: false, last_hash: None });
    let mut backend = Fake {
        screen: "12:34".into(),
        fail_ocr: true,
        ..Default::default()
    };
    assert!(node.process(&[], &mut backend).is_err());
    backend.fail_ocr = false;
    assert_eq!(
        node.process(&[], &mut backend).unwrap(),
        Some(Value::Text("12:34".into()))
    );
    assert_eq!(node.process(&[], &mut backend).unwrap(), None);
}

#[test]
fn compare_triggers_click_on_threshold() {
    let mut g = Graph::default();
    g.add_node(Node::new("ocr", Kind::Ocr { single_line: false, last_hash: None }))
        .unwrap();
    g.add_node(Node::new(
        "cmp",
        Kind::Compare {
            op: CompareOp::Greater,
            value: "100".into(),
            on_change: true,
            last: None,
        },
    ))
    .unwrap();
    g.add_node(Node::new(
        "click",
        Kind::Click {
            button: "left".into(),
        },
    ))
    .unwrap();
    g.connect("ocr", "cmp").unwrap();
    g.connect("cmp", "click").unwrap();
    let mut b = Fake::default();
    for (screen, clicks) in [("BTC 99.5", 0), ("BTC 101", 1), ("BTC 102", 1), ("BTC 90", 1), ("BTC 150", 2)] {
        b.screen = screen.into();
        g.run_once(&mut b).unwrap();
        assert_eq!(b.clicks, clicks, "after {screen}");
    }

    let Kind::Compare { on_change, .. } = &mut g.nodes[1].kind else {
        unreachable!()
    };
    *on_change = false;
    b.screen = "BTC 151".into();
    g.run_once(&mut b).unwrap();
    assert_eq!(b.clicks, 3);
}

#[test]
fn hysteresis_requires_opposite_crossing_to_rearm() {
    let mut node = Node::new(
        "th",
        Kind::Hysteresis {
            upper: 100.0,
            lower: 90.0,
            trigger: Crossing::Both,
            band: None,
        },
    );
    let mut b = Fake::default();
    let mut fires = |node: &mut Node, v: &str| {
        node.process(&[Value::Text(v.into())], &mut b).unwrap().is_some()
    };
    // First reading only sets the band; equal values do not count as crossing.
    assert!(!fires(&mut node, "BTC 101"));
    assert!(!fires(&mut node, "95"));
    assert!(!fires(&mut node, "100"));
    assert!(!fires(&mut node, "101"), "must fall below lower before re-arming");
    assert!(fires(&mut node, "89.5"));
    assert!(!fires(&mut node, "90"));
    assert!(!fires(&mut node, "85"));
    assert!(fires(&mut node, "100.01"));

    let Kind::Hysteresis { trigger, .. } = &mut node.kind else {
        unreachable!()
    };
    *trigger = Crossing::Rising;
    assert!(!fires(&mut node, "80"), "falling ignored when only rising triggers");
    assert!(fires(&mut node, "120"));
    assert!(!fires(&mut node, "no number"));
}

#[test]
fn compare_operators() {
    let num = Value::Num(5.0);
    let expected = [
        (CompareOp::Greater, [false, false, true]),
        (CompareOp::GreaterOrEqual, [false, true, true]),
        (CompareOp::Equal, [false, true, false]),
        (CompareOp::NotEqual, [true, false, true]),
        (CompareOp::LessOrEqual, [true, true, false]),
        (CompareOp::Less, [true, false, false]),
    ];
    for (op, results) in expected {
        for (target, result) in ["6", "5", "4"].into_iter().zip(results) {
            assert_eq!(op.test(&num, target), Some(result), "5 {} {target}", op.symbol());
        }
    }
    assert_eq!(CompareOp::Equal.test(&Value::Int(5), "5.0"), Some(true));
    assert_eq!(CompareOp::Equal.test(&Value::Text("ok".into()), " ok "), Some(true));
    assert_eq!(CompareOp::Greater.test(&Value::Text("abc".into()), "5"), None);
    assert_eq!(CompareOp::Greater.test(&Value::Text("abc".into()), "xyz"), None);
}

fn build() -> Graph {
    let mut g = Graph::default();
    g.add_node(Node::new("ocr", Kind::Ocr { single_line: false, last_hash: None }))
        .unwrap();
    g.add_node(Node::new(
        "num",
        Kind::Model {
            format: Format::Number,
        },
    ))
    .unwrap();
    g.add_node(Node::new(
        "fn",
        Kind::Function {
            code: "if inputs[0] > 10.0 { \"go\" } else { () }".into(),
        },
    ))
    .unwrap();
    g.add_node(Node::new(
        "kb",
        Kind::Keyboard {
            text: String::new(),
        },
    ))
    .unwrap();
    g.add_node(Node::new(
        "click",
        Kind::Click {
            button: "left".into(),
        },
    ))
    .unwrap();
    for (a, b) in [("ocr", "num"), ("num", "fn"), ("fn", "kb"), ("fn", "click")] {
        g.connect(a, b).unwrap();
    }
    g
}

#[test]
fn pipeline_and_change_detection() {
    let mut g = build();
    let mut b = Fake {
        screen: "Price: 42".into(),
        ..Default::default()
    };
    g.run_once(&mut b).unwrap();
    assert_eq!(b.typed, vec!["go"]);
    assert_eq!(b.clicks, 2);
    g.run_once(&mut b).unwrap(); // unchanged screen
    assert_eq!(b.typed.len(), 1);
    b.screen = "Price: 5".into();
    g.run_once(&mut b).unwrap();
    assert_eq!(b.typed.len(), 1);
    b.screen = "Price: 99".into();
    let out = g.run_once(&mut b).unwrap();
    assert_eq!(b.typed.len(), 2);
    assert_eq!(out["num"], Some(Value::Num(99.0)));
}

#[test]
fn roundtrip_and_cycle() {
    let mut g = build();
    g.settings.refresh_ms = 1250;
    g.settings.run_mode = RunMode::DisplayRefresh;
    g.nodes[3].region.as_mut().unwrap().x = 5;
    g.nodes[3].position = [425.0, 175.0];
    let mut g2 = Graph::from_json(&g.to_json()).unwrap();
    assert_eq!(g2.nodes[3].region.unwrap().x, 5);
    assert_eq!(g2.nodes[3].position, [425.0, 175.0]);
    assert_eq!(g2.edges, g.edges);
    assert_eq!(g2.settings.refresh_ms, 1250);
    assert_eq!(g2.settings.run_mode, RunMode::DisplayRefresh);

    let mut legacy_json = serde_json::to_value(&g).unwrap();
    legacy_json.as_object_mut().unwrap().remove("settings");
    for node in legacy_json["nodes"].as_array_mut().unwrap() {
        node.as_object_mut().unwrap().remove("position");
    }
    let legacy_graph = Graph::from_json(&legacy_json.to_string()).unwrap();
    assert_eq!(legacy_graph.nodes[0].position, [0.0, 0.0]);
    assert_eq!(legacy_graph.settings.refresh_ms, 1000);
    assert_eq!(legacy_graph.settings.run_mode, RunMode::Timed);

    g.connect("kb", "ocr").unwrap();
    assert!(g.run_once(&mut Fake::default()).is_err());
    assert!(g2.run_once(&mut Fake::default()).is_ok());
}

#[test]
fn graph_settings_validate_delay_and_default_missing_values() {
    for (settings, expected) in [(r#"{}"#, 1000), (r#"{"refresh_ms":100}"#, 100),
        (r#"{"refresh_ms":60000}"#, 60000)] {
        let graph = Graph::from_json(&format!(r#"{{"nodes":[],"edges":[],"settings":{settings}}}"#)).unwrap();
        assert_eq!(graph.settings.refresh_ms, expected);
        assert_eq!(graph.settings.run_mode, RunMode::Timed);
    }
    for value in ["0", "99", "60001", "-1", "1.5", r#""1500""#] {
        assert!(Graph::from_json(&format!(
            r#"{{"nodes":[],"edges":[],"settings":{{"refresh_ms":{value}}}}}"#
        )).is_err(), "accepted invalid delay: {value}");
    }
    assert!(Graph::from_json(
        r#"{"nodes":[],"edges":[],"settings":{"run_mode":"unknown"}}"#
    ).is_err());
    let event_graph = Graph::from_json(
        r#"{"nodes":[],"edges":[],"settings":{"run_mode":"hmi_event","refresh_ms":500}}"#
    ).unwrap();
    assert_eq!(event_graph.settings.run_mode, RunMode::HmiEvent);
    assert_eq!(Graph::from_json(&event_graph.to_json()).unwrap().settings, event_graph.settings);
}

#[test]
fn both_run_modes_skip_ocr_until_the_captured_region_changes() {
    for mode in [RunMode::Timed, RunMode::DisplayRefresh] {
        let mut graph = Graph::default();
        graph.settings.run_mode = mode;
        graph.add_node(Node::new("ocr", Kind::Ocr {
            single_line: false, last_hash: None,
        })).unwrap();
        let mut backend = Fake { screen: "12.5".into(), ..Default::default() };
        assert_eq!(graph.run_once(&mut backend).unwrap()["ocr"], Some(Value::Text("12.5".into())));
        backend.fail_ocr = true;
        assert_eq!(graph.run_once(&mut backend).unwrap()["ocr"], None);
        backend.screen = "13.5".into();
        assert!(graph.run_once(&mut backend).is_err());
    }
}
