// #![windows_subsystem = "windows"]
mod capture;
mod translator;

use capture::capture_selected_text;
use crossbeam_channel::unbounded;
use eframe::egui;
use notify_rust::Notification;
use rdev::{Event, EventType, listen};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use translator::{LocalSqliteDict, TranslateResult};
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

static MOUSE_X: AtomicI32 = AtomicI32::new(0);
static MOUSE_Y: AtomicI32 = AtomicI32::new(0);

pub fn show_notify(title: &str, body: &str) {
    let _ = Notification::new().summary(title).body(body).show();
}

fn generate_cat_icon() -> Icon {
    let mut rgba = vec![0u8; 16 * 16 * 4];
    for y in 0..16 {
        for x in 0..16 {
            let i = (y * 16 + x) * 4;
            if (y > 2 && y < 14 && x > 2 && x < 14)
                || (y <= 4 && (x == 3 || x == 4 || x == 11 || x == 12))
            {
                rgba[i] = 255;
                rgba[i + 1] = 165;
                rgba[i + 2] = 0;
                rgba[i + 3] = 255;
                if y == 7 && (x == 5 || x == 10) {
                    rgba[i] = 0;
                    rgba[i + 1] = 0;
                    rgba[i + 2] = 0;
                    rgba[i + 3] = 255;
                }
            }
        }
    }
    Icon::from_rgba(rgba, 16, 16).unwrap()
}

// 采用更稳健的黑体 TTF 格式，避免 TTC 解析错误导致 UI 引擎死掉
fn setup_custom_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Ok(font_data) = std::fs::read("C:\\Windows\\Fonts\\simhei.ttf") {
        fonts.font_data.insert(
            "chinese_font".to_owned(),
            egui::FontData::from_owned(font_data),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "chinese_font".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "chinese_font".to_owned());
    }
    ctx.set_fonts(fonts);
}

struct SharedState {
    current_result: Option<TranslateResult>,
    is_window_visible: bool,
    pending_pos: Option<(f32, f32)>, // 新增
}

struct HoverDictApp {
    shared_state: Arc<Mutex<SharedState>>,
    _tray_icon: tray_icon::TrayIcon,
    is_first_frame: bool,
}

impl eframe::App for HoverDictApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.is_first_frame {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.is_first_frame = false;
        }

        let mut state = self.shared_state.lock().unwrap();

        // 处理待显示的位置指令
        if let Some((x, y)) = state.pending_pos.take() {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, y)));
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        if state.is_window_visible {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(egui::Color32::from_rgb(250, 250, 250))
                        .rounding(8.0),
                )
                .show(ctx, |ui| {
                    if let Some(res) = &state.current_result {
                        ui.heading(
                            egui::RichText::new(&res.source_text).color(egui::Color32::BLACK),
                        );
                        if let Some(phonetic) = &res.phonetic {
                            ui.label(
                                egui::RichText::new(format!("[{}]", phonetic))
                                    .color(egui::Color32::GRAY),
                            );
                        }
                        ui.separator();
                        ui.label(
                            egui::RichText::new(&res.translation)
                                .size(14.0)
                                .color(egui::Color32::from_rgb(40, 40, 40)),
                        );
                    }
                });

            if ctx.input(|i| i.pointer.any_pressed() && !ctx.is_pointer_over_area())
                || ctx.input(|i| i.key_pressed(egui::Key::Escape))
            {
                state.is_window_visible = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }
}

fn main() -> eframe::Result<()> {
    if !std::path::Path::new("dict.db").exists() {
        show_notify("启动失败", "找不到 dict.db");
        std::process::exit(1);
    }

    let is_capture_enabled = Arc::new(Mutex::new(true));
    let shared_state = Arc::new(Mutex::new(SharedState {
        current_result: None,
        is_window_visible: false,
        pending_pos: None, // 新增
    }));

    let tray_menu = Menu::new();
    let toggle_item = CheckMenuItem::with_id("toggle", "开启划词翻译", true, true, None);
    let quit_item = MenuItem::with_id("quit", "彻底退出", true, None);
    let _ = tray_menu.append_items(&[&toggle_item, &PredefinedMenuItem::separator(), &quit_item]);

    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_icon(generate_cat_icon())
        .build()
        .unwrap();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_always_on_top()
            .with_transparent(true)
            .with_visible(false)
            .with_inner_size([300.0, 200.0]),
        ..Default::default()
    };

    eframe::run_native(
        "HoverDict",
        options,
        Box::new(move |cc| {
            setup_custom_fonts(&cc.egui_ctx);
            let ctx_clone = cc.egui_ctx.clone();

            let is_enabled_tray = Arc::clone(&is_capture_enabled);
            thread::spawn(move || {
                while let Ok(event) = MenuEvent::receiver().recv() {
                    if event.id.0 == "quit" {
                        std::process::exit(0);
                    } else if event.id.0 == "toggle" {
                        let mut enabled = is_enabled_tray.lock().unwrap();
                        *enabled = !*enabled;
                        show_notify("划词翻译", if *enabled { "已开启" } else { "已关闭" });
                    }
                }
            });

            let (capture_tx, capture_rx) = unbounded::<(i32, i32)>();

            let state_clone = Arc::clone(&shared_state);
            thread::spawn(move || {
                let dict = LocalSqliteDict::new("dict.db");

                while let Ok((up_x, up_y)) = capture_rx.recv() {
                    // == 调试通知 1：确认收到了划词动作 ==
                    // show_notify("后台收到了划词", "正在尝试抓取文本...");

                    if let Some(text) = capture_selected_text() {
                        // == 调试通知 2：确认成功抓到了字 ==
                        // show_notify("抓取成功", &text);

                        match dict.translate(&text) {
                            Ok(Some(res)) => {
                                show_notify("查询成功", &res.translation); // 加这行
                                let mut st = state_clone.lock().unwrap();
                                st.current_result = Some(res);
                                st.is_window_visible = true;
                                drop(st);

                                ctx_clone.request_repaint();
                            }
                            Ok(None) => {
                                show_notify("查询结果", "词库里没有这个词"); // 加这行
                            }
                            Err(e) => show_notify("查询错误", &e.to_string()),
                        }
                    } else {
                        // == 调试通知 3：如果没抓到，弹出来看看 ==
                        show_notify("抓取失败", "未能获取选中文本");
                    }
                }
            });

            let is_enabled_hook = Arc::clone(&is_capture_enabled);
            thread::spawn(move || {
                let mut down_x = 0;
                let mut down_y = 0;
                let callback = move |event: Event| match event.event_type {
                    EventType::MouseMove { x, y } => {
                        MOUSE_X.store(x as i32, Ordering::Relaxed);
                        MOUSE_Y.store(y as i32, Ordering::Relaxed);
                    }
                    EventType::ButtonPress(rdev::Button::Left) => {
                        down_x = MOUSE_X.load(Ordering::Relaxed);
                        down_y = MOUSE_Y.load(Ordering::Relaxed);
                    }
                    EventType::ButtonRelease(rdev::Button::Left) => {
                        if !*is_enabled_hook.lock().unwrap() {
                            return;
                        }
                        let up_x = MOUSE_X.load(Ordering::Relaxed);
                        let up_y = MOUSE_Y.load(Ordering::Relaxed);
                        if (((up_x - down_x).pow(2) + (up_y - down_y).pow(2)) as f64).sqrt() > 10.0
                        {
                            let _ = capture_tx.send((up_x, up_y));
                        }
                    }
                    _ => {}
                };
                if let Err(_) = listen(callback) {}
            });

            Box::new(HoverDictApp {
                shared_state,
                _tray_icon: tray_icon,
                is_first_frame: true,
            })
        }),
    )
}
