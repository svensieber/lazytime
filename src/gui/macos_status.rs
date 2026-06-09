use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use eframe::egui;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusCommand {
    Show,
    Hide,
    Quit,
}

pub struct MacosStatusItem {
    tray_icon: TrayIcon,
    commands: Arc<Mutex<VecDeque<StatusCommand>>>,
    title: String,
}

impl MacosStatusItem {
    pub fn new(ctx: &egui::Context) -> anyhow::Result<Self> {
        let menu = Menu::new();
        let show_id = MenuId::new("lazytime-show");
        let hide_id = MenuId::new("lazytime-hide");
        let quit_id = MenuId::new("lazytime-quit");
        let show = MenuItem::with_id(show_id.clone(), "Show LazyTime", true, None);
        let hide = MenuItem::with_id(hide_id.clone(), "Hide Window", true, None);
        let quit = MenuItem::with_id(quit_id.clone(), "Quit LazyTime", true, None);

        menu.append_items(&[&show, &hide, &PredefinedMenuItem::separator(), &quit])?;

        let icon = status_bar_icon()?;
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("LazyTime")
            .with_title("LT")
            .with_icon(icon)
            .with_icon_as_template(true)
            .with_menu_on_left_click(true)
            .with_menu_on_right_click(true)
            .build()?;

        let commands = Arc::new(Mutex::new(VecDeque::new()));
        let handler_commands = Arc::clone(&commands);
        let repaint_ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let command = if event.id() == &show_id {
                Some(StatusCommand::Show)
            } else if event.id() == &hide_id {
                Some(StatusCommand::Hide)
            } else if event.id() == &quit_id {
                Some(StatusCommand::Quit)
            } else {
                None
            };
            if let Some(command) = command {
                if let Ok(mut pending) = handler_commands.lock() {
                    pending.push_back(command);
                }
                repaint_ctx.request_repaint();
            }
        }));

        let repaint_ctx = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |_| {
            repaint_ctx.request_repaint();
        }));

        Ok(Self {
            tray_icon,
            commands,
            title: "LT".to_string(),
        })
    }

    pub fn set_title(&mut self, title: &str) {
        let next = title.trim();
        if next.is_empty() || next == self.title {
            return;
        }
        self.tray_icon.set_title(Some(next));
        self.title = next.to_string();
    }

    pub fn take_command(&self) -> Option<StatusCommand> {
        self.commands
            .lock()
            .ok()
            .and_then(|mut pending| pending.pop_front())
    }
}

fn status_bar_icon() -> anyhow::Result<Icon> {
    const WIDTH: u32 = 18;
    const HEIGHT: u32 = 18;
    let mut rgba = vec![0; (WIDTH * HEIGHT * 4) as usize];

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let dx = x as i32 - 9;
            let dy = y as i32 - 9;
            let on_outer_ring = (dx * dx + dy * dy) <= 64 && (dx * dx + dy * dy) >= 45;
            let on_clock_hand = (x == 9 && (5..=9).contains(&y)) || (y == 9 && (9..=13).contains(&x));

            if on_outer_ring || on_clock_hand {
                let offset = ((y * WIDTH + x) * 4) as usize;
                rgba[offset] = 255;
                rgba[offset + 1] = 255;
                rgba[offset + 2] = 255;
                rgba[offset + 3] = 255;
            }
        }
    }

    Ok(Icon::from_rgba(rgba, WIDTH, HEIGHT)?)
}

pub fn set_dock_visible(visible: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::warn!("cannot update macos activation policy away from main thread");
        return;
    };

    let app = NSApplication::sharedApplication(mtm);
    let policy = if visible {
        NSApplicationActivationPolicy::Regular
    } else {
        NSApplicationActivationPolicy::Accessory
    };
    let applied = app.setActivationPolicy(policy);
    if !applied {
        tracing::warn!("macos activation policy change was rejected");
    }
    if visible {
        app.activate();
    }
}
