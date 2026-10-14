//! Small pieces of interface used in more than one place.

use egui::{Align, Color32, CornerRadius, FontId, Layout, Response, RichText, Sense, Ui, Vec2};

use crate::theme::Palette;

/// A rounded, tinted label — a status, a count, a tag.
pub fn chip(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let padding = Vec2::new(8.0, 3.0);
    let (rect, response) = ui.allocate_exact_size(galley.size() + padding * 2.0, Sense::hover());

    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(9), Palette::tint(color));
        ui.painter().galley(rect.min + padding, galley, color);
    }

    response
}

/// A toggle shaped like a chip, for filtering the table by status.
pub fn filter_chip(
    ui: &mut Ui,
    palette: &Palette,
    text: &str,
    count: usize,
    selected: bool,
    color: Color32,
) -> Response {
    let label = if count > 0 {
        format!("{text}  {count}")
    } else {
        text.to_owned()
    };

    let font = FontId::proportional(12.0);
    let text_color = if selected { color } else { palette.text_muted };
    let galley = ui.painter().layout_no_wrap(label, font, text_color);
    let padding = Vec2::new(10.0, 4.0);
    let (rect, response) = ui.allocate_exact_size(galley.size() + padding * 2.0, Sense::click());

    if ui.is_rect_visible(rect) {
        let fill = if selected {
            Palette::tint(color)
        } else if response.hovered() {
            palette.surface_alt
        } else {
            Color32::TRANSPARENT
        };
        let radius = CornerRadius::same(10);
        ui.painter().rect_filled(rect, radius, fill);
        ui.painter().rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.0, if selected { color } else { palette.border }),
            egui::StrokeKind::Inside,
        );
        ui.painter().galley(rect.min + padding, galley, text_color);
    }

    response
}

/// What a [`searchable_combo`] will accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accepts {
    /// Only one of the listed choices.
    ListedOnly,
    /// Anything the user types, as well as the listed choices. Used where the
    /// list is a convenience rather than the whole truth — a Jira issue the
    /// search did not return is still a real issue.
    AnythingTyped,
}

/// A dropdown with a filter box, for lists too long to scroll through.
///
/// Kimai instances routinely have hundreds of activities; without a filter the
/// only way to a name near the end is a very long scroll.
///
/// Typing goes straight to the filter the moment the list opens — the caret is
/// put there rather than waiting to be clicked into, since filtering is what
/// anybody opening a list of hundreds is about to do.
///
/// Returns the new value when one is picked: `Some(None)` means the user chose
/// to clear it.
#[derive(Debug, Clone, Copy)]
pub struct Combo<'a> {
    /// What the row is set to now, if anything.
    pub selected: Option<&'a str>,
    pub choices: &'a [String],
    /// Shown in place of a value, e.g. `Pick a project`.
    pub placeholder: &'a str,
    pub width: f32,
    pub accepts: Accepts,
}

#[expect(
    clippy::option_option,
    reason = "outer: picked at all; inner: picked a value or cleared"
)]
pub fn searchable_combo(
    ui: &mut Ui,
    palette: &Palette,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    combo: Combo<'_>,
) -> Option<Option<String>> {
    let Combo {
        selected,
        choices,
        placeholder,
        width,
        accepts,
    } = combo;
    let id = ui.make_persistent_id(&id_salt);
    let mut picked = None;

    let label = selected.unwrap_or(placeholder);
    let color = if selected.is_some() {
        palette.text
    } else {
        palette.text_muted
    };

    egui::ComboBox::from_id_salt(id)
        .selected_text(RichText::new(label).color(color))
        .width(width)
        // Without this the list closes the moment the filter box is clicked,
        // which is the one thing inside it that has to be clickable.
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .height(380.0)
        .show_ui(ui, |ui| {
            ui.set_min_width(width.max(260.0));

            let mut filter = ui.data_mut(|data| data.get_temp::<String>(id).unwrap_or_default());
            let search = ui.add(
                egui::TextEdit::singleline(&mut filter)
                    .hint_text(match accepts {
                        Accepts::ListedOnly => "Type to filter",
                        Accepts::AnythingTyped => "Type to filter, or enter a value",
                    })
                    .desired_width(f32::INFINITY),
            );

            // Nothing else in here wants the keyboard, so the filter keeps it
            // for as long as the list is open.
            if !search.has_focus() {
                search.request_focus();
            }
            if search.changed() {
                ui.data_mut(|data| data.insert_temp(id, filter.clone()));
            }

            let typed = filter.trim().to_owned();
            let needle = typed.to_lowercase();
            let matches: Vec<&String> = choices
                .iter()
                .filter(|choice| needle.is_empty() || choice.to_lowercase().contains(&needle))
                .collect();

            // Taking what was typed is the obvious thing to do on Enter when
            // the list cannot offer it.
            let custom = accepts == Accepts::AnythingTyped
                && !typed.is_empty()
                && !matches.iter().any(|choice| choice.as_str() == typed);
            let entered = custom && ui.input(|input| input.key_pressed(egui::Key::Enter));

            ui.separator();

            egui::ScrollArea::vertical()
                .max_height(280.0)
                .show(ui, |ui| {
                    if custom
                        && (ui
                            .selectable_label(
                                false,
                                RichText::new(format!("Use \"{typed}\"")).color(palette.accent),
                            )
                            .clicked()
                            || entered)
                    {
                        picked = Some(Some(typed.clone()));
                        ui.close();
                    }

                    if selected.is_some()
                        && ui
                            .selectable_label(
                                false,
                                RichText::new("Clear").color(palette.text_muted),
                            )
                            .clicked()
                    {
                        picked = Some(None);
                        ui.close();
                    }

                    if matches.is_empty() && !custom {
                        ui.label(
                            RichText::new(if choices.is_empty() {
                                "Nothing to choose from yet - connect first."
                            } else {
                                "No match."
                            })
                            .color(palette.text_muted)
                            .italics(),
                        );
                    }

                    for choice in matches {
                        if ui
                            .selectable_label(selected == Some(choice.as_str()), choice)
                            .clicked()
                        {
                            picked = Some(Some(choice.clone()));
                            ui.close();
                        }
                    }
                });
        });

    // The filter is per-opening, not a setting; leaving it behind would make
    // the next open look empty.
    if picked.is_some() {
        ui.data_mut(|data| data.remove::<String>(id));
    }

    picked
}

/// A labelled field, sized up front.
///
/// The size is allocated before the contents are drawn so that a wrapping row
/// knows how wide the field is and can move it to the next line; measuring it
/// afterwards would let it run off the edge.
pub fn field(
    ui: &mut Ui,
    palette: &Palette,
    label: &str,
    width: f32,
    contents: impl FnOnce(&mut Ui),
) {
    ui.allocate_ui(egui::vec2(width, 50.0), |ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(label).size(11.0).color(palette.text_muted));
            ui.add_space(1.0);
            contents(ui);
        });
    });
}

/// The name of a row of controls, in the left margin.
pub fn row_label(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.allocate_ui(egui::vec2(84.0, 20.0), |ui| {
        ui.label(
            RichText::new(text.to_uppercase())
                .size(10.0)
                .strong()
                .color(palette.text_muted),
        );
    });
}

/// A right-aligned row, for the buttons at the end of a bar.
pub fn trailing<R>(ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> R {
    ui.with_layout(Layout::right_to_left(Align::Center), contents)
        .inner
}

/// Muted small print.
pub fn hint(ui: &mut Ui, palette: &Palette, text: impl Into<String>) {
    ui.label(
        RichText::new(text.into())
            .size(11.0)
            .color(palette.text_muted),
    );
}
