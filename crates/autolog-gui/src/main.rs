//! AutoLog: read a worklog export, review it, and post it to a tracker.
//!
//! The window is immediate-mode: [`eframe::App::update`] runs every frame and
//! redraws from the state. Nothing blocks in here — slow work goes to the
//! Tokio runtime in [`task`] and reports back between frames.

// A GUI has no console on Windows, and a flashing command window behind it
// looks like something has gone wrong.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod dialogs;
mod panels;
#[cfg(test)]
mod smoke;
mod table;
mod task;
mod theme;
mod widgets;

use app::AutoLogApp;
use egui::Color32;

/// The window icon, built into the binary so there is nothing to install.
const ICON: &[u8] = include_bytes!("../../../assets/icon.png");

fn main() -> eframe::Result {
    let logging = autolog_core::logging::init();
    let log_directory = logging.directory.clone();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting");

    // A path on the command line lets AutoLog be the program a worklog export
    // is opened with.
    let opened = std::env::args_os().nth(1).map(std::path::PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("AutoLog")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([920.0, 560.0])
            .with_app_id("autolog")
            .with_icon(load_icon()),
        persist_window: true,
        ..Default::default()
    };

    eframe::run_native(
        "AutoLog",
        options,
        Box::new(move |creation| {
            // The runtime and the log file are held by the app, so logging
            // keeps working for as long as the window is open.
            let mut app = AutoLogApp::new(&creation.egui_ctx, log_directory)?;
            app.opened_with = opened;
            let _ = &logging;
            Ok(Box::new(app))
        }),
    )
}

fn load_icon() -> egui::IconData {
    match image::load_from_memory(ICON) {
        Ok(image) => {
            let image = image.into_rgba8();
            let (width, height) = image.dimensions();
            egui::IconData {
                rgba: image.into_raw(),
                width,
                height,
            }
        }
        Err(error) => {
            tracing::warn!("could not read the window icon: {error}");
            egui::IconData::default()
        }
    }
}

impl eframe::App for AutoLogApp {
    /// Everything that changes state, run once before each repaint.
    ///
    /// Keeping it out of [`Self::ui`] means the frame is drawn from state that
    /// is already settled, rather than changing halfway down the window.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.on_first_frame();
        self.pump();
        self.follow_system_theme(ctx);
        self.take_dropped_files(ctx);
        self.rebuild_view();

        // A run reports its own progress, but the estimate in the footer has
        // to tick on its own.
        if self.is_running() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let palette = theme::palette(theme::resolve(self.settings.theme, &ctx));

        egui::Panel::top("header")
            .frame(bar(&palette, 10, 8))
            .show(ui, |ui| self.show_header(ui, &palette));

        egui::Panel::top("setup")
            .frame(bar(&palette, 10, 8))
            .show(ui, |ui| self.show_setup(ui, &palette));

        if !self.entries.is_empty() {
            egui::Panel::top("toolbar")
                .frame(bar(&palette, 10, 6))
                .show(ui, |ui| self.show_toolbar(ui, &palette));
        }

        egui::Panel::bottom("footer")
            .frame(bar(&palette, 10, 8))
            .show(ui, |ui| self.show_footer(ui, &palette));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(palette.bg)
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ui, |ui| self.show_table(ui, &palette));

        self.show_drop_hint(&ctx, &palette);
        self.show_modal(&ctx, &palette);
        self.show_notices(&ctx, &palette);
    }

    /// Called periodically and on exit by eframe.
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.save_settings();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_credentials();
        self.settings_dirty = true;
        self.save_settings();
    }
}

/// The frame every bar around the table shares.
fn bar(palette: &theme::Palette, horizontal: i8, vertical: i8) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.bg)
        .inner_margin(egui::Margin::symmetric(horizontal, vertical))
        .stroke(egui::Stroke::new(1.0, palette.border))
}

impl AutoLogApp {
    /// Re-apply the palette when the desktop switches between light and dark.
    fn follow_system_theme(&mut self, ctx: &egui::Context) {
        if self.settings.theme != autolog_core::config::Theme::System {
            return;
        }

        let wanted = theme::resolve(self.settings.theme, ctx);
        if self.applied_theme != Some(wanted) {
            theme::apply(ctx, wanted);
            self.applied_theme = Some(wanted);
        }
    }

    /// Load a file dragged onto the window.
    fn take_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .first()
                .map(|file| file.path().to_path_buf())
        });
        if let Some(path) = dropped {
            self.load(path);
        }
    }

    /// Say what dropping would do, while a file is over the window.
    fn show_drop_hint(&self, ctx: &egui::Context, palette: &theme::Palette) {
        if ctx.input(|input| input.raw.hovered_files.is_empty()) {
            return;
        }

        let screen = ctx.content_rect();
        egui::Area::new("drop-hint".into())
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .interactable(false)
            .show(ctx, |ui| {
                ui.painter()
                    .rect_filled(screen, 0.0, Color32::from_black_alpha(170));
                ui.painter().text(
                    screen.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("Drop to read it as a {} export", self.provider_spec().label),
                    egui::FontId::proportional(20.0),
                    palette.text,
                );
                ui.painter().text(
                    screen.center() + egui::vec2(0.0, 30.0),
                    egui::Align2::CENTER_CENTER,
                    "Release to load it",
                    egui::FontId::proportional(13.0),
                    palette.text_muted,
                );
            });
    }
}
