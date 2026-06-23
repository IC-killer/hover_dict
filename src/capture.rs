use arboard::Clipboard;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(not(target_os = "macos"))]
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

#[cfg(target_os = "macos")]
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

fn try_open_clipboard() -> Option<Clipboard> {
    for _ in 0..3 {
        if let Ok(cb) = Clipboard::new() {
            return Some(cb);
        }
        thread::sleep(Duration::from_millis(50));
    }
    None
}

fn try_set_clipboard_text(clipboard: &mut Clipboard, text: &str) -> bool {
    for _ in 0..3 {
        if clipboard.set_text(text.to_string()).is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

fn try_get_clipboard_text(clipboard: &mut Clipboard) -> Option<String> {
    for _ in 0..3 {
        match clipboard.get_text() {
            Ok(t) => return Some(t),
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
    None
}

pub fn capture_selected_text() -> Option<String> {
    let mut clipboard = try_open_clipboard()?;
    let backup_text = try_get_clipboard_text(&mut clipboard).unwrap_or_default();
    let backup_image = clipboard.get_image().ok();

    let mut enigo = Enigo::new(&Settings::default()).ok()?;
    let sentinel = format!(
        "__HOVER_DICT_CLIPBOARD_SENTINEL_{}_{}__",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    let sentinel_written = try_set_clipboard_text(&mut clipboard, &sentinel);

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

    // 增加到 10 次轮询 × 80ms = 800ms，给目标应用更多时间将文本放入剪贴板
    let mut selected_text = String::new();
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(80));
        if let Some(text) = try_get_clipboard_text(&mut clipboard) {
            let changed = if sentinel_written {
                text != sentinel
            } else {
                text != backup_text
            };
            if changed && !text.trim().is_empty() {
                selected_text = text;
                break;
            }
        }
    }

    // 恢复剪贴板
    if let Some(img) = backup_image {
        let _ = clipboard.set_image(img);
    } else if !backup_text.is_empty() {
        let _ = clipboard.set_text(&backup_text);
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
