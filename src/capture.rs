use arboard::Clipboard;
use std::thread;
use std::time::Duration;

#[cfg(not(target_os = "macos"))]
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

#[cfg(target_os = "macos")]
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

pub fn capture_selected_text() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorInfo, CURSORINFO, IDC_IBEAM, LoadCursorW};
        let mut ci: CURSORINFO = unsafe { std::mem::zeroed() };
        ci.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
        if unsafe { GetCursorInfo(&mut ci) } != 0 {
            let ibeam = unsafe { LoadCursorW(0 as _, IDC_IBEAM) };
            if ci.hCursor != ibeam {
                return None; // 仅在光标为文本选择模式（I-beam）时抓取，避免干扰截图工具
            }
        }
    }

    let mut clipboard = Clipboard::new().ok()?;
    let backup_text = clipboard.get_text().unwrap_or_default();
    let backup_image = clipboard.get_image().ok();

    let mut enigo = Enigo::new(&Settings::default()).ok()?;

    thread::sleep(Duration::from_millis(50));

    #[cfg(target_os = "macos")]
    {
        let _ = enigo.key(Key::Meta, Direction::Press);
        thread::sleep(Duration::from_millis(20));
        let _ = enigo.key(Key::Unicode('c'), Direction::Click);
        thread::sleep(Duration::from_millis(20));
        let _ = enigo.key(Key::Meta, Direction::Release);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = enigo.key(Key::Control, Direction::Press);
        thread::sleep(Duration::from_millis(20));
        // Windows 上用真实按键（Key::C）更稳，避免 Unicode 路径触发奇怪的修饰键行为（如 Alt 菜单提示）。
        #[cfg(target_os = "windows")]
        let _ = enigo.key(Key::C, Direction::Click);
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        let _ = enigo.key(Key::Unicode('c'), Direction::Click);
        thread::sleep(Duration::from_millis(20));
        let _ = enigo.key(Key::Control, Direction::Release);
    }

    let mut selected_text = String::new();
    for _ in 0..6 {
        thread::sleep(Duration::from_millis(50));
        if let Ok(text) = clipboard.get_text() {
            if text != backup_text && !text.is_empty() {
                selected_text = text;
                break;
            }
        }
    }

    // 恢复剪贴板
    if let Some(img) = backup_image {
        let _ = clipboard.set_image(img);
    } else if !backup_text.is_empty() {
        let _ = clipboard.set_text(backup_text);
    } else {
        let _ = clipboard.clear();
    }

    let trimmed = selected_text.trim().to_string();
    if trimmed.is_empty() || trimmed.len() > 2000 {
        None
    } else {
        Some(trimmed)
    }
}
