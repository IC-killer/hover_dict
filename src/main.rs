#![windows_subsystem = "windows"]

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
use translator::{LocalSqliteDict, LlmTranslator, ModelsConfig, TranslateResult};
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
};

#[cfg(windows)]
fn hide_window_from_taskbar(frame: &eframe::Frame) {
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_APPWINDOW,
        WS_EX_TOOLWINDOW, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos,
    };

    let Ok(handle) = frame.window_handle() else { return };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return };
    let hwnd = handle.hwnd.get();

    unsafe {
        let mut ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        ex_style |= WS_EX_TOOLWINDOW as isize;
        ex_style &= !(WS_EX_APPWINDOW as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style);
        SetWindowPos(
            hwnd,
            0,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
        );
    }
}

static MOUSE_X: AtomicI32 = AtomicI32::new(0);
static MOUSE_Y: AtomicI32 = AtomicI32::new(0);

pub fn show_notify(title: &str, body: &str) {
    let _ = Notification::new().summary(title).body(body).show();
}

fn get_cat_icon() -> Icon {
    let icon_bytes = include_bytes!("../icon.ico");
    let image = image::load_from_memory(icon_bytes)
        .expect("Failed to open icon path")
        .into_rgba8();

    let (width, height) = image.dimensions();
    let rgba = image.into_raw();

    Icon::from_rgba(rgba, width, height).unwrap()
}

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
    pending_pos: Option<(i32, i32)>,
    shown_at: Option<Instant>,
}

struct HoverDictApp {
    shared_state: Arc<Mutex<SharedState>>,
    is_capture_enabled: Arc<Mutex<bool>>,
    is_llm_enabled: Arc<Mutex<bool>>,
    _tray_icon: tray_icon::TrayIcon,
    _tray_menu_state: TrayMenuState,
    is_first_frame: bool,
    last_visible: bool,
}

impl eframe::App for HoverDictApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id.0.as_str();
            if id == "quit" {
                std::process::exit(0);
            } else if id == "toggle_capture" {
                let mut enabled = self.is_capture_enabled.lock().unwrap();
                *enabled = !*enabled;
                show_notify("划词翻译", if *enabled { "已开启" } else { "已关闭" });
                
                let config = ModelsConfig::load();
                let menu_state = build_tray_menu(*enabled, *self.is_llm_enabled.lock().unwrap(), &config);
                self._tray_icon.set_menu(Some(Box::new(menu_state.menu.clone())));
                self._tray_menu_state = menu_state;
            } else if id == "toggle_llm" {
                let mut enabled = self.is_llm_enabled.lock().unwrap();
                *enabled = !*enabled;
                show_notify("大模型翻译", if *enabled { "已开启" } else { "已关闭" });
                
                let config = ModelsConfig::load();
                let menu_state = build_tray_menu(*self.is_capture_enabled.lock().unwrap(), *enabled, &config);
                self._tray_icon.set_menu(Some(Box::new(menu_state.menu.clone())));
                self._tray_menu_state = menu_state;
            } else if id.starts_with("model_") {
                let selected_id = id.trim_start_matches("model_");
                let mut config = ModelsConfig::load();
                config.active_model = selected_id.to_string();
                config.save();
                
                let menu_state = build_tray_menu(*self.is_capture_enabled.lock().unwrap(), *self.is_llm_enabled.lock().unwrap(), &config);
                self._tray_icon.set_menu(Some(Box::new(menu_state.menu.clone())));
                self._tray_menu_state = menu_state;
            }
        }

        if self.is_first_frame {
            #[cfg(windows)]
            hide_window_from_taskbar(frame);

            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1.0, 1.0)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                -10_000.0, -10_000.0,
            )));
            self.is_first_frame = false;
            self.last_visible = false;
        }

        #[cfg(windows)]
        hide_window_from_taskbar(frame);

        let (pending_pos, is_window_visible, current_result, _shown_at) = {
            let mut state = self.shared_state.lock().unwrap();
            (
                state.pending_pos.take(),
                state.is_window_visible,
                state.current_result.clone(),
                state.shown_at,
            )
        };

        if is_window_visible && !self.last_visible {
            if let Some((px, py)) = pending_pos {
                let ppp = ctx.pixels_per_point();
                let x = px as f32 / ppp;
                let y = py as f32 / ppp;
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, y)));
            }
            let size = if current_result.as_ref().map(|r| r.is_llm).unwrap_or(false) {
                egui::vec2(400.0, 300.0)
            } else {
                egui::vec2(300.0, 200.0)
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }

        if !is_window_visible && self.last_visible {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1.0, 1.0)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                -10_000.0, -10_000.0,
            )));
        }

        if is_window_visible {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(egui::Color32::from_rgb(250, 250, 250))
                        .rounding(8.0),
                )
                .show(ctx, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if ui.button("×").clicked() {
                            if let Ok(mut st) = self.shared_state.lock() {
                                st.is_window_visible = false;
                            }
                        }
                    });

                    if let Some(res) = &current_result {
                        if res.is_llm {
                            ui.heading(
                                egui::RichText::new("大模型翻译").color(egui::Color32::from_rgb(0, 100, 200)),
                            );
                            ui.separator();
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(&res.translation)
                                        .size(15.0)
                                        .color(egui::Color32::from_rgb(40, 40, 40)),
                                );
                            });
                        } else {
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
                    }
                });
        }

        self.last_visible = is_window_visible;
        
        // Ensure continuous polling for menu events
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

struct TrayMenuState {
    pub menu: Menu,
    _toggle_capture: CheckMenuItem,
    _toggle_llm: CheckMenuItem,
    _model_menu: Submenu,
    _model_items: Vec<MenuItem>,
    _separator: PredefinedMenuItem,
    _quit_item: MenuItem,
}

fn build_tray_menu(is_capture_enabled: bool, is_llm_enabled: bool, config: &ModelsConfig) -> TrayMenuState {
    let tray_menu = Menu::new();
    let toggle_capture = CheckMenuItem::with_id("toggle_capture", "开启划词翻译", true, is_capture_enabled, None);
    let toggle_llm = CheckMenuItem::with_id("toggle_llm", "开启大模型翻译", true, is_llm_enabled, None);
    
    let mut model_items = Vec::new();
    let model_menu = Submenu::new("选择模型", true);
    for model in &config.models {
        let is_active = model.id == config.active_model;
        let prefix = if is_active { "√ " } else { "  " };
        let item = MenuItem::with_id(
            format!("model_{}", model.id), 
            format!("{}{}", prefix, model.name), 
            true, 
            None
        );
        let _ = model_menu.append(&item);
        model_items.push(item);
    }

    let quit_item = MenuItem::with_id("quit", "彻底退出", true, None);
    let separator = PredefinedMenuItem::separator();
    
    let _ = tray_menu.append_items(&[
        &toggle_capture, 
        &toggle_llm, 
        &model_menu, 
        &separator, 
        &quit_item
    ]);
    
    TrayMenuState {
        menu: tray_menu,
        _toggle_capture: toggle_capture,
        _toggle_llm: toggle_llm,
        _model_menu: model_menu,
        _model_items: model_items,
        _separator: separator,
        _quit_item: quit_item,
    }
}

fn main() -> eframe::Result<()> {
    if !std::path::Path::new("dict.db").exists() {
        show_notify("启动失败", "找不到 dict.db");
        std::process::exit(1);
    }

    let is_capture_enabled = Arc::new(Mutex::new(true));
    let is_llm_enabled = Arc::new(Mutex::new(true));
    let shared_state = Arc::new(Mutex::new(SharedState {
        current_result: None,
        is_window_visible: false,
        pending_pos: None,
        shown_at: None,
    }));

    let config = ModelsConfig::load();
    let tray_menu_state = build_tray_menu(true, true, &config);

    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu_state.menu.clone()))
        .with_icon(get_cat_icon())
        .build()
        .unwrap();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_always_on_top()
            .with_transparent(true)
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

            let (capture_tx, capture_rx) = unbounded::<(i32, i32)>();

            let state_clone = Arc::clone(&shared_state);
            let is_llm_for_thread = Arc::clone(&is_llm_enabled);
            
            thread::spawn(move || {
                let dict = LocalSqliteDict::new("dict.db");

                while let Ok((up_x, up_y)) = capture_rx.recv() {
                    if let Some(text) = capture_selected_text() {
                        let config = ModelsConfig::load();
                        let llm_enabled = *is_llm_for_thread.lock().unwrap();
                        
                        let word_count = text.split_whitespace().count();
                        let has_punct = text.chars().any(|c| c.is_ascii_punctuation() || "，。！？；：".contains(c));
                        let is_sentence = word_count >= 3 || has_punct;
                        
                        let mut final_res = None;
                        
                        if !is_sentence {
                            // 短文本：先查本地词典
                            if let Ok(Some(res)) = dict.translate(&text) {
                                final_res = Some(res);
                            } else if llm_enabled {
                                // 本地查词失败，且大模型已开启，则走大模型重试
                                match LlmTranslator::translate(&text, &config) {
                                    Ok(Some(res)) => final_res = Some(res),
                                    Err(e) => show_notify("大模型重试失败", &e.to_string()),
                                    _ => {}
                                }
                            }
                        } else {
                            // 长文本（句子/段落）：优先走大模型
                            if llm_enabled {
                                match LlmTranslator::translate(&text, &config) {
                                    Ok(Some(res)) => final_res = Some(res),
                                    Err(e) => {
                                        show_notify("大模型查询失败", &e.to_string());
                                        // 失败后回退查本地词典
                                        if let Ok(Some(res)) = dict.translate(&text) {
                                            final_res = Some(res);
                                        }
                                    }
                                    _ => {}
                                }
                            } else {
                                // 大模型未开启，强行查本地词典
                                if let Ok(Some(res)) = dict.translate(&text) {
                                    final_res = Some(res);
                                }
                            }
                        }
                        
                        if let Some(res) = final_res {
                            let mut st = state_clone.lock().unwrap();
                            st.current_result = Some(res);
                            st.is_window_visible = true;
                            st.pending_pos = Some((up_x + 12, up_y + 12));
                            st.shown_at = Some(Instant::now());
                            drop(st);
                            ctx_clone.request_repaint();
                        } else {
                            show_notify("查询结果", "翻译失败或词库中没有这个词");
                        }
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
                    EventType::KeyPress(rdev::Key::Escape) => {
                        if let Ok(mut st) = state_for_hook.lock() {
                            st.is_window_visible = false;
                        }
                        ctx_for_hook.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                            egui::vec2(1.0, 1.0),
                        ));
                        ctx_for_hook.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                            egui::pos2(-10_000.0, -10_000.0),
                        ));
                        ctx_for_hook.request_repaint();
                    }
                    _ => {}
                };
                if let Err(_) = listen(callback) {}
            });

            Box::new(HoverDictApp {
                shared_state,
                is_capture_enabled,
                is_llm_enabled,
                _tray_icon: tray_icon,
                _tray_menu_state: tray_menu_state,
                is_first_frame: true,
                last_visible: false,
            })
        }),
    )
}
