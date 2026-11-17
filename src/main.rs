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
    graph::{EventRuntime, Graph, GraphSettings, NodeEvent, RunMode, DEFAULT_REFRESH_MS},
    node::{CompareOp, Crossing, Format, Kind, Node, Value},
    session_log::SessionLog,
    screen_events::{ScreenEvents, ScreenRegion},
    event_sources::{EventRegistration, EventSource, EventSources},
    preview::{PreviewSession, StepResult},
    workspace::{GraphHistory, Recovery, Workspace},
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
mod editor;
use editor::{event_settings, optional_number};

#[derive(Clone, Debug, PartialEq, Eq)]
enum FileAction {
    New,
    Load,
    Exit,
    Recent(PathBuf),
}

#[derive(Clone, Copy)]
enum UnsavedChoice {
    Save,
    Discard,
    Cancel,
}

struct App {
    graph: Graph,
    snarl: Snarl<usize>,
    backend: Option<Desktop>,
    kind: usize,
    selected: Option<usize>,
    node_settings_open: bool,
    copy_drag: Option<egui_snarl::NodeId>,
    add_menu_open: bool,
    running: bool,
    overlay_ready_at: Option<Instant>,
    last_tick: Instant,
    last_run_duration: Option<Duration>,
    last_run_interval: Option<Duration>,
    last_run_frame: Option<u64>,
    refresh_input: String,
    timing_settings_open: bool,
    screen_events: Option<ScreenEvents>,
    event_regions: Vec<ScreenRegion>,
    event_definition: String,
    event_runtime: EventRuntime,
    region_scales: HashMap<String, f32>,
    /// When each node last produced output, and that output as text.
    activity: HashMap<String, (Instant, String)>,
    escape_pressed: Arc<AtomicBool>,
    overlay_anchors: HashMap<String, Region>,
    overlay_targets: HashMap<String, (Region, u8)>,
    file: Option<PathBuf>,
    saved_graph: String,
    pending_file_action: Option<FileAction>,
    save_error: Option<String>,
    close_confirmed: bool,
    history: GraphHistory,
    workspace: Option<Workspace>,
    recovery: Option<Recovery>,
    recovery_error: Option<String>,
    last_autosave: Instant,
    search_open: bool,
    search_text: String,
    fit_view: bool,
    focus_node: Option<String>,
    validation_open: bool,
    validation: Vec<screen_automation::graph::GraphIssue>,
    preview: Option<PreviewSession>,
    preview_steps: Vec<StepResult>,
    preview_open: bool,
    subflow_editor: Option<(String, Box<App>)>,
    extra_events: Option<EventSources>,
    status: String,
    logs: SessionLog,
    logs_expanded: bool,
}

const KINDS: [&str; 16] = [
    "ocr",
    "model",
    "function",
    "compare",
    "hysteresis",
    "threshold",
    "keyboard",
    "click",
    "compare_inputs",
    "constant",
    "validate_number",
    "event",
    "note",
    "subflow",
    "subflow_input",
    "subflow_output",
];
const GRAPH_ID: &str = "workflow-graph";
const CANVAS_RESPONSE_ID: &str = "canvas-background-response";
const ADD_BUTTON_ID: &str = "canvas-add-button";
const TRASH_BUTTON_ID: &str = "canvas-trash-button";
const TRASH_RECT_ID: &str = "trash-rect";
const OVERLAY_SETTLE_DELAY: Duration = Duration::from_millis(150);
const OVERLAY_MARGIN: f32 = 6.0;
const OVERLAY_HEADER: f32 = 24.0;
const OVERLAY_MIN_CAPTURE_SIZE: egui::Vec2 = egui::vec2(80.0, 1.0);
const OVERLAY_PLACEMENT_ATTEMPTS: u8 = 5;

fn configure_cursors(ctx: &egui::Context) {
    ctx.on_end_pass("clickable-cursors", Arc::new(|ctx| {
        if ctx.output(|output| output.cursor_icon) != egui::CursorIcon::Default
            || ctx.dragged_id().is_some()
        {
            return;
        }
        let canvas = ctx.data(|data| data.get_temp::<Id>(Id::new(CANVAS_RESPONSE_ID)));
        let hovered = ctx.interaction_snapshot(|snapshot| {
            snapshot.hovered.iter().copied().collect::<Vec<_>>()
        });
        if hovered.into_iter().any(|id| {
            Some(id) != canvas && ctx.read_response(id).is_some_and(|response| {
                response.enabled() && response.hovered() && response.sense.senses_click()
            })
        }) {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }));
}

fn overlay_capture_rect(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        rect.min + egui::vec2(OVERLAY_MARGIN, OVERLAY_HEADER),
        rect.max - egui::vec2(OVERLAY_MARGIN, OVERLAY_MARGIN),
    )
}

fn overlay_min_inner_size() -> egui::Vec2 {
    OVERLAY_MIN_CAPTURE_SIZE
        + egui::vec2(2.0 * OVERLAY_MARGIN, OVERLAY_HEADER + OVERLAY_MARGIN)
}

fn capture_region(inner_rect: egui::Rect) -> Region {
    let rect = overlay_capture_rect(inner_rect);
    Region {
        x: rect.min.x.round() as i32,
        y: rect.min.y.round() as i32,
        width: rect.width().round().max(OVERLAY_MIN_CAPTURE_SIZE.x) as u32,
        height: rect.height().round().max(OVERLAY_MIN_CAPTURE_SIZE.y) as u32,
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
    settings: &GraphSettings,
    overlay_ready_at: Option<Instant>,
) -> Instant {
    let interval = match settings.run_mode {
        RunMode::Timed => Duration::from_millis(settings.refresh_ms),
        RunMode::DisplayRefresh | RunMode::HmiEvent => Duration::ZERO,
    };
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
    with_physical_regions(graph, scales, |graph| graph.run_once(backend))
}

fn with_physical_regions<T>(
    graph: &mut Graph,
    scales: &HashMap<String, f32>,
    run: impl FnOnce(&mut Graph) -> Result<T, String>,
) -> Result<T, String> {
    let mut regions = HashMap::new();
    let mut missing = None;
    visit_nodes_mut(graph, "", &mut |node, key| {
        if let Some(region) = node.region {
            if let Some(&scale) = scales.get(key) {
                regions.insert(key.to_owned(), (region, physical_region(region, scale)));
            } else {
                missing = Some(format!("Screen region for {key} is not available yet; try again"));
            }
        }
    });
    if let Some(error) = missing { return Err(error); }
    visit_nodes_mut(graph, "", &mut |node, key| {
        if let Some((_, physical)) = regions.get(key) { node.region = Some(*physical); }
    });
    let result = run(graph);
    visit_nodes_mut(graph, "", &mut |node, key| {
        if let Some((original, _)) = regions.get(key) { node.region = Some(*original); }
    });
    result
}

fn region_key(prefix: &str, name: &str) -> String {
    format!("{prefix}{}", name.replace('~', "~0").replace('/', "~1"))
}

fn visit_nodes_mut(graph: &mut Graph, prefix: &str, visit: &mut impl FnMut(&mut Node, &str)) {
    for node in &mut graph.nodes {
        let key = region_key(prefix, &node.name);
        visit(node, &key);
        if let Kind::Subflow { graph } = &mut node.kind {
            visit_nodes_mut(graph, &format!("{key}/"), visit);
        }
    }
}

struct OverlayState<'a> {
    scales: &'a mut HashMap<String, f32>,
    anchors: &'a mut HashMap<String, Region>,
    targets: &'a mut HashMap<String, (Region, u8)>,
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
        Kind::CompareInputs { .. } => (Color32::from_rgb(226, 217, 110), "="),
        Kind::Constant { .. } => (Color32::from_rgb(196, 180, 220), "#"),
        Kind::Hysteresis { .. } => (Color32::from_rgb(215, 215, 160), "↕"),
        Kind::Threshold { .. } => (Color32::from_rgb(170, 205, 160), "↕"),
        Kind::Keyboard { .. } => (Color32::from_rgb(192, 184, 224), "⌨"),
        Kind::Click { .. } => (Color32::from_rgb(166, 187, 207), "🖱"),
        Kind::ValidateNumber { .. } => (Color32::from_rgb(130, 205, 170), "?"),
        Kind::Event { .. } => (Color32::from_rgb(230, 170, 110), "!"),
        Kind::Note { .. } => (Color32::from_rgb(245, 230, 150), "#"),
        Kind::Subflow { .. } => (Color32::from_rgb(160, 170, 225), "{}"),
        Kind::SubflowInput => (Color32::from_rgb(170, 195, 220), ">"),
        Kind::SubflowOutput => (Color32::from_rgb(170, 195, 220), "<"),
    }
}

fn node_label(node: &Node) -> String {
    let title = node.display_title();
    match &node.kind {
        Kind::Compare { op, value, .. } => format!("{title}  {} {value}", op.symbol()),
        Kind::CompareInputs { op, .. } => format!("{title}  A {} B", op.symbol()),
        Kind::Constant { value, .. } => format!("{title}  = {value}"),
        Kind::Hysteresis {
            upper,
            lower,
            trigger,
            ..
        } | Kind::Threshold {
            upper,
            lower,
            trigger,
            ..
        } => match trigger {
            Crossing::Rising => format!("{title}  ↑ {upper}"),
            Crossing::RisingOrEqual => format!("{title}  ≥ {upper}"),
            Crossing::Falling => format!("{title}  ↓ {lower}"),
            Crossing::FallingOrEqual => format!("{title}  ≤ {lower}"),
            Crossing::Both => format!("{title}  ↑ {upper} ↓ {lower}"),
            Crossing::EnteringRange => format!("{title}  ({lower}, {upper})"),
            Crossing::EnteringRangeInclusive => format!("{title}  [{lower}, {upper}]"),
        },
        _ => title.to_owned(),
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
        Kind::CompareInputs { op, trigger_when, .. } =>
            format!("Compare A {} B; trigger when {trigger_when}", op.symbol()),
        Kind::Constant { value, format } => format!("Output {value:?} as {format:?} each tick"),
        Kind::Hysteresis {
            upper,
            lower,
            trigger,
            ..
        } | Kind::Threshold {
            upper,
            lower,
            trigger,
            ..
        } => match trigger {
            Crossing::Rising => format!("Trigger when input rises above {upper}"),
            Crossing::RisingOrEqual => format!("Trigger when input rises to or above {upper}"),
            Crossing::Falling => format!("Trigger when input falls below {lower}"),
            Crossing::FallingOrEqual => format!("Trigger when input falls to or below {lower}"),
            Crossing::Both => {
                format!("Trigger when input rises above {upper} or falls below {lower}")
            }
            Crossing::EnteringRange => format!("Trigger on entering {lower} < value < {upper}"),
            Crossing::EnteringRangeInclusive => format!("Trigger on entering {lower} <= value <= {upper}"),
        },
        Kind::Keyboard { .. } => "Type text into a screen region".into(),
        Kind::Click { .. } => "Click a screen region".into(),
        Kind::ValidateNumber { .. } => "Reject invalid numeric/OCR readings before they reach actions".into(),
        Kind::Event { event } => format!("Event source: {}", event.label()),
        Kind::Note { text } => text.clone(),
        Kind::Subflow { graph } => format!("Embedded reusable workflow ({} nodes)", graph.nodes.len()),
        Kind::SubflowInput => "Input value supplied by the containing subflow".into(),
        Kind::SubflowOutput => "Return a value from the containing subflow".into(),
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
        0 => Kind::Ocr { single_line: false, last_hash: None },
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
        4 => Kind::Hysteresis {
            upper: 100.0,
            lower: 90.0,
            trigger: Crossing::Both,
            band: None,
        },
        5 => Kind::Threshold {
            upper: 100.0,
            lower: 90.0,
            trigger: Crossing::Both,
            band: None,
        },
        6 => Kind::Keyboard {
            text: String::new(),
        },
        7 => Kind::Click {
            button: "left".into(),
        },
        8 => Kind::CompareInputs {
            op: CompareOp::Greater,
            trigger_when: true,
            on_change: true,
            last: None,
            values: [None, None],
        },
        9 => Kind::Constant { value: "0".into(), format: Format::Number },
        10 => Kind::ValidateNumber { minimum: None, maximum: None, max_change: None, pattern: String::new(), last: None },
        11 => Kind::Event { event: EventSource::Timer { interval_ms: 1000 } },
        12 => Kind::Note { text: "Add your notes here".into() },
        13 => {
            let mut graph = Graph::default();
            graph.add_node(Node::new("input", Kind::SubflowInput)).expect("unique input");
            graph.add_node(Node::new("output", Kind::SubflowOutput)).expect("unique output");
            graph.connect("input", "output").expect("valid subflow edge");
            Kind::Subflow { graph: Box::new(graph) }
        }
        14 => Kind::SubflowInput,
        _ => Kind::SubflowOutput,
    }
}

fn format_settings(ui: &mut egui::Ui, format: &mut Format) {
    for (choice, label) in [
        (Format::Text, "Text"),
        (Format::Number, "Number"),
        (Format::Int, "Integer"),
        (Format::Bool, "Boolean"),
    ] {
        ui.radio_value(format, choice, label);
    }
}

struct GraphViewer<'a> {
    graph: &'a mut Graph,
    status: &'a mut String,
    activity: &'a HashMap<String, (Instant, String)>,
    selected: Option<usize>,
    now: Instant,
    fit: Option<(egui::Rect, egui::Rect)>,
    validation: &'a [screen_automation::graph::GraphIssue],
}

impl GraphViewer<'_> {
    fn input_glow(&self, index: usize, input: usize) -> f32 {
        let name = &self.graph.nodes[index].name;
        self.graph
            .edges
            .iter()
            .filter(|edge| &edge.target == name && edge.input == input)
            .map(|edge| wire_glow(self.activity, &edge.source, self.now))
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
        if self.validation.iter().any(|issue| issue.node.as_deref() == Some(&self.graph.nodes[index].name)) {
            frame.stroke(egui::Stroke::new(2.0, Color32::RED))
        } else if self.selected == Some(index) {
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
        self.graph.nodes[*node].input_count()
    }

    fn show_input(
        &mut self,
        pin: &InPin,
        _: &mut egui::Ui,
        snarl: &mut Snarl<usize>,
    ) -> impl egui_snarl::ui::SnarlPin + 'static {
        wire_pin(self.input_glow(snarl[pin.id.node], pin.id.input))
    }

    fn outputs(&mut self, node: &usize) -> usize {
        self.graph.nodes[*node].output_count()
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
        let node = &self.graph.nodes[index];
        if let Kind::Note { text } = &node.kind {
            ui.vertical(|ui| {
                ui.set_width(240.0);
                ui.strong(node.display_title());
                ui.label(text);
            });
            return;
        }
        let (_, icon) = node_style(&node.kind);
        let label = ui.painter().layout_no_wrap(
            node_label(node),
            egui::FontId::proportional(14.0),
            NODE_TEXT,
        );
        let width = (NODE_ICON_WIDTH + label.size().x + 24.0).max(NODE_MIN_WIDTH);
        let height = NODE_HEIGHT * inputs.len().max(1) as f32;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
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
        for (input, _) in inputs.iter().enumerate() {
            let input_glow = self.input_glow(index, input);
            let y = rect.top() + NODE_HEIGHT * (input as f32 + 0.5);
            let pin = egui::Rect::from_center_size(
                egui::pos2(rect.left(), y),
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
            if inputs.len() == 2 {
                painter.text(
                    egui::pos2(rect.left() + 8.0, y),
                    egui::Align2::LEFT_CENTER,
                    if input == 0 { "A" } else { "B" },
                    egui::FontId::proportional(11.0),
                    NODE_TEXT,
                );
            }
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
        ui.strong(format!("{} · {}", node.display_title(), node.type_name()));
        ui.label(node_description(node));
        if let Some((_, last)) = self.activity.get(&node.name) {
            ui.weak(format!("Last output: {last}"));
        }
    }

    fn connect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<usize>) {
        let source = self.graph.nodes[snarl[from.id.node]].name.clone();
        let destination = self.graph.nodes[snarl[to.id.node]].name.clone();
        match self.graph.connect_input(&source, &destination, to.id.input) {
            Ok(()) => {
                snarl.connect(from.id, to.id);
                self.status.clear();
            }
            Err(error) => {
                log::warn!("[GRAPH] {error}");
                *self.status = error;
            }
        }
    }

    fn disconnect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<usize>) {
        let source = self.graph.nodes[snarl[from.id.node]].name.clone();
        let destination = self.graph.nodes[snarl[to.id.node]].name.clone();
        self.graph.disconnect_input(&source, &destination, to.id.input);
        snarl.disconnect(from.id, to.id);
    }

    fn drop_inputs(&mut self, pin: &InPin, snarl: &mut Snarl<usize>) {
        for remote in pin.remotes.clone() {
            let from = snarl.out_pin(remote);
            self.disconnect(&from, pin, snarl);
        }
    }

    fn drop_outputs(&mut self, pin: &OutPin, snarl: &mut Snarl<usize>) {
        for remote in pin.remotes.clone() {
            let to = snarl.in_pin(remote);
            self.disconnect(pin, &to, snarl);
        }
    }

    fn current_transform(&mut self, transform: &mut egui::emath::TSTransform, _: &mut Snarl<usize>) {
        if let Some((bounds, canvas)) = self.fit {
            let scale = (canvas.width() / bounds.width()).min(canvas.height() / bounds.height()).clamp(f32::MIN_POSITIVE, 2.0);
            *transform = egui::emath::TSTransform::new(canvas.center().to_vec2() - bounds.center().to_vec2() * scale, scale);
        }
    }

    fn draw_background(
        &mut self, background: Option<&BackgroundPattern>, viewport: &egui::Rect,
        style: &SnarlStyle, ui_style: &egui::Style, painter: &egui::Painter, snarl: &Snarl<usize>,
    ) {
        if let Some(background) = background {
            background.draw(viewport, style, ui_style, painter);
        }
        let mut groups: HashMap<&str, egui::Rect> = HashMap::new();
        for (_, position, index) in snarl.nodes_pos_ids() {
            let node = &self.graph.nodes[*index];
            if !node.group.trim().is_empty() {
                let rect = egui::Rect::from_min_size(position, canvas_node_size(painter, node)).expand(18.0);
                groups.entry(&node.group).and_modify(|bounds| *bounds = bounds.union(rect)).or_insert(rect);
            }
        }
        for (name, rect) in groups {
            painter.rect(rect, 8, Color32::from_rgba_unmultiplied(100, 140, 200, 15),
                egui::Stroke::new(1.0, Color32::from_rgb(150, 175, 205)), egui::StrokeKind::Outside);
            painter.text(rect.left_top() + egui::vec2(4.0, -4.0), egui::Align2::LEFT_BOTTOM,
                name, egui::FontId::proportional(13.0), Color32::from_rgb(80, 105, 145));
        }
    }
}

fn canvas_node_size(painter: &egui::Painter, node: &Node) -> egui::Vec2 {
    let font = egui::FontId::proportional(14.0);
    if let Kind::Note { text } = &node.kind {
        let text = painter.layout(text.clone(), font, NODE_TEXT, 240.0);
        return egui::vec2(260.0, text.size().y + 40.0);
    }
    let label = painter.layout_no_wrap(node_label(node), font, NODE_TEXT);
    egui::vec2((NODE_ICON_WIDTH + label.size().x + 24.0).max(NODE_MIN_WIDTH) + 10.0,
        NODE_HEIGHT * node.input_count().max(1) as f32 + 10.0)
}

impl App {
    fn record_status(&mut self, level: log::Level, message: String) {
        self.logs.record(level, format_args!("[APP] {message}"));
        self.status = message;
    }

    fn has_unsaved_changes(&mut self) -> bool {
        self.sync_graph_positions();
        self.graph.to_json() != self.saved_graph
    }

    fn save_graph(&mut self, path: &Path) -> bool {
        self.sync_graph_positions();
        self.save_error = None;
        let contents = self.graph.to_json();
        match std::fs::write(path, &contents) {
            Ok(()) => {
                self.file = Some(path.to_path_buf());
                self.saved_graph = contents;
                self.status = "Saved workflow".into();
                self.logs.record(log::Level::Info, format_args!("[FILE] Saved {}", path.display()));
                self.remember_file(path);
                self.clear_recovery();
                true
            }
            Err(error) => {
                let message = format!("Could not save {}: {error}", path.display());
                self.save_error = Some(message.clone());
                self.record_status(log::Level::Error, message);
                false
            }
        }
    }

    fn replace_graph(&mut self, graph: Graph, file: Option<PathBuf>) {
        self.set_running(false);
        self.leave_subflows();
        self.graph = graph;
        self.refresh_input = self.graph.settings.refresh_ms.to_string();
        self.file = file;
        self.selected = None;
        self.node_settings_open = false;
        self.add_menu_open = false;
        self.region_scales.clear();
        self.activity.clear();
        self.overlay_anchors.clear();
        self.overlay_targets.clear();
        self.rebuild_snarl();
        self.saved_graph = self.graph.to_json();
        self.pending_file_action = None;
        self.save_error = None;
        self.close_confirmed = false;
        self.history = GraphHistory::new(self.graph.to_json());
        self.validation = self.graph.validate();
        self.preview = None;
        self.preview_steps.clear();
        self.subflow_editor = None;
        self.fit_view = true;
        self.focus_node = None;
        self.clear_recovery();
    }

    fn load_graph(&mut self, path: &Path) {
        match std::fs::read_to_string(path)
            .map_err(|error| error.to_string())
            .and_then(|contents| Graph::from_json(&contents))
        {
            Ok(graph) => {
                self.replace_graph(graph, Some(path.to_path_buf()));
                self.status = "Loaded workflow".into();
                self.logs.record(log::Level::Info, format_args!("[FILE] Loaded {}", path.display()));
                self.remember_file(path);
            }
            Err(error) => self.record_status(log::Level::Error, format!("Could not load {}: {error}", path.display())),
        }
    }

    fn graph_file_dialog(&self) -> rfd::FileDialog {
        let mut dialog = rfd::FileDialog::new()
            .add_filter("Graph JSON", &["json"])
            .set_file_name("graph.json");
        if let Some(path) = &self.file {
            if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
                dialog = dialog.set_directory(parent);
            }
            if let Some(name) = path.file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy());
            }
        }
        dialog
    }

    fn save_graph_as(&mut self) -> bool {
        self.set_running(false);
        self.save_error = None;
        if let Some(path) = self.graph_file_dialog().set_title("Save graph as").save_file() {
            self.save_graph(&path)
        } else {
            false
        }
    }

    fn save_current_graph(&mut self) -> bool {
        if let Some(path) = self.file.clone() {
            self.save_graph(&path)
        } else {
            self.save_graph_as()
        }
    }

    fn request_file_action(&mut self, action: FileAction, ctx: &egui::Context) {
        self.set_running(false);
        if self.has_unsaved_changes() {
            self.pending_file_action = Some(action);
            self.save_error = None;
        } else {
            self.perform_file_action(action, ctx);
        }
    }

    fn perform_file_action(&mut self, action: FileAction, ctx: &egui::Context) {
        match action {
            FileAction::New => {
                self.replace_graph(Graph::default(), None);
                self.timing_settings_open = false;
                self.record_status(log::Level::Info, "Created new workflow".into());
            }
            FileAction::Load => {
                if let Some(path) = self.graph_file_dialog().set_title("Load graph JSON").pick_file() {
                    self.load_graph(&path);
                }
            }
            FileAction::Exit => {
                self.clear_recovery();
                self.close_confirmed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            FileAction::Recent(path) => self.load_graph(&path),
        }
    }

    fn resolve_unsaved_changes(&mut self, choice: UnsavedChoice, ctx: &egui::Context) {
        let Some(action) = self.pending_file_action.clone() else {
            return;
        };
        if matches!(choice, UnsavedChoice::Save) && !self.save_current_graph() {
            return;
        }
        self.pending_file_action = None;
        self.save_error = None;
        if !matches!(choice, UnsavedChoice::Cancel) {
            self.perform_file_action(action, ctx);
        }
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.viewport().close_requested()) && !self.close_confirmed {
            if self.has_unsaved_changes() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.request_file_action(FileAction::Exit, ctx);
            } else {
                self.set_running(false);
                self.clear_recovery();
            }
        }
    }

    fn show_unsaved_changes(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending_file_action.clone() else {
            return;
        };
        let mut choice = None;
        let response = egui::Modal::new(Id::new("unsaved-changes")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading("Unsaved changes");
            let operation = match action {
                FileAction::New => "creating a new graph",
                FileAction::Load | FileAction::Recent(_) => "loading another graph",
                FileAction::Exit => "closing ScreenAutomation",
            };
            ui.label(format!("Save changes to {} before {operation}?",
                self.file.as_deref().and_then(Path::file_name)
                    .map(|name| name.to_string_lossy()).unwrap_or_else(|| "Untitled".into())));
            if let Some(error) = &self.save_error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() { choice = Some(UnsavedChoice::Save); }
                if ui.button("Discard").clicked() { choice = Some(UnsavedChoice::Discard); }
                if ui.button("Cancel").clicked() { choice = Some(UnsavedChoice::Cancel); }
            });
        });
        if let Some(choice) = choice {
            self.resolve_unsaved_changes(choice, ctx);
        } else if response.should_close() {
            self.resolve_unsaved_changes(UnsavedChoice::Cancel, ctx);
        }
    }

    fn window_title(&mut self) -> String {
        let modified = if self.has_unsaved_changes() { "*" } else { "" };
        let name = self.file.as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| "Untitled".into());
        format!("{name}{modified} - ScreenAutomation")
    }

    fn show_menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New").clicked() {
                    ui.close();
                    self.request_file_action(FileAction::New, ui.ctx());
                }
                if ui.button("Load graph JSON...").clicked() {
                    ui.close();
                    self.request_file_action(FileAction::Load, ui.ctx());
                }
                let recent = self.workspace.as_ref().map(|workspace| workspace.recent.clone()).unwrap_or_default();
                ui.add_enabled_ui(!recent.is_empty(), |ui| {
                    ui.menu_button("Recent files", |ui| {
                        for path in &recent {
                            if ui.button(path.file_name().unwrap_or(path.as_os_str()).to_string_lossy())
                                .on_hover_text(path.display().to_string()).clicked()
                            {
                                ui.close();
                                self.request_file_action(FileAction::Recent(path.clone()), ui.ctx());
                            }
                        }
                    });
                });
                ui.separator();
                if ui.button("Save").clicked() {
                    ui.close();
                    self.save_current_graph();
                }
                if ui.button("Save As...").clicked() {
                    ui.close();
                    self.save_graph_as();
                }
                ui.separator();
                if ui.button("Exit").clicked() {
                    ui.close();
                    self.request_file_action(FileAction::Exit, ui.ctx());
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui.add_enabled(self.history.can_undo(), egui::Button::new("Undo    Ctrl+Z")).clicked() {
                    self.undo_edit(ui.ctx(), false);
                    ui.close();
                }
                if ui.add_enabled(self.history.can_redo(), egui::Button::new("Redo    Ctrl+Y")).clicked() {
                    self.undo_edit(ui.ctx(), true);
                    ui.close();
                }
                if ui.button("Find node...    Ctrl+F").clicked() {
                    self.search_open = true;
                    ui.close();
                }
                if ui.button("Fit graph    Ctrl+0").clicked() {
                    self.fit_view = true;
                    self.focus_node = None;
                    ui.close();
                }
            });
            ui.menu_button("Tools", |ui| {
                if ui.button("Validate graph").clicked() {
                    self.validation = self.validation_issues();
                    self.validation_open = true;
                    ui.close();
                }
                if ui.button("Dry Run    F6").clicked() {
                    self.preview_run(false);
                    ui.close();
                }
                if ui.button("Step    F7").clicked() {
                    self.preview_run(true);
                    ui.close();
                }
            });
            ui.menu_button("Settings", |ui| {
                if ui.button("Run timing...").clicked() {
                    self.timing_settings_open = true;
                    ui.close();
                }
            });
            ui.separator();
            let response = ui.button(if self.running { "     Stop (Esc)" } else { "     Run" });
            let center = egui::pos2(response.rect.left() + 11.0, response.rect.center().y);
            if self.running {
                ui.painter().rect_filled(
                    egui::Rect::from_center_size(center, egui::vec2(9.0, 9.0)),
                    1.0,
                    Color32::from_rgb(210, 65, 65),
                );
            } else {
                ui.painter().add(egui::Shape::convex_polygon(
                    vec![center + egui::vec2(-4.0, -5.0), center + egui::vec2(5.0, 0.0),
                         center + egui::vec2(-4.0, 5.0)],
                    Color32::from_rgb(45, 160, 80),
                    egui::Stroke::NONE,
                ));
            }
            if response.on_hover_text("Start or stop automation. Esc also stops a running workflow.").clicked() {
                self.toggle_run();
            }
        });
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Title(self.window_title()));
    }

    fn show_timing_settings(&mut self, ctx: &egui::Context) {
        let previous = self.graph.settings.run_mode;
        egui::Window::new("Run timing")
            .id(Id::new("run-timing-settings"))
            .open(&mut self.timing_settings_open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.radio_value(&mut self.graph.settings.run_mode, RunMode::Timed, "Timed delay");
                ui.radio_value(
                    &mut self.graph.settings.run_mode,
                    RunMode::DisplayRefresh,
                    "Display refresh",
                );
                ui.radio_value(
                        &mut self.graph.settings.run_mode,
                        RunMode::HmiEvent,
                        "HMI Event",
                    );
                let timed = self.graph.settings.run_mode == RunMode::Timed;
                ui.add_enabled_ui(timed, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Delay (ms)");
                        if ui.add(
                            egui::TextEdit::singleline(&mut self.refresh_input)
                                .id(Id::new("refresh-delay-input"))
                                .desired_width(80.0),
                        ).changed() {
                            if let Some(value) = refresh_interval(&self.refresh_input) {
                                self.graph.settings.refresh_ms = value;
                            }
                        }
                    });
                });
                if timed && refresh_interval(&self.refresh_input).is_none() {
                    ui.colored_label(Color32::RED, "Enter 100–60000 ms");
                }
                ui.separator();
                if timed {
                    ui.label("Delay is measured between the starts of workflow checks.");
                    ui.weak("Invalid input keeps the last valid delay.");
                } else if self.graph.settings.run_mode == RunMode::DisplayRefresh {
                    ui.label("Checks once per app frame, paced by display VSync.");
                    ui.weak("Capture/OCR can reduce the rate. Minimizing the app may pause checks.");
                } else {
                    ui.label("Screen changes and configured Event nodes trigger connected branches.");
                    ui.weak("Screen changes and global hotkeys require Windows. Audio uses your default microphone.");
                    ui.weak("HTTP webhooks listen only on this computer. Only the latest event per source is kept.");
                }
                ui.label("OCR runs only when its captured region changes.");
                ui.weak("Mode and delay are saved with the graph.");
            });
        if self.running && previous != self.graph.settings.run_mode
            && (previous == RunMode::HmiEvent || self.graph.settings.run_mode == RunMode::HmiEvent)
        {
            self.set_running(false);
            self.record_status(log::Level::Info, "Event timing changed. Review the settings, then press Run.".into());
        }
    }

    fn rebuild_snarl(&mut self) {
        self.copy_drag = None;
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
        for edge in &self.graph.edges {
            let Some(source_index) = self.graph.nodes.iter().position(|n| n.name == edge.source) else {
                continue;
            };
            let Some(destination_index) =
                self.graph.nodes.iter().position(|n| n.name == edge.target)
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
                    input: edge.input,
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

    fn next_node_name(&self, kind_name: &str) -> String {
        let mut number = self.graph.nodes.len() + 1;
        loop {
            let candidate = format!("{kind_name}{number}");
            if self.graph.nodes.iter().all(|node| node.name != candidate) {
                return candidate;
            }
            number += 1;
        }
    }

    fn add_node(&mut self) {
        self.sync_graph_positions();
        let name = self.next_node_name(KINDS[self.kind]);
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
            Err(error) => self.record_status(log::Level::Error, error),
        }
    }

    fn handle_copy_drag(&mut self, ctx: &egui::Context) {
        if self.copy_drag.is_some_and(|id| ctx.dragged_id() != Some(node_frame_id(id))) {
            self.copy_drag = None;
        }
        if self.copy_drag.is_some() || !ctx.input(|input| input.modifiers.shift && !input.modifiers.command) {
            return;
        }
        let source = self.snarl.nodes_ids_data().find_map(|(id, node)| {
            ctx.read_response(node_frame_id(id))
                .is_some_and(|response| response.drag_started_by(egui::PointerButton::Primary))
                .then(|| node.clone())
        });
        let Some(source) = source else {
            return;
        };
        let name = self.next_node_name(self.graph.nodes[source.value].type_name());
        let mut copy = self.graph.nodes[source.value].duplicate(&name);
        copy.position = [source.pos.x, source.pos.y];
        let index = self.graph.nodes.len();
        match self.graph.add_node(copy) {
            Ok(()) => {
                let id = if source.open {
                    self.snarl.insert_node(source.pos, index)
                } else {
                    self.snarl.insert_node_collapsed(source.pos, index)
                };
                ctx.set_dragged_id(node_frame_id(id));
                self.copy_drag = Some(id);
                self.selected = Some(index);
                self.node_settings_open = false;
                if self.running {
                    self.overlay_ready_at = Some(Instant::now() + OVERLAY_SETTLE_DELAY);
                }
                self.record_status(log::Level::Info, format!("Duplicated node as {name}"));
                ctx.request_repaint();
            }
            Err(error) => self.record_status(log::Level::Error, error),
        }
    }

    fn delete_node(&mut self, index: usize) {
        if let Some(node) = self.graph.nodes.get(index) {
            let name = node.name.clone();
            let region = region_key("", &name);
            let prefix = format!("{region}/");
            self.sync_graph_positions();
            self.graph.remove_node(&name);
            self.region_scales.retain(|key, _| key != &region && !key.starts_with(&prefix));
            self.activity.remove(&name);
            self.overlay_anchors.retain(|key, _| key != &region && !key.starts_with(&prefix));
            self.overlay_targets.retain(|key, _| key != &region && !key.starts_with(&prefix));
            self.selected = None;
            self.node_settings_open = false;
            self.rebuild_snarl();
            self.record_status(log::Level::Info, format!("Deleted {name}"));
        }
    }

    fn run_graph(&mut self) {
        if let Some(backend) = self.backend.as_mut() {
            match run_with_physical_regions(&mut self.graph, backend, &self.region_scales) {
                Ok(outputs) => self.record_outputs(outputs),
                Err(error) => self.record_status(log::Level::Error, error),
            }
        } else {
            self.record_status(log::Level::Error, "Desktop backend is unavailable".into());
            self.set_running(false);
        }
    }

    fn record_outputs(&mut self, outputs: HashMap<String, Option<Value>>) {
        let now = Instant::now();
        for (name, value) in outputs {
            if let Some(value) = value {
                self.activity.insert(name, (now, value.to_text()));
            }
        }
    }

    fn run_hmi_events(&mut self, ctx: &egui::Context) -> Result<(), String> {
        let now = Instant::now();
        if let Some(ready) = self.overlay_ready_at.filter(|ready| now < *ready) {
            ctx.request_repaint_after(ready.duration_since(now));
            return Ok(());
        }
        self.overlay_ready_at = None;
        let mut regions = Vec::new();
        for node in &self.graph.nodes {
            if matches!(node.kind, Kind::Ocr { .. }) {
                let region = node.region.ok_or_else(|| format!("OCR node '{}' has no region", node.name))?;
                let scale = self.region_scales.get(&region_key("", &node.name))
                    .ok_or_else(|| format!("Screen region for '{}' is not available yet", node.name))?;
                regions.push(ScreenRegion {
                    name: node.name.clone(),
                    region: physical_region(region, *scale),
                });
            }
        }
        let registrations: Vec<_> = self.graph.nodes.iter().filter_map(|node| match &node.kind {
            Kind::Event { event } => Some(EventRegistration { name: node.name.clone(), source: event.clone() }),
            _ => None,
        }).collect();
        if regions.is_empty() && registrations.is_empty() {
            return Err("HMI Event mode requires an OCR region or an Event node".into());
        }
        let definition = serde_json::to_string(&(
            &self.graph.edges,
            self.graph.nodes.iter().map(|node| (&node.name, &node.kind)).collect::<Vec<_>>(),
        )).map_err(|error| error.to_string())?;
        if (self.screen_events.is_some() || self.extra_events.is_some())
            && (self.event_regions != regions || self.event_definition != definition)
        {
            self.set_running(false);
            self.record_status(log::Level::Info,
                "Workflow or regions changed. Review the graph, then press Run to restart HMI Event monitoring.".into());
            ctx.request_repaint();
            return Ok(());
        }
        if self.screen_events.is_none() && self.extra_events.is_none() {
            self.event_runtime = EventRuntime::default();
            for node in &mut self.graph.nodes {
                match &mut node.kind {
                    Kind::Ocr { last_hash, .. } => *last_hash = None,
                    Kind::CompareInputs { last, values, .. } => {
                        *last = None;
                        *values = [None, None];
                    }
                    _ => {}
                }
            }
            let wake_ctx = ctx.clone();
            let external_count = registrations.len();
            if !regions.is_empty() {
                self.screen_events = Some(ScreenEvents::start(
                    regions.clone(), Arc::new(move || wake_ctx.request_repaint()),
                )?);
            }
            if !registrations.is_empty() {
                let wake_ctx = ctx.clone();
                self.extra_events = Some(EventSources::start(
                    registrations, Arc::new(move || wake_ctx.request_repaint()),
                )?);
            }
            self.event_regions = regions;
            self.event_definition = definition;
            self.logs.record(log::Level::Info, format_args!(
                "[EVENT] Monitoring {} OCR region(s) and {} external source(s)", self.event_regions.len(), external_count,
            ));
        }
        let frames = match &self.screen_events {
            Some(events) => events.take_pending()?,
            None => Vec::new(),
        };
        let mut events = match &self.extra_events {
            Some(events) => events.take_pending()?,
            None => Vec::new(),
        };
        if frames.is_empty() && events.is_empty() {
            return Ok(());
        }
        let start = Instant::now();
        let mut unavailable = editor::UnavailableBackend;
        let backend: &mut dyn Backend = match self.backend.as_mut() {
            Some(backend) => backend,
            None => &mut unavailable,
        };
        for frame in frames {
            let node = self.graph.nodes.iter_mut().find(|node| node.name == frame.name)
                .ok_or_else(|| format!("Unknown screen event source: {}", frame.name))?;
            if let Some(value) = node.process_frame(&frame.region, &frame.pixels, backend)? {
                events.push(NodeEvent { source: frame.name, value });
            }
        }
        let outputs = with_physical_regions(&mut self.graph, &self.region_scales, |graph| {
            graph.run_events(&events, &mut self.event_runtime, backend)
        })?;
        self.record_outputs(outputs);
        if self.last_run_duration.is_some() {
            self.last_run_interval = Some(start.duration_since(self.last_tick));
        }
        self.last_tick = start;
        self.last_run_duration = Some(start.elapsed());
        ctx.request_repaint();
        Ok(())
    }

    fn show_settings(&mut self, ctx: &egui::Context) {
        if !self.node_settings_open {
            return;
        }
        let Some(index) = self
            .selected
            .filter(|index| *index < self.graph.nodes.len())
        else {
            return;
        };
        let mut open = true;
        let mut delete = false;
        let popup_clicked = egui::Popup::is_any_open(ctx)
            && ctx.pointer_interact_pos()
                .and_then(|pos| ctx.layer_id_at(pos))
                .is_some_and(|layer| layer.order == egui::Order::Foreground);
        let window = egui::Window::new("Node settings")
            .id(Id::new("node-settings"))
            .open(&mut open)
            .show(ctx, |settings| {
                let node = &mut self.graph.nodes[index];
                settings.horizontal(|ui| {
                    ui.label("Title");
                    ui.add(
                        egui::TextEdit::singleline(&mut node.title)
                            .id(Id::new(("node-title", index)))
                            .hint_text(&node.name),
                    );
                });
                settings.weak(node.type_name());
                settings.horizontal(|ui| {
                    ui.label("Group");
                    ui.text_edit_singleline(&mut node.group);
                });
                settings.separator();
                let hysteresis = matches!(node.kind, Kind::Hysteresis { .. });
                match &mut node.kind {
                    Kind::Model { format } => {
                        format_settings(settings, format);
                    }
                    Kind::Constant { value, format } => {
                        settings.label("Value");
                        settings.text_edit_singleline(value);
                        format_settings(settings, format);
                        settings.weak("Outputs the configured value on every workflow tick.");
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
                    Kind::CompareInputs { op, trigger_when, on_change, last, .. } => {
                        let before = (*op, *trigger_when, *on_change);
                        settings.label("Compare input A with input B");
                        egui::ComboBox::from_id_salt("compare-inputs-op")
                            .selected_text(format!("A {} B", op.symbol()))
                            .show_ui(settings, |ui| {
                                for choice in CompareOp::ALL {
                                    ui.selectable_value(op, choice, format!("A {} B", choice.symbol()));
                                }
                            });
                        settings.horizontal(|ui| {
                            ui.label("Trigger when");
                            ui.radio_value(trigger_when, true, "True");
                            ui.radio_value(trigger_when, false, "False");
                        });
                        settings.checkbox(on_change, "Only trigger when the result changes");
                        settings.weak("Top input: A. Bottom input: B. Waits for both inputs, then \
                            compares their latest values when either updates. Outputs the boolean \
                            result only when it matches the selected trigger state.");
                        if before != (*op, *trigger_when, *on_change) {
                            *last = None;
                        }
                    }
                    Kind::Hysteresis {
                        upper,
                        lower,
                        trigger,
                        band,
                    } | Kind::Threshold {
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
                                for choice in Crossing::ALL.into_iter()
                                    .filter(|choice| !hysteresis || !choice.is_range())
                                {
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
                        settings.weak(if hysteresis {
                            "Triggers above the upper threshold or below the lower threshold. \
                             The >= and <= options also trigger at equality. \
                             The value must cross the other threshold before the same direction \
                             can trigger again. The first reading never triggers. \
                             When it triggers, the input is passed on."
                        } else if trigger.is_range() {
                            "Triggers once when the value enters the range. \
                             Strict excludes the boundaries; inclusive includes them. \
                             Leave the range before it can trigger again. \
                             The first reading triggers if it is in range. \
                             When it triggers, the original input is passed on."
                        } else {
                            "Triggers above the upper threshold or below the lower threshold. \
                             The >= and <= options also trigger at equality. \
                             Cross back over the same boundary or cross the other threshold \
                             to re-arm. The first reading triggers if the selected condition \
                             holds. When it triggers, the input is passed on."
                        });
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
                    Kind::Ocr { single_line, last_hash } => {
                        settings.label("Reads text from its screen region.");
                        if settings.checkbox(single_line, "Single line (prices / numbers)").changed() {
                            *last_hash = None;
                        }
                        settings.weak(
                            "For one price, enable single-line mode and fit the region around \
                             the entire number with a small plain-background margin. \
                             Exclude other text and icons. \
                             This bypasses layout detection so decimal fragments stay in order. \
                             It cannot guarantee that every decimal point is recognized.",
                        );
                    }
                    Kind::ValidateNumber { minimum, maximum, max_change, pattern, last } => {
                        let before = (*minimum, *maximum, *max_change, pattern.clone());
                        optional_number(settings, "Minimum", minimum);
                        optional_number(settings, "Maximum", maximum);
                        optional_number(settings, "Maximum absolute change", max_change);
                        settings.label("Required text pattern (regular expression, optional)");
                        settings.text_edit_singleline(pattern);
                        settings.weak("Use ^ and $ for a whole-value match, e.g. ^[0-9]+\\.[0-9]{2}$. Rejected readings never reach actions or update the last accepted value.");
                        if before != (*minimum, *maximum, *max_change, pattern.clone()) {
                            *last = None;
                        }
                    }
                    Kind::Event { event } => event_settings(settings, event),
                    Kind::Note { text } => {
                        settings.add(egui::TextEdit::multiline(text).desired_rows(6).desired_width(300.0));
                    }
                    Kind::Subflow { .. } => {
                        settings.weak("A self-contained workflow with Subflow Input and Subflow Output nodes. Shift-drag to reuse it.");
                        if settings.button("Edit embedded workflow").clicked() {
                            settings.data_mut(|data| data.insert_temp(Id::new("edit-subflow-request"), index));
                        }
                        if settings.button("Import workflow JSON...").clicked() {
                            settings.data_mut(|data| data.insert_temp(Id::new("import-subflow-request"), index));
                        }
                    }
                    Kind::SubflowInput => { settings.label("Supplies the containing node's input inside a subflow."); }
                    Kind::SubflowOutput => { settings.label("Returns this input as the containing subflow's output."); }
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
        let selected_node_clicked = self.snarl.node_ids().any(|(id, node)| {
            *node == index && ctx.read_response(node_frame_id(id))
                .is_some_and(|response| response.contains_pointer())
        });
        let clicked_outside = window.is_some_and(|window| window.response.clicked_elsewhere())
            && !selected_node_clicked
            && !popup_clicked;
        if delete {
            self.delete_node(index);
        } else if !open || clicked_outside {
            self.selected = None;
            self.node_settings_open = false;
        }
        self.handle_subflow_requests(ctx);
    }

    fn show_canvas(&mut self, ui: &mut egui::Ui) {
        if self.subflow_editor.is_some() {
            self.show_subflow_canvas(ui);
            return;
        }
        let canvas_rect = ui.max_rect();
        let ctx = ui.ctx().clone();
        self.handle_copy_drag(&ctx);
        let shift = ctx.input(|input| input.modifiers.shift);
        if self.copy_drag.is_some() {
            // Shift normally selects nodes in snarl; this gesture drags only the new copy.
            ctx.input_mut(|input| input.modifiers.shift = false);
        }
        // Selectable labels swallow clicks, which stopped clicks on node text from selecting the node.
        ui.style_mut().interaction.selectable_labels = false;
        let graph_widget = SnarlWidget::new()
            .id(Id::new(GRAPH_ID))
            .style({
                let mut style = graph_style();
                style.min_scale = Some(f32::MIN_POSITIVE);
                style
            })
            .min_size(ui.available_size());
        let now = Instant::now();
        let focus = self.focus_node.take().and_then(|name| self.snarl.nodes_pos_ids()
            .find(|(_, _, index)| self.graph.nodes[**index].name == name)
            .map(|(_, pos, index)| egui::Rect::from_min_size(pos, canvas_node_size(ui.painter(), &self.graph.nodes[*index])).expand(40.0)));
        let fit = if let Some(bounds) = focus {
            Some((bounds, canvas_rect.shrink(60.0)))
        } else if self.fit_view && !self.graph.nodes.is_empty() {
            let bounds = self.snarl.nodes_pos_ids().fold(egui::Rect::NOTHING, |bounds, (_, pos, index)| {
                bounds.union(egui::Rect::from_min_size(pos, canvas_node_size(ui.painter(), &self.graph.nodes[*index])))
            }).expand(36.0);
            Some((bounds, canvas_rect.shrink(60.0)))
        } else { None };
        self.fit_view = false;
        let response = graph_widget.show(
            &mut self.snarl,
            &mut GraphViewer {
                graph: &mut self.graph,
                status: &mut self.status,
                activity: &self.activity,
                selected: self.selected,
                now,
                fit,
                validation: &self.validation,
            },
            ui,
        );
        ctx.input_mut(|input| input.modifiers.shift = shift);
        ctx.data_mut(|data| data.insert_temp(Id::new(CANVAS_RESPONSE_ID), response.id));
        if self
            .activity
            .values()
            .any(|(at, _)| now.saturating_duration_since(*at) < WIRE_FLASH)
        {
            ctx.request_repaint();
        }

        for (node_id, index) in self.snarl.node_ids() {
            if let Some(response) = ctx.read_response(node_frame_id(node_id))
                .filter(|response| response.clicked())
            {
                if self.selected != Some(*index) {
                    self.node_settings_open = false;
                }
                self.selected = Some(*index);
                if response.double_clicked() {
                    self.node_settings_open = true;
                }
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
                        egui::ScrollArea::vertical().max_height((canvas_rect.height() - 150.0).max(100.0)).show(ui, |ui| {
                        for (index, kind) in KINDS.iter().enumerate() {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(170.0, 30.0), egui::Sense::hover());
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
                                kind.replace('_', " "),
                                egui::FontId::proportional(14.0),
                                ui.visuals().text_color(),
                            );
                            if response.clicked() {
                                selected_kind = Some(index);
                            }
                        }
                        });
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
        let active_drag = dragging.or(stopped);
        let dragged_node = self.snarl.node_ids().find_map(|(id, index)| {
            (Some(node_frame_id(id)) == active_drag).then_some(*index)
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

        if dragging.is_none() && stopped.is_some()
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

    fn show_overlays(&mut self, ctx: &egui::Context) {
        let running = self.running;
        let locked = self.pending_file_action.is_some() || self.recovery.is_some() || self.recovery_error.is_some();
        let mut state = OverlayState {
            scales: &mut self.region_scales,
            anchors: &mut self.overlay_anchors,
            targets: &mut self.overlay_targets,
        };
        visit_nodes_mut(&mut self.graph, "", &mut |node, key| {
            Self::overlay_node(ctx, node, key, running, locked, &mut state);
        });
    }

    fn overlay_node(ctx: &egui::Context, node: &mut Node, key: &str, running: bool, locked: bool, state: &mut OverlayState<'_>) {
        let Some(region) = node.region else {
            return;
        };
        // The builder position/size must stay constant: egui re-applies any change to it as a
        // window move, and feeding the measured region back in makes the window creep each frame.
        let anchor = match state.anchors.get(key) {
            Some(anchor) => *anchor,
            None => {
                state.anchors.insert(key.to_owned(), region);
                state.targets
                    .insert(key.to_owned(), (region, OVERLAY_PLACEMENT_ATTEMPTS));
                region
            }
        };
        let id = ViewportId::from_hash_of(("overlay", key));
        let builder = ViewportBuilder::default()
            .with_title(node.display_title())
            .with_decorations(false)
            .with_transparent(true)
            .with_has_shadow(false)
            .with_resizable(true)
            .with_min_inner_size(overlay_min_inner_size())
            .with_always_on_top()
            .with_mouse_passthrough(running || locked)
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
        let name = key.to_owned();
        let label = node.display_title().to_owned();
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            let viewport_ctx = ctx.clone();
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    let rect = ui.max_rect();
                    let grip = paint_overlay(ui, rect, &label, fill, outline, title, running);
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
                            let size = (start_size + egui::vec2(delta.x, delta.y)).max(overlay_min_inner_size());
                            viewport_ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                                size,
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
        if let Some((target, attempts)) = state.targets.get_mut(key) {
            let correction = outer.and_then(|outer| overlay_correction(*target, inner, outer));
            match correction {
                Some(position) if *attempts > 0 => {
                    *attempts -= 1;
                    ctx.send_viewport_cmd_to(id, egui::ViewportCommand::OuterPosition(position));
                    ctx.request_repaint_of(id);
                    return;
                }
                _ => {
                    state.targets.remove(key);
                }
            }
        }
        node.region = Some(capture_region(inner));
        if scale.is_finite() && scale > 0.0 {
            state.scales.insert(key.to_owned(), scale);
        }
    }
}

impl App {
    fn show_logs(&mut self, ui: &mut egui::Ui) {
        let (entries, dropped) = self.logs.snapshot();
        let expanded = self.logs_expanded;
        egui::Panel::bottom("session-logs-summary")
            .exact_size(26.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(if expanded { "v" } else { "^" })
                            .on_hover_text(if expanded { "Collapse logs" } else { "Expand logs" })
                            .clicked()
                        {
                            self.logs_expanded = !expanded;
                            ui.ctx().request_repaint();
                        }
                        if expanded && ui.button("Copy logs").clicked() {
                            ui.ctx().copy_text(entries.iter().map(|entry| entry.text.as_ref())
                                .collect::<Vec<_>>().join("\n"));
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            if expanded {
                                ui.strong(format!("Session logs ({})", entries.len()));
                                if dropped > 0 {
                                    ui.weak(format!("{dropped} older entries discarded"));
                                }
                            } else if let Some(entry) = entries.last() {
                                let color = match entry.level {
                                    log::Level::Error => Color32::from_rgb(220, 65, 65),
                                    log::Level::Warn => Color32::from_rgb(190, 130, 25),
                                    _ => ui.visuals().text_color(),
                                };
                                let mut details = entry.text.to_string();
                                if let Some(path) = &self.file {
                                    details.push_str(&format!("\n{}", path.display()));
                                }
                                if let Some(duration) = self.last_run_duration {
                                    details.push_str(&format!("\nRun: {} ms", duration.as_millis()));
                                }
                                ui.add(egui::Label::new(egui::RichText::new(entry.text.as_ref())
                                    .monospace().color(color)).truncate()).on_hover_text(details);
                            } else {
                                ui.add(egui::Label::new(if self.status.is_empty() {
                                    "No logs yet. Press Run to start."
                                } else { &self.status }).truncate());
                            }
                        });
                    });
                });
            });
        if expanded {
            egui::Panel::bottom("session-logs-expanded")
                .resizable(true)
                .default_size(220.0)
                .min_size(100.0)
                .max_size((ui.available_height() * 0.7).max(100.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(&self.status);
                        if let Some(duration) = self.last_run_duration {
                            ui.weak(format!("Run: {} ms", duration.as_millis()));
                        }
                        if let Some(interval) = self.last_run_interval {
                            ui.weak(format!("Interval: {} ms", interval.as_millis()));
                        }
                        if cfg!(debug_assertions) {
                            ui.weak("Debug build: OCR is slower");
                        }
                    });
                    egui::Frame::new().fill(Color32::from_rgb(20, 23, 29))
                        .inner_margin(6.0).show(ui, |ui| {
                            let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
                            egui::ScrollArea::both().id_salt("session-log-scroll")
                                .auto_shrink([false, false]).stick_to_bottom(true)
                                .show_rows(ui, row_height, entries.len(), |ui, range| {
                                    for entry in &entries[range] {
                                        let color = match entry.level {
                                            log::Level::Error => Color32::from_rgb(255, 115, 115),
                                            log::Level::Warn => Color32::from_rgb(240, 195, 95),
                                            _ => Color32::from_gray(215),
                                        };
                                        ui.add(egui::Label::new(egui::RichText::new(entry.text.as_ref())
                                            .monospace().color(color)).wrap_mode(egui::TextWrapMode::Extend)
                                            .selectable(true));
                                    }
                                });
                        });
                });
        }
    }

    fn set_running(&mut self, running: bool) {
        if self.running != running {
            if running {
                match self.graph.settings.run_mode {
                    RunMode::Timed => self.logs.record(log::Level::Info, format_args!(
                        "[RUN] Workflow started (delay {} ms)", self.graph.settings.refresh_ms,
                    )),
                    RunMode::DisplayRefresh => self.logs.record(log::Level::Info, format_args!(
                        "[RUN] Workflow started (display refresh / VSync)",
                    )),
                    RunMode::HmiEvent => self.logs.record(log::Level::Info, format_args!(
                        "[RUN] Workflow started (HMI Event / screen and external sources)",
                    )),
                }
            } else {
                self.logs.record(log::Level::Info, format_args!("[RUN] Workflow stopped"));
            }
        }
        self.running = running;
        if !running {
            if let Some(mut events) = self.screen_events.take() {
                if let Err(error) = events.stop() {
                    self.record_status(log::Level::Error, format!("Could not stop screen events: {error}"));
                }
            }
            if let Some(mut events) = self.extra_events.take() {
                if let Err(error) = events.stop() {
                    self.record_status(log::Level::Error, format!("Could not stop event sources: {error}"));
                }
            }
            self.event_runtime = EventRuntime::default();
            self.event_regions.clear();
            self.event_definition.clear();
        }
        self.last_tick = Instant::now();
        self.last_run_duration = None;
        self.last_run_interval = None;
        self.last_run_frame = None;
        self.overlay_ready_at = running.then_some(self.last_tick + OVERLAY_SETTLE_DELAY);
    }

    /// Escape stops a running workflow, whether pressed in this window or (on Windows) any other.
    fn handle_escape(&mut self, ctx: &egui::Context) {
        let global = self.escape_pressed.swap(false, Ordering::Relaxed);
        let local = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        if self.running && (global || local) {
            self.set_running(false);
            self.record_status(log::Level::Info, "Stopped (Esc)".into());
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
        self.handle_shortcuts(&ctx, true);
        egui::Panel::top("menu-bar").show(ui, |ui| {
            if self.subflow_editor.is_some() { self.show_subflow_menu(ui); }
            else { self.show_menu_bar(ui); }
        });
        self.show_logs(ui);

        egui::CentralPanel::default().show(ui, |ui| {
            self.show_canvas(ui);
        });
        self.show_timing_settings(&ctx);

        self.show_overlays(&ctx);
        self.handle_close_request(&ctx);
        self.show_unsaved_changes(&ctx);
        self.show_editor_windows(&ctx);
        self.capture_history(&ctx);
        self.autosave(&ctx);

        if self.running {
            if self.graph.settings.run_mode == RunMode::HmiEvent {
                if let Err(error) = self.run_hmi_events(&ctx) {
                    self.set_running(false);
                    self.record_status(log::Level::Error, format!("HMI Event stopped: {error}"));
                    ctx.request_repaint();
                }
                return;
            }
            let now = Instant::now();
            let ready_at = next_run_at(
                self.last_tick,
                &self.graph.settings,
                self.overlay_ready_at,
            );
            let frame = ctx.cumulative_frame_nr();
            if now >= ready_at && self.last_run_frame != Some(frame) {
                if self.last_run_duration.is_some() {
                    self.last_run_interval = Some(now.duration_since(self.last_tick));
                }
                self.last_tick = now;
                self.last_run_frame = Some(frame);
                self.overlay_ready_at = None;
                self.run_graph();
                self.last_run_duration = Some(now.elapsed());
                ctx.request_repaint();
            }
            if self.running {
                let ready_at = next_run_at(
                    self.last_tick,
                    &self.graph.settings,
                    self.overlay_ready_at,
                );
                ctx.request_repaint_after(ready_at.saturating_duration_since(Instant::now()));
            }
        }
    }
}

fn main() -> eframe::Result {
    let logs = SessionLog::default();
    logs.install().map_err(|error| eframe::Error::AppCreation(Box::new(error)))?;
    let (backend, status) = match Desktop::new() {
        Ok(backend) => (Some(backend), String::new()),
        Err(error) => (None, format!("Desktop backend unavailable: {error}")),
    };
    if !status.is_empty() {
        logs.record(log::Level::Error, format_args!("[APP] {status}"));
    }
    let mut app = App::new(backend, status, logs);
    app.initialize_workspace();
    app.rebuild_snarl();
    eframe::run_native(
        "ScreenAutomation",
        Default::default(),
        Box::new(move |cc| {
            configure_cursors(&cc.egui_ctx);
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
    fn resizing_overlay_preserves_thin_capture_regions_and_a_nonzero_minimum() {
        let ctx = egui::Context::default();
        for (requested_height, expected_height) in [(0.0, 1), (1.0, 1), (12.0, 12), (30.0, 30), (59.0, 59), (120.0, 120)] {
            let size = egui::vec2(312.0, requested_height + OVERLAY_HEADER + OVERLAY_MARGIN)
                .max(overlay_min_inner_size());
            let window = egui::Rect::from_min_size(Pos2::new(94.0, 76.0), size);
            assert_eq!(capture_region(window), Region {
                x: 100, y: 100, width: 300, height: expected_height,
            });
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(window), ..Default::default()
            }, |ui| {
                let grip = paint_overlay(ui, window, "ocr1", Color32::BLUE, Color32::RED, Color32::BLACK, false);
                assert!(window.contains_rect(grip), "resize handle outside thin window");
            });
            output.textures_delta.clear();
        }
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
        let settings = GraphSettings { refresh_ms: 100, ..Default::default() };
        let interval = Duration::from_millis(settings.refresh_ms);
        let ready = start + OVERLAY_SETTLE_DELAY;
        assert_eq!(next_run_at(start, &settings, Some(ready)), ready);
        let deadline = next_run_at(ready, &settings, None);
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
    fn display_refresh_ignores_delay_but_waits_for_overlays() {
        let start = Instant::now();
        let mut settings = GraphSettings {
            run_mode: RunMode::DisplayRefresh,
            refresh_ms: 60_000,
        };
        let ready = start + OVERLAY_SETTLE_DELAY;
        assert_eq!(next_run_at(start, &settings, Some(ready)), ready);
        assert_eq!(next_run_at(ready, &settings, None), ready);
        settings.run_mode = RunMode::Timed;
        assert_eq!(next_run_at(ready, &settings, None), ready + Duration::from_secs(60));
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
        assert_eq!(DEFAULT_REFRESH_MS, 1000);
        assert_eq!(refresh_interval("3000"), Some(3000));
        assert_eq!(refresh_interval("100"), Some(100));
        assert_eq!(refresh_interval("60000"), Some(60000));
        for value in ["", "99", "60001", "-1", "abc", "1.5"] {
            assert_eq!(refresh_interval(value), None);
        }
    }

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

    #[test]
    fn workflow_uses_scaled_regions_and_restores_them_on_error() {
        let mut graph = Graph::default();
        graph
            .add_node(Node::new("source", Kind::Ocr { single_line: false, last_hash: None }))
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
        let mut app = App::new(None, String::new(), SessionLog::default());
        app.kind = 1;
        app
    }

    #[test]
    fn clickable_controls_use_hand_cursors_without_overriding_text_or_disabled_controls() {
        let ctx = egui::Context::default();
        configure_cursors(&ctx);
        let render = |pointer: Option<Pos2>| {
            let mut positions = HashMap::new();
            let mut checked = false;
            let mut text = "Title".to_owned();
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 700.0))),
                events: pointer.map(egui::Event::PointerMoved).into_iter().collect(),
                ..Default::default()
            }, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let controls = [
                        ("button", ui.button("Run")),
                        ("checkbox", ui.checkbox(&mut checked, "Only trigger on change")),
                        ("radio", ui.radio(false, "Display refresh")),
                        ("combo", egui::ComboBox::from_id_salt("cursor-combo")
                            .selected_text("Number").show_ui(ui, |_| {}).response),
                        ("menu", ui.menu_button("File", |_| {}).response),
                        ("text", ui.text_edit_singleline(&mut text)),
                        ("disabled", ui.add_enabled(false, egui::Button::new("Disabled"))),
                    ];
                    for (name, response) in controls {
                        positions.insert(name, response.rect.center());
                    }
                });
            });
            output.textures_delta.clear();
            (output, positions)
        };
        render(None);
        let (_, positions) = render(None);
        for name in ["button", "checkbox", "radio", "combo", "menu"] {
            let (output, _) = render(Some(positions[name]));
            assert_eq!(output.platform_output.cursor_icon, egui::CursorIcon::PointingHand, "{name}");
        }
        let (output, _) = render(Some(positions["text"]));
        assert_eq!(output.platform_output.cursor_icon, egui::CursorIcon::Text);
        let (output, _) = render(Some(positions["disabled"]));
        assert_eq!(output.platform_output.cursor_icon, egui::CursorIcon::Default);
    }

    #[test]
    fn canvas_controls_and_nodes_use_hand_cursors_but_empty_canvas_does_not() {
        let ctx = egui::Context::default();
        configure_cursors(&ctx);
        let mut app = app();
        app.add_node();
        let mut time = 0.0;
        let mut render = |app: &mut App, pointer: Option<Pos2>| {
            time += 1.0 / 60.0;
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 700.0))),
                time: Some(time),
                events: pointer.map(egui::Event::PointerMoved).into_iter().collect(),
                ..Default::default()
            }, |ui| {
                egui::CentralPanel::default().show(ui, |ui| app.show_canvas(ui));
            });
            output.textures_delta.clear();
            output.platform_output.cursor_icon
        };
        render(&mut app, None);
        render(&mut app, None);
        let node = app.snarl.node_ids().next().unwrap().0;
        let node = node_position(&ctx, node);
        let add = ctx.read_response(Id::new(ADD_BUTTON_ID)).unwrap().rect.center();
        let trash = ctx.read_response(Id::new(TRASH_BUTTON_ID)).unwrap().rect.center();
        for pointer in [node, add, trash] {
            assert_eq!(render(&mut app, Some(pointer)), egui::CursorIcon::PointingHand);
        }
        assert_eq!(render(&mut app, Some(Pos2::new(450.0, 600.0))), egui::CursorIcon::Default);
        app.add_menu_open = true;
        render(&mut app, None);
        render(&mut app, None);
        render(&mut app, None);
        let item = ctx.read_response(Id::new(("canvas-add-kind", 0usize))).unwrap().rect.center();
        assert_eq!(render(&mut app, Some(item)), egui::CursorIcon::PointingHand);
    }

    #[test]
    fn compare_inputs_renders_two_ports_and_selectable_false_output() {
        let ctx = egui::Context::default();
        let mut app = app();
        for kind in ["constant", "constant", "compare_inputs"] {
            app.kind = KINDS.iter().position(|&name| name == kind).unwrap();
            app.add_node();
        }
        let names: Vec<_> = app.graph.nodes.iter().map(|node| node.name.clone()).collect();
        app.graph.connect_input(&names[1], &names[2], 1).unwrap();
        app.graph.connect_input(&names[0], &names[2], 0).unwrap();
        app.rebuild_snarl();
        assert!(app.snarl.wires().any(|(source, target)| {
            target.input == 1 && app.snarl[source.node] == 1 && app.snarl[target.node] == 2
        }));
        let mut time = 0.0;
        let mut render = |app: &mut App, events| {
            time += 1.0 / 60.0;
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1100.0, 800.0))),
                time: Some(time), events, ..Default::default()
            }, |ui| {
                egui::CentralPanel::default().show(ui, |ui| app.show_canvas(ui));
            });
            output.textures_delta.clear();
            output
        };
        render(&mut app, vec![]);
        let output = render(&mut app, vec![]);
        for label in ["A", "B"] {
            assert!(output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label)
            }));
        }
        app.selected = Some(2);
        app.node_settings_open = true;
        render(&mut app, vec![]);
        let output = render(&mut app, vec![]);
        let position = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "False" =>
                Some(text.pos + text.galley.size() * 0.5),
            _ => None,
        }).unwrap();
        render(&mut app, vec![egui::Event::PointerMoved(position), button(position, true)]);
        render(&mut app, vec![button(position, false)]);
        assert!(matches!(app.graph.nodes[2].kind, Kind::CompareInputs { trigger_when: false, .. }));
    }

    #[test]
    fn threshold_settings_offer_both_range_options_but_hysteresis_does_not() {
        for kind in ["threshold", "hysteresis"] {
            let ctx = egui::Context::default();
            let mut app = app();
            app.kind = KINDS.iter().position(|&name| name == kind).unwrap();
            app.add_node();
            app.selected = Some(0);
            app.node_settings_open = true;
            let mut time = 0.0;
            let mut render = |app: &mut App, events| {
                time += 1.0 / 60.0;
                let mut output = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1100.0, 800.0))),
                    time: Some(time), events, ..Default::default()
                }, |ui| {
                    egui::CentralPanel::default().show(ui, |ui| app.show_canvas(ui));
                });
                output.textures_delta.clear();
                output
            };
            let text_position = |output: &egui::FullOutput, label: &str| {
                output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label =>
                        Some(text.pos + text.galley.size() * 0.5),
                    _ => None,
                })
            };
            render(&mut app, vec![]);
            let mut output = render(&mut app, vec![]);
            let mut selected = Crossing::Both;
            for choice in [Crossing::EnteringRange, Crossing::EnteringRangeInclusive] {
                let combo = text_position(&output, selected.label()).unwrap();
                render(&mut app, vec![egui::Event::PointerMoved(combo), button(combo, true)]);
                render(&mut app, vec![button(combo, false)]);
                render(&mut app, vec![]);
                output = render(&mut app, vec![]);
                if kind == "hysteresis" {
                    for range in [Crossing::EnteringRange, Crossing::EnteringRangeInclusive] {
                        assert!(text_position(&output, range.label()).is_none());
                    }
                    break;
                }
                let option = text_position(&output, choice.label()).unwrap();
                render(&mut app, vec![egui::Event::PointerMoved(option), button(option, true)]);
                render(&mut app, vec![button(option, false)]);
                assert_eq!(app.selected, Some(0));
                assert!(app.node_settings_open);
                assert!(matches!(app.graph.nodes[0].kind, Kind::Threshold { trigger, band: None, .. } if trigger == choice));
                let node = &app.graph.nodes[0];
                let Kind::Threshold { lower, upper, .. } = node.kind else { unreachable!() };
                let interval = if choice == Crossing::EnteringRange {
                    format!("({lower}, {upper})")
                } else {
                    format!("[{lower}, {upper}]")
                };
                assert!(node_label(node).ends_with(&interval));
                output = render(&mut app, vec![]);
                selected = choice;
            }
        }
    }

    #[test]
    fn adding_hysteresis_and_threshold_creates_distinct_behaviors() {
        let mut app = app();
        for kind in ["hysteresis", "threshold"] {
            app.kind = KINDS.iter().position(|&name| name == kind).unwrap();
            app.add_node();
        }
        let mut backend = RecordingBackend::default();
        assert_eq!(app.graph.nodes[0].type_name(), "hysteresis");
        assert_eq!(app.graph.nodes[1].type_name(), "threshold");
        assert_eq!(app.graph.nodes[0].process(&[Value::Num(101.0)], &mut backend).unwrap(), None);
        assert_eq!(app.graph.nodes[1].process(&[Value::Num(101.0)], &mut backend).unwrap(),
            Some(Value::Num(101.0)));
    }

    #[test]
    fn saving_and_loading_remembers_path_and_restores_workflow() {
        let path = std::env::temp_dir().join(format!("sa-file-roundtrip-{}.json", std::process::id()));
        let mut app = app();
        app.add_node();
        app.kind = 2;
        app.add_node();
        let source = app.graph.nodes[0].name.clone();
        let target = app.graph.nodes[1].name.clone();
        app.graph.connect(&source, &target).unwrap();
        app.graph.settings.refresh_ms = 750;
        app.graph.settings.run_mode = RunMode::DisplayRefresh;
        app.refresh_input = "750".into();
        app.save_graph(&path);
        assert!(!app.has_unsaved_changes());
        assert_eq!(app.file.as_deref(), Some(path.as_path()));
        let saved = std::fs::read_to_string(&path).unwrap();
        app.graph.settings.refresh_ms = 5000;
        app.graph.settings.run_mode = RunMode::Timed;
        app.refresh_input = "5000".into();
        app.add_node();
        app.selected = Some(2);
        app.add_menu_open = true;
        app.region_scales.insert(source.clone(), 1.5);
        app.activity.insert(source, (Instant::now(), "old output".into()));
        app.set_running(true);
        app.load_graph(&path);
        assert!(!app.has_unsaved_changes());
        assert_eq!(app.graph.to_json(), saved);
        assert_eq!(app.graph.settings.refresh_ms, 750);
        assert_eq!(app.graph.settings.run_mode, RunMode::DisplayRefresh);
        assert_eq!(app.refresh_input, "750");
        assert_eq!(app.file.as_deref(), Some(path.as_path()));
        assert_eq!(app.window_title(), format!("{} - ScreenAutomation", path.file_name().unwrap().to_string_lossy()));
        assert_eq!(app.snarl.node_ids().count(), 2);
        assert_eq!(app.snarl.wires().count(), 1);
        assert!(!app.running);
        assert!(!app.add_menu_open);
        assert_eq!(app.selected, None);
        assert!(app.region_scales.is_empty());
        assert!(app.activity.is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_file_operations_preserve_current_workflow_and_path() {
        let path = std::env::temp_dir().join(format!("sa-file-invalid-{}.json", std::process::id()));
        std::fs::write(&path, "not JSON").unwrap();
        let mut app = app();
        app.add_node();
        app.file = Some(PathBuf::from("existing.json"));
        let graph = app.graph.to_json();
        app.load_graph(&path);
        assert!(app.status.starts_with("Could not load"));
        assert_eq!(app.graph.to_json(), graph);
        assert_eq!(app.file.as_deref(), Some(Path::new("existing.json")));
        std::fs::write(&path, r#"{"nodes":[],"edges":[],"settings":{"refresh_ms":0}}"#).unwrap();
        app.load_graph(&path);
        assert!(app.status.contains("refresh_ms must be between"));
        assert_eq!(app.graph.to_json(), graph);
        app.save_graph(&std::env::temp_dir());
        assert!(app.status.starts_with("Could not save"));
        let (logs, _) = app.logs.snapshot();
        assert!(logs.iter().any(|entry| entry.level == log::Level::Error
            && entry.text.contains("Could not save")));
        assert_eq!(app.file.as_deref(), Some(Path::new("existing.json")));
        assert_eq!(app.graph.to_json(), graph);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn unsaved_changes_include_persisted_edits_but_not_execution_state() {
        let mut app = app();
        for kind in [0, 5, 6] {
            app.kind = kind;
            app.add_node();
        }
        let baseline = app.graph.to_json();
        app.replace_graph(Graph::from_json(&baseline).unwrap(), None);
        let edits: &[fn(&mut App)] = &[
            |app| app.graph.nodes[0].title = "Price".into(),
            |app| app.graph.nodes[0].region.as_mut().unwrap().x += 10,
            |app| app.graph.nodes[0].color = [255, 0, 0],
            |app| app.graph.nodes[0].opacity = 0.8,
            |app| {
                if let Kind::Threshold { upper, .. } = &mut app.graph.nodes[1].kind {
                    *upper += 1.0;
                }
            },
            |app| {
                let id = app.snarl.node_ids().next().unwrap().0;
                app.snarl.get_node_info_mut(id).unwrap().pos.x += 20.0;
            },
            |app| {
                let source = app.graph.nodes[0].name.clone();
                let target = app.graph.nodes[1].name.clone();
                app.graph.connect(&source, &target).unwrap();
            },
            |app| app.graph.settings.refresh_ms = 1500,
            |app| app.graph.settings.run_mode = RunMode::DisplayRefresh,
            |app| app.add_node(),
            |app| app.delete_node(1),
        ];
        for edit in edits {
            edit(&mut app);
            assert!(app.has_unsaved_changes());
            assert!(app.window_title().starts_with("Untitled*"));
            app.replace_graph(Graph::from_json(&baseline).unwrap(), None);
            assert!(!app.has_unsaved_changes());
        }
        app.graph.nodes[0].title = "Edited".into();
        assert!(app.has_unsaved_changes());
        app.graph.nodes[0].title.clear();
        let mut backend = RecordingBackend::default();
        app.graph.nodes[1].process(&[Value::Int(1000)], &mut backend).unwrap();
        if let Kind::Ocr { last_hash, .. } = &mut app.graph.nodes[0].kind {
            *last_hash = Some(123);
        }
        app.activity.insert("ocr1".into(), (Instant::now(), "1000".into()));
        app.logs.record(log::Level::Info, format_args!("runtime log"));
        app.set_running(true);
        app.set_running(false);
        assert!(!app.has_unsaved_changes());
    }

    #[test]
    fn save_before_new_or_exit_preserves_edits_and_only_then_continues() {
        for action in [FileAction::New, FileAction::Exit] {
            let path = std::env::temp_dir().join(format!(
                "sa-unsaved-{action:?}-{}.json", std::process::id()));
            let mut app = app();
            app.add_node();
            assert!(app.save_graph(&path));
            let saved = std::fs::read_to_string(&path).unwrap();
            app.graph.nodes[0].title = "Unsaved title".into();
            app.graph.settings.refresh_ms = 1500;
            app.refresh_input = "1500".into();
            let edited = app.graph.to_json();
            let ctx = egui::Context::default();
            app.set_running(true);
            app.request_file_action(action.clone(), &ctx);
            assert!(!app.running);
            assert_eq!(app.pending_file_action, Some(action.clone()));
            app.resolve_unsaved_changes(UnsavedChoice::Cancel, &ctx);
            assert_eq!(app.pending_file_action, None);
            assert_eq!(app.graph.to_json(), edited);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
            app.request_file_action(action.clone(), &ctx);
            let mut output = ctx.run_ui(Default::default(), |ui| {
                app.resolve_unsaved_changes(UnsavedChoice::Save, ui.ctx());
            });
            assert_eq!(app.pending_file_action, None);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
            assert!(!app.has_unsaved_changes());
            if action == FileAction::New {
                assert!(app.graph.nodes.is_empty());
                assert!(app.graph.edges.is_empty());
                assert_eq!(app.graph.settings, GraphSettings::default());
                assert_eq!(app.refresh_input, DEFAULT_REFRESH_MS.to_string());
                assert!(app.file.is_none());
                assert_eq!(app.snarl.node_ids().count(), 0);
            } else {
                assert!(app.close_confirmed);
                assert!(output.viewport_output[&ViewportId::ROOT].commands.iter()
                    .any(|command| matches!(command, egui::ViewportCommand::Close)));
            }
            output.textures_delta.clear();
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn failed_confirmation_save_keeps_graph_and_pending_action() {
        for action in [FileAction::New, FileAction::Load, FileAction::Exit] {
            let mut app = app();
            app.add_node();
            app.file = Some(std::env::temp_dir());
            let edited = app.graph.to_json();
            let baseline = app.saved_graph.clone();
            let ctx = egui::Context::default();
            app.request_file_action(action.clone(), &ctx);
            let mut output = ctx.run_ui(Default::default(), |ui| {
                app.resolve_unsaved_changes(UnsavedChoice::Save, ui.ctx());
                app.show_unsaved_changes(ui.ctx());
            });
            assert_eq!(app.pending_file_action, Some(action.clone()));
            assert_eq!(app.graph.to_json(), edited);
            assert_eq!(app.saved_graph, baseline);
            assert!(app.has_unsaved_changes());
            assert!(app.save_error.as_deref().unwrap().contains("Could not save"));
            assert!(!app.close_confirmed);
            assert!(!output.viewport_output[&ViewportId::ROOT].commands.iter()
                .any(|command| matches!(command, egui::ViewportCommand::Close)));
            output.textures_delta.clear();
        }
    }

    #[test]
    fn file_new_and_native_close_use_save_discard_cancel_dialog() {
        let ctx = egui::Context::default();
        let mut app = app();
        let mut time = 0.0;
        let mut render = |app: &mut App, events, close_requested| {
            time += 1.0 / 60.0;
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 700.0))),
                time: Some(time), events, ..Default::default()
            };
            if close_requested {
                input.viewports.entry(ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
            }
            let mut output = ctx.run_ui(input, |ui| {
                egui::Panel::top("menu-bar").show(ui, |ui| app.show_menu_bar(ui));
                egui::CentralPanel::default().show(ui, |_| {});
                app.handle_close_request(ui.ctx());
                app.show_unsaved_changes(ui.ctx());
            });
            output.textures_delta.clear();
            output
        };
        let text_position = |output: &egui::FullOutput, label: &str| {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label =>
                    Some(text.pos + text.galley.size() * 0.5),
                _ => None,
            }).unwrap_or_else(|| panic!("Missing control: {label}"))
        };
        app.add_node();
        let original = app.graph.to_json();
        for decision in ["Cancel", "Discard"] {
            render(&mut app, vec![], false);
            let output = render(&mut app, vec![], false);
            let file = text_position(&output, "File");
            render(&mut app, vec![egui::Event::PointerMoved(file), button(file, true)], false);
            render(&mut app, vec![button(file, false)], false);
            let output = render(&mut app, vec![], false);
            let new = text_position(&output, "New");
            render(&mut app, vec![egui::Event::PointerMoved(new), button(new, true)], false);
            render(&mut app, vec![button(new, false)], false);
            assert_eq!(app.pending_file_action, Some(FileAction::New));
            render(&mut app, vec![], false);
            let output = render(&mut app, vec![], false);
            for label in ["Unsaved changes", "Save", "Discard", "Cancel"] {
                text_position(&output, label);
            }
            let choice = text_position(&output, decision);
            render(&mut app, vec![egui::Event::PointerMoved(choice), button(choice, true)], false);
            render(&mut app, vec![button(choice, false)], false);
            assert_eq!(app.pending_file_action, None);
            if decision == "Cancel" {
                assert_eq!(app.graph.to_json(), original);
            }
        }
        assert!(app.graph.nodes.is_empty());
        assert!(!app.has_unsaved_changes());
        app.add_node();
        for decision in ["Cancel", "Discard"] {
            let output = render(&mut app, vec![], true);
            assert!(output.viewport_output[&ViewportId::ROOT].commands.iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose)));
            assert_eq!(app.pending_file_action, Some(FileAction::Exit));
            render(&mut app, vec![], false);
            let output = render(&mut app, vec![], false);
            let choice = text_position(&output, decision);
            render(&mut app, vec![egui::Event::PointerMoved(choice), button(choice, true)], false);
            let output = render(&mut app, vec![button(choice, false)], false);
            assert_eq!(app.pending_file_action, None);
            let closing = output.viewport_output[&ViewportId::ROOT].commands.iter()
                .any(|command| matches!(command, egui::ViewportCommand::Close));
            assert_eq!(closing, decision == "Discard");
            assert_eq!(app.graph.nodes.len(), 1);
        }
        let output = render(&mut app, vec![], true);
        assert!(!output.viewport_output[&ViewportId::ROOT].commands.iter()
            .any(|command| matches!(command, egui::ViewportCommand::CancelClose)));
        assert_eq!(app.pending_file_action, None);
        app.replace_graph(Graph::default(), None);
        let output = render(&mut app, vec![], true);
        assert!(!output.viewport_output[&ViewportId::ROOT].commands.iter()
            .any(|command| matches!(command, egui::ViewportCommand::CancelClose)));
        assert_eq!(app.pending_file_action, None);
    }

    #[test]
    fn session_logs_toggle_at_bottom_right_and_preserve_history() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.logs.record(log::Level::Info, format_args!("first event"));
        app.logs.record(log::Level::Warn, format_args!("latest OCR result"));
        let mut time = 0.0;
        let mut log_frame = |app: &mut App, events| {
            time += 1.0 / 60.0;
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 700.0))),
                time: Some(time), events, ..Default::default()
            }, |ui| app.show_logs(ui));
            output.textures_delta.clear();
            output
        };
        let text_position = |output: &egui::FullOutput, label: &str| {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
        };
        assert!(!app.logs_expanded);
        log_frame(&mut app, vec![]);
        let output = log_frame(&mut app, vec![]);
        let expand = text_position(&output, "^").unwrap();
        assert!(expand.x > 850.0 && expand.y > 650.0, "button is not bottom-right: {expand:?}");
        let (entries, _) = app.logs.snapshot();
        assert!(text_position(&output, &entries[1].text).is_some());
        assert!(text_position(&output, &entries[0].text).is_none());
        log_frame(&mut app, vec![egui::Event::PointerMoved(expand), button(expand, true)]);
        log_frame(&mut app, vec![button(expand, false)]);
        assert!(app.logs_expanded);
        log_frame(&mut app, vec![]);
        let output = log_frame(&mut app, vec![]);
        assert!(text_position(&output, &entries[0].text).is_some());
        assert!(text_position(&output, &entries[1].text).is_some());
        let copy = text_position(&output, "Copy logs").unwrap();
        log_frame(&mut app, vec![egui::Event::PointerMoved(copy), button(copy, true)]);
        let output = log_frame(&mut app, vec![button(copy, false)]);
        assert!(output.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(text)
                if text.contains("first event") && text.contains("latest OCR result"))
        }));
        let output = log_frame(&mut app, vec![]);
        let collapse = text_position(&output, "v").unwrap();
        assert!(collapse.x > 850.0 && collapse.y > 650.0, "collapse button moved: {collapse:?}");
        log_frame(&mut app, vec![egui::Event::PointerMoved(collapse), button(collapse, true)]);
        log_frame(&mut app, vec![button(collapse, false)]);
        assert!(!app.logs_expanded);
        assert_eq!(app.logs.snapshot().0.len(), 2);
    }

    #[test]
    fn menu_bar_controls_run_settings_and_file_exit() {
        let ctx = egui::Context::default();
        let mut app = app();
        let mut time = 0.0;
        let mut menu_frame = |app: &mut App, events| {
            time += 1.0 / 60.0;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        Pos2::ZERO, egui::vec2(900.0, 700.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::Panel::top("menu-bar").show(ui, |ui| app.show_menu_bar(ui));
                    app.show_timing_settings(ui.ctx());
                },
            );
            output.textures_delta.clear();
            output
        };
        let text_position = |output: &egui::FullOutput, label: &str| {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            }).unwrap_or_else(|| panic!("Missing menu item: {label}"))
        };
        let output = menu_frame(&mut app, vec![]);
        assert!(output.viewport_output[&ViewportId::ROOT].commands.iter()
            .any(|command| matches!(command, egui::ViewportCommand::Title(title) if title == "Untitled - ScreenAutomation")));
        let file = text_position(&output, "File");
        let run = text_position(&output, "     Run");
        menu_frame(&mut app, vec![egui::Event::PointerMoved(run), button(run, true)]);
        menu_frame(&mut app, vec![button(run, false)]);
        assert!(app.running);
        let output = menu_frame(&mut app, vec![]);
        let stop = text_position(&output, "     Stop (Esc)");
        menu_frame(&mut app, vec![egui::Event::PointerMoved(stop), button(stop, true)]);
        menu_frame(&mut app, vec![button(stop, false)]);
        assert!(!app.running);
        let settings = text_position(&output, "Settings");
        menu_frame(&mut app, vec![egui::Event::PointerMoved(settings), button(settings, true)]);
        menu_frame(&mut app, vec![button(settings, false)]);
        let output = menu_frame(&mut app, vec![]);
        let timing = text_position(&output, "Run timing...");
        assert!(!app.timing_settings_open);
        assert!(!output.shapes.iter().any(|shape| matches!(
            &shape.shape, egui::Shape::Text(text) if text.galley.text() == "Delay (ms)"
        )));
        menu_frame(&mut app, vec![egui::Event::PointerMoved(timing), button(timing, true)]);
        menu_frame(&mut app, vec![button(timing, false)]);
        menu_frame(&mut app, vec![]);
        let output = menu_frame(&mut app, vec![]);
        assert!(app.timing_settings_open);
        text_position(&output, "Delay (ms)");
        let input = text_position(&output, "1000");
        menu_frame(&mut app, vec![egui::Event::PointerMoved(input), button(input, true)]);
        menu_frame(&mut app, vec![button(input, false)]);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(Id::new("refresh-delay-input")));
        let select_all = egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers { ctrl: true, command: true, ..Default::default() },
        };
        menu_frame(&mut app, vec![select_all.clone(), egui::Event::Text("1500".into())]);
        assert_eq!(app.refresh_input, "1500");
        assert_eq!(app.graph.settings.refresh_ms, 1500);
        let output = menu_frame(&mut app, vec![select_all, egui::Event::Text("10".into())]);
        assert_eq!(app.graph.settings.refresh_ms, 1500);
        text_position(&output, "Enter 100–60000 ms");
        let display_refresh = text_position(&output, "Display refresh");
        menu_frame(&mut app, vec![egui::Event::PointerMoved(display_refresh), button(display_refresh, true)]);
        menu_frame(&mut app, vec![button(display_refresh, false)]);
        let output = menu_frame(&mut app, vec![]);
        assert_eq!(app.graph.settings.run_mode, RunMode::DisplayRefresh);
        assert!(!ctx.read_response(Id::new("refresh-delay-input")).unwrap().enabled());
        assert_eq!(app.graph.settings.refresh_ms, 1500);
        let timed = text_position(&output, "Timed delay");
        menu_frame(&mut app, vec![egui::Event::PointerMoved(timed), button(timed, true)]);
        menu_frame(&mut app, vec![button(timed, false)]);
        menu_frame(&mut app, vec![]);
        assert_eq!(app.graph.settings.run_mode, RunMode::Timed);
        assert!(ctx.read_response(Id::new("refresh-delay-input")).unwrap().enabled());
        assert_eq!(app.graph.settings.refresh_ms, 1500);
        #[cfg(windows)]
        {
            let output = menu_frame(&mut app, vec![]);
            let event_mode = text_position(&output, "HMI Event");
            menu_frame(&mut app, vec![egui::Event::PointerMoved(event_mode), button(event_mode, true)]);
            menu_frame(&mut app, vec![button(event_mode, false)]);
            menu_frame(&mut app, vec![]);
            assert_eq!(app.graph.settings.run_mode, RunMode::HmiEvent);
            assert!(!ctx.read_response(Id::new("refresh-delay-input")).unwrap().enabled());
            assert_eq!(Graph::from_json(&app.graph.to_json()).unwrap().settings.run_mode, RunMode::HmiEvent);
            app.set_running(true);
            menu_frame(&mut app, vec![egui::Event::PointerMoved(timed), button(timed, true)]);
            menu_frame(&mut app, vec![button(timed, false)]);
            assert_eq!(app.graph.settings.run_mode, RunMode::Timed);
            assert!(!app.running);
        }
        menu_frame(&mut app, vec![egui::Event::PointerMoved(file), button(file, true)]);
        menu_frame(&mut app, vec![button(file, false)]);
        let output = menu_frame(&mut app, vec![]);
        for label in ["New", "Load graph JSON...", "Save", "Save As...", "Exit"] {
            text_position(&output, label);
        }
        let exit = text_position(&output, "Exit");
        app.set_running(true);
        menu_frame(&mut app, vec![egui::Event::PointerMoved(exit), button(exit, true)]);
        let output = menu_frame(&mut app, vec![button(exit, false)]);
        assert!(!app.running);
        assert_eq!(app.pending_file_action, Some(FileAction::Exit));
        assert!(!output.viewport_output[&ViewportId::ROOT].commands.iter()
            .any(|command| matches!(command, egui::ViewportCommand::Close)));
    }

    fn frame(ctx: &egui::Context, app: &mut App, events: Vec<egui::Event>) {
        frame_with_modifiers(ctx, app, events, egui::Modifiers::NONE);
    }

    fn frame_with_modifiers(
        ctx: &egui::Context,
        app: &mut App,
        mut events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) {
        events.insert(0, egui::Event::ModifiersChanged(modifiers));
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
    fn node_title_can_be_edited_in_settings() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.add_node();
        let name = app.graph.nodes[0].name.clone();
        app.selected = Some(0);
        app.node_settings_open = true;
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![]);
        let input = ctx.read_response(Id::new(("node-title", 0usize))).unwrap().rect.center();
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(input), button(input, true)]);
        frame(&ctx, &mut app, vec![button(input, false)]);
        frame(&ctx, &mut app, vec![egui::Event::Text("BTC price".into())]);
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.graph.nodes[0].title, "BTC price");
        assert_eq!(node_label(&app.graph.nodes[0]), "BTC price");
        assert_eq!(app.graph.nodes[0].name, name);
    }

    #[test]
    fn node_settings_dismiss_outside_but_keep_node_and_dropdown_clicks() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.kind = KINDS.iter().position(|&kind| kind == "compare_inputs").unwrap();
        app.add_node();
        app.kind = 1;
        app.add_node();
        app.graph.nodes[1].position = [750.0, 80.0];
        app.rebuild_snarl();
        app.selected = Some(0);
        app.node_settings_open = true;
        let mut time = 0.0;
        let mut render = |app: &mut App, events| {
            time += 1.0 / 60.0;
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1100.0, 800.0))),
                time: Some(time), events, ..Default::default()
            }, |ui| {
                egui::CentralPanel::default().show(ui, |ui| app.show_canvas(ui));
            });
            output.textures_delta.clear();
            output
        };
        let text_position = |output: &egui::FullOutput, label: &str| {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label =>
                    Some(text.pos + text.galley.size() * 0.5),
                _ => None,
            }).unwrap_or_else(|| panic!("Missing settings text: {label}"))
        };
        render(&mut app, vec![]);
        let output = render(&mut app, vec![]);
        let inside = text_position(&output, "Compare input A with input B");
        render(&mut app, vec![egui::Event::PointerMoved(inside), button(inside, true)]);
        render(&mut app, vec![button(inside, false)]);
        assert_eq!(app.selected, Some(0));
        let node = app.snarl.node_ids().find(|(_, index)| **index == 0).unwrap().0;
        let same_node = node_position(&ctx, node);
        render(&mut app, vec![egui::Event::PointerMoved(same_node), button(same_node, true)]);
        render(&mut app, vec![button(same_node, false)]);
        assert_eq!(app.selected, Some(0));
        let output = render(&mut app, vec![]);
        let combo = text_position(&output, "A > B");
        render(&mut app, vec![egui::Event::PointerMoved(combo), button(combo, true)]);
        render(&mut app, vec![button(combo, false)]);
        render(&mut app, vec![]);
        let output = render(&mut app, vec![]);
        let option = text_position(&output, "A < B");
        render(&mut app, vec![egui::Event::PointerMoved(option), button(option, true)]);
        render(&mut app, vec![button(option, false)]);
        assert_eq!(app.selected, Some(0));
        assert!(matches!(app.graph.nodes[0].kind, Kind::CompareInputs { op: CompareOp::Less, .. }));
        // Let earlier clicks expire so the next pair is a double-click, not a triple-click.
        for _ in 0..40 {
            render(&mut app, vec![]);
        }
        let other = app.snarl.node_ids().find(|(_, index)| **index == 1).unwrap().0;
        let other_node = node_position(&ctx, other);
        render(&mut app, vec![egui::Event::PointerMoved(other_node), button(other_node, true)]);
        render(&mut app, vec![button(other_node, false)]);
        assert_eq!(app.selected, Some(1));
        assert!(!app.node_settings_open);
        render(&mut app, vec![button(other_node, true)]);
        render(&mut app, vec![button(other_node, false)]);
        assert!(app.node_settings_open);
        let outside = Pos2::new(20.0, 760.0);
        render(&mut app, vec![egui::Event::PointerMoved(outside), button(outside, true)]);
        render(&mut app, vec![button(outside, false)]);
        assert_eq!(app.selected, None);
        assert!(!app.node_settings_open);
    }

    #[test]
    fn single_click_selects_and_only_double_click_opens_node_settings() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.add_node();
        app.add_node();
        app.graph.nodes[1].position = [750.0, 80.0];
        app.rebuild_snarl();
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![]);
        let first = app.snarl.node_ids().find(|(_, index)| **index == 0).unwrap().0;
        let first = node_position(&ctx, first);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(first), button(first, true)]);
        frame(&ctx, &mut app, vec![button(first, false)]);
        assert_eq!(app.selected, Some(0));
        assert!(!app.node_settings_open);
        assert!(ctx.read_response(Id::new(("node-title", 0usize))).is_none());
        frame(&ctx, &mut app, vec![button(first, true)]);
        frame(&ctx, &mut app, vec![button(first, false)]);
        assert!(app.node_settings_open);
        frame(&ctx, &mut app, vec![]);
        assert!(ctx.read_response(Id::new(("node-title", 0usize))).is_some());

        for _ in 0..40 {
            frame(&ctx, &mut app, vec![]);
        }
        let second = app.snarl.node_ids().find(|(_, index)| **index == 1).unwrap().0;
        let second = node_position(&ctx, second);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(second), button(second, true)]);
        frame(&ctx, &mut app, vec![button(second, false)]);
        assert_eq!(app.selected, Some(1));
        assert!(!app.node_settings_open);
        frame(&ctx, &mut app, vec![button(second, true)]);
        frame(&ctx, &mut app, vec![button(second, false)]);
        assert_eq!(app.selected, Some(1));
        assert!(app.node_settings_open);
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
    fn shift_drag_copies_once_preserves_the_original_and_works_when_zoomed() {
        for zoom in [false, true] {
            let ctx = egui::Context::default();
            let mut app = app();
            app.kind = 0;
            app.add_node();
            app.kind = 1;
            app.add_node();
            app.graph.nodes[1].name = "ocr3".into();
            app.graph.nodes[0].title = "Price".into();
            app.graph.nodes[0].color = [10, 20, 30];
            app.graph.nodes[0].opacity = 0.6;
            let names: Vec<_> = app.graph.nodes.iter().map(|node| node.name.clone()).collect();
            app.graph.connect(&names[0], &names[1]).unwrap();
            app.rebuild_snarl();
            app.selected = Some(0);
            app.node_settings_open = true;
            frame(&ctx, &mut app, vec![]);
            frame(&ctx, &mut app, vec![]);
            let source = app.snarl.node_ids().find(|(_, index)| **index == 0).unwrap().0;
            if zoom {
                let center = node_position(&ctx, source);
                frame(&ctx, &mut app, vec![egui::Event::PointerMoved(center), egui::Event::Zoom(1.5)]);
                frame(&ctx, &mut app, vec![]);
            }
            let layer = ctx.read_response(node_frame_id(source)).unwrap().layer_id;
            let scaling = ctx.layer_transform_to_global(layer).unwrap().scaling;
            if zoom {
                assert!(scaling > 1.0);
            }
            let original = app.graph.nodes[0].clone();
            let edges = app.graph.edges.clone();
            let pos = node_position(&ctx, source);
            let shift = egui::Modifiers { shift: true, ..Default::default() };
            // Shift-click selects without copying, then Shift-drag starts a single copy.
            frame_with_modifiers(&ctx, &mut app,
                vec![egui::Event::PointerMoved(pos), button(pos, true)], shift);
            frame_with_modifiers(&ctx, &mut app, vec![button(pos, false)], shift);
            assert_eq!(app.graph.nodes.len(), 2);
            frame_with_modifiers(&ctx, &mut app, vec![button(pos, true)], shift);
            frame_with_modifiers(&ctx, &mut app,
                vec![egui::Event::PointerMoved(pos + egui::vec2(30.0, 40.0))], shift);
            assert_eq!(app.graph.nodes.len(), 3);
            frame_with_modifiers(&ctx, &mut app,
                vec![egui::Event::PointerMoved(pos + egui::vec2(90.0, 100.0))], shift);
            let end = pos + egui::vec2(150.0, 120.0);
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(end)]);
            frame(&ctx, &mut app, vec![button(end, false)]);
            assert_eq!(app.graph.nodes.len(), 3);
            assert_eq!(app.graph.nodes[0], original);
            assert_eq!(app.graph.edges, edges);
            assert_eq!(app.snarl.wires().count(), 1);
            assert_eq!(app.selected, Some(2));
            assert!(!app.node_settings_open);
            let copy = &app.graph.nodes[2];
            assert_eq!(copy.name, "ocr4");
            assert_eq!(copy.title, original.title);
            assert_eq!(copy.kind, original.kind);
            assert_eq!(copy.region, original.region);
            assert_eq!(copy.color, original.color);
            assert_eq!(copy.opacity, original.opacity);
            let expected = Pos2::from(original.position) + egui::vec2(150.0, 120.0) / scaling;
            assert!((Pos2::from(copy.position) - expected).length() < 1.0,
                "copy at {:?}, expected {expected:?} (scale {scaling})", copy.position);
            let restored = Graph::from_json(&app.graph.to_json()).unwrap();
            assert_eq!(restored.nodes[2], *copy);
            assert_eq!(restored.edges, edges);
        }
    }

    #[test]
    fn dropping_a_shift_drag_copy_in_trash_keeps_the_original() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.add_node();
        let original = app.graph.nodes[0].clone();
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![]);
        let source = app.snarl.node_ids().next().unwrap().0;
        let pos = node_position(&ctx, source);
        let trash = ctx.data(|data| data.get_temp::<egui::Rect>(Id::new(TRASH_RECT_ID)).unwrap()).center();
        let shift = egui::Modifiers { shift: true, ..Default::default() };
        frame_with_modifiers(&ctx, &mut app,
            vec![egui::Event::PointerMoved(pos), button(pos, true)], shift);
        frame_with_modifiers(&ctx, &mut app,
            vec![egui::Event::PointerMoved(pos + egui::vec2(20.0, 20.0))], shift);
        assert_eq!(app.graph.nodes.len(), 2);
        frame_with_modifiers(&ctx, &mut app, vec![egui::Event::PointerMoved(trash)], shift);
        frame_with_modifiers(&ctx, &mut app, vec![button(trash, false)], shift);
        assert_eq!(app.graph.nodes, vec![original]);
        assert_eq!(app.snarl.node_ids().count(), 1);
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
        assert_eq!(app.graph.edges.iter().map(|edge| (&edge.source, &edge.target, edge.input))
            .collect::<Vec<_>>(), vec![(&names[1], &names[2], 0)]);
        assert_eq!(app.snarl.wires().count(), 1);
    }
}
