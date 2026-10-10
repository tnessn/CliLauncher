use crate::model::{AppModel, PRESETS, Project, ProxyConfig};
use crate::proxy::ProxyRuntime;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(all(unix, not(target_os = "macos")))]
use std::process::Stdio;

pub fn config_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("CliLauncher");
    }
    #[cfg(target_os = "macos")]
    {
        return home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library")
            .join("Application Support")
            .join("CliLauncher");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|path| path.join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("CliLauncher");
    }
    #[allow(unreachable_code)]
    PathBuf::from(".").join("CliLauncher")
}

#[cfg(not(target_os = "windows"))]
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn detect_installed_clis() -> BTreeMap<String, bool> {
    PRESETS
        .iter()
        .map(|preset| (preset.id.to_string(), command_exists(preset.id)))
        .collect()
}

fn command_exists(command: &str) -> bool {
    command_path(command).is_some()
}

pub fn command_path(command: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut directories = std::env::split_paths(&path).collect::<Vec<_>>();
    #[cfg(target_os = "macos")]
    {
        directories.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
        if let Some(home) = home_dir() {
            directories.extend(
                [".local/bin", ".npm-global/bin", ".opencode/bin"]
                    .map(|directory| home.join(directory)),
            );
        }
    }
    let candidates = command_candidates(command);
    directories.into_iter().find_map(|directory| {
        candidates
            .iter()
            .map(|name| directory.join(name))
            .find(|path| executable(path))
    })
}

fn command_candidates(command: &str) -> Vec<OsString> {
    #[cfg(target_os = "windows")]
    {
        let pathext =
            std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        let mut names = vec![OsString::from(command)];
        for extension in pathext
            .split(';')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .chain([".PS1"])
        {
            let extension = if extension.starts_with('.') {
                extension.to_string()
            } else {
                format!(".{extension}")
            };
            let name = OsString::from(format!("{command}{extension}"));
            if !names.iter().any(|existing| {
                existing
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&name.to_string_lossy())
            }) {
                names.push(name);
            }
        }
        names
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![OsString::from(command)]
    }
}

fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    true
}

pub fn launch_project(
    model: &AppModel,
    project: &Project,
    command_id: &str,
    session_id: Option<&str>,
    proxy: &ProxyRuntime,
) -> Result<(), String> {
    if !project.path_available() {
        return Err("项目路径不可用，请先修复路径".to_string());
    }
    let (resume, fresh) = if let Some(index) = command_id.strip_prefix("custom:") {
        let index = index
            .parse::<usize>()
            .map_err(|_| "自定义命令不存在".to_string())?;
        let command = project
            .custom_commands
            .get(index)
            .filter(|command| !command.trim().is_empty())
            .ok_or_else(|| "自定义命令不存在".to_string())?;
        (command.clone(), String::new())
    } else {
        let preset = model
            .preset(command_id)
            .ok_or_else(|| "未知的 CLI 命令".to_string())?;
        if !model.installed.get(preset.id).copied().unwrap_or(false) {
            return Err(format!("{} 未安装或不在 PATH 中", preset.name));
        }
        (
            preset.resume_command.to_string(),
            preset.new_command.to_string(),
        )
    };
    let command = if let Some(session_id) = session_id {
        if session_id.is_empty()
            || !session_id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character))
        {
            return Err("会话 ID 格式无效".to_string());
        }
        match command_id {
            "codex" => format!("codex resume {session_id}"),
            "claude" => format!("claude --resume {session_id}"),
            "gemini" => format!("gemini --resume {session_id}"),
            "opencode" => format!("opencode --session {session_id}"),
            _ => return Err("自定义命令不支持选择历史会话".to_string()),
        }
    } else {
        fallback_command(&resume, &fresh)
    };
    open_terminal(&project.path, &command, proxy)
}

fn fallback_command(resume: &str, fresh: &str) -> String {
    if fresh.trim().is_empty() {
        return resume.to_string();
    }
    if cfg!(target_os = "windows") {
        format!("{resume}; if ($LASTEXITCODE -ne 0) {{ {fresh} }}")
    } else {
        format!("{resume} || {fresh}")
    }
}

pub fn open_plain_terminal(project: &Project) -> Result<(), String> {
    if !project.path_available() {
        return Err("项目路径不可用，请先修复路径".to_string());
    }
    open_terminal(&project.path, "", &ProxyRuntime::default())
}

pub fn open_folder(project: &Project) -> Result<(), String> {
    if !project.path_available() {
        return Err("项目路径不可用，请先修复路径".to_string());
    }
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");
    command
        .arg(&project.path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法打开文件管理器：{error}"))
}

fn open_terminal(path: &str, command_text: &str, proxy: &ProxyRuntime) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        let script = windows_terminal_script(path, command_text, proxy);
        let encoded = encode_powershell_command(&script);
        let (mut command, creation_flags) = if command_exists("wt") {
            let mut process = Command::new("wt.exe");
            process.args([
                "-w",
                "0",
                "new-tab",
                "-d",
                path,
                "powershell.exe",
                "-NoExit",
                "-NoLogo",
                "-EncodedCommand",
                &encoded,
            ]);
            (process, CREATE_NO_WINDOW)
        } else {
            let mut process = Command::new("powershell.exe");
            process.args(["-NoExit", "-NoLogo", "-EncodedCommand", &encoded]);
            (process, CREATE_NEW_CONSOLE)
        };
        command.creation_flags(creation_flags);
        command
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("无法打开终端：{error}"))
    }
    #[cfg(target_os = "macos")]
    {
        let shell = unix_terminal_script(path, command_text, proxy);
        let apple_script = format!(
            "tell application \"iTerm2\"\n\
                activate\n\
                if (count of windows) = 0 then\n\
                    create window with default profile\n\
                else\n\
                    tell current window to create tab with default profile\n\
                end if\n\
                tell current session of current window to write text \"{}\"\n\
            end tell",
            apple_script_escape(&shell)
        );
        let output = Command::new("osascript")
            .args(["-e", &apple_script])
            .output()
            .map_err(|error| format!("无法启动 AppleScript：{error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "无法打开 iTerm2：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let shell = unix_terminal_script(path, command_text, proxy);
        let choices: [(&str, &[&str]); 6] = [
            ("gnome-terminal", &["--", "sh", "-lc"]),
            ("konsole", &["-e", "sh", "-lc"]),
            ("kitty", &["sh", "-lc"]),
            ("alacritty", &["-e", "sh", "-lc"]),
            ("xterm", &["-e", "sh", "-lc"]),
            ("x-terminal-emulator", &["-e", "sh", "-lc"]),
        ];
        for (terminal, arguments) in choices {
            if command_exists(terminal) {
                return Command::new(terminal)
                    .args(arguments)
                    .arg(&shell)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map(|_| ())
                    .map_err(|error| format!("无法打开终端：{error}"));
            }
        }
        Err("未找到可用的终端模拟器".to_string())
    }
}

#[cfg(target_os = "windows")]
fn windows_terminal_script(path: &str, command: &str, proxy: &ProxyRuntime) -> String {
    let mut parts = vec![format!(
        "Set-Location -LiteralPath '{}'",
        path.replace("'", "''")
    )];
    if !proxy.socks_url.is_empty() {
        parts.push(format!(
            "$env:ALL_PROXY='{}'; $env:all_proxy='{}'",
            proxy.socks_url, proxy.socks_url
        ));
    }
    if !proxy.http_url.is_empty() {
        parts.push(format!(
            "$env:HTTP_PROXY='{0}'; $env:HTTPS_PROXY='{0}'; $env:http_proxy='{0}'; $env:https_proxy='{0}'",
            proxy.http_url
        ));
    }
    if !command.trim().is_empty() {
        parts.push(command.to_string());
    }
    parts.join("; ")
}

#[cfg(target_os = "windows")]
fn encode_powershell_command(script: &str) -> String {
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    base64_encode(&bytes)
}

#[cfg(target_os = "windows")]
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let a = chunk[0] as u32;
        let b = chunk.get(1).copied().unwrap_or(0) as u32;
        let c = chunk.get(2).copied().unwrap_or(0) as u32;
        let value = (a << 16) | (b << 8) | c;
        output.push(TABLE[((value >> 18) & 63) as usize] as char);
        output.push(TABLE[((value >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

#[cfg(unix)]
fn unix_terminal_script(path: &str, command: &str, proxy: &ProxyRuntime) -> String {
    let mut parts = vec![format!("cd {}", shell_quote(path))];
    if !proxy.socks_url.is_empty() {
        parts.push(format!(
            "export ALL_PROXY={0} all_proxy={0}",
            shell_quote(&proxy.socks_url)
        ));
    }
    if !proxy.http_url.is_empty() {
        parts.push(format!(
            "export HTTP_PROXY={0} HTTPS_PROXY={0} http_proxy={0} https_proxy={0}",
            shell_quote(&proxy.http_url)
        ));
    }
    if !command.trim().is_empty() {
        parts.push(command.to_string());
    }
    parts.push("exec \"${SHELL:-sh}\"".to_string());
    parts.join("; ")
}

#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace("'", "'\\''"))
}

#[cfg(target_os = "macos")]
fn apple_script_escape(value: &str) -> String {
    value.replace("\\", "\\\\").replace("\"", "\\\"")
}

pub fn proxy_label(config: &ProxyConfig) -> String {
    if config.enabled {
        format!("SOCKS5 {}:{}", config.host, config.port)
    } else {
        "代理关闭".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_candidates_are_not_empty() {
        assert!(!command_candidates("codex").is_empty());
    }

    #[test]
    fn fallback_command_matches_the_current_shell() {
        let command = fallback_command("codex resume --last", "codex");
        if cfg!(target_os = "windows") {
            assert!(!command.contains("||"));
            assert!(command.contains("$LASTEXITCODE"));
        } else {
            assert!(command.contains("||"));
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn powershell_command_uses_utf16_base64() {
        assert_eq!(encode_powershell_command("A"), "QQA=");
    }

    #[cfg(unix)]
    #[test]
    fn shell_paths_are_single_quoted() {
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
    }
}
