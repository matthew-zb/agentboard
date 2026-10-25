//! WebView2 settings: turn off browser accelerator keys (Ctrl+R, Ctrl+P, F5, ...) and the
//! default context menu so the keys reach the terminal / app key router.
//!
//! DevTools policy (R9): accelerators stay disabled in every build (otherwise Ctrl+R/F5
//! would reload the app in dev and diverge from release behaviour). F12 is an accelerator
//! and therefore dead; in debug builds the default context menu is kept (right-click ->
//! "Inspect"), and `AGENTBOARD_DEVTOOLS=1` opens the DevTools window at startup.
//! Release builds disable the context menu too.

/// Whether the main WebView may navigate to `url`: only the app's own pages (Windows serves
/// them from `http(s)://tauri.localhost`, other platforms from `tauri://localhost`) or, in
/// development, the configured dev server origin. Everything else — document links, image
/// maps, `file:`, remote sites — is refused; external http(s) links go through
/// `doc_open_url` (system browser) only.
pub fn nav_allowed(url: &tauri::Url, dev: Option<&tauri::Url>) -> bool {
    let host = url.host_str().unwrap_or("");
    let app = match url.scheme() {
        "tauri" => host == "localhost",
        "http" | "https" => host == "tauri.localhost" && url.port().is_none(),
        _ => false,
    };
    app || dev.is_some_and(|d| d.origin() == url.origin() && matches!(url.scheme(), "http" | "https"))
}

/// Plugin whose only job is the navigation allowlist ([`nav_allowed`]) for every webview.
pub fn nav_guard<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("nav-guard")
        .on_navigation(|wv, url| {
            let dev = tauri::Manager::config(wv).build.dev_url.clone();
            let ok = nav_allowed(url, dev.as_ref());
            if !ok {
                eprintln!("[agentboard] blocked navigation: {}", url.scheme());
            }
            ok
        })
        .build()
}

#[cfg(windows)]
pub fn disable_browser_accelerators(window: &tauri::WebviewWindow) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
    use webview2_com::NewWindowRequestedEventHandler;
    use windows::core::Interface;

    let res = window.with_webview(|wv| unsafe {
        let apply = || -> windows::core::Result<()> {
            let core = wv.controller().CoreWebView2()?;
            let settings = core.Settings()?;
            // debug builds keep the context menu so "Inspect" stays reachable
            settings.SetAreDefaultContextMenusEnabled(cfg!(debug_assertions))?;
            let s3: ICoreWebView2Settings3 = settings.cast()?;
            s3.SetAreBrowserAcceleratorKeysEnabled(false)?;
            // no new windows ever (window.open, target=_blank, Shift/middle click on a link):
            // marking the request handled without a new window cancels it
            let handler = NewWindowRequestedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    args.SetHandled(true)?;
                }
                Ok(())
            }));
            let mut token = 0i64;
            core.add_NewWindowRequested(&handler, &mut token)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_allowlist_is_the_app_origin_only() {
        let u = |s: &str| tauri::Url::parse(s).unwrap();
        for ok in ["http://tauri.localhost/", "https://tauri.localhost/index.html", "tauri://localhost/index.html"] {
            assert!(nav_allowed(&u(ok), None), "{ok}");
        }
        for bad in [
            "https://evil.example/",
            "http://tauri.localhost.evil.example/",
            "http://evil.example/?tauri.localhost",
            "http://tauri.localhost:8080/",
            "file:///C:/x.html",
            "file://attacker/share/x",
            "http://asset.localhost/x",
            "javascript:alert(1)",
            "about:blank",
            "data:text/html,x",
            "tauri://evil/",
        ] {
            assert!(!nav_allowed(&u(bad), None), "{bad}");
        }
        let dev = u("http://localhost:1420");
        assert!(nav_allowed(&u("http://localhost:1420/x"), Some(&dev)));
        assert!(!nav_allowed(&u("http://localhost:1421/x"), Some(&dev)));
        assert!(!nav_allowed(&u("https://evil.example/"), Some(&dev)));
    }
}
