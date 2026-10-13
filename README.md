# CLI Launcher

A cross-platform CLI project launcher built with Rust, Slint, and Winit. The same UI
code runs on Windows, macOS, and Linux without relying on WebView, Electron, Fyne, or Win32 UI.

## Features

- Reads and remains compatible with existing CliLauncher `config.json` files
- Add, search, edit, repair, and remove projects
- Detects Codex, Claude, Gemini, and OpenCode
- Launches a CLI for a project, or opens a terminal or file manager
- Supports custom commands
- SOCKS5 detection and a local HTTP proxy bridge
- Platform-specific terminal support for Windows, macOS, and Linux
- Cross-platform single-instance file locking

## Configuration Location

- Windows: `%APPDATA%\CliLauncher`
- macOS: `~/Library/Application Support/CliLauncher`
- Linux: `$XDG_CONFIG_HOME/CliLauncher`, or `~/.config/CliLauncher` if unset

The configuration format is unchanged from the previous version. Each operating system uses its standard configuration directory.

## Build

A stable Rust toolchain is required.

    cargo test --all-targets
    cargo build --release

Windows:

    .\scripts\package-windows.ps1

macOS:

    chmod +x scripts/package-macos.sh
    ./scripts/package-macos.sh

The script generates `build/CliLauncher.app` and `build/CliLauncher.dmg`. Before distributing,
configure Apple Developer signing and notarization on macOS. Set `APPLE_SIGN_IDENTITY` to
have the script run `codesign`.

Linux:

    chmod +x scripts/package-linux.sh
    ./scripts/package-linux.sh

The Linux file picker uses XDG Desktop Portal. The desktop environment should also provide
a supported terminal emulator, such as GNOME Terminal, Konsole, Kitty, Alacritty, or xterm.

## UI Backend

Slint uses the Winit backend and a software renderer, avoiding WebView while sharing the UI
across all three desktop platforms. Folder and message dialogs use RFD to access each
platform's native capabilities.
