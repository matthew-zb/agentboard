#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // `agentboard.exe --hook <Event>` (Claude Code hooks of role sessions): no Tauri/WebView.
    if let Some(event) = app_lib::hookcli::hook_event(&args) {
        std::process::exit(app_lib::hookcli::run(&event));
    }
    // `agentboard.exe --open-url <url>` (a document link, started outside the app's job so the
    // browser outlives the app): no Tauri/WebView.
    if let Some(url) = app_lib::docs::open_url_arg(&args) {
        std::process::exit(app_lib::run_open_url(&url));
    }
    app_lib::run()
}
