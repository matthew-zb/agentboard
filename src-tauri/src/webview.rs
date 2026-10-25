//! WebView2 settings: turn off browser accelerator keys (Ctrl+R, Ctrl+P, F5, ...) and the
//! default context menu so the keys reach the terminal / app key router.
//!
//! DevTools policy (R9): accelerators stay disabled in every build (otherwise Ctrl+R/F5
//! would reload the app in dev and diverge from release behaviour). F12 is an accelerator
//! and therefore dead; in debug builds the default context menu is kept (right-click ->
//! "Inspect"), and `AGENTBOARD_DEVTOOLS=1` opens the DevTools window at startup.
//! Release builds disable the context menu too.

#[cfg(windows)]
pub fn disable_browser_accelerators(window: &tauri::WebviewWindow) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
    use windows::core::Interface;

    let res = window.with_webview(|wv| unsafe {
        let apply = || -> windows::core::Result<()> {
            let core = wv.controller().CoreWebView2()?;
            let settings = core.Settings()?;
            // debug builds keep the context menu so "Inspect" stays reachable
            settings.SetAreDefaultContextMenusEnabled(cfg!(debug_assertions))?;
            let s3: ICoreWebView2Settings3 = settings.cast()?;
            s3.SetAreBrowserAcceleratorKeysEnabled(false)?;
            Ok(())
        };
        if let Err(e) = apply() {
            eprintln!("[agentboard] webview settings failed: {e}");
        }
    });
    if let Err(e) = res {
        eprintln!("[agentboard] with_webview failed: {e}");
    }
}

/// Debug builds only: `AGENTBOARD_DEVTOOLS=1` opens the DevTools window.
pub fn maybe_open_devtools(window: &tauri::WebviewWindow) {
    #[cfg(debug_assertions)]
    if std::env::var("AGENTBOARD_DEVTOOLS").as_deref() == Ok("1") {
        window.open_devtools();
    }
    #[cfg(not(debug_assertions))]
    let _ = window;
}

#[cfg(not(windows))]
pub fn disable_browser_accelerators(_window: &tauri::WebviewWindow) {}
