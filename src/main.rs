use eframe::egui::{self, Color32, Id, Pos2, ViewportBuilder, ViewportId};
use egui_snarl::{
    ui::{
        BackgroundPattern, Grid, NodeLayout, NodeLayoutKind, PinInfo, PinPlacement, SnarlStyle,
        SnarlViewer, SnarlWidget,
    },
    InPin, OutPin, Snarl,
};
use screen_automation::{
    backend::{Backend, Region},
    desktop::Desktop,
    graph::Graph,
    node::{CompareOp, Crossing, Format, Kind, Node, Value},
};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

struct App {
    graph: Graph,
    snarl: Snarl<usize>,
    backend: Option<Desktop>,
    kind: usize,
    selected: Option<usize>,
    add_menu_open: bool,
    running: bool,
    overlay_ready_at: Option<Instant>,
    last_tick: Instant,
    last_run_duration: Option<Duration>,
    last_run_interval: Option<Duration>,
    refresh_ms: u64,
    refresh_input: String,
    region_scales: HashMap<String, f32>,
    /// When each node last produced output, and that output as text.
    activity: HashMap<String, (Instant, String)>,
    escape_pressed: Arc<AtomicBool>,
    overlay_anchors: HashMap<String, Region>,
    overlay_targets: HashMap<String, (Region, u8)>,
    file: String,
    status: String,
}

const KINDS: [&str; 7] = [
    "ocr",
    "model",
    "function",
    "compare",
    "threshold",
    "keyboard",
    "click",
];
const GRAPH_ID: &str = "workflow-graph";
const ADD_BUTTON_ID: &str = "canvas-add-button";
const TRASH_BUTTON_ID: &str = "canvas-trash-button";
const TRASH_RECT_ID: &str = "trash-rect";
const DEFAULT_REFRESH_MS: u64 = 3000;
const OVERLAY_SETTLE_DELAY: Duration = Duration::from_millis(150);
const OVERLAY_MARGIN: f32 = 6.0;
const OVERLAY_HEADER: f32 = 24.0;
const OVERLAY_PLACEMENT_ATTEMPTS: u8 = 5;

fn overlay_capture_rect(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        rect.min + egui::vec2(OVERLAY_MARGIN, OVERLAY_HEADER),
        rect.max - egui::vec2(OVERLAY_MARGIN, OVERLAY_MARGIN),
    )
}

fn capture_region(inner_rect: egui::Rect) -> Region {
    let rect = overlay_capture_rect(inner_rect);
    Region {
        x: rect.min.x.round() as i32,
        y: rect.min.y.round() as i32,
        width: rect.width().round().max(80.0) as u32,
        height: rect.height().round().max(60.0) as u32,
    }
}

/// Outer window position that puts the capture area at `target`, or `None` when it is already there.
/// The inner/outer offset (e.g. the invisible resize frame on Windows) is taken into account.
fn overlay_correction(
    target: Region,
    inner_rect: egui::Rect,
    outer_rect: egui::Rect,
) -> Option<Pos2> {
    let current = capture_region(inner_rect);
    if current.x == target.x && current.y == target.y {
        return None;
    }
    Some(outer_rect.min + egui::vec2((target.x - current.x) as f32, (target.y - current.y) as f32))
}

fn paint_overlay(
    ui: &egui::Ui,
    rect: egui::Rect,
    name: &str,
    fill: Color32,
    outline: Color32,
    title: Color32,
    running: bool,
) -> egui::Rect {
    let capture_rect = overlay_capture_rect(rect);
    if !running {
        ui.painter().rect_filled(capture_rect, 0.0, fill);
    }
    ui.painter().rect_stroke(
        capture_rect.expand(4.0),
        0.0,
        egui::Stroke::new(2.0, outline),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        capture_rect.left_top() - egui::vec2(0.0, 6.0),
        egui::Align2::LEFT_BOTTOM,
        name,
        egui::FontId::proportional(14.0),
        title,
    );
    let grip = egui::Rect::from_min_size(
        capture_rect.right_bottom() - egui::vec2(18.0, 18.0),
        egui::vec2(18.0, 18.0),
    );
    if !running {
        ui.painter().rect_filled(grip, 2.0, outline);
    }
    grip
}

fn next_run_at(
    last_start: Instant,
    interval: Duration,
    overlay_ready_at: Option<Instant>,
) -> Instant {
    let deadline = last_start + interval;
    overlay_ready_at.map_or(deadline, |ready| deadline.max(ready))
}

fn physical_region(region: Region, scale: f32) -> Region {
    let left = (region.x as f32 * scale).round();
    let top = (region.y as f32 * scale).round();
    let right = ((region.x as f32 + region.width as f32) * scale).round();
    let bottom = ((region.y as f32 + region.height as f32) * scale).round();
    Region {
        x: left as i32,
        y: top as i32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    }
}

fn refresh_interval(input: &str) -> Option<u64> {
    input
        .parse::<u64>()
        .ok()
        .filter(|value| (100..=60_000).contains(value))
}

fn run_with_physical_regions(
    graph: &mut Graph,
    backend: &mut dyn Backend,
    scales: &HashMap<String, f32>,
) -> Result<HashMap<String, Option<Value>>, String> {
    let mut regions = Vec::new();
    for (index, node) in graph.nodes.iter().enumerate() {
        if let Some(region) = node.region {
            let Some(&scale) = scales.get(&node.name) else {
                return Err(format!(
                    "Screen region for {} is not available yet; try again",
                    node.name
                ));
            };
            regions.push((index, region, physical_region(region, scale)));
        }
    }
    for (index, _, physical) in &regions {
        graph.nodes[*index].region = Some(*physical);
    }
    let result = graph.run_once(backend);
    for (index, original, _) in regions {
        graph.nodes[index].region = Some(original);
    }
    result
}

const WIRE_FLASH: Duration = Duration::from_millis(1000);
const WIRE_IDLE: Color32 = Color32::from_gray(150);
const WIRE_ACTIVE: Color32 = Color32::from_rgb(30, 200, 80);

/// 1.0 right after `name` produced output, fading to 0.0 over `WIRE_FLASH`.
fn wire_glow(activity: &HashMap<String, (Instant, String)>, name: &str, now: Instant) -> f32 {
    activity.get(name).map_or(0.0, |(at, _)| {
        1.0 - (now.saturating_duration_since(*at).as_secs_f32() / WIRE_FLASH.as_secs_f32())
            .min(1.0)
    })
}

const PIN_SIZE: f32 = 10.0;
const PIN_FILL: Color32 = Color32::from_gray(217);
const PIN_STROKE: Color32 = Color32::from_gray(153);

fn wire_pin(glow: f32) -> PinInfo {
    let color = WIRE_IDLE.lerp_to_gamma(WIRE_ACTIVE, glow);
    PinInfo::square()
        .with_wire_color(color)
        .with_fill(if glow > 0.0 { color } else { PIN_FILL })
        .with_stroke(egui::Stroke::new(1.0, PIN_STROKE))
}

const NODE_HEIGHT: f32 = 34.0;
const NODE_ICON_WIDTH: f32 = 30.0;
const NODE_MIN_WIDTH: f32 = 120.0;
const NODE_RADIUS: u8 = 5;
const NODE_TEXT: Color32 = Color32::from_gray(51);
const NODE_SELECTED: Color32 = Color32::from_rgb(255, 127, 14);

/// Node-RED style palette and icon for each node type.
fn node_style(kind: &Kind) -> (Color32, &'static str) {
    match kind {
        Kind::Ocr { .. } => (Color32::from_rgb(135, 192, 200), "👁"),
        Kind::Model { .. } => (Color32::from_rgb(226, 162, 90), "#"),
        Kind::Function { .. } => (Color32::from_rgb(253, 208, 162), "ƒ"),
        Kind::Compare { .. } => (Color32::from_rgb(226, 217, 110), "⚖"),
        Kind::Threshold { .. } => (Color32::from_rgb(215, 215, 160), "↕"),
        Kind::Keyboard { .. } => (Color32::from_rgb(192, 184, 224), "⌨"),
        Kind::Click { .. } => (Color32::from_rgb(166, 187, 207), "🖱"),
    }
}

fn node_label(node: &Node) -> String {
    match &node.kind {
        Kind::Compare { op, value, .. } => format!("{}  {} {value}", node.name, op.symbol()),
        Kind::Threshold {
            upper,
            lower,
            trigger,
            ..
        } => match trigger {
            Crossing::Rising => format!("{}  ↑ {upper}", node.name),
            Crossing::Falling => format!("{}  ↓ {lower}", node.name),
            Crossing::Both => format!("{}  ↑ {upper} ↓ {lower}", node.name),
        },
        _ => node.name.clone(),
    }
}

fn node_description(node: &Node) -> String {
    match &node.kind {
        Kind::Ocr { .. } => "Read text from a screen region".into(),
        Kind::Model { format } => match format {
            Format::Text => "Convert input to text",
            Format::Number => "Convert input to number",
            Format::Int => "Convert input to integer",
            Format::Bool => "Convert input to boolean",
        }
        .into(),
        Kind::Function { .. } => "Evaluate a Rhai expression".into(),
        Kind::Compare { op, value, .. } => format!("Trigger if input {} {value}", op.symbol()),
        Kind::Threshold {
            upper,
            lower,
            trigger,
            ..
        } => match trigger {
            Crossing::Rising => format!("Trigger when input rises above {upper}"),
            Crossing::Falling => format!("Trigger when input falls below {lower}"),
            Crossing::Both => {
                format!("Trigger when input rises above {upper} or falls below {lower}")
            }
        },
        Kind::Keyboard { .. } => "Type text into a screen region".into(),
        Kind::Click { .. } => "Click a screen region".into(),
    }
}

fn graph_style() -> SnarlStyle {
    let mut style = SnarlStyle::new();
    style.collapsible = Some(false);
    style.header_drag_space = Some(egui::Vec2::ZERO);
    style.header_frame = Some(egui::Frame::NONE);
    style.node_frame = Some(
        egui::Frame::NONE
            .corner_radius(NODE_RADIUS)
            .stroke(egui::Stroke::new(1.0, PIN_STROKE)),
    );
    style.node_layout = Some(NodeLayout {
        kind: NodeLayoutKind::Coil,
        min_pin_row_height: NODE_HEIGHT,
        equal_pin_row_heights: true,
    });
    style.pin_size = Some(PIN_SIZE);
    style.pin_placement = Some(PinPlacement::Outside {
        margin: -PIN_SIZE / 2.0,
    });
    style.wire_width = Some(3.0);
    style.bg_frame = Some(egui::Frame::NONE.fill(Color32::WHITE));
    style.bg_pattern = Some(BackgroundPattern::Grid(Grid {
        spacing: egui::vec2(20.0, 20.0),
        angle: 0.0,
    }));
    style.bg_pattern_stroke = Some(egui::Stroke::new(1.0, Color32::from_gray(235)));
    style
}

// egui-snarl uses this ID for the draggable node frame.
fn node_frame_id(node: egui_snarl::NodeId) -> Id {
    Id::new(GRAPH_ID).with(("snarl-node", node)).with("frame")
}

fn icon_button(
    ui: &mut egui::Ui,
    id: Id,
    tooltip: &str,
    icon: CanvasIcon,
    active: bool,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
    let response = ui.interact(rect, id, egui::Sense::click());
    let visuals = ui.style().interact(&response);
    let fill = if active {
        Color32::from_rgb(170, 45, 45)
    } else {
        visuals.bg_fill
    };
    let stroke = egui::Stroke::new(1.5, visuals.fg_stroke.color);
    let center = rect.center();
    ui.painter().circle_filled(center, 20.0, fill);
    ui.painter().circle_stroke(center, 20.0, stroke);
    match icon {
        CanvasIcon::Add => {
            ui.painter().line_segment(
                [center - egui::vec2(7.0, 0.0), center + egui::vec2(7.0, 0.0)],
                egui::Stroke::new(2.5, visuals.fg_stroke.color),
            );
            ui.painter().line_segment(
                [center - egui::vec2(0.0, 7.0), center + egui::vec2(0.0, 7.0)],
                egui::Stroke::new(2.5, visuals.fg_stroke.color),
            );
        }
        CanvasIcon::Trash => {
            let body =
                egui::Rect::from_center_size(center + egui::vec2(0.0, 3.0), egui::vec2(13.0, 15.0));
            ui.painter().rect_stroke(
                body,
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            ui.painter().line_segment(
                [
                    center + egui::vec2(-9.0, -7.0),
                    center + egui::vec2(9.0, -7.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    center + egui::vec2(-4.0, -10.0),
                    center + egui::vec2(4.0, -10.0),
                ],
                stroke,
            );
            for x in [-3.0, 3.0] {
                ui.painter().line_segment(
                    [center + egui::vec2(x, -1.0), center + egui::vec2(x, 8.0)],
                    stroke,
                );
            }
        }
    }
    response.on_hover_text(tooltip)
}

#[derive(Clone, Copy)]
enum CanvasIcon {
    Add,
    Trash,
}

fn new_kind(i: usize) -> Kind {
    match i {
        0 => Kind::Ocr { last_hash: None },
        1 => Kind::Model {
            format: Format::Number,
        },
        2 => Kind::Function {
            code: "inputs[0]".into(),
        },
        3 => Kind::Compare {
            op: CompareOp::Greater,
            value: "0".into(),
            on_change: true,
            last: None,
        },
        4 => Kind::Threshold {
            upper: 100.0,
            lower: 90.0,
            trigger: Crossing::Both,
            band: None,
        },
        5 => Kind::Keyboard {
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
    activity: &'a HashMap<String, (Instant, String)>,
    selected: Option<usize>,
    now: Instant,
}

impl GraphViewer<'_> {
    fn input_glow(&self, index: usize) -> f32 {
        let name = &self.graph.nodes[index].name;
        self.graph
            .edges
            .iter()
            .filter(|(_, destination)| destination == name)
            .map(|(source, _)| wire_glow(self.activity, source, self.now))
            .fold(0.0, f32::max)
    }
}

impl SnarlViewer<usize> for GraphViewer<'_> {
    fn title(&mut self, node: &usize) -> String {
        node_label(&self.graph.nodes[*node])
    }

    fn node_frame(
        &mut self,
        default: egui::Frame,
        node: egui_snarl::NodeId,
        _: &[InPin],
        _: &[OutPin],
        snarl: &Snarl<usize>,
    ) -> egui::Frame {
        let index = snarl[node];
        let frame = default.fill(node_style(&self.graph.nodes[index].kind).0);
        if self.selected == Some(index) {
            frame.stroke(egui::Stroke::new(2.0, NODE_SELECTED))
        } else {
            frame
        }
    }

    fn has_node_style(
        &mut self,
        _: egui_snarl::NodeId,
        _: &[InPin],
        _: &[OutPin],
        _: &Snarl<usize>,
    ) -> bool {
        true
    }

    fn apply_node_style(
        &mut self,
        style: &mut egui::Style,
        _: egui_snarl::NodeId,
        _: &[InPin],
        _: &[OutPin],
        _: &Snarl<usize>,
    ) {
        // Lets the body fill the whole frame so pins sit exactly on its edges.
        style.spacing.item_spacing = egui::Vec2::ZERO;
    }

    fn show_header(
        &mut self,
        _: egui_snarl::NodeId,
        _: &[InPin],
        _: &[OutPin],
        _: &mut egui::Ui,
        _: &mut Snarl<usize>,
    ) {
    }

    fn inputs(&mut self, node: &usize) -> usize {
        usize::from(!matches!(self.graph.nodes[*node].kind, Kind::Ocr { .. }))
    }

    fn show_input(
        &mut self,
        pin: &InPin,
        _: &mut egui::Ui,
        snarl: &mut Snarl<usize>,
    ) -> impl egui_snarl::ui::SnarlPin + 'static {
        wire_pin(self.input_glow(snarl[pin.id.node]))
    }

    fn outputs(&mut self, _: &usize) -> usize {
        1
    }

    fn show_output(
        &mut self,
        pin: &OutPin,
        _: &mut egui::Ui,
        snarl: &mut Snarl<usize>,
    ) -> impl egui_snarl::ui::SnarlPin + 'static {
        let name = &self.graph.nodes[snarl[pin.id.node]].name;
        wire_pin(wire_glow(self.activity, name, self.now))
    }

    fn has_body(&mut self, _: &usize) -> bool {
        true
    }

    fn show_body(
        &mut self,
        node: egui_snarl::NodeId,
        inputs: &[InPin],
        _: &[OutPin],
        ui: &mut egui::Ui,
        snarl: &mut Snarl<usize>,
    ) {
        let index = snarl[node];
        let input_glow = self.input_glow(index);
        let node = &self.graph.nodes[index];
        let (_, icon) = node_style(&node.kind);
        let label = ui.painter().layout_no_wrap(
            node_label(node),
            egui::FontId::proportional(14.0),
            NODE_TEXT,
        );
        let width = (NODE_ICON_WIDTH + label.size().x + 24.0).max(NODE_MIN_WIDTH);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, NODE_HEIGHT), egui::Sense::hover());
        let painter = ui.painter();

        let radius = NODE_RADIUS;
        let icon_rect =
            egui::Rect::from_min_size(rect.min, egui::vec2(NODE_ICON_WIDTH, rect.height()));
        painter.rect_filled(
            icon_rect,
            egui::CornerRadius { nw: radius, sw: radius, ne: 0, se: 0 },
            Color32::from_black_alpha(20),
        );
        painter.vline(
            icon_rect.right(),
            icon_rect.y_range(),
            egui::Stroke::new(1.0, Color32::from_black_alpha(30)),
        );
        painter.text(
            icon_rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            egui::FontId::proportional(16.0),
            Color32::WHITE,
        );
        painter.galley(
            egui::pos2(icon_rect.right() + 12.0, rect.center().y - label.size().y / 2.0),
            label,
            NODE_TEXT,
        );
        // The icon strip is painted after the pins; redraw the input pin's inner half over it.
        if !inputs.is_empty() {
            let pin = egui::Rect::from_center_size(
                egui::pos2(rect.left(), rect.center().y),
                egui::vec2(PIN_SIZE, PIN_SIZE),
            );
            let fill = if input_glow > 0.0 {
                WIRE_IDLE.lerp_to_gamma(WIRE_ACTIVE, input_glow)
            } else {
                PIN_FILL
            };
            painter.rect(
                pin,
                2,
                fill,
                egui::Stroke::new(1.0, PIN_STROKE),
                egui::StrokeKind::Inside,
            );
        }
    }

    fn final_node_rect(
        &mut self,
        node: egui_snarl::NodeId,
        rect: egui::Rect,
        ui: &mut egui::Ui,
        snarl: &mut Snarl<usize>,
    ) {
        let name = &self.graph.nodes[snarl[node]].name;
        let Some((_, last)) = self.activity.get(name) else {
            return;
        };
        let mut last = last.replace('\n', " ");
        if last.chars().count() > 32 {
            last = last.chars().take(31).collect::<String>() + "…";
        }
        let glow = wire_glow(self.activity, name, self.now);
        let dot = egui::pos2(rect.left() + 6.0, rect.bottom() + 10.0);
        ui.painter().rect_filled(
            egui::Rect::from_center_size(dot, egui::vec2(8.0, 8.0)),
            1,
            Color32::from_gray(170).lerp_to_gamma(WIRE_ACTIVE, glow),
        );
        ui.painter().text(
            dot + egui::vec2(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            last,
            egui::FontId::proportional(11.0),
            Color32::from_gray(110),
        );
    }

    fn has_on_hover_popup(&mut self, _: &usize) -> bool {
        true
    }

    fn show_on_hover_popup(
        &mut self,
        node: egui_snarl::NodeId,
        _: &[InPin],
        _: &[OutPin],
        ui: &mut egui::Ui,
        snarl: &mut Snarl<usize>,
    ) {
        let node = &self.graph.nodes[snarl[node]];
        ui.strong(format!("{} · {}", node.name, node.type_name()));
        ui.label(node_description(node));
        if let Some((_, last)) = self.activity.get(&node.name) {
            ui.weak(format!("Last output: {last}"));
        }
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
        self.sync_graph_positions();
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
                if self.running {
                    self.overlay_ready_at = Some(Instant::now() + OVERLAY_SETTLE_DELAY);
                }
                self.rebuild_snarl();
                self.status.clear();
            }
            Err(error) => self.status = error,
        }
    }

    fn delete_node(&mut self, index: usize) {
        if let Some(node) = self.graph.nodes.get(index) {
            let name = node.name.clone();
            self.sync_graph_positions();
            self.graph.remove_node(&name);
            self.region_scales.remove(&name);
            self.activity.remove(&name);
            self.overlay_anchors.remove(&name);
            self.overlay_targets.remove(&name);
            self.selected = None;
            self.rebuild_snarl();
            self.status = format!("Deleted {name}");
        }
    }

    fn run_graph(&mut self) {
        if let Some(backend) = self.backend.as_mut() {
            match run_with_physical_regions(&mut self.graph, backend, &self.region_scales) {
                Ok(outputs) => {
                    let now = Instant::now();
                    for (name, value) in outputs {
                        if let Some(value) = value {
                            self.activity.insert(name, (now, value.to_text()));
                        }
                    }
                }
                Err(error) => self.status = error,
            }
        } else {
            self.status = "Desktop backend is unavailable".into();
            self.running = false;
        }
    }

    fn show_settings(&mut self, ctx: &egui::Context) {
        let Some(index) = self
            .selected
            .filter(|index| *index < self.graph.nodes.len())
        else {
            return;
        };
        let mut open = true;
        let mut delete = false;
        egui::Window::new("Node settings")
            .id(Id::new("node-settings"))
            .open(&mut open)
            .show(ctx, |settings| {
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
                    Kind::Compare {
                        op,
                        value,
                        on_change,
                        last,
                    } => {
                        let before = (*op, value.clone(), *on_change);
                        settings.label("Trigger when the input is");
                        egui::ComboBox::from_id_salt("compare-op")
                            .selected_text(format!("{}  {}", op.symbol(), op.label()))
                            .show_ui(settings, |ui| {
                                for choice in CompareOp::ALL {
                                    ui.selectable_value(
                                        op,
                                        choice,
                                        format!("{}  {}", choice.symbol(), choice.label()),
                                    );
                                }
                            });
                        settings.label("Value");
                        settings.text_edit_singleline(value);
                        settings.checkbox(on_change, "Only trigger when the condition becomes true");
                        settings.weak(
                            "Numbers are read from the input text. When the condition holds, \
                             the input is passed on, e.g. to a Click node.",
                        );
                        if before != (*op, value.clone(), *on_change) {
                            *last = None;
                        }
                    }
                    Kind::Threshold {
                        upper,
                        lower,
                        trigger,
                        band,
                    } => {
                        let before = (*upper, *lower, *trigger);
                        settings.label("Trigger when the input");
                        egui::ComboBox::from_id_salt("threshold-trigger")
                            .selected_text(trigger.label())
                            .show_ui(settings, |ui| {
                                for choice in Crossing::ALL {
                                    ui.selectable_value(trigger, choice, choice.label());
                                }
                            });
                        egui::Grid::new("threshold-values").show(settings, |ui| {
                            ui.label("Upper threshold");
                            ui.add(egui::DragValue::new(upper).speed(0.01).max_decimals(6));
                            ui.end_row();
                            ui.label("Lower threshold");
                            ui.add(egui::DragValue::new(lower).speed(0.01).max_decimals(6));
                            ui.end_row();
                        });
                        if lower > upper {
                            settings.colored_label(
                                settings.visuals().error_fg_color,
                                "Lower threshold must not be above the upper threshold.",
                            );
                        }
                        settings.weak(
                            "Triggers when the value moves from at or below the upper threshold \
                             to above it, or from at or above the lower threshold to below it. \
                             After a crossing, the value must cross the other threshold before \
                             the same direction can trigger again. The first reading never \
                             triggers. When it triggers, the input is passed on.",
                        );
                        if before != (*upper, *lower, *trigger) {
                            *band = None;
                        }
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
                    let mut color = Color32::from_rgb(node.color[0], node.color[1], node.color[2]);
                    settings.color_edit_button_srgba(&mut color);
                    node.color = [color.r(), color.g(), color.b()];
                    settings.add(
                        egui::Slider::new(&mut node.opacity, 0.05..=1.0).text("Overlay opacity"),
                    );
                    settings.label("Opacity applies to the fill, outline, label, and resize grip.");
                }
                settings.separator();
                delete = settings.button("Delete node").clicked();
            });
        if delete {
            self.delete_node(index);
        } else if !open {
            self.selected = None;
        }
    }

    fn show_canvas(&mut self, ui: &mut egui::Ui) {
        let canvas_rect = ui.max_rect();
        let ctx = ui.ctx().clone();
        // Selectable labels swallow clicks, which stopped clicks on node text from selecting the node.
        ui.style_mut().interaction.selectable_labels = false;
        let graph_widget = SnarlWidget::new()
            .id(Id::new(GRAPH_ID))
            .style(graph_style())
            .min_size(ui.available_size());
        let now = Instant::now();
        graph_widget.show(
            &mut self.snarl,
            &mut GraphViewer {
                graph: &mut self.graph,
                status: &mut self.status,
                activity: &self.activity,
                selected: self.selected,
                now,
            },
            ui,
        );
        if self
            .activity
            .values()
            .any(|(at, _)| now.saturating_duration_since(*at) < WIRE_FLASH)
        {
            ctx.request_repaint();
        }

        for (node_id, index) in self.snarl.node_ids() {
            if ctx
                .read_response(node_frame_id(node_id))
                .is_some_and(|response| response.clicked())
            {
                self.selected = Some(*index);
            }
        }

        egui::Area::new(Id::new("canvas-add"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(canvas_rect.right_top() + egui::vec2(-12.0, 12.0))
            .show(&ctx, |ui| {
                let response = icon_button(
                    ui,
                    Id::new(ADD_BUTTON_ID),
                    "Add node",
                    CanvasIcon::Add,
                    self.add_menu_open,
                );
                ui.ctx()
                    .data_mut(|data| data.insert_temp(Id::new("add-button-rect"), response.rect));
                if response.clicked() {
                    self.add_menu_open = !self.add_menu_open;
                }
            });
        if self.add_menu_open {
            let mut selected_kind = None;
            egui::Area::new(Id::new("canvas-add-menu"))
                .order(egui::Order::Foreground)
                .pivot(egui::Align2::RIGHT_TOP)
                .fixed_pos(canvas_rect.right_top() + egui::vec2(-12.0, 64.0))
                .show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        for (index, kind) in KINDS.iter().enumerate() {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(112.0, 30.0), egui::Sense::hover());
                            let response = ui.interact(
                                rect,
                                Id::new(("canvas-add-kind", index)),
                                egui::Sense::click(),
                            );
                            ui.ctx().data_mut(|data| {
                                data.insert_temp(Id::new(("canvas-add-kind-rect", index)), rect);
                            });
                            ui.painter().rect_filled(
                                rect,
                                egui::CornerRadius::same(3),
                                if response.hovered() {
                                    ui.visuals().widgets.hovered.bg_fill
                                } else {
                                    ui.visuals().widgets.inactive.bg_fill
                                },
                            );
                            ui.painter().text(
                                rect.left_center() + egui::vec2(8.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                kind,
                                egui::FontId::proportional(14.0),
                                ui.visuals().text_color(),
                            );
                            if response.clicked() {
                                selected_kind = Some(index);
                            }
                        }
                    });
                });
            if let Some(index) = selected_kind {
                self.kind = index;
                self.add_node();
                self.add_menu_open = false;
            }
        }

        let dragging = ctx.dragged_id();
        let stopped = ctx.drag_stopped_id();
        let dragged_node = self.snarl.node_ids().find_map(|(id, index)| {
            (Some(node_frame_id(id)) == dragging || Some(node_frame_id(id)) == stopped)
                .then_some(*index)
        });
        let trash_pos = canvas_rect.right_bottom() + egui::vec2(-12.0, -12.0);
        let mut trash_rect = egui::Rect::NOTHING;
        let previous_trash_rect =
            ctx.data(|data| data.get_temp::<egui::Rect>(Id::new(TRASH_RECT_ID)));
        let mut trash_clicked = false;
        egui::Area::new(Id::new("canvas-trash"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_BOTTOM)
            .fixed_pos(trash_pos)
            .show(&ctx, |ui| {
                let over = dragged_node.is_some()
                    && ctx.pointer_latest_pos().is_some_and(|pos| {
                        previous_trash_rect.is_some_and(|rect| rect.contains(pos))
                    });
                let response = icon_button(
                    ui,
                    Id::new(TRASH_BUTTON_ID),
                    if over {
                        "Release to delete node"
                    } else {
                        "Delete selected node, or drop a node here"
                    },
                    CanvasIcon::Trash,
                    over,
                );
                trash_rect = response.rect.expand(4.0);
                trash_clicked = response.clicked();
            });
        ctx.data_mut(|data| data.insert_temp(Id::new(TRASH_RECT_ID), trash_rect));

        if stopped.is_some()
            && ctx
                .pointer_latest_pos()
                .is_some_and(|pos| trash_rect.contains(pos))
        {
            if let Some(index) = dragged_node {
                self.delete_node(index);
            }
        } else if trash_clicked {
            if let Some(index) = self.selected {
                self.delete_node(index);
            } else {
                self.status = "Select a node or drag one onto the trash icon".into();
            }
        }
        self.sync_graph_positions();
        self.show_settings(&ctx);
    }

    fn overlay(&mut self, ctx: &egui::Context, idx: usize) {
        let running = self.running;
        let node = &mut self.graph.nodes[idx];
        let Some(region) = node.region else {
            return;
        };
        // The builder position/size must stay constant: egui re-applies any change to it as a
        // window move, and feeding the measured region back in makes the window creep each frame.
        let anchor = match self.overlay_anchors.get(&node.name) {
            Some(anchor) => *anchor,
            None => {
                self.overlay_anchors.insert(node.name.clone(), region);
                self.overlay_targets
                    .insert(node.name.clone(), (region, OVERLAY_PLACEMENT_ATTEMPTS));
                region
            }
        };
        let id = ViewportId::from_hash_of(("overlay", &node.name));
        let builder = ViewportBuilder::default()
            .with_title(node.name.clone())
            .with_decorations(false)
            .with_transparent(true)
            .with_has_shadow(false)
            .with_resizable(true)
            .with_min_inner_size([
                80.0 + 2.0 * OVERLAY_MARGIN,
                60.0 + OVERLAY_HEADER + OVERLAY_MARGIN,
            ])
            .with_always_on_top()
            .with_mouse_passthrough(running)
            .with_position([
                anchor.x as f32 - OVERLAY_MARGIN,
                anchor.y as f32 - OVERLAY_HEADER,
            ])
            .with_inner_size([
                anchor.width as f32 + 2.0 * OVERLAY_MARGIN,
                anchor.height as f32 + OVERLAY_HEADER + OVERLAY_MARGIN,
            ]);
        let [red, green, blue] = node.color;
        let alpha = (node.opacity.clamp(0.0, 1.0) * 255.0) as u8;
        let fill = Color32::from_rgba_unmultiplied(red, green, blue, alpha);
        let outline = Color32::from_rgba_unmultiplied(255, 40, 40, alpha);
        let title = Color32::from_rgba_unmultiplied(0, 0, 0, alpha);
        let name = node.name.clone();
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            let viewport_ctx = ctx.clone();
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    let rect = ui.max_rect();
                    let grip = paint_overlay(ui, rect, &name, fill, outline, title, running);
                    let drag = ui.interact(rect, ui.id().with("move-overlay"), egui::Sense::drag());
                    if drag.drag_started()
                        && drag
                            .interact_pointer_pos()
                            .is_some_and(|position| !grip.contains(position))
                    {
                        viewport_ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                    let resize =
                        ui.interact(grip, ui.id().with("resize-grip"), egui::Sense::drag());
                    let start_size_id = Id::new(("overlay-resize-start", &name));
                    let start_pos_id = Id::new(("overlay-resize-start-pos", &name));
                    if resize.drag_started() {
                        if let Some(pos) = ui.ctx().pointer_latest_pos() {
                            viewport_ctx.data_mut(|data| {
                                data.insert_temp(start_size_id, rect.size());
                                data.insert_temp(start_pos_id, pos);
                            });
                        }
                    }
                    if resize.dragged() {
                        let start_size =
                            viewport_ctx.data(|data| data.get_temp::<egui::Vec2>(start_size_id));
                        if let (Some(start_size), Some(current_pos)) =
                            (start_size, ui.ctx().pointer_latest_pos())
                        {
                            let start_pos = viewport_ctx
                                .data(|data| data.get_temp::<egui::Pos2>(start_pos_id))
                                .unwrap_or(current_pos);
                            let delta = current_pos - start_pos;
                            let size = start_size + egui::vec2(delta.x, delta.y);
                            viewport_ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                                [
                                    size.x.max(80.0 + 2.0 * OVERLAY_MARGIN),
                                    size.y.max(60.0 + OVERLAY_HEADER + OVERLAY_MARGIN),
                                ]
                                .into(),
                            ));
                        }
                    }
                    if resize.drag_stopped() {
                        viewport_ctx.data_mut(|data| {
                            data.remove::<egui::Vec2>(start_size_id);
                            data.remove::<egui::Pos2>(start_pos_id);
                        });
                    }
                });
        });

        let (inner, outer, scale) = ctx.input_for(id, |input| {
            let viewport = input.viewport();
            (viewport.inner_rect, viewport.outer_rect, input.pixels_per_point)
        });
        let Some(inner) = inner else {
            return;
        };
        if let Some((target, attempts)) = self.overlay_targets.get_mut(&node.name) {
            let correction = outer.and_then(|outer| overlay_correction(*target, inner, outer));
            match correction {
                Some(position) if *attempts > 0 => {
                    *attempts -= 1;
                    ctx.send_viewport_cmd_to(id, egui::ViewportCommand::OuterPosition(position));
                    ctx.request_repaint_of(id);
                    return;
                }
                _ => {
                    self.overlay_targets.remove(&node.name);
                }
            }
        }
        node.region = Some(capture_region(inner));
        if scale.is_finite() && scale > 0.0 {
            self.region_scales.insert(node.name.clone(), scale);
        }
    }
}

impl App {
    fn set_running(&mut self, running: bool) {
        self.running = running;
        self.last_tick = Instant::now();
        self.last_run_duration = None;
        self.last_run_interval = None;
        self.overlay_ready_at = running.then_some(self.last_tick + OVERLAY_SETTLE_DELAY);
    }

    /// Escape stops a running workflow, whether pressed in this window or (on Windows) any other.
    fn handle_escape(&mut self, ctx: &egui::Context) {
        let global = self.escape_pressed.swap(false, Ordering::Relaxed);
        let local = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        if self.running && (global || local) {
            self.set_running(false);
            self.status = "Stopped (Esc)".into();
        }
    }
}

#[cfg(windows)]
fn watch_escape_key(ctx: egui::Context, pressed: Arc<AtomicBool>) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
    std::thread::spawn(move || {
        let mut was_down = false;
        loop {
            // SAFETY: GetAsyncKeyState only reads global key state.
            let down = unsafe { GetAsyncKeyState(i32::from(VK_ESCAPE)) } as u16 & 0x8000 != 0;
            if down && !was_down {
                pressed.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            }
            was_down = down;
            std::thread::sleep(Duration::from_millis(30));
        }
    });
}

#[cfg(not(windows))]
fn watch_escape_key(_: egui::Context, _: Arc<AtomicBool>) {}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_escape(&ctx);
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button(if self.running { "Stop (Esc)" } else { "Run" })
                    .clicked()
                {
                    self.set_running(!self.running);
                }
                ui.label("Delay (ms)");
                if ui
                    .add(egui::TextEdit::singleline(&mut self.refresh_input).desired_width(65.0))
                    .changed()
                {
                    if let Some(value) = refresh_interval(&self.refresh_input) {
                        self.refresh_ms = value;
                    }
                }
                if refresh_interval(&self.refresh_input).is_none() {
                    ui.colored_label(Color32::RED, "Enter 100–60000 ms");
                }
                if let Some(duration) = self.last_run_duration {
                    let elapsed = duration.as_millis();
                    let interval = self.last_run_interval.map(|value| value.as_millis());
                    let text = match interval {
                        Some(interval) => format!("Run: {elapsed} ms, interval: {interval} ms"),
                        None => format!("Run: {elapsed} ms"),
                    };
                    ui.label(text).on_hover_text(
                        "Intervals are best-effort; slow captures or OCR can exceed the requested delay.",
                    );
                }
                if cfg!(debug_assertions) {
                    ui.label("Debug build: OCR is slower")
                        .on_hover_text("Run with cargo run --release --features gui for faster OCR.");
                }
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
                            self.region_scales.clear();
                            self.activity.clear();
                            self.overlay_anchors.clear();
                            self.overlay_targets.clear();
                            if self.running {
                                self.overlay_ready_at = Some(Instant::now() + OVERLAY_SETTLE_DELAY);
                            }
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
            self.show_canvas(ui);
        });

        for index in 0..self.graph.nodes.len() {
            self.overlay(&ctx, index);
        }

        if self.running {
            let now = Instant::now();
            let ready_at = next_run_at(
                self.last_tick,
                Duration::from_millis(self.refresh_ms),
                self.overlay_ready_at,
            );
            if now >= ready_at {
                if self.last_run_duration.is_some() {
                    self.last_run_interval = Some(now.duration_since(self.last_tick));
                }
                self.last_tick = now;
                self.overlay_ready_at = None;
                self.run_graph();
                self.last_run_duration = Some(now.elapsed());
            }
            if self.running {
                let ready_at = next_run_at(
                    self.last_tick,
                    Duration::from_millis(self.refresh_ms),
                    self.overlay_ready_at,
                );
                ctx.request_repaint_after(ready_at.saturating_duration_since(Instant::now()));
            }
        }
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
        add_menu_open: false,
        running: false,
        overlay_ready_at: None,
        last_tick: Instant::now(),
        last_run_duration: None,
        last_run_interval: None,
        refresh_ms: DEFAULT_REFRESH_MS,
        refresh_input: DEFAULT_REFRESH_MS.to_string(),
        region_scales: HashMap::new(),
        activity: HashMap::new(),
        escape_pressed: Arc::new(AtomicBool::new(false)),
        overlay_anchors: HashMap::new(),
        overlay_targets: HashMap::new(),
        file: "graph.json".into(),
        status,
    };
    app.rebuild_snarl();
    eframe::run_native(
        "ScreenAutomation",
        Default::default(),
        Box::new(move |cc| {
            watch_escape_key(cc.egui_ctx.clone(), app.escape_pressed.clone());
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_title_and_outline_are_outside_capture_region() {
        let window = egui::Rect::from_min_size(Pos2::new(94.0, 76.0), egui::vec2(312.0, 150.0));
        let capture = overlay_capture_rect(window);
        assert_eq!(
            capture,
            egui::Rect::from_min_size(Pos2::new(100.0, 100.0), egui::vec2(300.0, 120.0))
        );
        let title_bottom = capture.top() - 6.0;
        let outline_inner = capture.expand(4.0).shrink(2.0);
        assert!(title_bottom < capture.top());
        assert!(outline_inner.contains_rect(capture));
        assert_eq!(capture.left() - outline_inner.left(), 2.0);
        assert_eq!(capture.top() - outline_inner.top(), 2.0);
    }

    #[test]
    fn overlay_correction_accounts_for_invisible_window_frame() {
        let target = Region { x: 100, y: 100, width: 300, height: 120 };
        // Window placed at target minus margins, but the OS frame shifts the client area by (8, 1).
        let outer = egui::Rect::from_min_size(Pos2::new(94.0, 76.0), egui::vec2(328.0, 159.0));
        let inner = egui::Rect::from_min_size(Pos2::new(102.0, 77.0), egui::vec2(312.0, 150.0));
        assert_eq!(capture_region(inner), Region { x: 108, y: 101, width: 300, height: 120 });
        let position = overlay_correction(target, inner, outer).unwrap();
        assert_eq!(position, Pos2::new(86.0, 75.0));

        let moved = inner.translate(position - outer.min);
        assert_eq!(capture_region(moved), target);
        assert_eq!(
            overlay_correction(target, moved, outer.translate(position - outer.min)),
            None
        );
    }

    #[test]
    fn running_overlay_leaves_capture_region_clear() {
        let ctx = egui::Context::default();
        let window = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(312.0, 150.0));
        let capture = overlay_capture_rect(window);
        for running in [false, true] {
            let input = egui::RawInput {
                screen_rect: Some(window),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                paint_overlay(
                    ui,
                    window,
                    "ocr1",
                    Color32::BLUE,
                    Color32::RED,
                    Color32::BLACK,
                    running,
                );
            });
            let rects: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| {
                    if let egui::Shape::Rect(rect) = &shape.shape {
                        Some(rect)
                    } else {
                        None
                    }
                })
                .collect();
            let texts: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| {
                    if let egui::Shape::Text(text) = &shape.shape {
                        Some(text)
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(rects.len(), if running { 1 } else { 3 });
            assert_eq!(
                rects
                    .iter()
                    .filter(|rect| rect.fill == Color32::BLUE)
                    .count(),
                usize::from(!running)
            );
            let outline = rects
                .iter()
                .find(|rect| rect.stroke.color == Color32::RED)
                .unwrap();
            assert_eq!(outline.rect, capture.expand(4.0));
            assert_eq!(texts.len(), 1);
            assert!(texts[0].pos.y + texts[0].galley.size().y < capture.top());
            output.textures_delta.clear();
        }
    }

    #[test]
    fn interval_is_start_to_start_without_added_delay() {
        let start = Instant::now();
        let interval = Duration::from_millis(100);
        let ready = start + OVERLAY_SETTLE_DELAY;
        assert_eq!(next_run_at(start, interval, Some(ready)), ready);
        let deadline = next_run_at(ready, interval, None);
        assert_eq!(deadline.duration_since(ready), interval);
        assert_eq!(
            deadline.saturating_duration_since(ready + Duration::from_millis(40)),
            Duration::from_millis(60)
        );
        assert_eq!(
            deadline.saturating_duration_since(ready + Duration::from_millis(160)),
            Duration::ZERO
        );
    }

    #[test]
    fn converts_overlay_coordinates_to_physical_pixels() {
        let region = Region {
            x: -120,
            y: 80,
            width: 300,
            height: 120,
        };
        assert_eq!(
            physical_region(region, 1.5),
            Region {
                x: -180,
                y: 120,
                width: 450,
                height: 180
            }
        );
        assert_eq!(physical_region(region, 1.0), region);
    }

    #[test]
    fn validates_delay_input() {
        assert_eq!(DEFAULT_REFRESH_MS, 3000);
        assert_eq!(refresh_interval("3000"), Some(3000));
        assert_eq!(refresh_interval("100"), Some(100));
        assert_eq!(refresh_interval("60000"), Some(60000));
        for value in ["", "99", "60001", "-1", "abc", "1.5"] {
            assert_eq!(refresh_interval(value), None);
        }
    }

    #[test]
    fn workflow_uses_scaled_regions_and_restores_them_on_error() {
        #[derive(Default)]
        struct RecordingBackend {
            captured: Vec<Region>,
            clicks: Vec<(i32, i32)>,
            fail: bool,
        }
        impl Backend for RecordingBackend {
            fn capture(&mut self, region: &Region) -> Result<Vec<u8>, String> {
                self.captured.push(*region);
                Ok(vec![1])
            }
            fn ocr(&mut self, _: &Region, _: &[u8]) -> Result<String, String> {
                if self.fail {
                    return Err("OCR failed".into());
                }
                Ok("hello".into())
            }
            fn type_text(&mut self, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn click(&mut self, x: i32, y: i32, _: &str) -> Result<(), String> {
                self.clicks.push((x, y));
                Ok(())
            }
        }

        let mut graph = Graph::default();
        graph
            .add_node(Node::new("source", Kind::Ocr { last_hash: None }))
            .unwrap();
        graph
            .add_node(Node::new(
                "target",
                Kind::Keyboard {
                    text: String::new(),
                },
            ))
            .unwrap();
        graph.connect("source", "target").unwrap();
        let original = graph.nodes[0].region.unwrap();
        let scales = HashMap::from([("source".into(), 1.5), ("target".into(), 2.0)]);
        let mut backend = RecordingBackend {
            fail: true,
            ..Default::default()
        };
        assert!(run_with_physical_regions(&mut graph, &mut backend, &scales).is_err());
        assert_eq!(graph.nodes[0].region, Some(original));
        assert_eq!(graph.nodes[1].region, Some(original));
        backend.fail = false;
        run_with_physical_regions(&mut graph, &mut backend, &scales).unwrap();
        assert_eq!(backend.captured, vec![physical_region(original, 1.5); 2]);
        assert_eq!(
            backend.clicks,
            vec![physical_region(original, 2.0).center()]
        );
        assert_eq!(graph.nodes[0].region, Some(original));
        assert_eq!(graph.nodes[1].region, Some(original));
    }

    fn app() -> App {
        App {
            graph: Graph::default(),
            snarl: Snarl::new(),
            backend: None,
            kind: 1,
            selected: None,
            add_menu_open: false,
            running: false,
            overlay_ready_at: None,
            last_tick: Instant::now(),
            last_run_duration: None,
            last_run_interval: None,
            refresh_ms: DEFAULT_REFRESH_MS,
            refresh_input: DEFAULT_REFRESH_MS.to_string(),
            region_scales: HashMap::new(),
            activity: HashMap::new(),
            escape_pressed: Arc::new(AtomicBool::new(false)),
            overlay_anchors: HashMap::new(),
            overlay_targets: HashMap::new(),
            file: "graph.json".into(),
            status: String::new(),
        }
    }

    fn frame(ctx: &egui::Context, app: &mut App, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| app.show_canvas(ui));
        });
        output.textures_delta.clear();
    }

    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn node_position(ctx: &egui::Context, node: egui_snarl::NodeId) -> Pos2 {
        let response = ctx.read_response(node_frame_id(node)).unwrap();
        let transform = ctx.layer_transform_to_global(response.layer_id).unwrap();
        transform * response.rect.center()
    }

    #[test]
    fn escape_stops_a_running_workflow() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.set_running(true);
        app.escape_pressed.store(true, Ordering::Relaxed);
        app.handle_escape(&ctx);
        assert!(!app.running);
        assert!(!app.escape_pressed.load(Ordering::Relaxed));

        app.set_running(true);
        let key = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let input = egui::RawInput {
            events: vec![key],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.handle_escape(ui.ctx()));
        output.textures_delta.clear();
        assert!(!app.running);
        assert_eq!(app.status, "Stopped (Esc)");
    }

    #[test]
    fn clicking_node_text_selects_the_node() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.kind = 2;
        app.add_node();
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![]);
        let node = app.snarl.node_ids().next().unwrap().0;
        let response = ctx.read_response(node_frame_id(node)).unwrap();
        let rect = ctx.layer_transform_to_global(response.layer_id).unwrap() * response.rect;
        // Points across the title and body text, away from the pins and collapse button.
        for fy in [0.15, 0.5, 0.75] {
            for fx in [0.35, 0.5, 0.65] {
                app.selected = None;
                let pos = rect.lerp_inside(egui::vec2(fx, fy));
                frame(
                    &ctx,
                    &mut app,
                    vec![egui::Event::PointerMoved(pos), button(pos, true)],
                );
                frame(&ctx, &mut app, vec![button(pos, false)]);
                assert_eq!(app.selected, Some(0), "click at {fx}, {fy} of {rect:?}");
            }
        }
    }

    #[test]
    fn clicking_and_dragging_nodes_to_trash() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.add_node();
        app.add_node();
        let names: Vec<_> = app
            .graph
            .nodes
            .iter()
            .map(|node| node.name.clone())
            .collect();
        app.graph.connect(&names[0], &names[1]).unwrap();
        app.rebuild_snarl();

        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![]);
        let node = app.snarl.node_ids().next().unwrap().0;
        let pos = node_position(&ctx, node);
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(pos), button(pos, true)],
        );
        frame(&ctx, &mut app, vec![button(pos, false)]);
        assert_eq!(app.selected, Some(0));
        app.selected = None;
        frame(&ctx, &mut app, vec![]);

        let trash_rect =
            ctx.data(|data| data.get_temp::<egui::Rect>(Id::new(TRASH_RECT_ID)).unwrap());
        assert!(trash_rect.center().x > 450.0);
        let trash = trash_rect.center();
        let pos = node_position(&ctx, node);
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(pos), button(pos, true)],
        );
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(pos + egui::vec2(20.0, 20.0))],
        );
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(trash)]);
        frame(&ctx, &mut app, vec![button(trash, false)]);

        assert_eq!(app.graph.nodes.len(), 1);
        assert_eq!(app.graph.nodes[0].name, names[1]);
        assert!(app.graph.edges.is_empty());
        assert_eq!(app.snarl.node_ids().count(), 1);
    }

    #[test]
    fn floating_add_menu_creates_a_node() {
        let ctx = egui::Context::default();
        let mut app = app();
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![]);

        let add_rect = ctx.data(|data| {
            data.get_temp::<egui::Rect>(Id::new("add-button-rect"))
                .unwrap()
        });
        assert!(add_rect.center().x > 450.0);
        let add = add_rect.center();
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(add), button(add, true)],
        );
        frame(&ctx, &mut app, vec![button(add, false)]);
        frame(&ctx, &mut app, vec![]);
        let choice = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(Id::new(("canvas-add-kind-rect", 0)))
                    .unwrap()
            })
            .center();
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(choice), button(choice, true)],
        );
        frame(&ctx, &mut app, vec![button(choice, false)]);

        assert_eq!(app.graph.nodes.len(), 1);
        assert!(matches!(app.graph.nodes[0].kind, Kind::Ocr { .. }));
    }

    #[test]
    fn ordinary_drag_and_trash_click_do_not_delete() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.add_node();
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![]);
        let trash = ctx
            .data(|data| data.get_temp::<egui::Rect>(Id::new(TRASH_RECT_ID)).unwrap())
            .center();
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(trash), button(trash, true)],
        );
        frame(&ctx, &mut app, vec![button(trash, false)]);
        let node = app.snarl.node_ids().next().unwrap().0;
        let pos = node_position(&ctx, node);
        let end = pos + egui::vec2(80.0, 40.0);
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(pos), button(pos, true)],
        );
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(end)]);
        frame(&ctx, &mut app, vec![button(end, false)]);
        assert_eq!(app.graph.nodes.len(), 1);
        assert_ne!(app.graph.nodes[0].position, [80.0, 80.0]);
    }

    #[test]
    fn clicking_trash_deletes_selected_node() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.add_node();
        app.selected = Some(0);
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![]);
        let trash = ctx
            .data(|data| data.get_temp::<egui::Rect>(Id::new(TRASH_RECT_ID)).unwrap())
            .center();
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(trash), button(trash, true)],
        );
        frame(&ctx, &mut app, vec![button(trash, false)]);
        assert!(app.graph.nodes.is_empty());
    }

    #[test]
    fn rebuilds_preserve_positions_and_remaining_edges() {
        let mut app = app();
        app.add_node();
        app.add_node();
        app.add_node();
        let names: Vec<_> = app
            .graph
            .nodes
            .iter()
            .map(|node| node.name.clone())
            .collect();
        app.graph.connect(&names[1], &names[2]).unwrap();
        app.rebuild_snarl();
        let first = app.snarl.node_ids().next().unwrap().0;
        app.snarl
            .nodes_ids_data_mut()
            .find(|(id, _)| *id == first)
            .unwrap()
            .1
            .pos
            .x = 345.0;
        app.add_node();
        assert_eq!(app.graph.nodes[0].position[0], 345.0);
        app.delete_node(0);
        assert_eq!(app.graph.edges, vec![(names[1].clone(), names[2].clone())]);
        assert_eq!(app.snarl.wires().count(), 1);
    }
}
