use super::*;

impl App {
    pub(super) fn new(backend: Option<Desktop>, status: String, logs: SessionLog) -> Self {
        Self {
            graph: Graph::default(),
            snarl: Snarl::new(),
            backend,
            kind: 0,
            selected: None,
            node_settings_open: false,
            copy_drag: None,
            add_menu_open: false,
            running: false,
            overlay_ready_at: None,
            last_tick: Instant::now(),
            last_run_duration: None,
            last_run_interval: None,
            last_run_frame: None,
            refresh_input: DEFAULT_REFRESH_MS.to_string(),
            timing_settings_open: false,
            screen_events: None,
            event_regions: Vec::new(),
            event_definition: String::new(),
            event_runtime: EventRuntime::default(),
            region_scales: HashMap::new(),
            activity: HashMap::new(),
            escape_pressed: Arc::new(AtomicBool::new(false)),
            overlay_anchors: HashMap::new(),
            overlay_targets: HashMap::new(),
            file: None,
            saved_graph: Graph::default().to_json(),
            pending_file_action: None,
            save_error: None,
            close_confirmed: false,
            history: GraphHistory::new(Graph::default().to_json()),
            workspace: None,
            recovery: None,
            recovery_error: None,
            last_autosave: Instant::now(),
            search_open: false,
            search_text: String::new(),
            fit_view: false,
            focus_node: None,
            validation_open: false,
            validation: Vec::new(),
            preview: None,
            preview_steps: Vec::new(),
            preview_open: false,
            subflow_editor: None,
            extra_events: None,
            status,
            logs,
            logs_expanded: false,
        }
    }

    pub(super) fn initialize_workspace(&mut self) {
        match Workspace::open() {
            Ok(mut workspace) => {
                if let Err(error) = workspace.load_recent() {
                    self.record_status(log::Level::Error, error);
                }
                match workspace.recovery() {
                    Ok(recovery) => self.recovery = recovery,
                    Err(error) => {
                        self.recovery_error = Some(error.clone());
                        self.record_status(log::Level::Error, error);
                    }
                }
                self.workspace = Some(workspace);
            }
            Err(error) => self.record_status(
                log::Level::Error,
                format!("Recovery/recent files unavailable: {error}"),
            ),
        }
    }

    pub(super) fn remember_file(&mut self, path: &Path) {
        if let Some(workspace) = &mut self.workspace {
            if let Err(error) = workspace.remember(path) {
                self.record_status(
                    log::Level::Warn,
                    format!(
                        "Workflow opened/saved, but recent files could not be updated: {error}"
                    ),
                );
            }
        }
    }

    pub(super) fn clear_recovery(&mut self) {
        if self.recovery.is_some() || self.recovery_error.is_some() {
            return;
        }
        if let Some(workspace) = &self.workspace {
            if let Err(error) = workspace.clear_recovery() {
                self.record_status(log::Level::Warn, error);
            }
        }
    }

    pub(super) fn autosave(&mut self, ctx: &egui::Context) {
        if self.workspace.is_none() || self.recovery.is_some() || self.recovery_error.is_some() {
            return;
        }
        if self.has_unsaved_changes() {
            let interval = Duration::from_secs(2);
            if self.last_autosave.elapsed() >= interval {
                self.last_autosave = Instant::now();
                let recovery = Recovery {
                    graph: self.graph.to_json(),
                    file: self.file.clone(),
                };
                if let Err(error) = self
                    .workspace
                    .as_ref()
                    .expect("checked workspace")
                    .save_recovery(&recovery)
                {
                    self.record_status(
                        log::Level::Error,
                        format!("Could not autosave recovery: {error}"),
                    );
                }
            }
            ctx.request_repaint_after(interval.saturating_sub(self.last_autosave.elapsed()));
        } else if self.last_autosave.elapsed() >= Duration::from_secs(2) {
            self.last_autosave = Instant::now();
            self.clear_recovery();
        }
    }

    fn show_recovery(&mut self, ctx: &egui::Context) {
        if let Some(error) = self.recovery_error.clone() {
            let mut discard = false;
            let mut keep = false;
            egui::Modal::new(Id::new("recovery-error")).show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.heading("Recovery copy could not be read");
                ui.colored_label(ui.visuals().error_fg_color, error);
                ui.label("The existing backup has not been changed.");
                discard = ui.button("Discard damaged recovery copy").clicked();
                keep = ui
                    .button("Keep backup; disable recovery for this session")
                    .clicked();
            });
            if discard {
                self.recovery_error = None;
                self.clear_recovery();
            } else if keep {
                self.workspace = None;
                self.recovery_error = None;
                self.record_status(log::Level::Warn, "Recovery and recent-file persistence disabled for this session; the existing backup was preserved".into());
            }
            return;
        }
        let Some(recovery) = self.recovery.clone() else {
            return;
        };
        let mut recover = false;
        let mut discard = false;
        egui::Modal::new(Id::new("recover-workflow")).show(ctx, |ui| {
            ui.heading("Recover unsaved workflow?");
            ui.label("A separate recovery copy was found. Your manually saved JSON has not been overwritten.");
            ui.horizontal(|ui| {
                recover = ui.button("Recover").clicked();
                discard = ui.button("Discard recovery").clicked();
            });
        });
        if recover {
            match Graph::from_json(&recovery.graph) {
                Ok(graph) => {
                    let baseline = if let Some(path) = &recovery.file {
                        match std::fs::read_to_string(path)
                            .map_err(|error| error.to_string())
                            .and_then(|contents| Graph::from_json(&contents))
                        {
                            Ok(graph) => graph.to_json(),
                            Err(error) => {
                                self.record_status(log::Level::Warn, format!("Original file unavailable; recovered edits still need saving: {error}"));
                                Graph::default().to_json()
                            }
                        }
                    } else {
                        Graph::default().to_json()
                    };
                    let workspace = self.workspace.take();
                    self.replace_graph(graph, recovery.file);
                    self.workspace = workspace;
                    self.saved_graph = baseline;
                    self.recovery = None;
                    self.record_status(
                        log::Level::Info,
                        "Recovered workflow; review and save your changes".into(),
                    );
                }
                Err(error) => self.record_status(
                    log::Level::Error,
                    format!("Could not recover workflow: {error}"),
                ),
            }
        } else if discard {
            self.recovery = None;
            self.clear_recovery();
        }
    }

    pub(super) fn capture_history(&mut self, ctx: &egui::Context) {
        self.sync_graph_positions();
        let group = ctx
            .dragged_id()
            .or_else(|| ctx.memory(|memory| memory.focused()))
            .map(|id| id.value());
        if self.history.record(self.graph.to_json(), group) {
            self.validation = self.graph.validate();
            self.preview = None;
            self.preview_steps.clear();
        }
    }

    pub(super) fn undo_edit(&mut self, ctx: &egui::Context, redo: bool) {
        self.capture_history(ctx);
        let snapshot = if redo {
            self.history.redo()
        } else {
            self.history.undo()
        };
        let Some(snapshot) = snapshot else { return };
        match Graph::from_json(&snapshot) {
            Ok(graph) => {
                let baseline = self.saved_graph.clone();
                let fit_view = self.fit_view;
                let history = std::mem::replace(&mut self.history, GraphHistory::new(snapshot));
                self.replace_graph(graph, self.file.clone());
                self.saved_graph = baseline;
                self.history = history;
                self.fit_view = fit_view;
                self.record_status(log::Level::Info, if redo { "Redo" } else { "Undo" }.into());
                ctx.request_repaint();
            }
            Err(error) => self.record_status(
                log::Level::Error,
                format!("Could not restore edit history: {error}"),
            ),
        }
    }

    pub(super) fn handle_shortcuts(&mut self, ctx: &egui::Context, files: bool) {
        if self.pending_file_action.is_some()
            || self.recovery.is_some()
            || self.recovery_error.is_some()
        {
            return;
        }
        if let Some((_, editor)) = &mut self.subflow_editor {
            editor.handle_shortcuts(ctx, false);
            return;
        }
        let shortcut = |modifiers, key| ctx.input_mut(|input| input.consume_key(modifiers, key));
        if files {
            if shortcut(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::S,
            ) {
                self.save_graph_as();
            } else if shortcut(egui::Modifiers::COMMAND, egui::Key::S) {
                self.save_current_graph();
            }
            if shortcut(egui::Modifiers::COMMAND, egui::Key::N) {
                self.request_file_action(FileAction::New, ctx);
            }
            if shortcut(egui::Modifiers::COMMAND, egui::Key::O) {
                self.request_file_action(FileAction::Load, ctx);
            }
        }
        if shortcut(egui::Modifiers::COMMAND, egui::Key::F) {
            self.search_open = true;
        }
        if !ctx.egui_wants_keyboard_input() {
            if shortcut(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::Z,
            ) || shortcut(egui::Modifiers::COMMAND, egui::Key::Y)
            {
                self.undo_edit(ctx, true);
            } else if shortcut(egui::Modifiers::COMMAND, egui::Key::Z) {
                self.undo_edit(ctx, false);
            }
            if shortcut(egui::Modifiers::COMMAND, egui::Key::Num0) {
                self.fit_view = true;
                self.focus_node = None;
            }
            if shortcut(egui::Modifiers::NONE, egui::Key::Delete) {
                if let Some(index) = self.selected {
                    self.delete_node(index);
                }
            }
            if shortcut(egui::Modifiers::NONE, egui::Key::F6) {
                self.preview_run(false);
            }
            if shortcut(egui::Modifiers::NONE, egui::Key::F7) {
                self.preview_run(true);
            }
            if files && shortcut(egui::Modifiers::NONE, egui::Key::F5) {
                self.toggle_run();
            }
        }
    }

    pub(super) fn toggle_run(&mut self) {
        if !self.running {
            self.validation = self.validation_issues();
            if !self.validation.is_empty() {
                self.validation_open = true;
                self.record_status(
                    log::Level::Warn,
                    "Resolve graph validation problems before running".into(),
                );
                return;
            }
            self.preview = None;
        }
        self.set_running(!self.running);
    }

    pub(super) fn validation_issues(&self) -> Vec<screen_automation::graph::GraphIssue> {
        let mut issues = self.graph.validate();
        if self.graph.settings.run_mode != RunMode::HmiEvent {
            for node in &self.graph.nodes {
                if matches!(node.kind, Kind::Event { .. }) {
                    issues.push(screen_automation::graph::GraphIssue {
                        node: Some(node.name.clone()),
                        message: "Event sources require HMI Event timing".into(),
                    });
                }
            }
        }
        if self.graph.settings.run_mode == RunMode::HmiEvent
            && !self
                .graph
                .nodes
                .iter()
                .any(|node| matches!(node.kind, Kind::Ocr { .. } | Kind::Event { .. }))
        {
            issues.push(screen_automation::graph::GraphIssue {
                node: None,
                message: "HMI Event requires a top-level OCR or Event source".into(),
            });
        }
        fn has_regions(graph: &Graph) -> bool {
            graph.nodes.iter().any(|node| {
                node.region.is_some()
                    || matches!(&node.kind, Kind::Subflow { graph } if has_regions(graph))
            })
        }
        if self.backend.is_none() && self.subflow_editor.is_none() && has_regions(&self.graph) {
            issues.push(screen_automation::graph::GraphIssue {
                node: None,
                message: "Desktop backend is unavailable; real capture/click/typing cannot run"
                    .into(),
            });
        }
        issues
    }

    pub(super) fn preview_run(&mut self, single_step: bool) {
        self.set_running(false);
        self.preview_open = true;
        if !single_step
            || self
                .preview
                .as_ref()
                .is_none_or(PreviewSession::is_finished)
        {
            let mut graph = self.graph.clone();
            match with_physical_regions(&mut graph, &self.region_scales, |graph| {
                PreviewSession::new(graph)
            }) {
                Ok(session) => {
                    self.preview = Some(session);
                    self.preview_steps.clear();
                }
                Err(error) => {
                    self.record_status(
                        log::Level::Error,
                        format!("Could not start preview: {error}"),
                    );
                    return;
                }
            }
        }
        loop {
            let mut unavailable = UnavailableBackend;
            let backend: &mut dyn Backend = match self.backend.as_mut() {
                Some(backend) => backend,
                None => &mut unavailable,
            };
            let result = self
                .preview
                .as_mut()
                .expect("initialized preview")
                .step(backend);
            match result {
                Ok(Some(step)) => {
                    for action in &step.actions {
                        self.logs
                            .record(log::Level::Info, format_args!("[PREVIEW] {action}"));
                    }
                    if let Some(value) = &step.output {
                        self.activity
                            .insert(step.node.clone(), (Instant::now(), value.to_text()));
                    }
                    self.preview_steps.push(step);
                    if single_step {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    self.preview = None;
                    self.record_status(log::Level::Error, format!("Preview stopped: {error}"));
                    break;
                }
            }
        }
    }

    pub(super) fn show_editor_windows(&mut self, ctx: &egui::Context) {
        egui::Window::new("Find node")
            .id(Id::new("node-search"))
            .open(&mut self.search_open)
            .show(ctx, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.search_text)
                        .hint_text("Title, name, type, or group"),
                );
                let query = self.search_text.trim().to_lowercase();
                egui::ScrollArea::vertical()
                    .max_height(280.0)
                    .show(ui, |ui| {
                        for (index, node) in self.graph.nodes.iter().enumerate() {
                            let description = format!(
                                "{}  ({})  {}",
                                node.display_title(),
                                node.type_name(),
                                node.group
                            );
                            if format!("{description} {}", node.name)
                                .to_lowercase()
                                .contains(&query)
                                && ui
                                    .selectable_label(self.selected == Some(index), description)
                                    .clicked()
                            {
                                self.selected = Some(index);
                                self.node_settings_open = true;
                                self.focus_node = Some(node.name.clone());
                            }
                        }
                    });
            });
        egui::Window::new("Graph validation")
            .open(&mut self.validation_open)
            .show(ctx, |ui| {
                if self.validation.is_empty() {
                    ui.label("No graph problems found.");
                }
                for issue in &self.validation {
                    let label = match &issue.node {
                        Some(node) => format!("{node}: {}", issue.message),
                        None => issue.message.clone(),
                    };
                    if ui.selectable_label(false, label).clicked() {
                        self.selected = self
                            .graph
                            .nodes
                            .iter()
                            .position(|node| Some(&node.name) == issue.node.as_ref());
                        self.node_settings_open = self.selected.is_some();
                        self.focus_node = issue.node.clone();
                    }
                }
            });
        let mut step = false;
        let mut restart = false;
        egui::Window::new("Execution preview (no real clicks or typing)")
            .open(&mut self.preview_open).default_width(540.0).show(ctx, |ui| {
                ui.weak("Capture/OCR is real; mouse and keyboard actions are only recorded. External event listeners are not started.");
                ui.horizontal(|ui| {
                    step = ui.button("Next step").clicked();
                    restart = ui.button("Restart dry run").clicked();
                });
                egui::ScrollArea::both().max_height(380.0).stick_to_bottom(true).show(ui, |ui| {
                    for (index, result) in self.preview_steps.iter().enumerate() {
                        ui.strong(format!("{}. {}", index + 1, result.node));
                        ui.monospace(format!("Inputs: {:?}\nOutput: {:?}", result.inputs, result.output));
                        for action in &result.actions { ui.colored_label(Color32::from_rgb(180, 110, 0), action); }
                        ui.separator();
                    }
                });
            });
        if step {
            self.preview_run(true);
        }
        if restart {
            self.preview_run(false);
        }
        self.show_recovery(ctx);
    }

    pub(super) fn handle_subflow_requests(&mut self, ctx: &egui::Context) {
        let edit = ctx.data_mut(|data| data.remove_temp::<usize>(Id::new("edit-subflow-request")));
        let import =
            ctx.data_mut(|data| data.remove_temp::<usize>(Id::new("import-subflow-request")));
        if let Some(index) = import {
            self.set_running(false);
            if let Some(path) = self
                .graph_file_dialog()
                .set_title("Embed workflow JSON (copied, not linked)")
                .pick_file()
            {
                match std::fs::read_to_string(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|contents| Graph::from_json(&contents))
                {
                    Ok(graph) => {
                        if let Some(Node {
                            kind: Kind::Subflow { graph: embedded },
                            ..
                        }) = self.graph.nodes.get_mut(index)
                        {
                            *embedded = Box::new(graph);
                        }
                    }
                    Err(error) => self.record_status(
                        log::Level::Error,
                        format!("Could not import subflow: {error}"),
                    ),
                }
            }
        }
        if let Some(index) = edit {
            self.set_running(false);
            if let Some(Node {
                name,
                kind: Kind::Subflow { graph },
                ..
            }) = self.graph.nodes.get(index)
            {
                let mut editor = App::new(None, String::new(), self.logs.clone());
                editor.replace_graph((**graph).clone(), None);
                editor.backend = self.backend.take();
                editor.region_scales = self
                    .region_scales
                    .iter()
                    .filter_map(|(key, value)| {
                        key.strip_prefix(&format!("{}/", region_key("", name)))
                            .map(|key| (key.to_owned(), *value))
                    })
                    .collect();
                self.subflow_editor = Some((name.clone(), Box::new(editor)));
                self.node_settings_open = false;
                self.preview_open = false;
                self.validation_open = false;
                self.search_open = false;
            }
        }
    }

    pub(super) fn show_subflow_menu(&mut self, ui: &mut egui::Ui) {
        let mut back = false;
        if let Some((name, editor)) = &mut self.subflow_editor {
            if editor.subflow_editor.is_some() {
                editor.show_subflow_menu(ui);
                return;
            }
            egui::MenuBar::new().ui(ui, |ui| {
                back = ui.button("< Back to parent graph").clicked();
                ui.strong(format!(
                    "Embedded: {name} (edits are included in the parent file)"
                ));
                if ui
                    .add_enabled(editor.history.can_undo(), egui::Button::new("Undo"))
                    .clicked()
                {
                    editor.undo_edit(ui.ctx(), false);
                }
                if ui
                    .add_enabled(editor.history.can_redo(), egui::Button::new("Redo"))
                    .clicked()
                {
                    editor.undo_edit(ui.ctx(), true);
                }
                if ui.button("Fit graph").clicked() {
                    editor.fit_view = true;
                }
                if ui.button("Find").clicked() {
                    editor.search_open = true;
                }
                if ui.button("Validate").clicked() {
                    editor.validation = editor.graph.validate();
                    editor.validation_open = true;
                }
                if ui.button("Dry Run").clicked() {
                    editor.preview_run(false);
                }
                if ui.button("Step").clicked() {
                    editor.preview_run(true);
                }
            });
        }
        if back {
            if let Some((_, mut editor)) = self.subflow_editor.take() {
                self.backend = editor.backend.take();
                self.fit_view = true;
            }
        }
    }

    pub(super) fn show_subflow_canvas(&mut self, ui: &mut egui::Ui) {
        if let Some((name, mut editor)) = self.subflow_editor.take() {
            let prefix = format!("{}/", region_key("", &name));
            editor.region_scales = self
                .region_scales
                .iter()
                .filter_map(|(key, value)| {
                    key.strip_prefix(&prefix)
                        .map(|key| (key.to_owned(), *value))
                })
                .collect();
            if let Some(Node {
                kind: Kind::Subflow { graph },
                ..
            }) = self.graph.nodes.iter_mut().find(|node| node.name == name)
            {
                let mut regions = HashMap::new();
                visit_nodes_mut(graph, "", &mut |node, key| {
                    if let Some(region) = node.region {
                        regions.insert(key.to_owned(), region);
                    }
                });
                visit_nodes_mut(&mut editor.graph, "", &mut |node, key| {
                    if let Some(region) = regions.get(key) {
                        node.region = Some(*region);
                    }
                });
            }
            editor.show_canvas(ui);
            editor.show_editor_windows(ui.ctx());
            editor.capture_history(ui.ctx());
            if let Some(Node {
                kind: Kind::Subflow { graph },
                ..
            }) = self.graph.nodes.iter_mut().find(|node| node.name == name)
            {
                *graph = Box::new(editor.graph.clone());
            }
            self.subflow_editor = Some((name, editor));
        }
    }

    pub(super) fn leave_subflows(&mut self) {
        if let Some((_, mut editor)) = self.subflow_editor.take() {
            editor.leave_subflows();
            self.backend = editor.backend.take();
        }
    }
}

pub(super) fn optional_number(ui: &mut egui::Ui, label: &str, value: &mut Option<f64>) {
    ui.horizontal(|ui| {
        let mut enabled = value.is_some();
        if ui.checkbox(&mut enabled, label).changed() {
            *value = enabled.then_some(0.0);
        }
        if let Some(value) = value {
            ui.add(
                egui::DragValue::new(value)
                    .speed(0.1)
                    .custom_parser(|text| {
                        text.parse::<f64>().ok().filter(|value| value.is_finite())
                    }),
            );
        }
    });
}

pub(super) fn event_settings(ui: &mut egui::Ui, event: &mut EventSource) {
    let mut selected = match event {
        EventSource::Hotkey { .. } => 0,
        EventSource::AudioLevel { .. } => 1,
        EventSource::Webhook { .. } => 2,
        EventSource::FileChange { .. } => 3,
        EventSource::Timer { .. } => 4,
    };
    let before = selected;
    egui::ComboBox::from_id_salt("event-source")
        .selected_text(event.label())
        .show_ui(ui, |ui| {
            for (index, name) in [
                "Global hotkey",
                "Microphone level",
                "HTTP webhook",
                "File change",
                "Timer",
            ]
            .iter()
            .enumerate()
            {
                ui.selectable_value(&mut selected, index, *name);
            }
        });
    if selected != before {
        *event = match selected {
            0 => EventSource::Hotkey { key: "F8".into() },
            1 => EventSource::AudioLevel { threshold: 0.1 },
            2 => EventSource::Webhook {
                port: 8765,
                path: "/trigger".into(),
            },
            3 => EventSource::FileChange {
                path: String::new(),
            },
            _ => EventSource::Timer { interval_ms: 1000 },
        };
    }
    match event {
        EventSource::Hotkey { key } => {
            ui.label("Key combination (e.g. Ctrl+Alt+K, F8)");
            ui.text_edit_singleline(key);
        }
        EventSource::AudioLevel { threshold } => {
            ui.add(egui::Slider::new(threshold, 0.001..=1.0).text("RMS threshold"));
            ui.weak("Uses your default microphone only while running. Emits RMS on rising across the threshold.");
        }
        EventSource::Webhook { port, path } => {
            ui.add(
                egui::DragValue::new(port)
                    .range(1024..=65535)
                    .prefix("Port "),
            );
            ui.text_edit_singleline(path);
            ui.monospace(format!("POST http://127.0.0.1:{port}{path}"));
            ui.weak("UTF-8 request body becomes the output. Localhost only; maximum 64 KiB. Use a distinct port per webhook.");
        }
        EventSource::FileChange { path } => {
            ui.label("Existing UTF-8 text file");
            ui.text_edit_singleline(path);
        }
        EventSource::Timer { interval_ms } => {
            ui.add(
                egui::DragValue::new(interval_ms)
                    .range(100..=86_400_000)
                    .suffix(" ms"),
            );
        }
    }
    ui.weak("Select HMI Event in Run timing to activate event sources. Events run only their connected branches.");
    if let Err(error) = event.validate() {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
}

pub(super) struct UnavailableBackend;

impl Backend for UnavailableBackend {
    fn capture(&mut self, _: &Region) -> Result<Vec<u8>, String> {
        Err("Desktop capture is unavailable".into())
    }
    fn ocr(&mut self, _: &Region, _: &[u8]) -> Result<String, String> {
        Err("Desktop OCR is unavailable".into())
    }
    fn type_text(&mut self, _: &str) -> Result<(), String> {
        Err("Desktop typing is unavailable".into())
    }
    fn click(&mut self, _: i32, _: i32, _: &str) -> Result<(), String> {
        Err("Desktop clicking is unavailable".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(None, String::new(), SessionLog::default())
    }

    fn render(ctx: &egui::Context, app: &mut App, events: Vec<egui::Event>) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(1000.0, 750.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| app.show_canvas(ui));
                app.show_editor_windows(ui.ctx());
                app.capture_history(ui.ctx());
            },
        );
        output.textures_delta.clear();
        output
    }

    #[test]
    fn undo_redo_restores_deleted_nodes_connections_and_saved_baseline() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.kind = 9;
        app.add_node();
        app.kind = 7;
        app.add_node();
        let source = app.graph.nodes[0].name.clone();
        let target = app.graph.nodes[1].name.clone();
        app.graph.connect(&source, &target).unwrap();
        app.graph.settings.refresh_ms = 1500;
        app.capture_history(&ctx);
        app.saved_graph = app.graph.to_json();
        let saved = app.saved_graph.clone();
        app.delete_node(0);
        app.capture_history(&ctx);
        app.undo_edit(&ctx, false);
        assert_eq!(app.graph.to_json(), saved);
        assert_eq!(app.snarl.wires().count(), 1);
        assert!(!app.has_unsaved_changes());
        app.undo_edit(&ctx, true);
        assert_eq!(app.graph.nodes.len(), 1);
        assert!(app.graph.edges.is_empty());
        assert!(app.has_unsaved_changes());
    }

    #[test]
    fn preview_steps_actions_without_a_real_input_backend() {
        let mut app = app();
        app.kind = 9;
        app.add_node();
        app.kind = 7;
        app.add_node();
        let source = app.graph.nodes[0].name.clone();
        let target = app.graph.nodes[1].name.clone();
        app.graph.connect(&source, &target).unwrap();
        app.region_scales.insert(target, 1.0);
        let original = app.graph.to_json();
        app.preview_run(true);
        assert_eq!(app.preview_steps.len(), 1);
        assert!(app.preview_steps[0].actions.is_empty());
        app.preview_run(true);
        assert_eq!(app.preview_steps.len(), 2);
        assert!(!app.preview_steps[1].actions.is_empty());
        assert_eq!(app.graph.to_json(), original);
        assert!(!app.running);
        app.preview_run(false);
        assert_eq!(app.preview_steps.len(), 2);
        assert!(!app.status.contains("stopped"));
    }

    #[test]
    fn timer_event_runs_only_connected_branch_without_desktop_backend() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.kind = 11;
        app.add_node();
        let Kind::Event { event } = &mut app.graph.nodes[0].kind else {
            unreachable!()
        };
        *event = EventSource::Timer { interval_ms: 100 };
        app.kind = 1;
        app.add_node();
        app.kind = 9;
        app.add_node();
        let source = app.graph.nodes[0].name.clone();
        let target = app.graph.nodes[1].name.clone();
        let unrelated = app.graph.nodes[2].name.clone();
        app.graph.connect(&source, &target).unwrap();
        app.graph.settings.run_mode = RunMode::HmiEvent;
        assert!(app.validation_issues().is_empty());
        app.set_running(true);
        app.overlay_ready_at = None;
        let deadline = Instant::now() + Duration::from_secs(3);
        while !app.activity.contains_key(&target) && Instant::now() < deadline {
            app.run_hmi_events(&ctx).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(app.activity.get(&target).unwrap().1.parse::<u64>().unwrap() >= 1);
        assert!(!app.activity.contains_key(&unrelated));
        app.set_running(false);
        assert!(app.extra_events.is_none());
        assert!(app.backend.is_none());
    }

    #[test]
    fn disconnecting_pin_wires_updates_graph_and_is_undoable() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.kind = 9;
        app.add_node();
        app.kind = 7;
        app.add_node();
        let source = app.graph.nodes[0].name.clone();
        let target = app.graph.nodes[1].name.clone();
        app.graph.connect(&source, &target).unwrap();
        app.rebuild_snarl();
        app.capture_history(&ctx);
        for drop_inputs in [true, false] {
            let source = app
                .snarl
                .node_ids()
                .find(|(_, index)| **index == 0)
                .unwrap()
                .0;
            let target = app
                .snarl
                .node_ids()
                .find(|(_, index)| **index == 1)
                .unwrap()
                .0;
            let input = app.snarl.in_pin(egui_snarl::InPinId {
                node: target,
                input: 0,
            });
            let output = app.snarl.out_pin(egui_snarl::OutPinId {
                node: source,
                output: 0,
            });
            let mut viewer = GraphViewer {
                graph: &mut app.graph,
                status: &mut app.status,
                activity: &app.activity,
                selected: None,
                now: Instant::now(),
                fit: None,
                validation: &[],
            };
            if drop_inputs {
                viewer.drop_inputs(&input, &mut app.snarl);
            } else {
                viewer.drop_outputs(&output, &mut app.snarl);
            }
            assert!(app.graph.edges.is_empty());
            assert_eq!(app.snarl.wires().count(), 0);
            app.capture_history(&ctx);
            app.undo_edit(&ctx, false);
            assert_eq!(app.graph.edges.len(), 1);
            assert_eq!(app.snarl.wires().count(), 1);
        }
    }

    #[test]
    fn embedded_canvas_edits_are_saved_in_parent_and_undoable() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.kind = 13;
        app.add_node();
        app.capture_history(&ctx);
        let original = app.graph.to_json();
        ctx.data_mut(|data| data.insert_temp(Id::new("edit-subflow-request"), 0usize));
        app.handle_subflow_requests(&ctx);
        let editor = &mut app.subflow_editor.as_mut().unwrap().1;
        editor.graph.nodes[0].title = "Nested input".into();
        editor.graph.nodes[0].group = "Reusable".into();
        render(&ctx, &mut app, vec![]);
        render(&ctx, &mut app, vec![]);
        let restored = Graph::from_json(&app.graph.to_json()).unwrap();
        let Kind::Subflow { graph } = &restored.nodes[0].kind else {
            panic!("missing subflow")
        };
        assert_eq!(graph.nodes[0].title, "Nested input");
        assert_eq!(graph.nodes[0].group, "Reusable");
        app.leave_subflows();
        app.undo_edit(&ctx, false);
        assert_eq!(app.graph.to_json(), original);
    }

    #[test]
    fn fit_graph_contains_widely_spaced_nodes_and_notes() {
        let ctx = egui::Context::default();
        let mut app = app();
        for kind in [9, 9, 12] {
            app.kind = kind;
            app.add_node();
        }
        app.graph.nodes[0].position = [-4_000_000.0, -2_500_000.0];
        app.graph.nodes[0].title =
            "A long node title that must stay inside the fitted canvas".into();
        app.graph.nodes[1].position = [8_500_000.0, 5_000_000.0];
        app.graph.nodes[2].position = [4_000_000.0, -1_000_000.0];
        app.graph.nodes[2].group = "Notes".into();
        app.rebuild_snarl();
        app.fit_view = true;
        for _ in 0..4 {
            render(&ctx, &mut app, vec![]);
        }
        let canvas = egui::Rect::from_min_max(Pos2::new(8.0, 8.0), Pos2::new(992.0, 742.0));
        for (id, _) in app.snarl.node_ids() {
            let response = ctx.read_response(node_frame_id(id)).unwrap();
            let rect = ctx.layer_transform_to_global(response.layer_id).unwrap() * response.rect;
            assert!(canvas.contains_rect(rect), "node outside canvas: {rect:?}");
        }
    }

    #[test]
    fn search_filters_and_focuses_a_node_without_changing_positions() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.kind = 12;
        app.add_node();
        app.graph.nodes[0].group = "Documentation".into();
        app.kind = 9;
        app.add_node();
        app.graph.nodes[1].title = "BTC price".into();
        app.graph.nodes[1].position = [7000.0, 4000.0];
        app.rebuild_snarl();
        let original = app.graph.to_json();
        app.search_open = true;
        app.search_text = "BTC".into();
        render(&ctx, &mut app, vec![]);
        let output = render(&ctx, &mut app, vec![]);
        let result = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text)
                    if text.galley.text().starts_with("BTC price  (constant)") =>
                {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap();
        let button = |pressed| egui::Event::PointerButton {
            pos: result,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(result), button(true)],
        );
        render(&ctx, &mut app, vec![button(false)]);
        assert_eq!(app.selected, Some(1));
        assert!(app.node_settings_open);
        for _ in 0..3 {
            render(&ctx, &mut app, vec![]);
        }
        let id = app
            .snarl
            .node_ids()
            .find(|(_, index)| **index == 1)
            .unwrap()
            .0;
        let response = ctx.read_response(node_frame_id(id)).unwrap();
        let rect = ctx.layer_transform_to_global(response.layer_id).unwrap() * response.rect;
        assert!(
            rect.center().distance(Pos2::new(500.0, 375.0)) < 50.0,
            "{rect:?}"
        );
        assert_eq!(app.graph.to_json(), original);
        let restored = Graph::from_json(&original).unwrap();
        assert_eq!(restored.nodes[0].group, "Documentation");
        assert!(
            matches!(&restored.nodes[0].kind, Kind::Note { text } if text == "Add your notes here")
        );
    }

    #[test]
    fn delete_and_undo_keyboard_shortcuts_edit_the_graph() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.kind = 9;
        app.add_node();
        app.selected = Some(0);
        app.capture_history(&ctx);
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let command = egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        for (events, count) in [
            (vec![key(egui::Key::Delete, egui::Modifiers::NONE)], 0),
            (
                vec![
                    egui::Event::ModifiersChanged(command),
                    key(egui::Key::Z, command),
                ],
                1,
            ),
            (
                vec![
                    egui::Event::ModifiersChanged(command),
                    key(egui::Key::Y, command),
                ],
                0,
            ),
        ] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    app.handle_shortcuts(ui.ctx(), false);
                    app.capture_history(ui.ctx());
                },
            );
            output.textures_delta.clear();
            assert_eq!(app.graph.nodes.len(), count);
        }
    }

    #[test]
    fn nested_regions_use_scoped_scales_and_restore_after_execution() {
        let Kind::Subflow { mut graph } = new_kind(13) else {
            unreachable!()
        };
        graph.disconnect_input("input", "output", 0);
        graph
            .add_node(Node::new(
                "action",
                Kind::Click {
                    button: "left".into(),
                },
            ))
            .unwrap();
        graph.connect("input", "action").unwrap();
        graph.connect("action", "output").unwrap();
        let mut root = Graph::default();
        root.add_node(Node::new(
            "source",
            Kind::Constant {
                value: "1".into(),
                format: Format::Int,
            },
        ))
        .unwrap();
        for name in ["one", "two"] {
            root.add_node(Node::new(
                name,
                Kind::Subflow {
                    graph: graph.clone(),
                },
            ))
            .unwrap();
            root.connect("source", name).unwrap();
        }
        #[derive(Default)]
        struct Recording {
            clicks: Vec<(i32, i32)>,
        }
        impl Backend for Recording {
            fn capture(&mut self, _: &Region) -> Result<Vec<u8>, String> {
                Err("unexpected capture".into())
            }
            fn ocr(&mut self, _: &Region, _: &[u8]) -> Result<String, String> {
                Err("unexpected OCR".into())
            }
            fn type_text(&mut self, _: &str) -> Result<(), String> {
                Err("unexpected typing".into())
            }
            fn click(&mut self, x: i32, y: i32, _: &str) -> Result<(), String> {
                self.clicks.push((x, y));
                Ok(())
            }
        }
        let scales = HashMap::from([("one/action".into(), 2.0), ("two/action".into(), 1.0)]);
        let original = root.to_json();
        let mut backend = Recording::default();
        run_with_physical_regions(&mut root, &mut backend, &scales).unwrap();
        assert_eq!(backend.clicks, vec![(500, 320), (250, 160)]);
        assert_eq!(root.to_json(), original);
    }

    #[test]
    fn recovery_restores_unsaved_edits_without_overwriting_manual_file() {
        let directory =
            std::env::temp_dir().join(format!("sa-editor-recovery-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("manual.json");
        let mut app = app();
        app.workspace = Some(Workspace::at(directory.clone()).unwrap());
        app.kind = 9;
        app.add_node();
        assert!(app.save_graph(&path));
        let manual = std::fs::read_to_string(&path).unwrap();
        app.graph.nodes[0].title = "Recovered edit".into();
        app.last_autosave = Instant::now() - Duration::from_secs(3);
        let ctx = egui::Context::default();
        app.autosave(&ctx);
        let recovery = app.workspace.as_ref().unwrap().recovery().unwrap().unwrap();
        let workspace = app.workspace.take();
        let mut restored = App::new(None, String::new(), SessionLog::default());
        restored.workspace = workspace;
        restored.recovery = Some(recovery);
        render(&ctx, &mut restored, vec![]);
        let output = render(&ctx, &mut restored, vec![]);
        let recover = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Recover" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap();
        let button = |pressed| egui::Event::PointerButton {
            pos: recover,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(
            &ctx,
            &mut restored,
            vec![egui::Event::PointerMoved(recover), button(true)],
        );
        render(&ctx, &mut restored, vec![button(false)]);
        assert!(restored.recovery.is_none());
        assert_eq!(restored.graph.nodes[0].title, "Recovered edit");
        assert!(restored.has_unsaved_changes());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), manual);
        assert!(restored
            .workspace
            .as_ref()
            .unwrap()
            .recovery()
            .unwrap()
            .is_some());
        assert!(restored.save_graph(&path));
        assert!(restored
            .workspace
            .as_ref()
            .unwrap()
            .recovery()
            .unwrap()
            .is_none());
        drop(restored);
        for name in ["manual.json", "recent.json", "workspace.lock"] {
            std::fs::remove_file(directory.join(name)).unwrap();
        }
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn unreadable_recovery_is_preserved_until_user_decides() {
        let directory =
            std::env::temp_dir().join(format!("sa-invalid-recovery-{}", std::process::id()));
        let mut app = app();
        app.workspace = Some(Workspace::at(directory.clone()).unwrap());
        let backup = directory.join("recovery.json");
        std::fs::write(&backup, "damaged backup needing manual inspection").unwrap();
        app.recovery_error = Some(app.workspace.as_ref().unwrap().recovery().err().unwrap());
        app.kind = 9;
        app.add_node();
        app.last_autosave = Instant::now() - Duration::from_secs(3);
        app.autosave(&egui::Context::default());
        app.clear_recovery();
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "damaged backup needing manual inspection"
        );
        drop(app);
        std::fs::remove_file(backup).unwrap();
        std::fs::remove_file(directory.join("workspace.lock")).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
