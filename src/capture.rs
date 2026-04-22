use arboard::Clipboard;
use enigo::{Enigo, Key, KeyboardControllable};
use std::thread;
use std::time::Duration;

pub fn capture_selected_text() -> Option<String> {
    let mut clipboard = Clipboard::new().ok()?;

    let backup_text = clipboard.get_text().unwrap_or_default();
    let backup_image = clipboard.get_image().ok();

    let mut enigo = Enigo::new();

    // 给系统一点时间反应用户的鼠标抬起动作
    thread::sleep(Duration::from_millis(50));

    // 稳健地发送 Ctrl + C
    #[cfg(target_os = "macos")]
    {
        enigo.key_down(Key::Meta);
        thread::sleep(Duration::from_millis(20));
        enigo.key_click(Key::Layout('c'));
        thread::sleep(Duration::from_millis(20));
        enigo.key_up(Key::Meta);
    }
    #[cfg(not(target_os = "macos"))]
    {
        enigo.key_down(Key::Control);
        thread::sleep(Duration::from_millis(20));
        enigo.key_click(Key::Layout('c'));
        thread::sleep(Duration::from_millis(20));
        enigo.key_up(Key::Control);
    }

    // 【智能重试机制】：最多等待 300ms (6次 * 50ms)
    let mut selected_text = String::new();
    for _ in 0..6 {
        thread::sleep(Duration::from_millis(50));
        if let Ok(text) = clipboard.get_text() {
            // 如果剪贴板内容变化了，说明复制成功！
            if text != backup_text && !text.is_empty() {
                selected_text = text;
                break;
            }
        }
    }

    // 恢复剪贴板原状
    if let Some(img) = backup_image {
        let _ = clipboard.set_image(img);
    } else if !backup_text.is_empty() {
        let _ = clipboard.set_text(backup_text);
    } else {
        let _ = clipboard.clear();
    }

    let trimmed = selected_text.trim();
    if trimmed.is_empty() || trimmed.len() > 150 || trimmed.contains('\n') {
        None
    } else {
        Some(trimmed.to_string())
    }
}
