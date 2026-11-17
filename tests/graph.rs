use screen_automation::backend::{Backend, Region};
use screen_automation::graph::Graph;
use screen_automation::node::{CompareOp, Crossing, Format, Kind, Node, Value};

#[derive(Default)]
struct Fake {
    screen: String,
    typed: Vec<String>,
    clicks: usize,
    fail_ocr: bool,
}

impl Backend for Fake {
    fn capture(&mut self, _: &Region) -> Result<Vec<u8>, String> {
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
fn inclusive_thresholds_trigger_at_equality_and_keep_hysteresis() {
    let mut backend = Fake::default();
    for (trigger, values) in [
        (Crossing::RisingOrEqual, [95.0, 100.0, 100.0, 95.0, 100.0, 89.0, 100.0]),
        (Crossing::FallingOrEqual, [95.0, 90.0, 90.0, 95.0, 90.0, 101.0, 90.0]),
    ] {
        let mut node = Node::new("threshold", Kind::Threshold {
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
fn threshold_triggers_with_hysteresis() {
    let mut node = Node::new(
        "th",
        Kind::Threshold {
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

    let Kind::Threshold { trigger, .. } = &mut node.kind else {
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
    g.nodes[3].region.as_mut().unwrap().x = 5;
    g.nodes[3].position = [425.0, 175.0];
    let mut g2 = Graph::from_json(&g.to_json()).unwrap();
    assert_eq!(g2.nodes[3].region.unwrap().x, 5);
    assert_eq!(g2.nodes[3].position, [425.0, 175.0]);
    assert_eq!(g2.edges, g.edges);

    let mut legacy_json = serde_json::to_value(&g).unwrap();
    for node in legacy_json["nodes"].as_array_mut().unwrap() {
        node.as_object_mut().unwrap().remove("position");
    }
    let legacy_graph = Graph::from_json(&legacy_json.to_string()).unwrap();
    assert_eq!(legacy_graph.nodes[0].position, [0.0, 0.0]);

    g.connect("kb", "ocr").unwrap();
    assert!(g.run_once(&mut Fake::default()).is_err());
    assert!(g2.run_once(&mut Fake::default()).is_ok());
}
