# CLI Launcher

使用 Rust、Slint 和 Winit 构建的跨平台 CLI 项目启动器。界面代码在 Windows、macOS
和 Linux 上共用，不依赖 WebView、Electron、Fyne 或 Win32 UI。

## 当前能力

- 读取并兼容现有 CliLauncher config.json
- 添加、搜索、编辑、修复和移除项目
- 检测 Codex、Claude、Gemini、OpenCode
- 按项目启动 CLI、打开普通终端或文件管理器
- 自定义命令
- SOCKS5 检测及本地 HTTP 代理桥接
- Windows、macOS、Linux 平台终端适配
- 跨平台单实例文件锁

## 配置位置

- Windows：%APPDATA%\CliLauncher
- macOS：~/Library/Application Support/CliLauncher
- Linux：$XDG_CONFIG_HOME/CliLauncher，未设置时使用 ~/.config/CliLauncher

配置结构与原版本相同。不同操作系统使用各自的标准配置目录。

## 构建

需要稳定版 Rust 工具链。

    cargo test --all-targets
    cargo build --release

Windows：

    .\scripts\package-windows.ps1

macOS：

    chmod +x scripts/package-macos.sh
    ./scripts/package-macos.sh

脚本生成 build/CliLauncher.app 和 build/CliLauncher.dmg。正式分发前，
需要在 macOS 上配置 Apple Developer 签名和公证。设置 APPLE_SIGN_IDENTITY 可让
脚本执行 codesign。

Linux：

    chmod +x scripts/package-linux.sh
    ./scripts/package-linux.sh

Linux 文件选择器使用 XDG Desktop Portal；桌面环境还应提供可用终端模拟器，例如
GNOME Terminal、Konsole、Kitty、Alacritty 或 xterm。

## UI 后端

Slint 使用 Winit 后端和软件渲染器，避免 WebView，同时在三个桌面平台共享界面。
文件夹与消息对话框由 RFD 调用各平台原生能力。
