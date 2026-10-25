#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `agentboard.exe --hook <Event>` (Claude Code hooks of role sessions): no Tauri/WebView.
    let args: Vec<String> = std::env::args().collect();
    if let Some(event) = app_lib::hookcli::hook_event(&args) {
        std::process::exit(app_lib::hookcli::run(&event));
    }
    app_lib::run()
}
