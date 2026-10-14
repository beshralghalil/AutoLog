//! The entries table: every row of the file, and everywhere it is going.
//!
//! Rows are drawn only where they are on screen, so a twenty-thousand line
//! export scrolls as smoothly as a twenty line one. Destinations are picked
//! in the cell itself rather than behind a dialog, because reviewing a file is
//! mostly fixing a handful of rows and that should take one click each.

use autolog_core::model::{EntryId, EntryStatus, WorklogEntry};
use egui::{Align, Color32, Layout, RichText, Sense, Stroke, Ui};
use egui_extras::{Column, TableBuilder};

use crate::app::{AutoLogApp, ColumnKey, Editing, Modal};
use crate::theme::{self, Palette};
use crate::widgets;

/// A change the table wants made, collected while drawing and applied after.
///
/// The rows are drawn from a shared borrow of the state, so nothing can be
/// changed in the middle of drawing them.
enum Action {
    Select(EntryId, bool),
    SelectAll(bool),
    Sort(ColumnKey),
    Retarget(EntryId, String, String),
    BeginEdit(EntryId, ColumnKey, String),
    CommitEdit,
    CancelEdit,
    ShowError(Box<autolog_core::UserError>),
}

impl AutoLogApp {
    pub fn show_table(&mut self, ui: &mut Ui, palette: &Palette) {
        if self.entries.is_empty() {
            self.show_empty_state(ui, palette);
            return;
        }

        let mut actions: Vec<Action> = Vec::new();
        let mut editing = self.editing.take();

        {
            let app = &*self;
            let columns = app.table_columns();
            let all_selected =
                !app.view.is_empty() && app.view.iter().all(|id| app.selection.contains(id));

            let mut builder = TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::exact(28.0))
                .min_scrolled_height(0.0)
                .auto_shrink([false, false]);

            for column in &columns {
                builder = builder.column(width_for(column));
            }

            builder
                .header(theme::HEADER_HEIGHT, |mut header| {
                    header.col(|ui| {
                        let mut checked = all_selected;
                        if ui
                            .add(egui::Checkbox::without_text(&mut checked))
                            .on_hover_text("Select every row shown")
                            .changed()
                        {
                            actions.push(Action::SelectAll(checked));
                        }
                    });

                    for column in &columns {
                        header.col(|ui| {
                            let arrow = match &app.sort {
                                Some(sort) if sort.column == *column => {
                                    if sort.ascending {
                                        " ^"
                                    } else {
                                        " v"
                                    }
                                }
                                _ => "",
                            };

                            let label = RichText::new(format!("{}{arrow}", heading(column)))
                                .size(11.0)
                                .strong()
                                .color(if arrow.is_empty() {
                                    palette.text_muted
                                } else {
                                    palette.accent
                                });

                            if ui
                                .add(egui::Label::new(label).sense(Sense::click()))
                                .on_hover_text("Sort by this column")
                                .clicked()
                            {
                                actions.push(Action::Sort(column.clone()));
                            }
                        });
                    }
                })
                .body(|body| {
                    body.rows(theme::ROW_HEIGHT, app.view.len(), |mut row| {
                        let Some(&id) = app.view.get(row.index()) else {
                            return;
                        };
                        let Some(entry) = app.entry(id) else {
                            return;
                        };

                        row.set_selected(app.selection.contains(&id));

                        row.col(|ui| {
                            let mut checked = app.selection.contains(&id);
                            if ui.add(egui::Checkbox::without_text(&mut checked)).changed() {
                                actions.push(Action::Select(id, checked));
                            }
                        });

                        for column in &columns {
                            row.col(|ui| {
                                app.cell(ui, palette, entry, column, &mut editing, &mut actions);
                            });
                        }
                    });
                });
        }

        self.editing = editing;
        for action in actions {
            self.apply(action);
        }
    }

    /// The columns to draw, in order.
    fn table_columns(&self) -> Vec<ColumnKey> {
        let targets = self.consumer_spec().targets;

        let mut columns = vec![ColumnKey::Status, ColumnKey::Started, ColumnKey::Duration];

        // Where the tracker owns the issue as a destination of its own, that
        // column is the issue; showing the key parsed out of the file beside
        // it would be two columns saying nearly the same thing.
        if !targets.iter().any(|field| field.key == "issue") {
            columns.push(ColumnKey::Issue);
        }

        columns.extend(
            targets
                .iter()
                .map(|field| ColumnKey::Target(field.key.to_owned())),
        );

        columns.extend(self.extra_columns.iter().cloned().map(ColumnKey::Source));
        columns.push(ColumnKey::Message);
        columns
    }

    fn cell(
        &self,
        ui: &mut Ui,
        palette: &Palette,
        entry: &WorklogEntry,
        column: &ColumnKey,
        editing: &mut Option<Editing>,
        actions: &mut Vec<Action>,
    ) {
        match column {
            ColumnKey::Status => Self::status_cell(ui, palette, entry),
            ColumnKey::Started => {
                ui.label(entry.started.format("%Y-%m-%d %H:%M").to_string())
                    .on_hover_text(format!(
                        "{}\nends {}",
                        entry.started.format("%A %-d %B %Y, %H:%M:%S %Z"),
                        entry.end().format("%H:%M:%S")
                    ));
            }
            ColumnKey::Duration => {
                ui.label(RichText::new(entry.duration_label()).color(palette.text));
            }
            ColumnKey::Issue => Self::issue_cell(ui, palette, entry, editing, actions),
            ColumnKey::Target(field) => self.target_cell(ui, palette, entry, field, actions),
            ColumnKey::Source(name) => {
                let value = entry.raw.get(name).map(String::as_str).unwrap_or_default();
                if value.is_empty() {
                    widgets::hint(ui, palette, "-");
                } else {
                    ui.label(RichText::new(value).color(palette.text_muted))
                        .on_hover_text(value);
                }
            }
            ColumnKey::Message => Self::message_cell(ui, palette, entry, actions),
        }
    }

    fn status_cell(ui: &mut Ui, palette: &Palette, entry: &WorklogEntry) {
        if entry.status == EntryStatus::Submitting {
            ui.add(egui::Spinner::new().size(12.0));
            ui.add_space(4.0);
        }
        widgets::chip(ui, entry.status.label(), palette.status(entry.status));
    }

    fn issue_cell(
        ui: &mut Ui,
        palette: &Palette,
        entry: &WorklogEntry,
        editing: &mut Option<Editing>,
        actions: &mut Vec<Action>,
    ) {
        let being_edited = editing
            .as_ref()
            .is_some_and(|edit| edit.id == entry.id && edit.column == ColumnKey::Issue);

        if being_edited {
            let Some(edit) = editing.as_mut() else {
                return;
            };
            let response = ui.add(
                egui::TextEdit::singleline(&mut edit.buffer)
                    .desired_width(f32::INFINITY)
                    .hint_text("ABC-123"),
            );

            if !edit.focused {
                response.request_focus();
                edit.focused = true;
            }

            if response.lost_focus() {
                actions.push(if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                    Action::CancelEdit
                } else {
                    Action::CommitEdit
                });
            }
            return;
        }

        let editable = entry.status != EntryStatus::Success;
        let found = entry.issue_key.is_some();
        // With no key found, show the text it was looked for in: that is what
        // the user has to work from, and it is what they are about to edit.
        let text = entry
            .issue_key
            .clone()
            .unwrap_or_else(|| entry.raw_issue_key.clone());

        let response = editable_cell(
            ui,
            palette,
            &text,
            if found { palette.text } else { palette.warning },
            editable,
        );

        let response = response.on_hover_text(match (found, editable) {
            (true, true) => format!("{}\n\nClick to edit", entry.raw_issue_key),
            (true, false) => entry.raw_issue_key.clone(),
            (false, true) => format!(
                "No issue key found in:\n{}\n\nClick to type one, like ABC-123",
                entry.raw_issue_key
            ),
            (false, false) => format!("No issue key found in:\n{}", entry.raw_issue_key),
        });

        if editable && response.clicked() {
            actions.push(Action::BeginEdit(entry.id, ColumnKey::Issue, text));
        }
    }

    fn target_cell(
        &self,
        ui: &mut Ui,
        palette: &Palette,
        entry: &WorklogEntry,
        field: &str,
        actions: &mut Vec<Action>,
    ) {
        let Some(consumer) = &self.consumer else {
            widgets::hint(ui, palette, "connect first");
            return;
        };

        if entry.status == EntryStatus::Success {
            ui.label(RichText::new(entry.target(field).unwrap_or("-")).color(palette.text_muted));
            return;
        }

        let choices = consumer.choices(field, entry);
        let width = ui.available_width();
        let spec = self.consumer_spec().target(field);

        // The list is the whole truth for a Kimai project; for a Jira issue it
        // is only what the search happened to return, so anything can be typed.
        let accepts = if spec.is_some_and(|spec| spec.key == "issue") {
            widgets::Accepts::AnythingTyped
        } else {
            widgets::Accepts::ListedOnly
        };

        let label = spec.map_or("value", |spec| spec.label).to_lowercase();

        if let Some(picked) = widgets::searchable_combo(
            ui,
            palette,
            ("target", entry.id, field),
            widgets::Combo {
                selected: entry.target(field),
                choices: &choices,
                placeholder: &format!("Pick a {label}"),
                width,
                accepts,
            },
        ) {
            actions.push(Action::Retarget(
                entry.id,
                field.to_owned(),
                picked.unwrap_or_default(),
            ));
        }
    }

    fn message_cell(
        ui: &mut Ui,
        palette: &Palette,
        entry: &WorklogEntry,
        actions: &mut Vec<Action>,
    ) {
        let Some(outcome) = &entry.outcome else {
            return;
        };

        let color = palette.status(entry.status);
        let label = ui.add(
            egui::Label::new(RichText::new(outcome.summary()).color(color).size(12.0))
                .sense(Sense::click()),
        );

        if label
            .on_hover_text(format!(
                "{}\n\n{}\n\nClick for details",
                outcome.title,
                outcome.summary()
            ))
            .clicked()
        {
            actions.push(Action::ShowError(Box::new(outcome.clone())));
        }
    }

    fn show_empty_state(&mut self, ui: &mut Ui, palette: &Palette) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.28);
            ui.label(
                RichText::new(if self.loading {
                    "Reading the file..."
                } else {
                    "No worklog loaded"
                })
                .size(16.0)
                .color(palette.text),
            );
            ui.add_space(6.0);

            if self.loading {
                ui.add(egui::Spinner::new());
                return;
            }

            widgets::hint(
                ui,
                palette,
                format!(
                    "Choose a {} export to get started.",
                    self.provider_spec().label
                ),
            );
            ui.add_space(12.0);
            if ui.button("Choose file...").clicked() {
                self.pick_file();
            }
            ui.add_space(8.0);
            widgets::hint(ui, palette, self.provider_spec().about);
        });
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Select(id, true) => {
                self.selection.insert(id);
            }
            Action::Select(id, false) => {
                self.selection.remove(&id);
            }
            Action::SelectAll(true) => self.selection = self.view.iter().copied().collect(),
            Action::SelectAll(false) => self.selection.clear(),
            Action::Sort(column) => self.toggle_sort(column),
            Action::Retarget(id, field, value) => self.retarget(id, &field, &value),
            Action::BeginEdit(id, column, buffer) => {
                self.editing = Some(Editing {
                    id,
                    column,
                    buffer,
                    focused: false,
                });
            }
            Action::CommitEdit => {
                if let Some(edit) = self.editing.take() {
                    self.set_issue_key(edit.id, &edit.buffer);
                }
            }
            Action::CancelEdit => self.editing = None,
            Action::ShowError(error) => self.modal = Some(Modal::Error(error)),
        }
    }
}

/// A cell that reads as a field you can type into, rather than as a label.
///
/// The old build marked unresolved cells by sticking a `?` on the front of the
/// text, which left the table looking full of broken values. The frame and the
/// colour carry that meaning now, and the text stays the text.
fn editable_cell(
    ui: &mut Ui,
    palette: &Palette,
    text: &str,
    color: Color32,
    editable: bool,
) -> egui::Response {
    let sense = if editable {
        Sense::click()
    } else {
        Sense::hover()
    };
    let height = (ui.available_height() - 4.0).max(22.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), sense);

    if ui.is_rect_visible(rect) {
        let radius = egui::CornerRadius::same(5);
        if editable {
            let stroke = if response.hovered() {
                Stroke::new(1.0, palette.accent)
            } else {
                Stroke::new(1.0, palette.border)
            };
            ui.painter().rect_filled(rect, radius, palette.surface);
            ui.painter()
                .rect_stroke(rect, radius, stroke, egui::StrokeKind::Inside);
        }

        let shown = if text.trim().is_empty() { "-" } else { text };
        let galley = ui.painter().layout(
            shown.to_owned(),
            egui::FontId::proportional(13.0),
            color,
            rect.width() - 16.0,
        );
        ui.painter().galley(
            egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
    }

    response
}

fn heading(column: &ColumnKey) -> String {
    match column {
        ColumnKey::Status => "Status".to_owned(),
        ColumnKey::Started => "Started".to_owned(),
        ColumnKey::Duration => "Length".to_owned(),
        ColumnKey::Issue => "Issue".to_owned(),
        ColumnKey::Target(field) => field
            .char_indices()
            .map(|(index, ch)| {
                if index == 0 {
                    ch.to_ascii_uppercase()
                } else {
                    ch
                }
            })
            .collect(),
        ColumnKey::Source(name) => name.clone(),
        ColumnKey::Message => "Message".to_owned(),
    }
}

fn width_for(column: &ColumnKey) -> Column {
    match column {
        ColumnKey::Status => Column::initial(104.0).at_least(80.0),
        ColumnKey::Started => Column::initial(136.0).at_least(110.0),
        ColumnKey::Duration => Column::initial(78.0).at_least(60.0),
        ColumnKey::Issue => Column::initial(140.0).at_least(90.0),
        ColumnKey::Target(_) => Column::initial(180.0).at_least(110.0),
        ColumnKey::Source(_) => Column::initial(150.0).at_least(70.0),
        ColumnKey::Message => Column::remainder().at_least(140.0),
    }
}
