//! The entries table: every row of the file, and everywhere it is going.
//!
//! Rows are drawn only where they are on screen, so a twenty-thousand line
//! export scrolls as smoothly as a twenty line one. Destinations are picked
//! in the cell itself rather than behind a dialog, because reviewing a file is
//! mostly fixing a handful of rows and that should take one click each.

use autolog_core::model::{EntryId, EntryStatus, WorklogEntry};
use egui::{Align, Layout, RichText, Sense, Ui};
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
        let mut columns = vec![
            ColumnKey::Status,
            ColumnKey::Started,
            ColumnKey::Duration,
            ColumnKey::Issue,
        ];

        columns.extend(
            self.consumer_spec()
                .targets
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
            response.request_focus();

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
        let (text, color) = match entry.issue_key.as_deref() {
            Some(key) => (key.to_owned(), palette.text),
            None => (unmatched(&entry.raw_issue_key), palette.warning),
        };

        let label = ui.add(egui::Label::new(RichText::new(&text).color(color)).sense(
            if editable {
                Sense::click()
            } else {
                Sense::hover()
            },
        ));

        let label = label.on_hover_text(if editable {
            format!("{}\n\nClick to edit", entry.raw_issue_key)
        } else {
            entry.raw_issue_key.clone()
        });

        if editable && label.clicked() {
            actions.push(Action::BeginEdit(
                entry.id,
                ColumnKey::Issue,
                entry.issue_key.clone().unwrap_or_default(),
            ));
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

        if let Some(picked) = widgets::searchable_combo(
            ui,
            palette,
            ("target", entry.id, field),
            entry.target(field),
            &choices,
            &unmatched(&entry.activity),
            width,
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
                self.editing = Some(Editing { id, column, buffer });
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

/// Text shown where nothing was matched: the thing that was matched against,
/// marked so it reads as a question rather than an answer.
fn unmatched(source: &str) -> String {
    if source.trim().is_empty() {
        "? (blank)".to_owned()
    } else {
        format!("? {source}")
    }
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
