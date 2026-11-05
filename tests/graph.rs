use screen_automation::backend::{Backend, Region};
use screen_automation::graph::Graph;
use screen_automation::node::{Format, Kind, Node, Value};

#[derive(Default)]
struct Fake {
    screen: String,
    typed: Vec<String>,
    clicks: usize,
}

impl Backend for Fake {
    fn capture(&mut self, _: &Region) -> Result<Vec<u8>, String> {
        Ok(self.screen.clone().into_bytes())
    }
    fn ocr(&mut self, _: &Region, p: &[u8]) -> Result<String, String> {
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

fn build() -> Graph {
    let mut g = Graph::default();
    g.add_node(Node::new("ocr", Kind::Ocr { last_hash: None }))
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
