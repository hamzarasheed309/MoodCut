// Ensures the embedded GUI folder exists so `cargo build` works before the
// web UI has been built (the placeholder explains how to build it).
fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../gui/dist");
    if !dist.join("index.html").exists() {
        let _ = std::fs::create_dir_all(&dist);
        let _ = std::fs::write(
            dist.join("index.html"),
            "<!doctype html><meta charset=utf-8><title>MoodCut</title><body style='font:16px system-ui;background:#111;color:#eee;padding:40px'>\
             <h1>MoodCut</h1><p>The web GUI hasn't been built. Run <code>npm install && npm run build</code> in <code>gui/</code> and rebuild.</p>\
             <p>The MCP endpoint is available at <code>/mcp</code>.</p></body>",
        );
    }
    println!("cargo:rerun-if-changed=../../gui/dist");
    #[cfg(windows)]
    windows_resources();
}

/// Icon, product name and version shown by Explorer, the taskbar and Task Manager.
#[cfg(windows)]
fn windows_resources() {
    println!("cargo:rerun-if-changed=assets/moodcut.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut r = winresource::WindowsResource::new();
    r.set_icon("assets/moodcut.ico");
    r.set("ProductName", "MoodCut");
    r.set("FileDescription", "MoodCut video editor");
    r.set("LegalCopyright", "MIT License");
    r.set_manifest(MANIFEST);
    if let Err(e) = r.compile() {
        println!("cargo:warning=couldn't embed the Windows icon/version info: {e}");
    }
}

/// Themed controls (common controls v6), Windows 10/11 behaviour, and paths longer than 260
/// characters (deep project and cache folders). DPI scaling is set by the window code.
#[cfg(windows)]
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <ws2:longPathAware xmlns:ws2="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</ws2:longPathAware>
    </windowsSettings>
  </application>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#;
