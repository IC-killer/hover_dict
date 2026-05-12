#![windows_subsystem = "windows"]

mod capture;
mod translator;

use capture::capture_selected_text;
use crossbeam_channel::unbounded;
use eframe::egui;
use notify_rust::Notification;
use rdev::{listen, Event, EventType};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;
use translator::{LlmTranslator, LocalSqliteDict, ModelsConfig, TranslateResult};
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    Icon, TrayIconBuilder,
};

#[cfg(windows)]
fn hide_window_from_taskbar(frame: &eframe::Frame) {
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, SWP_FRAMECHANGED,
        SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    };

    let Ok(handle) = frame.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
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
const POPUP_GAP_PX: i32 = 14;
const SCREEN_MARGIN_PX: i32 = 12;

pub fn show_notify(title: &str, body: &str) {
    let _ = Notification::new().summary(title).body(body).show();
}

#[derive(Clone, Copy)]
struct WorkArea {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl WorkArea {
    fn width(self) -> i32 {
        self.right - self.left
    }

    fn height(self) -> i32 {
        self.bottom - self.top
    }
}

#[cfg(windows)]
fn work_area_for_point(x: i32, y: i32) -> Option<WorkArea> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };

    let monitor = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
    if monitor == 0 {
        return None;
    }

    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;

    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }

    Some(WorkArea {
        left: info.rcWork.left,
        top: info.rcWork.top,
        right: info.rcWork.right,
        bottom: info.rcWork.bottom,
    })
}

#[cfg(not(windows))]
fn work_area_for_point(_x: i32, _y: i32) -> Option<WorkArea> {
    None
}

fn popup_size_for_result(result: Option<&TranslateResult>) -> egui::Vec2 {
    match result {
        Some(res) if res.is_error => egui::vec2(460.0, 300.0),
        Some(res) if res.is_llm => egui::vec2(460.0, 340.0),
        Some(_) => egui::vec2(360.0, 240.0),
        None => egui::vec2(320.0, 220.0),
    }
}

fn adjusted_popup_layout(
    anchor_x: i32,
    anchor_y: i32,
    desired_size: egui::Vec2,
    pixels_per_point: f32,
) -> (egui::Vec2, egui::Pos2) {
    let ppp = pixels_per_point.max(0.1);
    let mut width_px = (desired_size.x * ppp).round() as i32;
    let mut height_px = (desired_size.y * ppp).round() as i32;

    if let Some(work_area) = work_area_for_point(anchor_x, anchor_y) {
        width_px = width_px.min((work_area.width() - SCREEN_MARGIN_PX * 2).max(240));
        height_px = height_px.min((work_area.height() - SCREEN_MARGIN_PX * 2).max(160));

        let min_x = work_area.left + SCREEN_MARGIN_PX;
        let max_x = work_area.right - width_px - SCREEN_MARGIN_PX;
        let min_y = work_area.top + SCREEN_MARGIN_PX;
        let max_y = work_area.bottom - height_px - SCREEN_MARGIN_PX;

        let mut x = anchor_x + POPUP_GAP_PX;
        if x > max_x {
            x = anchor_x - width_px - POPUP_GAP_PX;
        }
        let mut y = anchor_y + POPUP_GAP_PX;
        if y > max_y {
            y = anchor_y - height_px - POPUP_GAP_PX;
        }

        let x = x.clamp(min_x, max_x.max(min_x));
        let y = y.clamp(min_y, max_y.max(min_y));
        (
            egui::vec2(width_px as f32 / ppp, height_px as f32 / ppp),
            egui::pos2(x as f32 / ppp, y as f32 / ppp),
        )
    } else {
        (
            desired_size,
            egui::pos2(
                (anchor_x + POPUP_GAP_PX) as f32 / ppp,
                (anchor_y + POPUP_GAP_PX) as f32 / ppp,
            ),
        )
    }
}

fn show_result_window(
    state: &Arc<Mutex<SharedState>>,
    ctx: &egui::Context,
    result: TranslateResult,
    x: i32,
    y: i32,
) {
    let mut st = state.lock().unwrap();
    st.current_result = Some(result);
    st.is_window_visible = true;
    st.pending_pos = Some((x, y));
    st.shown_at = Some(Instant::now());
    drop(st);
    ctx.request_repaint();
}

fn result_theme(result: &TranslateResult) -> (&'static str, &'static str, egui::Color32) {
    if result.is_error {
        (
            "大模型翻译失败",
            "请求未完成",
            egui::Color32::from_rgb(196, 61, 58),
        )
    } else if result.is_llm {
        (
            "大模型翻译",
            "智能翻译",
            egui::Color32::from_rgb(40, 112, 198),
        )
    } else {
        (
            "本地词典",
            "离线词库",
            egui::Color32::from_rgb(43, 132, 101),
        )
    }
}

fn preview_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let mut preview: String = trimmed.chars().take(max_chars).collect();
    if trimmed.chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

fn render_popup_result(
    ui: &mut egui::Ui,
    shared_state: &Arc<Mutex<SharedState>>,
    result: &TranslateResult,
) {
    let (title, subtitle, accent) = result_theme(result);
    let muted = egui::Color32::from_rgb(96, 102, 112);
    let text = egui::Color32::from_rgb(30, 34, 40);
    let subtle_fill = accent.linear_multiply(0.08);

    egui::Frame::none()
        .fill(subtle_fill)
        .inner_margin(egui::Margin::symmetric(14.0, 10.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(title).size(15.5).strong().color(text));
                    ui.label(
                        egui::RichText::new(subtitle)
                            .size(11.5)
                            .color(accent.linear_multiply(0.85)),
                    );
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let close = egui::Button::new(
                        egui::RichText::new("×").size(18.0).strong().color(muted),
                    )
                    .frame(false)
                    .min_size(egui::vec2(28.0, 28.0));
                    if ui.add(close).clicked() {
                        if let Ok(mut st) = shared_state.lock() {
                            st.is_window_visible = false;
                        }
                    }
                });
            });
        });

    egui::Frame::none()
        .inner_margin(egui::Margin::symmetric(14.0, 12.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);

            if !result.is_llm && !result.is_error {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&result.source_text)
                            .size(20.0)
                            .strong()
                            .color(text),
                    )
                    .wrap(true),
                );
                if let Some(phonetic) = &result.phonetic {
                    ui.label(
                        egui::RichText::new(format!("[{}]", phonetic))
                            .size(13.0)
                            .color(muted),
                    );
                }
            } else {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(preview_text(&result.source_text, 120))
                            .size(12.5)
                            .color(muted),
                    )
                    .wrap(true),
                );
            }

            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let body_color = if result.is_error {
                        egui::Color32::from_rgb(112, 38, 38)
                    } else {
                        text
                    };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&result.translation)
                                .size(if result.is_llm { 15.0 } else { 14.0 })
                                .color(body_color),
                        )
                        .wrap(true),
                    );
                });
        });
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

    let has_segoe = if let Ok(font_data) = std::fs::read("C:\\Windows\\Fonts\\segoeui.ttf") {
        fonts
            .font_data
            .insert("segoeui".to_owned(), egui::FontData::from_owned(font_data));
        true
    } else {
        false
    };

    let has_yahei = if let Ok(font_data) = std::fs::read("C:\\Windows\\Fonts\\msyh.ttf") {
        fonts
            .font_data
            .insert("msyh".to_owned(), egui::FontData::from_owned(font_data));
        true
    } else {
        false
    };

    let has_simhei = if !has_yahei {
        if let Ok(font_data) = std::fs::read("C:\\Windows\\Fonts\\simhei.ttf") {
            fonts
                .font_data
                .insert("simhei".to_owned(), egui::FontData::from_owned(font_data));
            true
        } else {
            false
        }
    } else {
        false
    };

    if has_segoe {
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "segoeui".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "segoeui".to_owned());
    }

    if has_yahei {
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .push("msyh".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("msyh".to_owned());
    } else if has_simhei {
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .push("simhei".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("simhei".to_owned());
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
    is_pinned: Arc<Mutex<bool>>,
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
                let menu_state = build_tray_menu(
                    *enabled,
                    *self.is_llm_enabled.lock().unwrap(),
                    *self.is_pinned.lock().unwrap(),
                    &config,
                );
                self._tray_icon
                    .set_menu(Some(Box::new(menu_state.menu.clone())));
                self._tray_menu_state = menu_state;
            } else if id == "toggle_llm" {
                let mut enabled = self.is_llm_enabled.lock().unwrap();
                *enabled = !*enabled;
                show_notify("大模型翻译", if *enabled { "已开启" } else { "已关闭" });

                let config = ModelsConfig::load();
                let menu_state = build_tray_menu(
                    *self.is_capture_enabled.lock().unwrap(),
                    *enabled,
                    *self.is_pinned.lock().unwrap(),
                    &config,
                );
                self._tray_icon
                    .set_menu(Some(Box::new(menu_state.menu.clone())));
                self._tray_menu_state = menu_state;
            } else if id == "toggle_pin" {
                let mut pinned = self.is_pinned.lock().unwrap();
                *pinned = !*pinned;
                show_notify("翻译窗口置顶", if *pinned { "已开启" } else { "已关闭" });

                let config = ModelsConfig::load();
                let menu_state = build_tray_menu(
                    *self.is_capture_enabled.lock().unwrap(),
                    *self.is_llm_enabled.lock().unwrap(),
                    *pinned,
                    &config,
                );
                self._tray_icon
                    .set_menu(Some(Box::new(menu_state.menu.clone())));
                self._tray_menu_state = menu_state;

                // If the window is currently visible, update its window level immediately
                if self.last_visible {
                    let window_level = if *pinned {
                        egui::WindowLevel::AlwaysOnTop
                    } else {
                        egui::WindowLevel::Normal
                    };
                    ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(window_level));
                }
            } else if id.starts_with("model_") {
                let selected_id = id.trim_start_matches("model_");
                let mut config = ModelsConfig::load();
                config.active_model = selected_id.to_string();
                config.save();

                let menu_state = build_tray_menu(
                    *self.is_capture_enabled.lock().unwrap(),
                    *self.is_llm_enabled.lock().unwrap(),
                    *self.is_pinned.lock().unwrap(),
                    &config,
                );
                self._tray_icon
                    .set_menu(Some(Box::new(menu_state.menu.clone())));
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

        let mut is_window_visible = {
            let state = self.shared_state.lock().unwrap();
            state.is_window_visible
        };

        if is_window_visible && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if let Ok(mut st) = self.shared_state.lock() {
                st.is_window_visible = false;
            }
            is_window_visible = false;
        }

        let (pending_pos, current_result, _shown_at) = {
            let mut state = self.shared_state.lock().unwrap();
            (
                state.pending_pos.take(),
                state.current_result.clone(),
                state.shown_at,
            )
        };

        if is_window_visible {
            if !self.last_visible {
                let pinned = *self.is_pinned.lock().unwrap();
                let window_level = if pinned {
                    egui::WindowLevel::AlwaysOnTop
                } else {
                    egui::WindowLevel::Normal
                };
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(window_level));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }

            if let Some((px, py)) = pending_pos {
                let desired_size = popup_size_for_result(current_result.as_ref());
                let (size, position) =
                    adjusted_popup_layout(px, py, desired_size, ctx.pixels_per_point());
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(position));
            }
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
                    egui::Frame::none()
                        .fill(egui::Color32::TRANSPARENT)
                        .inner_margin(egui::Margin::same(10.0)),
                )
                .show(ctx, |ui| {
                    let card_size = ui.available_size();
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(255, 255, 255))
                        .stroke(egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgb(218, 224, 232),
                        ))
                        .rounding(8.0)
                        .shadow(egui::epaint::Shadow {
                            extrusion: 18.0,
                            color: egui::Color32::from_black_alpha(42),
                        })
                        .inner_margin(egui::Margin::same(0.0))
                        .show(ui, |ui| {
                            ui.set_min_size(card_size);
                            if let Some(res) = &current_result {
                                render_popup_result(ui, &self.shared_state, res);
                            }
                        });
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
    _toggle_pin: CheckMenuItem,
    _model_menu: Submenu,
    _model_items: Vec<MenuItem>,
    _separator: PredefinedMenuItem,
    _quit_item: MenuItem,
}

fn build_tray_menu(
    is_capture_enabled: bool,
    is_llm_enabled: bool,
    is_pinned: bool,
    config: &ModelsConfig,
) -> TrayMenuState {
    let tray_menu = Menu::new();
    let toggle_capture = CheckMenuItem::with_id(
        "toggle_capture",
        "开启划词翻译",
        true,
        is_capture_enabled,
        None,
    );
    let toggle_llm =
        CheckMenuItem::with_id("toggle_llm", "开启大模型翻译", true, is_llm_enabled, None);
    let toggle_pin = CheckMenuItem::with_id("toggle_pin", "翻译窗口置顶", true, is_pinned, None);

    let mut model_items = Vec::new();
    let model_menu = Submenu::new("选择模型", true);
    for model in &config.models {
        let is_active = model.id == config.active_model;
        let prefix = if is_active { "√ " } else { "  " };
        let item = MenuItem::with_id(
            format!("model_{}", model.id),
            format!("{}{}", prefix, model.name),
            true,
            None,
        );
        let _ = model_menu.append(&item);
        model_items.push(item);
    }

    let quit_item = MenuItem::with_id("quit", "彻底退出", true, None);
    let separator = PredefinedMenuItem::separator();

    let _ = tray_menu.append_items(&[
        &toggle_capture,
        &toggle_llm,
        &toggle_pin,
        &model_menu,
        &separator,
        &quit_item,
    ]);

    TrayMenuState {
        menu: tray_menu,
        _toggle_capture: toggle_capture,
        _toggle_llm: toggle_llm,
        _toggle_pin: toggle_pin,
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
    let is_pinned = Arc::new(Mutex::new(false));
    let shared_state = Arc::new(Mutex::new(SharedState {
        current_result: None,
        is_window_visible: false,
        pending_pos: None,
        shown_at: None,
    }));

    let config = ModelsConfig::load();
    let tray_menu_state = build_tray_menu(true, true, false, &config);

    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu_state.menu.clone()))
        .with_icon(get_cat_icon())
        .build()
        .unwrap();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
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
                        let has_punct = text
                            .chars()
                            .any(|c| c.is_ascii_punctuation() || "，。！？；：".contains(c));
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
                                    Ok(None) => {
                                        final_res = Some(TranslateResult::error(
                                            &text,
                                            "大模型没有返回翻译结果",
                                        ));
                                    }
                                    Err(e) => {
                                        final_res =
                                            Some(TranslateResult::error(&text, e.to_string()));
                                    }
                                }
                            }
                        } else {
                            // 长文本（句子/段落）：优先走大模型
                            if llm_enabled {
                                match LlmTranslator::translate(&text, &config) {
                                    Ok(Some(res)) => final_res = Some(res),
                                    Ok(None) => {
                                        if let Ok(Some(res)) = dict.translate(&text) {
                                            final_res = Some(res);
                                        } else {
                                            final_res = Some(TranslateResult::error(
                                                &text,
                                                "大模型没有返回翻译结果",
                                            ));
                                        }
                                    }
                                    Err(e) => {
                                        let error_message = e.to_string();
                                        if let Ok(Some(res)) = dict.translate(&text) {
                                            final_res = Some(res);
                                        } else {
                                            final_res =
                                                Some(TranslateResult::error(&text, error_message));
                                        }
                                    }
                                }
                            } else {
                                // 大模型未开启，强行查本地词典
                                if let Ok(Some(res)) = dict.translate(&text) {
                                    final_res = Some(res);
                                }
                            }
                        }

                        if let Some(res) = final_res {
                            if res.is_error {
                                show_notify("大模型翻译失败", &res.translation);
                            }
                            show_result_window(&state_clone, &ctx_clone, res, up_x, up_y);
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
                is_pinned,
                _tray_icon: tray_icon,
                _tray_menu_state: tray_menu_state,
                is_first_frame: true,
                last_visible: false,
            })
        }),
    )
}
