use anyhow::Result;
use chrono::Utc;
use eframe::egui;
use egui_phosphor_icons::icons;
use std::time::{Duration, Instant};

use crate::config::{Config, ThemePreference};
use crate::db;
use crate::platform;

#[cfg(target_os = "macos")]
use super::macos_status::{MacosStatusItem, StatusCommand, set_dock_visible};
use super::style;
use super::views;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Current,
    Trackings,
    VisualDay,
    Projects,
    Jira,
    Daemon,
    Settings,
}

pub fn run(config: &Config, config_path: Option<&str>) -> Result<()> {
    tracing::info!("gui startup: preparing native options");
    let icon_data = eframe::icon_data::from_png_bytes(include_bytes!("../../icon_black.png")).ok();
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LazyTime GUI")
            .with_app_id("com.lazytime.app")
            .with_icon(icon_data.unwrap_or_default())
            .with_inner_size([1280.0, 820.0]),
        ..Default::default()
    };

    let cfg = config.clone();
    let path = config_path.map(ToString::to_string);
    tracing::info!("gui startup: entering eframe::run_native");
    eframe::run_native(
        "LazyTime GUI",
        native_options,
        Box::new(move |cc| {
            tracing::info!("gui startup: eframe app creator invoked");
            let mut fonts = egui::FontDefinitions::default();
            egui_phosphor_icons::add_fonts(&mut fonts);
            let icon_family = egui::FontFamily::Name("phosphor-regular".into());
            let proportional_fallbacks = fonts
                .families
                .get(&egui::FontFamily::Proportional)
                .cloned()
                .unwrap_or_default();
            if let Some(icon_fonts) = fonts.families.get_mut(&icon_family) {
                for fallback in proportional_fallbacks {
                    if !icon_fonts.contains(&fallback) {
                        icon_fonts.push(fallback);
                    }
                }
            }
            cc.egui_ctx.set_fonts(fonts);
            style::apply_base_style(&cc.egui_ctx);
            tracing::info!("gui startup: creating GuiApp state");
            Ok(Box::new(GuiApp::new(cfg, path, &cc.egui_ctx)))
        }),
    )
    .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    Ok(())
}

struct GuiApp {
    config: Config,
    config_path: Option<String>,
    mode: ViewMode,
    backend_name: String,
    toast: Option<ToastMessage>,
    current: views::CurrentView,
    trackings: views::TrackingsView,
    visual_day: views::VisualDayView,
    projects: views::ProjectsView,
    jira: views::JiraSyncView,
    daemon: views::DaemonView,
    settings: views::SettingsView,
    onboarding: views::OnboardingView,
    undo: views::UndoState,
    header_icon_light: Option<egui::TextureHandle>,
    header_icon_dark: Option<egui::TextureHandle>,
    header_icon_size: egui::Vec2,
    #[cfg(target_os = "macos")]
    macos_status: Option<MacosStatusItem>,
    #[cfg(target_os = "macos")]
    macos_status_attempted: bool,
    #[cfg(target_os = "macos")]
    allow_close: bool,
}

struct ToastMessage {
    text: String,
    created_at: Instant,
}

impl GuiApp {
    fn new(config: Config, config_path: Option<String>, egui_ctx: &egui::Context) -> Self {
        tracing::info!("gui startup: initializing SettingsView");
        let settings = views::SettingsView::new(&config);
        tracing::info!("gui startup: initializing OnboardingView");
        let onboarding = views::OnboardingView::new(&config);
        tracing::info!("gui startup: constructing app shell");
        let mut app = Self {
            config,
            config_path,
            mode: ViewMode::Current,
            backend_name: platform::detected_backend_name().to_string(),
            toast: None,
            current: views::CurrentView::default(),
            trackings: views::TrackingsView::default(),
            visual_day: views::VisualDayView::default(),
            projects: views::ProjectsView::default(),
            jira: views::JiraSyncView::default(),
            daemon: views::DaemonView::default(),
            settings,
            onboarding,
            undo: views::UndoState::new(),
            header_icon_light: None,
            header_icon_dark: None,
            header_icon_size: egui::vec2(0.0, 0.0),
            #[cfg(target_os = "macos")]
            macos_status: None,
            #[cfg(target_os = "macos")]
            macos_status_attempted: false,
            #[cfg(target_os = "macos")]
            allow_close: false,
        };
        let (header_icon_light, header_icon_size) = Self::load_header_icon(
            egui_ctx,
            include_bytes!("../../icon_black.png"),
            "lazytime_header_icon_light",
        );
        let (header_icon_dark, _) = Self::load_header_icon(
            egui_ctx,
            include_bytes!("../../icon_white.png"),
            "lazytime_header_icon_dark",
        );
        app.header_icon_light = header_icon_light;
        app.header_icon_dark = header_icon_dark;
        app.header_icon_size = header_icon_size;
        if app.config.onboarding_done
            && let Some(msg) = app.daemon.auto_start_on_gui_launch(&app.config)
        {
            app.push_toast(msg);
        }
        tracing::info!("gui startup: GuiApp ready");
        app
    }

    fn load_header_icon(
        ctx: &egui::Context,
        png: &[u8],
        texture_name: &'static str,
    ) -> (Option<egui::TextureHandle>, egui::Vec2) {
        let Some(icon) = eframe::icon_data::from_png_bytes(png).ok() else {
            return (None, egui::vec2(0.0, 0.0));
        };
        let image = crop_transparent_edges(&icon);
        let texture = ctx.load_texture(texture_name, image, egui::TextureOptions::LINEAR);
        let target_height = 34.0;
        let aspect = texture.size()[0] as f32 / texture.size()[1].max(1) as f32;
        let size = egui::vec2(target_height * aspect, target_height);
        (Some(texture), size)
    }

    fn push_toast(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        self.toast = Some(ToastMessage {
            text,
            created_at: Instant::now(),
        });
    }

    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some(toast) = self.toast.as_ref() else {
            return;
        };
        if toast.created_at.elapsed() >= Duration::from_secs(5) {
            self.toast = None;
            return;
        }

        let mut dismiss = false;
        egui::Area::new(egui::Id::new("gui_toast"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 12.0))
            .interactable(true)
            .show(ctx, |ui| {
                let inner = egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_max_width(420.0);
                        ui.label(&toast.text);
                    });
                let click_resp = ui.interact(
                    inner.response.rect,
                    ui.id().with("toast_click"),
                    egui::Sense::click(),
                );
                if click_resp.clicked() {
                    dismiss = true;
                }
            });
        if dismiss {
            self.toast = None;
        }
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        let pref = match self.config.theme_preference {
            ThemePreference::Auto => egui::ThemePreference::System,
            ThemePreference::Light => egui::ThemePreference::Light,
            ThemePreference::Dark => egui::ThemePreference::Dark,
        };
        ctx.set_theme(pref);
        // Keep layout metrics identical across light/dark; only visuals should change.
        style::apply_base_style(ctx);
    }

    #[cfg(target_os = "macos")]
    fn handle_macos_status_item(&mut self, ctx: &egui::Context) {
        if !self.macos_status_attempted {
            self.macos_status_attempted = true;
            self.macos_status = MacosStatusItem::new(ctx)
                .inspect_err(|err| tracing::warn!("macos status item unavailable: {err}"))
                .ok();
        }

        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            set_dock_visible(false);
        }

        let status_title = self.title_tracking_text();
        let Some(status_item) = self.macos_status.as_mut() else {
            return;
        };
        status_item.set_title(&status_title);

        while let Some(command) = status_item.take_command() {
            match command {
                StatusCommand::Show => {
                    set_dock_visible(true);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                StatusCommand::Hide => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    set_dock_visible(false);
                }
                StatusCommand::Quit => {
                    set_dock_visible(true);
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn set_mode(&mut self, mode: ViewMode) {
        self.mode = mode;
    }

    fn title_tracking_text(&self) -> String {
        let Ok(conn) = db::open(self.config.db_path()) else {
            return "(none) 0:00 | 0:00".to_string();
        };

        let now = Utc::now();
        let total_secs = db::list_today(&conn)
            .ok()
            .map(|rows| {
                rows.into_iter().fold(0i64, |acc, row| {
                    let start = crate::time::parse_ts(&row.start_ts).ok();
                    let end = row
                        .end_ts
                        .as_ref()
                        .and_then(|e| crate::time::parse_ts(e).ok())
                        .or(Some(now));
                    match (start, end) {
                        (Some(s), Some(e)) => acc + e.signed_duration_since(s).num_seconds().max(0),
                        _ => acc,
                    }
                })
            })
            .unwrap_or(0);
        let total_text = format_duration_hm(total_secs);

        if let Ok(Some(active)) = db::get_active_tracking(&conn) {
            let current_secs = crate::time::parse_ts(&active.start_ts)
                .ok()
                .map(|s| now.signed_duration_since(s).num_seconds().max(0))
                .unwrap_or(0);
            format!(
                "{} {} | {}",
                active.project_name,
                format_duration_hm(current_secs),
                total_text
            )
        } else {
            format!("(none) 0:00 | {}", total_text)
        }
    }

    fn current_undo_domain(&self) -> Option<views::UndoDomain> {
        match self.mode {
            ViewMode::Trackings => Some(views::UndoDomain::Trackings),
            ViewMode::VisualDay => Some(views::UndoDomain::VisualDay),
            ViewMode::Projects => Some(views::UndoDomain::Projects),
            _ => None,
        }
    }

    fn handle_global_shortcuts(&mut self, ctx: &egui::Context) -> Option<String> {
        if ctx.wants_keyboard_input() {
            return None;
        }

        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Z))
            && let Some(domain) = self.current_undo_domain()
        {
            let mut conn = match db::open(self.config.db_path()) {
                Ok(conn) => conn,
                Err(err) => return Some(format!("error: {err}")),
            };
            return match self.undo.undo(&mut conn, domain) {
                Ok(Some(msg)) => Some(msg),
                Ok(None) => Some("nothing to undo".to_string()),
                Err(err) => Some(format!("error: {err}")),
            };
        }

        if ctx.input(|i| i.key_pressed(egui::Key::C)) {
            self.set_mode(ViewMode::Current);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::T)) {
            self.set_mode(ViewMode::Trackings);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::V)) {
            self.set_mode(ViewMode::VisualDay);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::P)) {
            self.set_mode(ViewMode::Projects);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::J)) {
            self.set_mode(ViewMode::Jira);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::O)) {
            self.set_mode(ViewMode::Daemon);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::X)) {
            self.set_mode(ViewMode::Settings);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Q)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        None
    }

    fn sidebar_entry(
        ui: &mut egui::Ui,
        collapsed: bool,
        selected: bool,
        icon: egui_phosphor_icons::Icon,
        label: &str,
    ) -> egui::Response {
        let text = style::icon_label(ui, icon, if collapsed { "" } else { label });
        let button = if collapsed {
            egui::Button::new(text).selected(selected)
        } else {
            egui::Button::new(text).selected(selected).right_text("")
        };
        let response = ui.add_sized(
            [
                ui.available_width(),
                (style::BUTTON_PAD_Y as f32 * 2.0) + ui.spacing().interact_size.y,
            ],
            button,
        );
        if collapsed {
            response.on_hover_text(label)
        } else {
            response
        }
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        self.handle_macos_status_item(ctx);

        self.apply_theme(ctx);
        if !self.config.onboarding_done {
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some(msg) =
                    self.onboarding
                        .ui(ctx, ui, &mut self.config, self.config_path.as_deref())
                {
                    if self.config.onboarding_done {
                        if let Some(start_msg) = self.daemon.auto_start_on_gui_launch(&self.config)
                        {
                            self.push_toast(start_msg);
                        }
                    }
                    self.push_toast(msg);
                }
            });
            self.draw_toast(ctx);
            return;
        }

        if let Some(msg) = self.handle_global_shortcuts(ctx) {
            self.push_toast(msg);
        }
        self.daemon.poll(&self.config);

        let tracking_title = self.title_tracking_text();

        egui::TopBottomPanel::top("gui_top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let header_icon = if ctx.style().visuals.dark_mode {
                    self.header_icon_dark.as_ref()
                } else {
                    self.header_icon_light.as_ref()
                };
                if let Some(icon) = header_icon {
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.add(egui::Image::new(icon).fit_to_exact_size(self.header_icon_size));
                        });
                    ui.add_space(8.0);
                }
                ui.label(egui::RichText::new("LazyTime GUI").size(32.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(&tracking_title).weak());
                });
            });
        });

        egui::SidePanel::left("gui_sidebar")
            .resizable(false)
            .min_width(if self.config.sidebar_collapsed {
                style::SIDEBAR_COLLAPSED
            } else {
                style::SIDEBAR_EXPANDED
            })
            .max_width(if self.config.sidebar_collapsed {
                style::SIDEBAR_COLLAPSED
            } else {
                style::SIDEBAR_EXPANDED
            })
            .show(ctx, |ui| {
                ui.add_space(style::BUTTON_PAD_Y as f32);
                let collapse_button_width =
                    style::SIDEBAR_COLLAPSED - (style::BUTTON_PAD_X as f32 * 2.0);
                if ui
                    .add_sized(
                        [
                            collapse_button_width,
                            (style::BUTTON_PAD_Y as f32 * 2.0) + ui.spacing().interact_size.y,
                        ],
                        egui::Button::new(style::icon_label(
                            ui,
                            if self.config.sidebar_collapsed {
                                icons::CARET_RIGHT
                            } else {
                                icons::CARET_LEFT
                            },
                            "",
                        )),
                    )
                    .clicked()
                {
                    self.config.sidebar_collapsed = !self.config.sidebar_collapsed;
                }
                ui.separator();

                let collapsed = self.config.sidebar_collapsed;
                if Self::sidebar_entry(
                    ui,
                    collapsed,
                    self.mode == ViewMode::Current,
                    icons::CLOCK,
                    "Current",
                )
                .clicked()
                {
                    self.set_mode(ViewMode::Current);
                }
                if Self::sidebar_entry(
                    ui,
                    collapsed,
                    self.mode == ViewMode::Trackings,
                    icons::CALENDAR,
                    "Trackings",
                )
                .clicked()
                {
                    self.set_mode(ViewMode::Trackings);
                }
                if Self::sidebar_entry(
                    ui,
                    collapsed,
                    self.mode == ViewMode::VisualDay,
                    icons::CHART_BAR_HORIZONTAL,
                    "Visual Day",
                )
                .clicked()
                {
                    self.set_mode(ViewMode::VisualDay);
                }
                if Self::sidebar_entry(
                    ui,
                    collapsed,
                    self.mode == ViewMode::Projects,
                    icons::PACKAGE,
                    "Projects",
                )
                .clicked()
                {
                    self.set_mode(ViewMode::Projects);
                }
                if Self::sidebar_entry(
                    ui,
                    collapsed,
                    self.mode == ViewMode::Jira,
                    icons::CLOUD_ARROW_UP,
                    "Jira",
                )
                .clicked()
                {
                    self.set_mode(ViewMode::Jira);
                }
                if Self::sidebar_entry(
                    ui,
                    collapsed,
                    self.mode == ViewMode::Daemon,
                    icons::HAMMER,
                    "Daemon",
                )
                .clicked()
                {
                    self.set_mode(ViewMode::Daemon);
                }
                if Self::sidebar_entry(
                    ui,
                    collapsed,
                    self.mode == ViewMode::Settings,
                    icons::GEAR,
                    "Settings",
                )
                .clicked()
                {
                    self.set_mode(ViewMode::Settings);
                }
            });

        egui::TopBottomPanel::bottom("gui_status").show(ctx, |ui| {
            let daemon_state = self.daemon.status_text(&self.config);
            let autotrack_status = self.current.autotrack_status_sentence(&self.config);
            let autotrack_snoozed = self.current.autotrack_is_snoozed(&self.config);
            let autotrack_suspended = self.current.autotrack_is_suspended(&self.config);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(autotrack_status).weak());
                if autotrack_snoozed
                    && ui.small_button("Unsnooze").clicked()
                    && let Some(msg) = self.current.unsnooze_autotracking(&self.config)
                {
                    self.push_toast(msg);
                }
                if autotrack_suspended
                    && ui.small_button("Unsuspend").clicked()
                    && let Some(msg) = self.current.unsuspend_autotracking(&self.config)
                {
                    self.push_toast(msg);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "backend: {} | daemon: {}",
                            self.backend_name, daemon_state
                        ))
                        .weak(),
                    );
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let msg = match self.mode {
                ViewMode::Current => self.current.ui(ctx, ui, &self.config),
                ViewMode::Trackings => self.trackings.ui(ctx, ui, &self.config, &mut self.undo),
                ViewMode::VisualDay => self.visual_day.ui(ctx, ui, &self.config, &mut self.undo),
                ViewMode::Projects => self.projects.ui(ctx, ui, &self.config, &mut self.undo),
                ViewMode::Jira => self.jira.ui(ui, &self.config),
                ViewMode::Daemon => self.daemon.ui(ui, &self.config),
                ViewMode::Settings => {
                    self.settings
                        .ui(ctx, ui, &mut self.config, self.config_path.as_deref())
                }
            };
            if let Some(msg) = msg {
                self.push_toast(msg);
            }
        });

        self.draw_toast(ctx);

        if self.settings.take_pref_changed() {
            self.config.theme_preference = self.settings.theme_preference.clone();
            self.config.sidebar_collapsed = self.settings.sidebar_collapsed;
        }
        if self.settings.take_onboarding_requested() {
            self.onboarding = views::OnboardingView::new(&self.config);
        }

        if let Ok(conn) = db::open(self.config.db_path()) {
            let _ = db::migrate(&conn);
        }
    }
}

fn format_duration_hm(secs: i64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    format!("{}:{:02}", h, m)
}

fn crop_transparent_edges(icon: &egui::IconData) -> egui::ColorImage {
    let width = icon.width as usize;
    let height = icon.height as usize;
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0usize;
    let mut max_y = 0usize;

    for y in 0..height {
        for x in 0..width {
            let alpha = icon.rgba[(y * width + x) * 4 + 3];
            if alpha > 0 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    if min_x > max_x || min_y > max_y {
        return egui::ColorImage::from_rgba_unmultiplied([width, height], &icon.rgba);
    }

    let cropped_w = max_x - min_x + 1;
    let cropped_h = max_y - min_y + 1;
    let mut rgba = vec![0u8; cropped_w * cropped_h * 4];

    for y in 0..cropped_h {
        let src_y = min_y + y;
        for x in 0..cropped_w {
            let src_x = min_x + x;
            let src = (src_y * width + src_x) * 4;
            let dst = (y * cropped_w + x) * 4;
            rgba[dst..dst + 4].copy_from_slice(&icon.rgba[src..src + 4]);
        }
    }

    egui::ColorImage::from_rgba_unmultiplied([cropped_w, cropped_h], &rgba)
}
