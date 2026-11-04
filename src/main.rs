use eframe::egui::{self, Color32, ViewportBuilder, ViewportId};
use screen_automation::{
    desktop::Desktop,
    graph::Graph,
    node::{Format, Kind, Node},
};
use std::time::{Duration, Instant};

struct App {
    graph: Graph,
    backend: Option<Desktop>,
    kind: usize,
    selected: Option<String>,
    connect_to: String,
    running: bool,
    last_tick: Instant,
    refresh_ms: u64,
    file: String,
    status: String,
}

const KINDS: [&str; 5] = ["ocr", "model", "function", "keyboard", "click"];

fn new_kind(i: usize) -> Kind {
    match i {
        0 => Kind::Ocr { last_hash: None },
        1 => Kind::Model { format: Format::Number },
        2 => Kind::Function { code: "inputs[0]".into() },
        3 => Kind::Keyboard { text: String::new() },
        _ => Kind::Click { button: "left".into() },
    }
}

impl App {
    fn overlay(&mut self, ctx: &egui::Context, idx: usize) {
        let node = &mut self.graph.nodes[idx];
        let Some(r) = node.region else { return };
        let id = ViewportId::from_hash_of(("overlay", &node.name));
        let builder = ViewportBuilder::default()
            .with_title(node.name.clone())
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_position([r.x as f32, r.y as f32])
            .with_inner_size([r.width as f32, r.height as f32]);
        let [cr, cg, cb] = node.color;
        let fill = Color32::from_rgba_unmultiplied(cr, cg, cb, (node.opacity * 255.0) as u8);
        let name = node.name.clone();
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(fill)).show(ctx, |ui| {
                let rect = ui.max_rect();
                ui.painter().rect_stroke(rect.shrink(1.5), 0.0, egui::Stroke::new(3.0, Color32::RED), egui::StrokeKind::Inside);
                ui.painter().text(rect.left_top() + egui::vec2(8.0, 6.0), egui::Align2::LEFT_TOP, &name,
                    egui::FontId::proportional(14.0), Color32::BLACK);
                let grip = egui::Rect::from_min_size(rect.right_bottom() - egui::vec2(14.0, 14.0), egui::vec2(14.0, 14.0));
                ui.painter().rect_filled(grip, 0.0, Color32::RED);
                let resp = ui.interact(rect, ui.id().with("drag"), egui::Sense::click_and_drag());
                if resp.drag_started() {
                    let on_grip = resp.interact_pointer_pos().is_some_and(|p| grip.contains(p));
                    ctx.send_viewport_cmd(if on_grip {
                        egui::ViewportCommand::BeginResize(egui::viewport::ResizeDirection::SouthEast)
                    } else {
                        egui::ViewportCommand::StartDrag
                    });
                }
            });
        });
        // Sync the region from the actual window rectangle (reported by the overlay viewport).
        let info = ctx.input_for(id, |i| i.viewport().inner_rect);
        if let Some(rect) = info {
            node.region = Some(screen_automation::backend::Region {
                x: rect.min.x as i32,
                y: rect.min.y as i32,
                width: rect.width() as u32,
                height: rect.height() as u32,
            });
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("kind").selected_text(KINDS[self.kind]).show_ui(ui, |ui| {
                    for (i, k) in KINDS.iter().enumerate() {
                        ui.selectable_value(&mut self.kind, i, *k);
                    }
                });
                if ui.button("＋ Add").clicked() {
                    let name = format!("{}{}", KINDS[self.kind], self.graph.nodes.len() + 1);
                    if let Err(e) = self.graph.add_node(Node::new(&name, new_kind(self.kind))) {
                        self.status = e;
                    }
                }
                if ui.button(if self.running { "Stop" } else { "Run" }).clicked() {
                    self.running = !self.running;
                }
                ui.add(egui::DragValue::new(&mut self.refresh_ms).range(100..=60000).suffix(" ms"));
            });
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.file);
                if ui.button("Save").clicked() {
                    self.status = std::fs::write(&self.file, self.graph.to_json()).map_or_else(|e| e.to_string(), |_| "saved".into());
                }
                if ui.button("Load").clicked() {
                    self.status = match std::fs::read_to_string(&self.file).map_err(|e| e.to_string()).and_then(|s| Graph::from_json(&s)) {
                        Ok(g) => { self.graph = g; self.selected = None; "loaded".into() }
                        Err(e) => e,
                    };
                }
                ui.label(&self.status);
            });
            ui.separator();
            let mut delete = None;
            let edges = self.graph.edges.clone();
            let names: Vec<String> = self.graph.nodes.iter().map(|n| n.name.clone()).collect();
            for node in &mut self.graph.nodes {
                let sel = self.selected.as_deref() == Some(&node.name);
                ui.horizontal(|ui| {
                    if ui.selectable_label(sel, format!("{} [{}]", node.name, node.type_name())).clicked() {
                        self.selected = Some(node.name.clone());
                    }
                    let outs: Vec<&str> = edges.iter().filter(|(s, _)| *s == node.name).map(|(_, d)| d.as_str()).collect();
                    if !outs.is_empty() { ui.label(format!("→ {}", outs.join(", "))); }
                    if ui.small_button("✕").clicked() { delete = Some(node.name.clone()); }
                });
                if sel {
                    ui.indent("cfg", |ui| {
                        match &mut node.kind {
                            Kind::Model { format } => {
                                for (f, l) in [(Format::Text, "text"), (Format::Number, "number"), (Format::Int, "int"), (Format::Bool, "bool")] {
                                    ui.radio_value(format, f, l);
                                }
                            }
                            Kind::Function { code } => { ui.add(egui::TextEdit::multiline(code).code_editor()); }
                            Kind::Keyboard { text } => { ui.horizontal(|ui| { ui.label("text (empty = input)"); ui.text_edit_singleline(text); }); }
                            Kind::Click { button } => { ui.horizontal(|ui| { ui.label("button"); ui.text_edit_singleline(button); }); }
                            Kind::Ocr { .. } => {}
                        }
                        if node.region.is_some() {
                            let mut c = Color32::from_rgb(node.color[0], node.color[1], node.color[2]);
                            ui.horizontal(|ui| {
                                ui.color_edit_button_srgba(&mut c);
                                node.color = [c.r(), c.g(), c.b()];
                                ui.add(egui::Slider::new(&mut node.opacity, 0.05..=1.0).text("opacity"));
                            });
                        }
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("to").selected_text(&self.connect_to).show_ui(ui, |ui| {
                                for n in &names { ui.selectable_value(&mut self.connect_to, n.clone(), n); }
                            });
                            if ui.button("Connect").clicked() {
                                let (src, dst) = (node.name.clone(), self.connect_to.clone());
                                self.status = format!("connect {src}->{dst}");
                                ctx.data_mut(|d| d.insert_temp(egui::Id::new("conn"), (src, dst)));
                            }
                        });
                    });
                }
            }
            if let Some((s, d)) = ctx.data_mut(|d| d.remove_temp::<(String, String)>(egui::Id::new("conn"))) {
                if let Err(e) = self.graph.connect(&s, &d) { self.status = e; }
            }
            if let Some(n) = delete { self.graph.remove_node(&n); }
        });

        for i in 0..self.graph.nodes.len() {
            self.overlay(ctx, i);
        }

        if self.running && self.last_tick.elapsed() >= Duration::from_millis(self.refresh_ms) {
            self.last_tick = Instant::now();
            if let Some(b) = self.backend.as_mut() {
                if let Err(e) = self.graph.run_once(b) { self.status = e; }
            }
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

fn main() -> eframe::Result {
    let app = App {
        graph: Graph::default(),
        backend: Desktop::new().ok(),
        kind: 0,
        selected: None,
        connect_to: String::new(),
        running: false,
        last_tick: Instant::now(),
        refresh_ms: 1000,
        file: "graph.json".into(),
        status: String::new(),
    };
    eframe::run_native("ScreenAutomation", Default::default(), Box::new(|_| Ok(Box::new(app))))
}
