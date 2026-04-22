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
use std::time::Instant;
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
    // 来自鼠标钩子的屏幕像素坐标（physical pixels），需要在 egui 中换算为 logical points。
    pending_pos: Option<(i32, i32)>,
    shown_at: Option<Instant>,
}

struct HoverDictApp {
    shared_state: Arc<Mutex<SharedState>>,
    _tray_icon: tray_icon::TrayIcon,
    is_first_frame: bool,
    // 只在状态变化时发送窗口命令，避免每帧 OuterPosition/InnerSize 导致卡死
    last_visible: bool,
}

impl eframe::App for HoverDictApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.is_first_frame {
            // 不要把根窗口设为 Visible(false)：
            // 在 Windows 上根窗口隐藏后，可能不会再驱动 update，从而无法“再显示”弹窗。
            // 采用更稳健的策略：窗口始终存在，但默认移到屏幕外 + 缩成 1x1。
            // 注意：MousePassthrough 在部分 Windows 环境下配合透明窗口/连点会导致窗口消息异常（表现为卡死），因此不用它。
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1.0, 1.0)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(-10_000.0, -10_000.0)));
            self.is_first_frame = false;
            self.last_visible = false;
        }

        // 注意：不要在整个 update() 周期持有 Mutex。
        // 否则后台线程（划词/查询）一旦也需要拿锁，就可能造成 UI 线程阻塞，
        // 表现为窗口“未响应”，连 Esc/鼠标也失效。
        let (pending_pos, is_window_visible, current_result, _shown_at) = {
            let mut state = self.shared_state.lock().unwrap();
            (
                state.pending_pos.take(),
                state.is_window_visible,
                state.current_result.clone(),
                state.shown_at,
            )
        };

        // 只在“隐藏 -> 显示”的那一刻设置位置/大小/穿透/焦点
        if is_window_visible && !self.last_visible {
            if let Some((px, py)) = pending_pos {
                let ppp = ctx.pixels_per_point();
                let x = px as f32 / ppp;
                let y = py as f32 / ppp;
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, y)));
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(300.0, 200.0)));
            // 不强制抢焦点：交互关闭改用按钮/全局 Esc 兜底，避免透明置顶窗口在频繁点击时出现焦点/事件异常。
        }

        // 只在“显示 -> 隐藏”的那一刻把窗口挪走/缩小/穿透
        if !is_window_visible && self.last_visible {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1.0, 1.0)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(-10_000.0, -10_000.0)));
        }

        if is_window_visible {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(egui::Color32::from_rgb(250, 250, 250))
                        .rounding(8.0),
                )
                .show(ctx, |ui| {
                    // 显式关闭按钮（比“点击外部关闭”稳定）
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if ui.button("×").clicked() {
                            if let Ok(mut st) = self.shared_state.lock() {
                                st.is_window_visible = false;
                            }
                        }
                    });

                    if let Some(res) = &current_result {
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
        }

        self.last_visible = is_window_visible;
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
        pending_pos: None,
        shown_at: None,
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
            // 根窗口保持可见，避免隐藏后无法唤醒
            .with_visible(true)
            .with_inner_size([1.0, 1.0]),
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
                    if let Some(text) = capture_selected_text() {
                        match dict.translate(&text) {
                            Ok(Some(res)) => {
                                show_notify("查询成功", &res.translation); // 弹出通知
                                let mut st = state_clone.lock().unwrap(); // 锁住状态
                                st.current_result = Some(res); // 设置翻译结果
                                st.is_window_visible = true; // 设置窗口可见
                                // 在鼠标抬起位置附近弹出窗口；注意此处是屏幕像素坐标
                                st.pending_pos = Some((up_x + 12, up_y + 12));
                                st.shown_at = Some(Instant::now());
                                drop(st); // 释放锁

                                ctx_clone.request_repaint(); // 请求重新绘制
                            }
                            Ok(None) => {
                                show_notify("查询结果", "词库里没有这个词");
                            }
                            Err(e) => show_notify("查询错误", &e.to_string()),
                        }
                    } else {
                        show_notify("抓取失败", "未能获取选中文本");
                    }
                }
            });

            let is_enabled_hook = Arc::clone(&is_capture_enabled);
            let state_for_hook = Arc::clone(&shared_state);
            let ctx_for_hook = cc.egui_ctx.clone();
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
                    // 全局兜底：即使 UI 线程卡住，也能用 Esc 强制隐藏弹窗
                    EventType::KeyPress(rdev::Key::Escape) => {
                        if let Ok(mut st) = state_for_hook.lock() {
                            st.is_window_visible = false;
                        }
                        ctx_for_hook.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1.0, 1.0)));
                        ctx_for_hook.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(-10_000.0, -10_000.0)));
                        ctx_for_hook.request_repaint();
                    }
                    _ => {}
                };
                if let Err(_) = listen(callback) {}
            });

            Box::new(HoverDictApp {
                shared_state,
                _tray_icon: tray_icon,
                is_first_frame: true,
                last_visible: false,
            })
        }),
    )
}
