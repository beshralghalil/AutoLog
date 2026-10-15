//! Things that take over the window, and things that drift past the corner.

use autolog_core::MatchStrategy;
use autolog_core::config::RateLimit;
use autolog_core::error::UserError;
use autolog_core::time::DateOrder;
use egui::{Align2, Color32, Context, Order, RichText, Ui};

use crate::app::{AutoLogApp, Modal, NoticeKind};
use crate::theme::Palette;
use crate::widgets;

/// Date conventions offered in settings; `None` means the provider decides.
const DATE_ORDERS: [(Option<DateOrder>, &str); 3] = [
    (None, "As the provider writes them"),
    (
        Some(DateOrder::MonthFirst),
        "Month first (5-2-2024 is 2 May)",
    ),
    (
        Some(DateOrder::DayFirst),
        "Day first (5-2-2024 is 5 February)",
    ),
];

impl AutoLogApp {
    pub fn show_modal(&mut self, ctx: &Context, palette: &Palette) {
        let Some(modal) = &self.modal else {
            return;
        };

        let title = match modal {
            Modal::Error(error) => error.title.clone(),
            Modal::Settings => "Settings".to_owned(),
            Modal::Skipped => "Rows that could not be read".to_owned(),
        };

        // `Modal` dims what is behind and takes every click aimed at it, so
        // the dialog reads as the only thing that can be answered now.
        let shown = egui::Modal::new("modal".into()).show(ctx, |ui| {
            ui.set_max_width(620.0);
            ui.label(
                RichText::new(&title)
                    .size(16.0)
                    .strong()
                    .color(palette.text),
            );
            ui.add_space(10.0);

            // Taken out so the body can change state freely, then put back
            // unless something asked for the dialog to close.
            let modal = self.modal.take();
            let close = match &modal {
                Some(Modal::Error(error)) => self.show_error(ui, palette, error),
                Some(Modal::Settings) => self.show_settings(ui, palette),
                Some(Modal::Skipped) => self.show_skipped(ui, palette),
                None => true,
            };
            if !close {
                self.modal = modal;
            }
            close
        });

        if shown.inner || shown.should_close() {
            self.modal = None;
        }
    }

    fn show_error(&mut self, ui: &mut Ui, palette: &Palette, error: &UserError) -> bool {
        ui.label(RichText::new(&error.message).size(14.0).color(palette.text));

        if let Some(hint) = &error.hint {
            ui.add_space(8.0);
            egui::Frame::new()
                .fill(Palette::tint(palette.info))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.label(RichText::new(hint).size(12.0).color(palette.text));
                });
        }

        if let Some(detail) = &error.detail {
            ui.add_space(8.0);
            ui.collapsing("Technical details", |ui| {
                ui.label(
                    RichText::new(detail)
                        .monospace()
                        .size(11.0)
                        .color(palette.text_muted),
                );
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(detail.clone());
                }
            });
        }

        ui.add_space(12.0);
        let mut close = false;
        ui.horizontal(|ui| {
            if ui.button("Close").clicked() {
                close = true;
            }
            if self.log_directory.is_some()
                && ui.button("Open logs").clicked()
                && let Some(directory) = &self.log_directory
            {
                let _ = open::that_detached(directory);
            }
        });
        close
    }

    fn show_skipped(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        widgets::hint(
            ui,
            palette,
            format!(
                "{} rows in the file had no usable date or length, so they were left out.",
                self.skipped.len()
            ),
        );
        ui.add_space(8.0);

        let room = (ui.ctx().content_rect().height() - 220.0).max(160.0);
        egui::ScrollArea::vertical()
            .max_height(room)
            .show(ui, |ui| {
                for skipped in &self.skipped {
                    ui.horizontal(|ui| {
                        widgets::chip(ui, &format!("row {}", skipped.number), palette.warning);
                        ui.label(
                            RichText::new(&skipped.reason)
                                .size(12.0)
                                .color(palette.text_muted),
                        );
                    });
                }
            });

        ui.add_space(12.0);
        ui.button("Close").clicked()
    }

    // ----- settings ----------------------------------------------------------

    fn show_settings(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut close = false;

        // Leave room for the title above and the Done button below, or a
        // short window pushes them off the edge of the dialog.
        let room = (ui.ctx().content_rect().height() - 170.0).max(200.0);

        egui::ScrollArea::vertical()
            .max_height(room)
            .show(ui, |ui| {
                self.show_time_settings(ui, palette);
                ui.add_space(14.0);
                self.show_matching_settings(ui, palette);
                ui.add_space(14.0);
                self.show_posting_settings(ui, palette);
                ui.add_space(14.0);
                self.show_storage_settings(ui, palette);
            });

        ui.add_space(12.0);
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Done").clicked() {
                self.save_settings();
                close = true;
            }
            widgets::hint(ui, palette, "Settings are remembered between sessions.");
        });
        close
    }

    fn show_time_settings(&mut self, ui: &mut Ui, palette: &Palette) {
        ui.label(RichText::new("Time").strong().color(palette.text));
        ui.add_space(4.0);

        let zones: Vec<String> = chrono_tz::TZ_VARIANTS
            .iter()
            .map(|zone| zone.name().to_owned())
            .collect();

        ui.horizontal(|ui| {
            widgets::field(ui, palette, "Time zone of the worklog", 280.0, |ui| {
                if let Some(Some(picked)) = widgets::searchable_combo(
                    ui,
                    palette,
                    "timezone",
                    widgets::Combo {
                        selected: Some(self.settings.timezone.name()),
                        choices: &zones,
                        placeholder: "Pick a zone",
                        width: 260.0,
                        accepts: widgets::Accepts::ListedOnly,
                    },
                ) && let Ok(zone) = picked.parse()
                {
                    self.settings.timezone = zone;
                    self.settings_dirty = true;
                    self.reload();
                }
            });

            widgets::field(ui, palette, "Ambiguous dates", 260.0, |ui| {
                let current = DATE_ORDERS
                    .iter()
                    .find(|(order, _)| *order == self.settings.date_order)
                    .map_or("", |(_, label)| *label);

                egui::ComboBox::from_id_salt("date-order")
                    .selected_text(current)
                    .width(250.0)
                    .show_ui(ui, |ui| {
                        for (order, label) in DATE_ORDERS {
                            if ui
                                .selectable_label(self.settings.date_order == order, label)
                                .clicked()
                            {
                                self.settings.date_order = order;
                                self.settings_dirty = true;
                                self.reload();
                            }
                        }
                    });
            });
        });

        widgets::hint(
            ui,
            palette,
            "Entries with no zone of their own are read as being in this one.",
        );
    }

    fn show_matching_settings(&mut self, ui: &mut Ui, palette: &Palette) {
        let spec = self.consumer_spec();
        ui.label(
            RichText::new(format!("How {} destinations are matched", spec.label))
                .strong()
                .color(palette.text),
        );
        ui.add_space(4.0);

        if spec.targets.is_empty() {
            widgets::hint(ui, palette, "This tracker needs no destination picked.");
            return;
        }

        for field in spec.targets {
            ui.horizontal(|ui| {
                widgets::field(ui, palette, field.label, 150.0, |ui| {
                    let current = self.strategy_for(field.key);
                    egui::ComboBox::from_id_salt(("strategy", field.key))
                        .selected_text(current.label())
                        .width(140.0)
                        .show_ui(ui, |ui| {
                            for strategy in MatchStrategy::ALL {
                                if ui
                                    .selectable_label(current == strategy, strategy.label())
                                    .on_hover_text(strategy.description())
                                    .clicked()
                                {
                                    self.set_strategy(field.key, strategy);
                                }
                            }
                        });
                });

                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.set_max_width(400.0);
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    widgets::hint(ui, palette, field.about);
                });
            });
            ui.add_space(6.0);
        }

        if self.is_connected() && ui.button("Match every row again").clicked() {
            self.rematch_all();
        }
    }

    fn show_posting_settings(&mut self, ui: &mut Ui, palette: &Palette) {
        ui.label(RichText::new("Posting").strong().color(palette.text));
        ui.add_space(4.0);

        let mut prevent = self.settings.prevent_duplicates;
        if ui
            .checkbox(&mut prevent, "Skip entries the tracker already has")
            .on_hover_text(
                "Reads back what is already logged before posting, and skips anything with the \
                 same start, length and comment.",
            )
            .changed()
        {
            self.settings.prevent_duplicates = prevent;
            self.settings_dirty = true;
        }

        let mut limit: RateLimit = self.settings.rate_limit;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            widgets::field(ui, palette, "Entries at once", 170.0, |ui| {
                if ui
                    .add(egui::Slider::new(&mut limit.concurrency, 1..=16))
                    .on_hover_text("How many entries are posted in parallel.")
                    .changed()
                {
                    self.settings.rate_limit = limit.clamped();
                    self.settings_dirty = true;
                }
            });

            let ceiling = if limit.per_minute == 0 {
                "no limit"
            } else {
                ""
            };
            widgets::field(ui, palette, "Requests per minute", 220.0, |ui| {
                if ui
                    .add(egui::Slider::new(&mut limit.per_minute, 0..=600).text(ceiling))
                    .on_hover_text(
                        "Keeps AutoLog under the tracker's rate limit. 0 removes the ceiling.",
                    )
                    .changed()
                {
                    self.settings.rate_limit = limit.clamped();
                    self.settings_dirty = true;
                }
            });
        });
    }

    fn show_storage_settings(&mut self, ui: &mut Ui, palette: &Palette) {
        ui.label(RichText::new("Credentials").strong().color(palette.text));
        ui.add_space(4.0);

        if self.secrets.unavailable().is_none() {
            widgets::hint(
                ui,
                palette,
                "Tokens are kept in your system credential store. Only addresses and emails are \
             written to the settings file.",
            );
        } else {
            widgets::chip(ui, "No system credential store", palette.warning);
            widgets::hint(
                ui,
                palette,
                "Tokens will be forgotten when AutoLog closes. They are never written to disk.",
            );
        }

        if let Some(path) = autolog_core::config::settings_path() {
            ui.add_space(6.0);
            widgets::hint(ui, palette, format!("Settings file: {}", path.display()));
        }
    }

    // ----- the corner --------------------------------------------------------

    /// Short-lived messages, newest at the bottom.
    pub fn show_notices(&mut self, ctx: &Context, palette: &Palette) {
        self.expire_notices();
        if self.notices.is_empty() {
            return;
        }

        egui::Area::new("notices".into())
            .order(Order::Foreground)
            .anchor(Align2::RIGHT_BOTTOM, [-16.0, -16.0])
            .interactable(false)
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    for notice in &self.notices {
                        let color = match notice.kind {
                            NoticeKind::Info => palette.info,
                            NoticeKind::Success => palette.success,
                            NoticeKind::Warning => palette.warning,
                        };

                        egui::Frame::new()
                            .fill(palette.surface)
                            .stroke(egui::Stroke::new(1.0, color))
                            .corner_radius(egui::CornerRadius::same(8))
                            .inner_margin(egui::Margin::symmetric(12, 9))
                            .shadow(egui::epaint::Shadow {
                                offset: [0, 3],
                                blur: 12,
                                spread: 0,
                                color: Color32::from_black_alpha(60),
                            })
                            .show(ui, |ui| {
                                ui.set_max_width(340.0);
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                                ui.label(
                                    RichText::new(&notice.text).size(12.0).color(palette.text),
                                );
                            });
                        ui.add_space(6.0);
                    }
                });
            });

        // Notices fade on a timer, so the window has to keep painting.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}
