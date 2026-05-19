/// Returns the HWND of the current foreground window. Called before
/// recording starts so we can restore focus to the right window after paste.
pub fn get_focused_hwnd() -> usize {
    #[cfg(target_os = "windows")]
    {
        extern "system" { fn GetForegroundWindow() -> usize; }
        let hwnd = unsafe { GetForegroundWindow() };
        println!("[Typr Paste] Saved HWND: {}", hwnd);
        return hwnd;
    }
    0
}

pub fn paste_text(text: &str, restore_hwnd: usize) -> Result<(), String> {
    println!("[Typr Paste] paste_text called — len={} restore_hwnd={}", text.len(), restore_hwnd);

    // Set clipboard
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard.set_text(text).map_err(|e| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_millis(50));

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("osascript")
            .args(["-e", r#"tell application "System Events" to keystroke "v" using command down"#])
            .output()
            .map_err(|e| format!("Failed to simulate paste: {}", e))?;
    }

    #[cfg(target_os = "windows")]
    {
        use enigo::{Direction, Enigo, Key, Keyboard, Settings};

        extern "system" {
            fn GetForegroundWindow() -> usize;
            fn SetForegroundWindow(hwnd: usize) -> i32;
        }

        std::thread::sleep(std::time::Duration::from_millis(100));

        if restore_hwnd != 0 {
            let before = unsafe { GetForegroundWindow() };
            println!("[Typr Paste] Foreground before restore: {}", before);

            let ok = unsafe { SetForegroundWindow(restore_hwnd) };
            println!("[Typr Paste] SetForegroundWindow({}) -> {}", restore_hwnd, ok);

            std::thread::sleep(std::time::Duration::from_millis(150));

            let after = unsafe { GetForegroundWindow() };
            println!("[Typr Paste] Foreground after restore: {} (wanted {})", after, restore_hwnd);
        }

        println!("[Typr Paste] Sending Ctrl+V");
        let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
        enigo
            .key(Key::Control, Direction::Press)
            .map_err(|e| e.to_string())?;
        enigo
            .key(Key::Unicode('v'), Direction::Click)
            .map_err(|e| e.to_string())?;
        enigo
            .key(Key::Control, Direction::Release)
            .map_err(|e| e.to_string())?;
        println!("[Typr Paste] Done");
    }

    Ok(())
}
