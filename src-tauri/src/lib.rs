// 이후 태스크에서 pub mod 추가: ring, pty, winjob, layout, config, profiles,
// workspace, launcher, persist, webview

pub mod ring;
pub mod pty;
#[cfg(windows)]
pub mod winjob;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .run(tauri::generate_context!())
        .expect("error while running AgentBoard");
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_builds() {}
}
