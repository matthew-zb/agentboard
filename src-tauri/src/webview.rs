//! WebView2 settings: turn off browser accelerator keys (Ctrl+R, Ctrl+P, F5, ...) and the
//! default context menu so the keys reach the terminal / app key router.

#[cfg(windows)]
pub fn disable_browser_accelerators(window: &tauri::WebviewWindow) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
    use windows::core::Interface;

    let res = window.with_webview(|wv| unsafe {
        let apply = || -> windows::core::Result<()> {
            let core = wv.controller().CoreWebView2()?;
            let settings = core.Settings()?;
            settings.SetAreDefaultContextMenusEnabled(false)?;
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

#[cfg(not(windows))]
pub fn disable_browser_accelerators(_window: &tauri::WebviewWindow) {}
