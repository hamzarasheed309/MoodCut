//! The editor window.
//!
//! On Windows the web GUI runs inside MoodCut's own native window (an embedded WebView2 view
//! hosted by `tao`), in the same process as the engine: one program with one icon, one taskbar
//! button and one title bar, like Premiere Pro, not a tab in a browser. It remembers its size
//! and position, shows the project name in the title bar, asks before quitting with unsaved
//! changes or a running render, and keeps links and stray drops from navigating away from the
//! editor. If WebView2 can't start, the caller falls back to a browser app window.

#[cfg(windows)]
pub use imp::{allow_foreground, focus, run};

/// Not Windows: there is no native window, so the caller opens a browser app window.
#[cfg(not(windows))]
pub fn run(_url: &str, _port: u16) -> anyhow::Error {
    anyhow::anyhow!("the native editor window is only built for Windows")
}

#[cfg(not(windows))]
pub fn focus() -> bool {
    false
}

#[cfg(not(windows))]
pub fn allow_foreground() {}

#[cfg(windows)]
mod imp {
    use anyhow::Result;
    use serde::{Deserialize, Serialize};
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use tao::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
    use tao::event::{Event, StartCause, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
    use tao::platform::windows::{IconExtWindows, WindowBuilderExtWindows, WindowExtWindows};
    use tao::window::{Icon, Theme, Window, WindowBuilder};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, MessageBoxW, ASFW_ANY, IDNO, IDYES, MB_ICONERROR, MB_ICONQUESTION, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_YESNO, MB_YESNOCANCEL,
    };
    use wry::{NewWindowResponse, PageLoadEvent, WebContext, WebViewBuilder, WebViewBuilderExtWindows};

    /// What other threads (the HTTP server) can ask the window to do.
    enum Msg {
        Focus,
    }

    /// Set while the window's event loop runs, so a second launch can bring this window forward.
    static PROXY: Mutex<Option<EventLoopProxy<Msg>>> = Mutex::new(None);

    /// Brings the open editor window to the front. False when this process has no native window.
    pub fn focus() -> bool {
        match PROXY.lock() {
            Ok(g) => g.as_ref().is_some_and(|p| p.send_event(Msg::Focus).is_ok()),
            Err(_) => false,
        }
    }

    /// Lets the editor that is already open come to the front when this (just launched, so
    /// foreground-entitled) process asks it to.
    pub fn allow_foreground() {
        unsafe {
            AllowSetForegroundWindow(ASFW_ANY);
        }
    }

    /// Window geometry that survives restarts.
    #[derive(Serialize, Deserialize, Clone, Copy)]
    struct Saved {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        max: bool,
    }

    fn state_file() -> PathBuf {
        crate::config::ensure_data_dir().join("window.json")
    }

    /// Only a position that is still on a connected screen is reused (a monitor may be gone).
    fn on_a_screen(el: &EventLoopWindowTarget<Msg>, s: &Saved) -> bool {
        el.available_monitors().any(|m| {
            let (p, sz) = (m.position(), m.size());
            s.x + 80 > p.x && s.x + 80 < p.x + sz.width as i32 && s.y >= p.y - 10 && s.y + 40 < p.y + sz.height as i32
        })
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn message(parent: HWND, text: &str, style: u32) -> i32 {
        let (title, body) = (wide("MoodCut"), wide(text));
        unsafe { MessageBoxW(parent, body.as_ptr(), title.as_ptr(), style | MB_SETFOREGROUND) }
    }

    /// Opens a web link in the default browser (nothing but web and mail links).
    fn open_external(url: &str) {
        let lower = url.to_ascii_lowercase();
        if ["http://", "https://", "mailto:"].iter().any(|p| lower.starts_with(p)) {
            let _ = crate::render::ffmpeg::command(Path::new("rundll32")).args(["url.dll,FileProtocolHandler", url]).spawn();
        }
    }

    /// The editor stays on its own address: web links go to the browser, and a file dropped
    /// outside the timeline can't replace the editor with that file.
    fn allow_navigation(port: u16, url: &str) -> bool {
        if is_editor_page(port, url) {
            return true;
        }
        open_external(url);
        false
    }

    /// The editor's own address (and the in-page schemes it uses), not another site or a file.
    fn is_editor_page(port: u16, url: &str) -> bool {
        let own = [format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}")];
        own.iter().any(|o| url == o || ["/", "?", "#"].iter().any(|c| url.starts_with(&format!("{o}{c}"))))
            || url.starts_with("about:")
            || url.starts_with("blob:")
            || url.starts_with("data:")
    }

    fn geometry(w: &Window) -> Option<Saved> {
        let p = w.outer_position().ok()?;
        let s = w.inner_size();
        Some(Saved { x: p.x, y: p.y, w: s.width, h: s.height, max: false })
    }

    #[cfg(test)]
    pub fn is_editor_page_for_tests(port: u16, url: &str) -> bool {
        is_editor_page(port, url)
    }

    /// Asks before closing when that would lose something. False keeps the editor open.
    fn confirm_quit(window: &Window, port: u16) -> bool {
        let Some(info) = crate::server::running_instance(port) else { return true };
        let parent = window.hwnd() as HWND;
        if info["rendering"].as_u64().unwrap_or(0) > 0 && message(parent, "A render is still running. Quit and cancel it?", MB_YESNO | MB_ICONWARNING) != IDYES {
            return false;
        }
        // a project without a file autosaves and can be recovered from Home: only saved-once
        // projects with newer edits can lose work
        if info["dirty"].as_bool() == Some(true) && info["path"].is_string() {
            let name = info["name"].as_str().unwrap_or("this project");
            match message(parent, &format!("Save changes to \"{name}\" before quitting?"), MB_YESNOCANCEL | MB_ICONQUESTION) {
                IDYES => {
                    let saved = crate::server::local_request(port, "POST", "/api/project", Some(r#"{"action":"save"}"#), 5000)
                        .and_then(|(_, body)| serde_json::from_str::<serde_json::Value>(&body).ok())
                        .is_some_and(|v| v["ok"] == true);
                    if !saved {
                        message(parent, "The project couldn't be saved, so MoodCut stays open.", MB_OK | MB_ICONERROR);
                        return false;
                    }
                }
                IDNO => {}
                _ => return false,
            }
        }
        true
    }

    /// Opens the editor window on `url` and runs until it is closed, then ends the process.
    /// It only returns (with the reason) when the window couldn't be created.
    pub fn run(url: &str, port: u16) -> anyhow::Error {
        match open(url.to_string(), port) {
            Ok(never) => match never {},
            Err(e) => e,
        }
    }

    fn open(url: String, port: u16) -> Result<std::convert::Infallible> {
        let event_loop = EventLoopBuilder::<Msg>::with_user_event().build();
        if let Ok(mut g) = PROXY.lock() {
            *g = Some(event_loop.create_proxy());
        }

        let saved: Option<Saved> = std::fs::read_to_string(state_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .filter(|s: &Saved| s.w >= 400 && s.h >= 300 && on_a_screen(&event_loop, s));

        // hidden until the page has painted, so there is no white flash while it loads
        let mut wb = WindowBuilder::new().with_title("MoodCut").with_min_inner_size(LogicalSize::new(960.0, 600.0)).with_theme(Some(Theme::Dark)).with_visible(false);
        if let Ok(icon) = Icon::from_resource(1, None) {
            wb = wb.with_window_icon(Some(icon));
        }
        if let Ok(icon) = Icon::from_resource(1, None) {
            wb = wb.with_taskbar_icon(Some(icon));
        }
        wb = match saved {
            Some(s) => wb.with_inner_size(PhysicalSize::new(s.w, s.h)).with_position(PhysicalPosition::new(s.x, s.y)).with_maximized(s.max),
            None => wb.with_inner_size(LogicalSize::new(1440.0, 860.0)).with_maximized(true),
        };
        let window = Rc::new(wb.build(&event_loop).map_err(|e| anyhow::anyhow!("couldn't create the window: {e}"))?);

        let data_dir = crate::config::ensure_data_dir().join("webview2");
        let _ = std::fs::create_dir_all(&data_dir);
        let mut ctx = WebContext::new(Some(data_dir));
        // MOODCUT_DEVTOOLS=1 turns on F12, the inspector and the browser's own context menu
        let dev = std::env::var_os("MOODCUT_DEVTOOLS").is_some();
        let shown = Rc::new(Cell::new(false));
        let (for_title, for_load, shown_on_load) = (window.clone(), window.clone(), shown.clone());
        let webview = WebViewBuilder::new_with_web_context(&mut ctx)
            .with_url(url)
            .with_background_color((17, 17, 17, 255))
            .with_autoplay(true)
            .with_clipboard(true)
            .with_devtools(dev)
            .with_browser_accelerator_keys(dev)
            .with_default_context_menus(dev)
            .with_theme(wry::Theme::Dark)
            .with_navigation_handler(move |u| allow_navigation(port, &u))
            .with_new_window_req_handler(|u, _| {
                open_external(&u);
                NewWindowResponse::Deny
            })
            .with_document_title_changed_handler(move |t| for_title.set_title(&t))
            .with_on_page_load_handler(move |ev, _| {
                if matches!(ev, PageLoadEvent::Finished) && !shown_on_load.replace(true) {
                    for_load.set_visible(true);
                    for_load.set_focus();
                }
            })
            .build(&*window)
            .map_err(|e| anyhow::anyhow!("couldn't start WebView2: {e}"))?;

        let mut normal = geometry(&window);
        // shown anyway if the page never reports it has loaded
        let deadline = Instant::now() + Duration::from_secs(8);

        event_loop.run(move |event, _, flow| {
            // the view and its data folder live exactly as long as the loop
            let _keep = (&webview, &ctx);
            match event {
                Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                    if !shown.replace(true) {
                        window.set_visible(true);
                        window.set_focus();
                    }
                }
                Event::UserEvent(Msg::Focus) => {
                    shown.set(true);
                    window.set_visible(true);
                    if window.is_minimized() {
                        window.set_minimized(false);
                    }
                    window.set_focus();
                }
                Event::WindowEvent { event: WindowEvent::Resized(_) | WindowEvent::Moved(_), .. } => {
                    // a maximized or minimized window's rectangle isn't the one to restore
                    if !window.is_maximized() && !window.is_minimized() {
                        normal = geometry(&window).or(normal);
                    }
                }
                Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                    if confirm_quit(&window, port) {
                        if let Some(mut s) = normal {
                            s.max = window.is_maximized();
                            let _ = std::fs::write(state_file(), serde_json::to_string(&s).unwrap_or_default());
                        }
                        // a web link started from this window must not outlive it
                        crate::server::shutdown();
                        *flow = ControlFlow::ExitWithCode(0);
                        return;
                    }
                }
                _ => {}
            }
            *flow = if shown.get() { ControlFlow::Wait } else { ControlFlow::WaitUntil(deadline) };
        })
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::imp::is_editor_page_for_tests as editor_page;

    #[test]
    fn the_window_stays_on_the_editor() {
        assert!(editor_page(7878, "http://127.0.0.1:7878/"));
        assert!(editor_page(7878, "http://127.0.0.1:7878/?token=x"));
        assert!(editor_page(7878, "http://localhost:7878/#/home"));
        assert!(editor_page(7878, "blob:http://127.0.0.1:7878/6c1f"));
        // other ports, other sites, dropped files and look-alike hosts leave through the browser
        assert!(!editor_page(7878, "http://127.0.0.1:7879/"));
        assert!(!editor_page(7878, "http://127.0.0.1:78789/"));
        assert!(!editor_page(7878, "https://example.com/"));
        assert!(!editor_page(7878, "http://127.0.0.1:7878.evil.example/"));
        assert!(!editor_page(7878, "file:///C:/Users/me/clip.mp4"));
    }
}
