use super::*;

impl App {
    pub(super) fn new(backend: Option<Desktop>, status: String, logs: SessionLog) -> Self {
        Self {
            graph: Graph::default(),
            snarl: Snarl::new(),
            backend,
            kind: 0,
            selected: None,
            selected_wire: None,
            node_settings_open: false,
            copy_drag: None,
            add_menu_open: false,
            group_dialog: None,
            regex_dialog: None,
            running: false,
            overlays_enabled: true,
            overlays_hidden: false,
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
            overlay_clicks: Arc::new(Mutex::new(Vec::new())),
            overlay_paint: HashMap::new(),
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
            pending_preview: None,
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
            || self.group_dialog.is_some()
            || self.regex_dialog.is_some()
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
                if self.selected_wire.is_some() {
                    self.delete_selected_wire();
                } else if let Some(index) = self.selected {
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
        self.pending_preview = Some(single_step);
    }

    pub(super) fn has_pending_preview(&self) -> bool {
        self.pending_preview.is_some()
            || self.subflow_editor.as_ref().is_some_and(|(_, editor)| editor.has_pending_preview())
    }

    pub(super) fn cancel_pending_previews(&mut self) {
        self.pending_preview = None;
        if let Some((_, editor)) = &mut self.subflow_editor {
            editor.cancel_pending_previews();
        }
    }

    pub(super) fn poll_preview(&mut self, ctx: &egui::Context) {
        if !self.has_pending_preview() {
            return;
        }
        let now = Instant::now();
        if !self.overlays_hidden {
            ctx.request_repaint();
        } else if let Some(ready) = self.overlay_ready_at.filter(|ready| now < *ready) {
            ctx.request_repaint_after(ready.duration_since(now));
        } else {
            self.execute_pending_previews();
            ctx.request_repaint();
        }
    }

    fn execute_pending_previews(&mut self) {
        if let Some(single_step) = self.pending_preview.take() {
            self.execute_preview(single_step);
        }
        if let Some((_, editor)) = &mut self.subflow_editor {
            editor.execute_pending_previews();
        }
    }

    fn execute_preview(&mut self, single_step: bool) {
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
                    if self.preview.as_ref().is_some_and(PreviewSession::is_waiting) {
                        self.logs.record(log::Level::Info,
                            format_args!("[PREVIEW] Waiting for screen text. Simulated actions do not change the screen; Step rechecks this wait."));
                        break;
                    }
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
        let mut search_selection = None;
        egui::Window::new("Find node")
            .id(Id::new("node-search"))
            .default_size([440.0, 480.0])
            .min_size(egui::vec2(300.0, 200.0))
            .open(&mut self.search_open)
            .show(ctx, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.search_text)
                        .hint_text("Title, name, type, or group"),
                );
                let query = self.search_text.trim().to_lowercase();
                let groups = find_node_groups(&self.graph, &query);
                let count: usize = groups.iter().map(|(_, nodes)| nodes.len()).sum();
                ui.weak(format!("{count} of {} nodes", self.graph.nodes.len()));
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(ui.available_height().max(120.0))
                    .show(ui, |ui| {
                        if count == 0 { ui.label("No matching nodes."); }
                        for (group, nodes) in groups {
                            let title = if group.is_empty() { "Ungrouped" } else { &group };
                            egui::CollapsingHeader::new(format!("{title} ({})", nodes.len()))
                                .id_salt(("find-node-group", &group))
                                .default_open(true)
                                .open((!query.is_empty()).then_some(true))
                                .show(ui, |ui| {
                                    for index in nodes {
                                        let node = &self.graph.nodes[index];
                                        let style = node_style(node.type_name());
                                        let label = format!("{} {}  ({})",
                                            style.icon, node.display_title(), style.label);
                                        let response = ui.selectable_label(self.selected == Some(index), label)
                                            .on_hover_text(format!("{}\nName: {}\nType: {}",
                                                node.display_title(), node.name, node.type_name()));
                                        if response.clicked() {
                                            search_selection = Some(index);
                                        }
                                    }
                                });
                        }
                    });
            });
        if let Some(index) = search_selection {
            self.selected = Some(index);
            self.selected_wire = None;
            self.node_settings_open = true;
            self.focus_node = Some(self.graph.nodes[index].name.clone());
        }
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
        if !self.preview_open {
            self.pending_preview = None;
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
            self.open_subflow(index);
        }
    }

    pub(super) fn open_subflow(&mut self, index: usize) {
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

impl App {
    pub(super) fn open_regex_generator(&mut self, index: usize) {
        let node = &self.graph.nodes[index];
        self.regex_dialog = Some(RegexDialog {
            node: node.name.clone(), sample: String::new(),
            normalize_whitespace: matches!(node.kind, Kind::AssertText { .. } | Kind::WaitForText { .. }),
            whole_text: true, flexible_whitespace: true, generalize_digits: false,
            focus_sample: true, error: None,
        });
    }

    fn apply_generated_regex(&mut self, name: &str, pattern: &str) -> Result<(), String> {
        regex::Regex::new(pattern).map_err(|error| format!("Invalid generated regex: {error}"))?;
        let node = self.graph.nodes.iter_mut().find(|node| node.name == name)
            .ok_or_else(|| format!("Cannot apply regex: node {name:?} no longer exists."))?;
        match &mut node.kind {
            Kind::AssertText { expected, matching, .. } => {
                *expected = pattern.into();
                *matching = TextMatch::Regex;
            }
            Kind::WaitForText { expected, matching, pending, .. } => {
                *expected = pattern.into();
                *matching = TextMatch::Regex;
                *pending = None;
            }
            Kind::ValidateNumber { pattern: current, last, .. } => {
                *current = pattern.into();
                *last = None;
            }
            _ => return Err("This node does not support a regular expression.".into()),
        }
        self.set_running(false);
        self.record_status(log::Level::Info, format!("Applied generated regex to {name:?}; automation remains stopped"));
        Ok(())
    }

    pub(super) fn show_regex_generator(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.regex_dialog else { return };
        let mut apply = None;
        let mut cancel = false;
        let response = egui::Modal::new(Id::new("regex-generator")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading("Generate regular expression");
            ui.label("Type or paste sample text");
            let sample = ui.add(egui::TextEdit::multiline(&mut dialog.sample)
                .id(Id::new("regex-sample")).desired_rows(3).desired_width(f32::INFINITY));
            if dialog.focus_sample {
                sample.request_focus();
                dialog.focus_sample = false;
            }
            ui.checkbox(&mut dialog.whole_text, "Match the whole text");
            ui.checkbox(&mut dialog.flexible_whitespace, "Allow flexible whitespace");
            ui.checkbox(&mut dialog.generalize_digits, "Allow different digits (same digit counts)");
            if dialog.generalize_digits {
                ui.colored_label(Color32::from_rgb(180, 110, 0),
                    "This broadens matching. Review quantities, prices, and identifiers carefully.");
            }
            let sample = if dialog.normalize_whitespace {
                ui.weak("Text conditions normalize whitespace before matching.");
                dialog.sample.split_whitespace().collect::<Vec<_>>().join(" ")
            } else {
                dialog.sample.clone()
            };
            ui.separator();
            let generated = regex_from_sample(&sample, dialog.whole_text,
                dialog.flexible_whitespace, dialog.generalize_digits);
            match &generated {
                Ok(pattern) => {
                    ui.label("Generated regex (review before applying)");
                    ui.add(egui::Label::new(egui::RichText::new(pattern).monospace()).wrap());
                }
                Err(error) => { ui.label(error); }
            }
            if let Some(error) = &dialog.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(generated.is_ok(), egui::Button::new("Apply regex")).clicked() {
                    apply = generated.ok().map(|pattern| (dialog.node.clone(), pattern));
                }
                cancel = ui.button("Cancel").clicked();
            });
        });
        if cancel || response.should_close() {
            self.regex_dialog = None;
        } else if let Some((name, pattern)) = apply {
            match self.apply_generated_regex(&name, &pattern) {
                Ok(()) => self.regex_dialog = None,
                Err(error) => self.regex_dialog.as_mut().expect("regex dialog is open").error = Some(error),
            }
        }
    }
}

fn find_node_groups(graph: &Graph, query: &str) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<_> = graph.group_names().into_iter().map(str::to_owned).collect();
    if graph.nodes.iter().any(|node| node.group.trim().is_empty()) {
        groups.insert(0, String::new());
    }
    groups.into_iter().filter_map(|group| {
        let mut nodes: Vec<_> = graph.nodes.iter().enumerate()
            .filter(|(_, node)| (node.group == group || (group.is_empty() && node.group.trim().is_empty()))
                && format!("{} {} {} {} {}", node.display_title(), node.name,
                    node.type_name(), node_style(node.type_name()).label, group)
                    .to_lowercase().contains(query))
            .map(|(index, _)| index).collect();
        nodes.sort_by_key(|index| (graph.nodes[*index].display_title().to_lowercase(), &graph.nodes[*index].name));
        (query.is_empty() || !nodes.is_empty()).then_some((group, nodes))
    }).collect()
}

fn regex_from_sample(sample: &str, whole_text: bool, flexible_whitespace: bool, generalize_digits: bool) -> Result<String, String> {
    if sample.trim().is_empty() {
        return Err("Enter sample text to generate a regex.".into());
    }
    let mut pattern = String::new();
    let mut chars = sample.chars().peekable();
    while let Some(character) = chars.next() {
        if flexible_whitespace && character.is_whitespace() {
            while chars.peek().is_some_and(|next| next.is_whitespace()) { chars.next(); }
            pattern.push_str(r"\s+");
        } else if generalize_digits && character.is_ascii_digit() {
            let mut count = 1;
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
                count += 1;
            }
            pattern.push_str("[0-9]");
            if count > 1 { pattern.push_str(&format!("{{{count}}}")); }
        } else {
            pattern.push_str(&regex::escape(&character.to_string()));
        }
    }
    Ok(if whole_text { format!("^{pattern}$") } else { pattern })
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
    fn find_tree_includes_every_node_once_and_filters_names_types_and_groups() {
        let mut graph = Graph::default();
        graph.add_group("Empty").unwrap();
        for (index, _) in KINDS.iter().enumerate() {
            let mut node = Node::new(&format!("node-{index}"), new_kind(index));
            node.title = format!("Title {index:02}");
            node.group = ["Ticket", "Review", ""][index % 3].into();
            graph.add_node(node).unwrap();
        }
        let original = graph.to_json();
        let groups = find_node_groups(&graph, "");
        assert!(groups.iter().any(|(name, nodes)| name == "Empty" && nodes.is_empty()));
        assert!(groups.iter().any(|(name, nodes)| name.is_empty() && !nodes.is_empty()));
        let mut indices: Vec<_> = groups.into_iter().flat_map(|(_, nodes)| nodes).collect();
        indices.sort_unstable();
        assert_eq!(indices, (0..KINDS.len()).collect::<Vec<_>>());
        assert_eq!(find_node_groups(&graph, "assert text")[0].1, [17]);
        assert_eq!(find_node_groups(&graph, "node-20")[0].1, [20]);
        let tickets = find_node_groups(&graph, "ticket");
        assert_eq!(tickets.len(), 1);
        assert_eq!(tickets[0].1.len(), KINDS.len().div_ceil(3));
        assert!(tickets[0].1.iter().all(|index| graph.nodes[*index].group == "Ticket"));
        assert!(find_node_groups(&graph, "not present").is_empty());
        assert_eq!(graph.to_json(), original);
    }

    #[test]
    fn generated_regex_escapes_literals_and_generalizes_only_explicit_options() {
        let sample = "[Buy] NVIDIA (NVDA) $165.05 + fee?";
        let literal = regex::Regex::new(&regex_from_sample(sample, true, false, false).unwrap()).unwrap();
        assert!(literal.is_match(sample));
        assert!(!literal.is_match("[Buy] NVIDIA (NVDA) $999.05 + fee?"));
        assert!(!literal.is_match(&format!("prefix {sample}")));
        let flexible = regex::Regex::new(&regex_from_sample("PAPER TRADING", true, true, false).unwrap()).unwrap();
        assert!(flexible.is_match("PAPER \t TRADING"));
        assert!(!flexible.is_match("PAPERTRADING"));
        let numbers = regex::Regex::new(&regex_from_sample(sample, true, true, true).unwrap()).unwrap();
        assert!(numbers.is_match("[Buy] NVIDIA (NVDA) $999.12 + fee?"));
        assert!(!numbers.is_match("[Buy] NVIDIA (NVDA) $9999.12 + fee?"));
        assert!(!numbers.is_match("[Sell] NVIDIA (NVDA) $999.12 + fee?"));
        let partial = regex::Regex::new(&regex_from_sample("NVDA", false, false, false).unwrap()).unwrap();
        assert!(partial.is_match("Buy NVDA shares"));
        assert!(regex_from_sample(" \n ", true, true, false).is_err());
    }

    #[test]
    fn regex_generator_is_regex_only_and_applies_pasted_text_without_screen_access() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.kind = 17;
        app.add_node();
        app.selected = Some(0);
        app.node_settings_open = true;
        for mode in TextMatch::ALL {
            if let Kind::AssertText { matching, .. } = &mut app.graph.nodes[0].kind {
                *matching = mode;
            }
            render(&ctx, &mut app, vec![]);
            let output = render(&ctx, &mut app, vec![]);
            let generate = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Generate..." =>
                    Some(text.pos + text.galley.size() * 0.5),
                _ => None,
            });
            assert_eq!(generate.is_some(), mode == TextMatch::Regex);
            if let Some(pos) = generate {
                let label_y = output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == "Expected screen text" => Some(text.pos.y),
                    _ => None,
                }).unwrap();
                assert!(pos.y - label_y < 50.0, "regex input must occupy a single compact row");
                let event = |pressed| egui::Event::PointerButton {
                    pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
                };
                render(&ctx, &mut app, vec![egui::Event::PointerMoved(pos), event(true)]);
                render(&ctx, &mut app, vec![event(false)]);
            }
        }
        assert!(app.regex_dialog.is_some());
        assert!(app.node_settings_open);
        let before = app.graph.to_json();
        render(&ctx, &mut app, vec![]);
        render(&ctx, &mut app, vec![egui::Event::Text("discard this sample".into())]);
        render(&ctx, &mut app, vec![egui::Event::Key {
            key: egui::Key::Escape, physical_key: None, pressed: true,
            repeat: false, modifiers: egui::Modifiers::NONE,
        }]);
        assert!(app.regex_dialog.is_none());
        assert_eq!(app.graph.to_json(), before, "cancelling must preserve the pattern");
        app.open_regex_generator(0);
        render(&ctx, &mut app, vec![egui::Event::Key {
            key: egui::Key::Escape, physical_key: None, pressed: false,
            repeat: false, modifiers: egui::Modifiers::NONE,
        }]);
        let output = render(&ctx, &mut app, vec![egui::Event::Text(" PAPER \nTRADING (demo)+ ".into())]);
        assert_eq!(app.graph.to_json(), before, "preview must not apply automatically");
        let pos = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Apply regex" =>
                Some(text.pos + text.galley.size() * 0.5),
            _ => None,
        }).unwrap();
        let event = |pressed| egui::Event::PointerButton {
            pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
        };
        render(&ctx, &mut app, vec![egui::Event::PointerMoved(pos), event(true)]);
        render(&ctx, &mut app, vec![event(false)]);
        assert!(app.regex_dialog.is_none());
        assert!(!app.running);
        let Kind::AssertText { expected, matching, .. } = &app.graph.nodes[0].kind else { panic!("wrong node") };
        assert_eq!(*matching, TextMatch::Regex);
        assert!(regex::Regex::new(expected).unwrap().is_match("PAPER TRADING (demo)+"));
        app.undo_edit(&ctx, false);
        assert_eq!(app.graph.to_json(), before);
    }

    #[test]
    fn applying_regex_stops_execution_resets_numeric_state_and_reports_invalid_targets() {
        let mut app = app();
        for kind in [10, 16] {
            app.kind = kind;
            app.add_node();
            let index = app.graph.nodes.len() - 1;
            let name = app.graph.nodes[index].name.clone();
            if let Kind::ValidateNumber { last, .. } = &mut app.graph.nodes[index].kind {
                *last = Some(42.0);
            }
            app.running = true;
            app.apply_generated_regex(&name, r"^[0-9]{2}$").unwrap();
            assert!(!app.running);
            match &app.graph.nodes[index].kind {
                Kind::ValidateNumber { pattern, last, .. } => {
                    assert_eq!(pattern, r"^[0-9]{2}$");
                    assert!(last.is_none());
                }
                Kind::WaitForText { expected, matching, .. } => {
                    assert_eq!(expected, r"^[0-9]{2}$");
                    assert_eq!(*matching, TextMatch::Regex);
                }
                _ => panic!("wrong node"),
            }
            let before = app.graph.to_json();
            assert!(app.apply_generated_regex(&name, "[").is_err());
            assert!(app.apply_generated_regex("deleted-node", "^test$").is_err());
            assert_eq!(app.graph.to_json(), before);
        }
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
        let ctx = egui::Context::default();
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
        app.poll_preview(&ctx);
        assert!(app.preview_steps.is_empty(), "must hide overlays before executing");
        app.overlays_hidden = true;
        app.overlay_ready_at = Some(Instant::now() + OVERLAY_SETTLE_DELAY);
        app.poll_preview(&ctx);
        assert!(app.preview_steps.is_empty(), "must let hidden overlays settle");
        app.overlay_ready_at = Some(Instant::now() - OVERLAY_SETTLE_DELAY);
        app.poll_preview(&ctx);
        assert_eq!(app.preview_steps.len(), 1);
        assert!(app.preview_steps[0].actions.is_empty());
        app.preview_run(true);
        app.poll_preview(&ctx);
        assert_eq!(app.preview_steps.len(), 2);
        assert!(!app.preview_steps[1].actions.is_empty());
        assert_eq!(app.graph.to_json(), original);
        assert!(!app.running);
        app.preview_run(false);
        app.poll_preview(&ctx);
        assert_eq!(app.preview_steps.len(), 2);
        assert!(!app.status.contains("stopped"));
    }

    #[test]
    fn pending_subflow_preview_is_hidden_and_cancelled_with_parent() {
        let mut parent = app();
        let mut child = app();
        child.preview_run(false);
        parent.subflow_editor = Some(("embedded".into(), Box::new(child)));
        let ctx = egui::Context::default();
        assert!(parent.has_pending_preview());
        parent.poll_preview(&ctx);
        assert!(parent.subflow_editor.as_ref().unwrap().1.preview.is_none());
        parent.set_running(false);
        assert!(!parent.has_pending_preview());
        parent.overlays_hidden = true;
        parent.poll_preview(&ctx);
        assert!(parent.subflow_editor.as_ref().unwrap().1.preview.is_none());
    }

    #[test]
    fn compare_setting_checkbox_adds_labeled_pin_and_disables_fixed_editor() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.kind = 3;
        app.add_node();
        if let Kind::Compare { value, .. } = &mut app.graph.nodes[0].kind {
            *value = "123.45".into();
        }
        app.selected = Some(0);
        app.node_settings_open = true;
        let checkbox = format!("{} from input", InputParameter::CompareValue.label());
        for enabled in [true, false] {
            render(&ctx, &mut app, vec![]);
            let output = render(&ctx, &mut app, vec![]);
            let position = text_position(&output, &checkbox);
            render(&ctx, &mut app, vec![
                egui::Event::PointerMoved(position), pointer_button(position, true),
            ]);
            render(&ctx, &mut app, vec![pointer_button(position, false)]);
            render(&ctx, &mut app, vec![]);
            let output = render(&ctx, &mut app, vec![]);
            let node = &app.graph.nodes[0];
            assert_eq!(node.input_count(), if enabled { 2 } else { 1 });
            let field = Id::new(("fixed-parameter", 0usize, InputParameter::CompareValue.label()));
            assert_eq!(ctx.read_response(field).unwrap().enabled(), !enabled);
            assert!(matches!(&node.kind, Kind::Compare { value, .. } if value == "123.45"));
            if enabled {
                text_position(&output, node.input_label(1));
                assert!(node_label(node).contains("input"));
                assert!(!node_label(node).contains("123.45"));
            } else {
                assert!(node_label(node).contains("123.45"));
            }
        }
    }

    #[test]
    fn bound_checkboxes_preserve_other_wires_when_ports_change_and_support_undo() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.kind = 5;
        app.add_node();
        for _ in 0..3 {
            app.kind = 9;
            app.add_node();
        }
        let target = app.graph.nodes[0].name.clone();
        let main = app.graph.nodes[1].name.clone();
        let upper = app.graph.nodes[2].name.clone();
        let lower = app.graph.nodes[3].name.clone();
        app.graph.connect(&main, &target).unwrap();
        app.rebuild_snarl();
        app.selected = Some(0);
        app.node_settings_open = true;
        let toggle = |app: &mut App, parameter: InputParameter| {
            render(&ctx, app, vec![]);
            let output = render(&ctx, app, vec![]);
            let pos = text_position(&output, &format!("{} from input", parameter.label()));
            render(&ctx, app, vec![egui::Event::PointerMoved(pos), pointer_button(pos, true)]);
            render(&ctx, app, vec![pointer_button(pos, false)]);
            render(&ctx, app, vec![]);
        };
        toggle(&mut app, InputParameter::Upper);
        let port = app.graph.nodes[0].parameter_port(InputParameter::Upper).unwrap();
        app.graph.connect_input(&upper, &target, port).unwrap();
        app.rebuild_snarl();
        toggle(&mut app, InputParameter::Lower);
        let node = &app.graph.nodes[0];
        assert_eq!(node.input_count(), 3);
        assert!(app.graph.edges.iter().any(|edge| edge.source == upper
            && Some(edge.input) == node.parameter_port(InputParameter::Upper)));
        let port = node.parameter_port(InputParameter::Lower).unwrap();
        app.graph.connect_input(&lower, &target, port).unwrap();
        app.rebuild_snarl();
        toggle(&mut app, InputParameter::Lower);
        let node = &app.graph.nodes[0];
        assert_eq!(node.input_count(), 2);
        assert!(!app.graph.edges.iter().any(|edge| edge.source == lower));
        assert!(app.graph.edges.iter().any(|edge| edge.source == main && edge.input == 0));
        assert!(app.graph.edges.iter().any(|edge| edge.source == upper
            && Some(edge.input) == node.parameter_port(InputParameter::Upper)));
        assert_eq!(app.snarl.wires().count(), 2);
        app.undo_edit(&ctx, false);
        assert_eq!(app.graph.nodes[0].input_count(), 3);
        assert_eq!(app.snarl.wires().count(), 3);
        let restored = Graph::from_json(&app.graph.to_json()).unwrap();
        assert_eq!(restored.nodes[0].input_parameters, app.graph.nodes[0].input_parameters);
    }

    #[test]
    fn hysteresis_and_validation_offer_independent_setting_inputs_but_keyboard_does_not() {
        for (kind, parameters) in [
            (4, vec![InputParameter::Lower, InputParameter::Upper]),
            (10, vec![InputParameter::Minimum, InputParameter::Maximum, InputParameter::MaxChange]),
            (6, vec![]),
        ] {
            let ctx = egui::Context::default();
            let mut app = app();
            app.kind = kind;
            app.add_node();
            app.selected = Some(0);
            app.node_settings_open = true;
            for parameter in &parameters {
                render(&ctx, &mut app, vec![]);
                let output = render(&ctx, &mut app, vec![]);
                let pos = text_position(&output, &format!("{} from input", parameter.label()));
                render(&ctx, &mut app, vec![egui::Event::PointerMoved(pos), pointer_button(pos, true)]);
                render(&ctx, &mut app, vec![pointer_button(pos, false)]);
            }
            render(&ctx, &mut app, vec![]);
            let output = render(&ctx, &mut app, vec![]);
            assert_eq!(app.graph.nodes[0].input_count(), 1 + parameters.len());
            if parameters.is_empty() {
                assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Text(text) if text.galley.text() == "Values from input pins")));
            }
        }
    }

    fn pointer_button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
        }
    }

    fn text_position(output: &egui::FullOutput, label: &str) -> Pos2 {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label =>
                Some(text.pos + text.galley.size() * 0.5),
            _ => None,
        }).unwrap_or_else(|| panic!("Missing control or pin label: {label}"))
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
                selected_wire: None,
                canvas_ui_id: None,
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
                    if text.galley.text() == "# BTC price  (Constant)" =>
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
