use arboard::Clipboard;
use std::thread;
use std::time::Duration;

#[cfg(not(target_os = "macos"))]
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

#[cfg(target_os = "macos")]
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

pub fn capture_selected_text() -> Option<String> {
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
    if trimmed.is_empty() || trimmed.len() > 150 || trimmed.contains('\n') {
        None
    } else {
        Some(trimmed)
    }
}
