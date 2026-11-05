use eframe::egui::{self, Color32, Id, Pos2, ViewportBuilder, ViewportId};
use egui_snarl::{
    ui::{PinInfo, SnarlStyle, SnarlViewer, SnarlWidget},
    InPin, OutPin, Snarl,
};
use screen_automation::{
    backend::Region,
    desktop::Desktop,
    graph::Graph,
    node::{Format, Kind, Node},
};
use std::time::{Duration, Instant};

struct App {
    graph: Graph,
    snarl: Snarl<usize>,
    backend: Option<Desktop>,
    kind: usize,
    selected: Option<usize>,
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
        1 => Kind::Model {
            format: Format::Number,
        },
        2 => Kind::Function {
            code: "inputs[0]".into(),
        },
        3 => Kind::Keyboard {
            text: String::new(),
        },
        _ => Kind::Click {
            button: "left".into(),
        },
    }
}

struct GraphViewer<'a> {
    graph: &'a mut Graph,
    status: &'a mut String,
}

impl SnarlViewer<usize> for GraphViewer<'_> {
    fn title(&mut self, node: &usize) -> String {
        let node = &self.graph.nodes[*node];
        format!("{} · {}", node.name, node.type_name())
    }

    fn inputs(&mut self, node: &usize) -> usize {
        usize::from(!matches!(self.graph.nodes[*node].kind, Kind::Ocr { .. }))
    }

    fn show_input(
        &mut self,
        _: &InPin,
        ui: &mut egui::Ui,
        _: &mut Snarl<usize>,
    ) -> impl egui_snarl::ui::SnarlPin + 'static {
        ui.label("input");
        PinInfo::circle()
    }

    fn outputs(&mut self, _: &usize) -> usize {
        1
    }

    fn show_output(
        &mut self,
        _: &OutPin,
        ui: &mut egui::Ui,
        _: &mut Snarl<usize>,
    ) -> impl egui_snarl::ui::SnarlPin + 'static {
        ui.label("output");
        PinInfo::circle()
    }

    fn has_body(&mut self, _: &usize) -> bool {
        true
    }

    fn show_body(
        &mut self,
        node: egui_snarl::NodeId,
        _: &[InPin],
        _: &[OutPin],
        ui: &mut egui::Ui,
        snarl: &mut Snarl<usize>,
    ) {
        let index = snarl[node];
        let node = &self.graph.nodes[index];
        let detail = match &node.kind {
            Kind::Ocr { .. } => "Read text from a screen region",
            Kind::Model { format } => match format {
                Format::Text => "Convert input to text",
                Format::Number => "Convert input to number",
                Format::Int => "Convert input to integer",
                Format::Bool => "Convert input to boolean",
            },
            Kind::Function { .. } => "Evaluate a Rhai expression",
            Kind::Keyboard { .. } => "Type text into a screen region",
            Kind::Click { .. } => "Click a screen region",
        };
        ui.label(detail);
    }

    fn connect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<usize>) {
        let source = self.graph.nodes[snarl[from.id.node]].name.clone();
        let destination = self.graph.nodes[snarl[to.id.node]].name.clone();
        match self.graph.connect(&source, &destination) {
            Ok(()) => {
                snarl.connect(from.id, to.id);
                self.status.clear();
            }
            Err(error) => *self.status = error,
        }
    }

    fn disconnect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<usize>) {
        let source = self.graph.nodes[snarl[from.id.node]].name.clone();
        let destination = self.graph.nodes[snarl[to.id.node]].name.clone();
        self.graph
            .edges
            .retain(|edge| edge != &(source.clone(), destination.clone()));
        snarl.disconnect(from.id, to.id);
    }
}

impl App {
    fn rebuild_snarl(&mut self) {
        self.snarl = Snarl::new();
        let mut ids = Vec::with_capacity(self.graph.nodes.len());
        for (index, node) in self.graph.nodes.iter_mut().enumerate() {
            if node.position == [0.0, 0.0] {
                node.position = [
                    80.0 + (index % 3) as f32 * 280.0,
                    80.0 + (index / 3) as f32 * 190.0,
                ];
            }
            ids.push(
                self.snarl
                    .insert_node(Pos2::new(node.position[0], node.position[1]), index),
            );
        }
        for (source, destination) in &self.graph.edges {
            let Some(source_index) = self.graph.nodes.iter().position(|n| n.name == *source) else {
                continue;
            };
            let Some(destination_index) =
                self.graph.nodes.iter().position(|n| n.name == *destination)
            else {
                continue;
            };
            self.snarl.connect(
                egui_snarl::OutPinId {
                    node: ids[source_index],
                    output: 0,
                },
                egui_snarl::InPinId {
                    node: ids[destination_index],
                    input: 0,
                },
            );
        }
    }

    fn sync_graph_positions(&mut self) {
        for (_, position, index) in self.snarl.nodes_pos_ids() {
            if let Some(node) = self.graph.nodes.get_mut(*index) {
                node.position = [position.x, position.y];
            }
        }
    }

    fn add_node(&mut self) {
        let kind_name = KINDS[self.kind];
        let mut number = self.graph.nodes.len() + 1;
        let name = loop {
            let candidate = format!("{kind_name}{number}");
            if self.graph.nodes.iter().all(|node| node.name != candidate) {
                break candidate;
            }
            number += 1;
        };
        let mut node = Node::new(&name, new_kind(self.kind));
        let index = self.graph.nodes.len();
        node.position = [
            80.0 + (index % 3) as f32 * 280.0,
            80.0 + (index / 3) as f32 * 190.0,
        ];
        match self.graph.add_node(node) {
            Ok(()) => {
                self.rebuild_snarl();
                self.status.clear();
            }
            Err(error) => self.status = error,
        }
    }

    fn delete_selected(&mut self) {
        if let Some(index) = self.selected.take() {
            if let Some(node) = self.graph.nodes.get(index) {
                let name = node.name.clone();
                self.graph.remove_node(&name);
                self.rebuild_snarl();
            }
        }
    }

    fn overlay(&mut self, ctx: &egui::Context, idx: usize) {
        let node = &mut self.graph.nodes[idx];
        let Some(region) = node.region else {
            return;
        };
        let id = ViewportId::from_hash_of(("overlay", &node.name));
        let builder = ViewportBuilder::default()
            .with_title(node.name.clone())
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(true)
            .with_min_inner_size([80.0, 60.0])
            .with_always_on_top()
            .with_position([region.x as f32, region.y as f32])
            .with_inner_size([region.width as f32, region.height as f32]);
        let [red, green, blue] = node.color;
        let alpha = (node.opacity.clamp(0.0, 1.0) * 255.0) as u8;
        let fill = Color32::from_rgba_unmultiplied(red, green, blue, alpha);
        let outline = Color32::from_rgba_unmultiplied(255, 40, 40, alpha);
        let title = Color32::from_rgba_unmultiplied(0, 0, 0, alpha);
        let name = node.name.clone();
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            let viewport_ctx = ctx.clone();
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(fill))
                .show(ctx, |ui| {
                    let rect = ui.max_rect();
                    ui.painter().rect_stroke(
                        rect.shrink(1.5),
                        0.0,
                        egui::Stroke::new(3.0, outline),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        rect.left_top() + egui::vec2(8.0, 6.0),
                        egui::Align2::LEFT_TOP,
                        &name,
                        egui::FontId::proportional(14.0),
                        title,
                    );

                    let grip = egui::Rect::from_min_size(
                        rect.right_bottom() - egui::vec2(18.0, 18.0),
                        egui::vec2(18.0, 18.0),
                    );
                    let drag = ui.interact(rect, ui.id().with("move-overlay"), egui::Sense::drag());
                    if drag.drag_started()
                        && drag
                            .interact_pointer_pos()
                            .is_some_and(|position| !grip.contains(position))
                    {
                        viewport_ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                    ui.painter().rect_filled(grip, 2.0, outline);
                    let resize =
                        ui.interact(grip, ui.id().with("resize-grip"), egui::Sense::drag());
                    let start_size_id = Id::new(("overlay-resize-start", &name));
                    if resize.drag_started() {
                        viewport_ctx.data_mut(|data| data.insert_temp(start_size_id, rect.size()));
                    }
                    if resize.dragged() {
                        let start_size = viewport_ctx
                            .data(|data| data.get_temp::<egui::Vec2>(start_size_id))
                            .unwrap_or(rect.size());
                        let size = start_size + resize.drag_delta();
                        viewport_ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                            [size.x.max(80.0), size.y.max(60.0)].into(),
                        ));
                    }
                    if resize.drag_stopped() {
                        viewport_ctx.data_mut(|data| data.remove::<egui::Vec2>(start_size_id));
                    }
                });
        });

        if let Some(rect) = ctx.input_for(id, |input| input.viewport().inner_rect) {
            node.region = Some(Region {
                x: rect.min.x as i32,
                y: rect.min.y as i32,
                width: rect.width().max(80.0) as u32,
                height: rect.height().max(60.0) as u32,
            });
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("kind")
                    .selected_text(KINDS[self.kind])
                    .show_ui(ui, |ui| {
                        for (index, kind) in KINDS.iter().enumerate() {
                            ui.selectable_value(&mut self.kind, index, *kind);
                        }
                    });
                if ui.button("＋ Add node").clicked() {
                    self.add_node();
                }
                if ui
                    .button(if self.running { "Stop" } else { "Run" })
                    .clicked()
                {
                    self.running = !self.running;
                    self.last_tick = Instant::now();
                }
                ui.add(
                    egui::DragValue::new(&mut self.refresh_ms)
                        .range(100..=60_000)
                        .suffix(" ms"),
                );
            });
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.file);
                if ui.button("Save").clicked() {
                    self.sync_graph_positions();
                    self.status = std::fs::write(&self.file, self.graph.to_json())
                        .map_or_else(|error| error.to_string(), |_| "Saved workflow".into());
                }
                if ui.button("Load").clicked() {
                    match std::fs::read_to_string(&self.file)
                        .map_err(|error| error.to_string())
                        .and_then(|contents| Graph::from_json(&contents))
                    {
                        Ok(graph) => {
                            self.graph = graph;
                            self.selected = None;
                            self.rebuild_snarl();
                            self.status = "Loaded workflow".into();
                        }
                        Err(error) => self.status = error,
                    }
                }
                ui.label(&self.status);
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.columns(2, |columns| {
                let canvas = &mut columns[0];
                canvas.heading("Workflow");
                let graph_widget = SnarlWidget::new()
                    .id(Id::new("workflow-graph"))
                    .style(SnarlStyle::new())
                    .min_size(canvas.available_size());
                graph_widget.show(
                    &mut self.snarl,
                    &mut GraphViewer {
                        graph: &mut self.graph,
                        status: &mut self.status,
                    },
                    canvas,
                );
                self.selected = graph_widget
                    .get_selected_nodes_at(canvas.id(), canvas.ctx())
                    .first()
                    .map(|node| node.0);

                let settings = &mut columns[1];
                settings.heading("Node settings");
                if let Some(index) = self
                    .selected
                    .filter(|index| *index < self.graph.nodes.len())
                {
                    let node = &mut self.graph.nodes[index];
                    settings.label(format!("{} · {}", node.name, node.type_name()));
                    settings.separator();
                    match &mut node.kind {
                        Kind::Model { format } => {
                            for (choice, label) in [
                                (Format::Text, "Text"),
                                (Format::Number, "Number"),
                                (Format::Int, "Integer"),
                                (Format::Bool, "Boolean"),
                            ] {
                                settings.radio_value(format, choice, label);
                            }
                        }
                        Kind::Function { code } => {
                            settings.label("Rhai expression");
                            settings.add(
                                egui::TextEdit::multiline(code)
                                    .code_editor()
                                    .desired_rows(5),
                            );
                        }
                        Kind::Keyboard { text } => {
                            settings.label("Text to type (empty uses input)");
                            settings.text_edit_singleline(text);
                        }
                        Kind::Click { button } => {
                            settings.label("Mouse button");
                            settings.text_edit_singleline(button);
                        }
                        Kind::Ocr { .. } => {
                            settings.label("Reads text from its screen region.");
                        }
                    }
                    if node.region.is_some() {
                        settings.separator();
                        let mut color =
                            Color32::from_rgb(node.color[0], node.color[1], node.color[2]);
                        settings.horizontal(|ui| {
                            ui.color_edit_button_srgba(&mut color);
                            node.color = [color.r(), color.g(), color.b()];
                        });
                        settings.add(
                            egui::Slider::new(&mut node.opacity, 0.05..=1.0)
                                .text("Overlay opacity"),
                        );
                        settings
                            .label("Opacity applies to the fill, outline, label, and resize grip.");
                    }
                    settings.separator();
                    if settings.button("Delete node").clicked() {
                        self.delete_selected();
                    }
                } else {
                    settings.label("Select a node in the canvas to edit it.");
                    settings.label("Drag between pins to create or remove connections.");
                }
            });
        });

        for index in 0..self.graph.nodes.len() {
            self.overlay(&ctx, index);
        }

        if self.running && self.last_tick.elapsed() >= Duration::from_millis(self.refresh_ms) {
            self.last_tick = Instant::now();
            if let Some(backend) = self.backend.as_mut() {
                if let Err(error) = self.graph.run_once(backend) {
                    self.status = error;
                }
            } else {
                self.status = "Desktop backend is unavailable".into();
                self.running = false;
            }
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

fn main() -> eframe::Result {
    let (backend, status) = match Desktop::new() {
        Ok(backend) => (Some(backend), String::new()),
        Err(error) => (None, format!("Desktop backend unavailable: {error}")),
    };
    let mut app = App {
        graph: Graph::default(),
        snarl: Snarl::new(),
        backend,
        kind: 0,
        selected: None,
        running: false,
        last_tick: Instant::now(),
        refresh_ms: 1000,
        file: "graph.json".into(),
        status,
    };
    app.rebuild_snarl();
    eframe::run_native(
        "ScreenAutomation",
        Default::default(),
        Box::new(move |_| Ok(Box::new(app))),
    )
}
