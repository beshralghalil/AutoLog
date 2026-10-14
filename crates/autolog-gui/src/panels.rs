//! The bars around the table: what is being read, where it is going, what is
//! on screen, and what happens when the button is pressed.

use autolog_core::EntryStatus;
use autolog_core::config::Theme;
use autolog_core::consumer::CONSUMERS;
use autolog_core::model::format_total;
use autolog_core::source::PROVIDERS;
use egui::{RichText, Ui};

use crate::app::{AutoLogApp, Connection, Modal, NoticeKind};
use crate::theme::Palette;
use crate::widgets;

/// Statuses offered as filter chips, in the order they are shown.
const FILTERABLE: [EntryStatus; 5] = [
    EntryStatus::Pending,
    EntryStatus::Success,
    EntryStatus::Failed,
    EntryStatus::Skipped,
    EntryStatus::Invalid,
];

impl AutoLogApp {
    // ----- the title bar ----------------------------------------------------

    pub fn show_header(&mut self, ui: &mut Ui, palette: &Palette) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(
                RichText::new("AutoLog")
                    .size(17.0)
                    .strong()
                    .color(palette.text),
            );
            widgets::hint(ui, palette, format!("v{}", env!("CARGO_PKG_VERSION")));
            ui.add_space(10.0);
            self.show_connection_chip(ui, palette);

            widgets::trailing(ui, |ui| {
                ui.add_space(4.0);
                if ui.button("Settings").clicked() {
                    self.modal = Some(Modal::Settings);
                }

                egui::ComboBox::from_id_salt("theme")
                    .selected_text(self.settings.theme.label())
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for theme in Theme::ALL {
                            if ui
                                .selectable_label(self.settings.theme == theme, theme.label())
                                .clicked()
                            {
                                let ctx = ui.ctx().clone();
                                self.set_theme(&ctx, theme);
                            }
                        }
                    });

                if self.log_directory.is_some() && ui.button("Logs").clicked() {
                    self.open_logs();
                }
            });
        });
    }

    fn show_connection_chip(&mut self, ui: &mut Ui, palette: &Palette) {
        match &self.connection {
            Connection::Disconnected => {
                widgets::chip(ui, "Not connected", palette.text_muted);
            }
            Connection::Connecting => {
                ui.add(egui::Spinner::new().size(12.0));
                widgets::chip(ui, "Connecting", palette.info);
            }
            Connection::Connected => {
                widgets::chip(
                    ui,
                    &format!("{} connected", self.consumer_spec().label),
                    palette.success,
                );
            }
            Connection::Failed(error) => {
                let error = error.clone();
                if widgets::chip(ui, "Connection failed", palette.danger)
                    .interact(egui::Sense::click())
                    .on_hover_text(error.summary())
                    .clicked()
                {
                    self.modal = Some(Modal::Error(error));
                }
            }
        }
    }

    fn open_logs(&mut self) {
        if let Some(directory) = &self.log_directory
            && let Err(error) = open::that_detached(directory)
        {
            self.notify(
                NoticeKind::Warning,
                format!("Could not open the log folder: {error}"),
            );
        }
    }

    // ----- source and destination -------------------------------------------

    /// Two full-width rows rather than two columns: the credential fields are
    /// the widest thing in the window, and halving the room for them pushes
    /// the API token off the edge of a narrow window.
    pub fn show_setup(&mut self, ui: &mut Ui, palette: &Palette) {
        ui.horizontal_wrapped(|ui| {
            widgets::row_label(ui, palette, "Source");
            self.show_source(ui, palette);
        });

        ui.add_space(4.0);
        ui.separator();
        ui.add_space(2.0);

        ui.horizontal_wrapped(|ui| {
            widgets::row_label(ui, palette, "Destination");
            self.show_destination(ui, palette);
        });
    }

    fn show_source(&mut self, ui: &mut Ui, palette: &Palette) {
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("provider")
                .selected_text(self.provider_spec().label)
                .width(120.0)
                .show_ui(ui, |ui| {
                    for spec in PROVIDERS.iter() {
                        if ui
                            .selectable_label(self.settings.provider == spec.id, spec.label)
                            .on_hover_text(spec.about)
                            .clicked()
                        {
                            self.select_provider(spec.id);
                        }
                    }
                });

            if ui.button("Choose file...").clicked() {
                self.pick_file();
            }

            if self.file.is_some()
                && ui
                    .button("Reload")
                    .on_hover_text("Read the file again")
                    .clicked()
            {
                self.reload();
            }

            if self.loading {
                ui.add(egui::Spinner::new().size(14.0));
            }

            let name = self.file.as_ref().map_or_else(
                || "Drop a file here, or choose one".to_owned(),
                |path| {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                },
            );
            let full = self.file.as_ref().map(|path| path.display().to_string());

            let label = ui.label(
                RichText::new(name)
                    .color(if self.file.is_some() {
                        palette.text
                    } else {
                        palette.text_muted
                    })
                    .size(12.0),
            );
            if let Some(full) = full {
                label.on_hover_text(full);
            }
        });

        if !self.skipped.is_empty() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                widgets::chip(
                    ui,
                    &format!("{} rows could not be read", self.skipped.len()),
                    palette.warning,
                );
                if ui.small_button("Show").clicked() {
                    self.modal = Some(Modal::Skipped);
                }
            });
        }
    }

    fn show_destination(&mut self, ui: &mut Ui, palette: &Palette) {
        let spec = self.consumer_spec();

        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("consumer")
                .selected_text(spec.label)
                .width(110.0)
                .show_ui(ui, |ui| {
                    for consumer in CONSUMERS.iter() {
                        if ui
                            .selectable_label(self.settings.consumer == consumer.id, consumer.label)
                            .on_hover_text(consumer.about)
                            .clicked()
                        {
                            self.select_consumer(consumer.id);
                        }
                    }
                });

            for field in spec.credentials {
                let width = if field.secret { 150.0 } else { 190.0 };
                widgets::field(ui, palette, field.label, width, |ui| {
                    let mut value = self.form_value(field.key).to_owned();
                    let changed = ui
                        .add(
                            egui::TextEdit::singleline(&mut value)
                                .password(field.secret)
                                .hint_text(field.placeholder)
                                .desired_width(width),
                        )
                        .changed();

                    if changed {
                        let key = field.key.to_owned();
                        self.form_mut().insert(key, value);
                        self.settings_dirty = true;
                    }
                });
            }

            ui.add_space(6.0);
            widgets::field(ui, palette, " ", 92.0, |ui| {
                let label = if self.is_connected() {
                    "Refresh"
                } else {
                    "Connect"
                };
                let enabled = self.credentials_complete() && !self.connection.is_busy();
                if ui
                    .add_enabled(enabled, egui::Button::new(label))
                    .on_hover_text(if self.is_connected() {
                        "Fetch the tracker's projects and activities again"
                    } else {
                        "Sign in and fetch this tracker's names"
                    })
                    .clicked()
                {
                    self.connect();
                }
            });
        });
    }

    // ----- what is on screen -------------------------------------------------

    pub fn show_toolbar(&mut self, ui: &mut Ui, palette: &Palette) {
        let counts = self.counts();

        ui.horizontal_wrapped(|ui| {
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search every column...")
                    .desired_width(200.0),
            );
            if search.changed() {
                self.mark_view_dirty();
            }
            if !self.search.is_empty()
                && ui.small_button("x").on_hover_text("Clear search").clicked()
            {
                self.search.clear();
                self.mark_view_dirty();
            }

            ui.separator();

            for status in FILTERABLE {
                let count = counts.get(&status).copied().unwrap_or(0);
                let selected = self.filters.contains(&status);
                if widgets::filter_chip(
                    ui,
                    palette,
                    status.label(),
                    count,
                    selected,
                    palette.status(status),
                )
                .clicked()
                {
                    if selected {
                        self.filters.remove(&status);
                    } else {
                        self.filters.insert(status);
                    }
                    self.mark_view_dirty();
                }
            }

            ui.separator();
            self.show_bulk_pickers(ui, palette);
            ui.separator();
            self.show_column_menu(ui, palette);

            widgets::trailing(ui, |ui| {
                ui.add_space(4.0);
                if self.selection.is_empty() {
                    widgets::hint(ui, palette, format!("{} shown", self.view.len()));
                } else {
                    if ui.small_button("Clear selection").clicked() {
                        self.selection.clear();
                    }
                    widgets::chip(
                        ui,
                        &format!("{} selected", self.selection.len()),
                        palette.accent,
                    );
                }
            });
        });
    }

    /// Set a destination on many rows at once: the selection where there is
    /// one, otherwise everything the filters have left on screen.
    fn show_bulk_pickers(&mut self, ui: &mut Ui, palette: &Palette) {
        let Some(consumer) = self.consumer.clone() else {
            widgets::hint(ui, palette, "Connect to pick projects");
            return;
        };

        let scope = if self.selection.is_empty() {
            format!("all {} shown", self.view.len())
        } else {
            format!("{} selected", self.selection.len())
        };

        // Choices are gathered across every visible row, so the list holds
        // everything any of them could use.
        for field in self.consumer_spec().targets {
            let mut choices: Vec<String> = self
                .view
                .iter()
                .filter_map(|id| self.entry(*id))
                .flat_map(|entry| consumer.choices(field.key, entry))
                .collect();
            choices.sort_unstable();
            choices.dedup();

            if let Some(picked) = widgets::searchable_combo(
                ui,
                palette,
                ("bulk", field.key),
                None,
                &choices,
                &format!("Set {} for {scope}", field.label.to_lowercase()),
                200.0,
            ) {
                self.retarget_many(field.key, &picked.unwrap_or_default());
            }
        }
    }

    fn show_column_menu(&mut self, ui: &mut Ui, palette: &Palette) {
        if self.source_columns.is_empty() {
            return;
        }

        let shown = self.extra_columns.len();
        ui.menu_button(format!("Columns ({shown})"), |ui| {
            ui.set_min_width(220.0);
            widgets::hint(ui, palette, "Columns from the file");
            ui.separator();

            for column in self.source_columns.clone() {
                let mut visible = self.extra_columns.contains(&column);
                if ui.checkbox(&mut visible, &column).changed() {
                    self.toggle_column(&column);
                }
            }

            ui.separator();
            if ui.button("Show none").clicked() {
                self.extra_columns.clear();
                self.settings_dirty = true;
            }
            if ui.button("Show all").clicked() {
                self.extra_columns.clone_from(&self.source_columns);
                self.settings_dirty = true;
            }
        });
    }

    // ----- the bar at the bottom ---------------------------------------------

    pub fn show_footer(&mut self, ui: &mut Ui, palette: &Palette) {
        if let Some(run) = &self.run {
            let remaining = run.remaining();
            let fraction = run.fraction();
            let (done, total) = (run.done, run.total);

            ui.horizontal(|ui| {
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(ui.available_width() - 150.0)
                        .fill(palette.accent)
                        .text(format!("{done} / {total}")),
                );
                if let Some(remaining) = remaining {
                    widgets::hint(ui, palette, format!("about {}s left", remaining.as_secs()));
                }
            });
            ui.add_space(4.0);
        }

        ui.horizontal(|ui| {
            ui.add_space(2.0);
            if self.entries.is_empty() {
                widgets::hint(ui, palette, "Nothing loaded");
            } else {
                ui.label(
                    RichText::new(format!("{} entries", self.entries.len()))
                        .color(palette.text)
                        .size(12.0),
                );
                widgets::hint(
                    ui,
                    palette,
                    format!("{} total", format_total(self.total_seconds)),
                );
                if self.settings.prevent_duplicates {
                    widgets::chip(ui, "duplicate check on", palette.info);
                }
            }

            widgets::trailing(ui, |ui| {
                ui.add_space(2.0);
                self.show_run_buttons(ui, palette);
            });
        });
    }

    fn show_run_buttons(&mut self, ui: &mut Ui, palette: &Palette) {
        if self.is_running() {
            if ui
                .add(egui::Button::new(
                    RichText::new("Stop").color(palette.danger),
                ))
                .on_hover_text("Stop after the entries already in flight")
                .clicked()
            {
                self.stop_run();
            }
            return;
        }

        let ready = self.is_connected() && !self.entries.is_empty();
        let pending = self.submittable();

        let post = ui.add_enabled(
            ready && pending > 0,
            egui::Button::new(
                RichText::new(format!("Post {pending} entries")).color(palette.on_accent),
            )
            .fill(palette.accent),
        );
        if post
            .on_hover_text("Send every entry that has not been posted yet")
            .clicked()
        {
            self.start_run();
        }

        if ui
            .add_enabled(ready, egui::Button::new("Dry run"))
            .on_hover_text("Check every entry without sending anything")
            .clicked()
        {
            self.dry_run();
        }

        if !ready && !self.entries.is_empty() {
            widgets::hint(ui, palette, "Connect to post");
        }
    }
}
