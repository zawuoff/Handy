// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;
use handy_app_lib::CliArgs;

fn main() {
    let cli_args = CliArgs::parse();

    #[cfg(target_os = "linux")]
    {
        // DMABUF renderer causes crashes on various GPU/display server configurations
        // See: https://github.com/tauri-apps/tauri/issues/9394
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");

        // On GNOME Wayland, apps cannot position their own windows and Mutter
        // does not support the layer-shell protocol, so the recording overlay
        // (and the meeting captions panel) gets stranded in the center of the
        // screen. Running under XWayland restores window placement — the same
        // behavior the packaged AppImage builds get from their launcher.
        //
        // This deliberately overrides an inherited GDK_BACKEND=wayland: desktop
        // sessions commonly export it globally, and honoring it costs core UX.
        // Set HANDY_NO_X11_FALLBACK=1 to opt out. Only forced when an X server
        // (XWayland) is actually available.
        let is_gnome_wayland = std::env::var("XDG_SESSION_TYPE")
            .map(|v| v == "wayland")
            .unwrap_or(false)
            && std::env::var("XDG_CURRENT_DESKTOP")
                .map(|v| v.to_ascii_lowercase().contains("gnome"))
                .unwrap_or(false);
        if is_gnome_wayland
            && std::env::var_os("HANDY_NO_X11_FALLBACK").is_none()
            && std::env::var_os("DISPLAY").is_some()
        {
            std::env::set_var("GDK_BACKEND", "x11");
        }
    }

    handy_app_lib::run(cli_args)
}
