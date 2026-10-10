use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub const CONFIG_VERSION: i32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub path: String,
    pub default_cli: String,
    #[serde(default)]
    pub custom_commands: Vec<String>,
    #[serde(default)]
    pub added_at: String,
    #[serde(default)]
    pub last_launched: String,
}

impl Project {
    pub fn name(&self) -> String {
        Path::new(&self.path)
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| self.path.clone())
    }

    pub fn path_available(&self) -> bool {
        Path::new(&self.path).is_dir()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default)]
    pub width: i32,
    #[serde(default)]
    pub height: i32,
    #[serde(default)]
    pub maximized: bool,
    #[serde(default)]
    pub valid: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_proxy_host")]
    pub host: String,
    #[serde(default = "default_proxy_port")]
    pub port: u16,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_proxy_host(),
            port: default_proxy_port(),
        }
    }
}

fn default_proxy_host() -> String {
    "127.0.0.1".to_string()
}

fn default_proxy_port() -> u16 {
    7890
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "config_version")]
    pub version: i32,
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub window: WindowState,
    #[serde(default)]
    pub proxy: ProxyConfig,
}

fn config_version() -> i32 {
    CONFIG_VERSION
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            projects: Vec::new(),
            window: WindowState::default(),
            proxy: ProxyConfig::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CliPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub resume_command: &'static str,
    pub new_command: &'static str,
}

pub const PRESETS: [CliPreset; 4] = [
    CliPreset {
        id: "codex",
        name: "Codex",
        resume_command: "codex resume --last",
        new_command: "codex",
    },
    CliPreset {
        id: "claude",
        name: "Claude",
        resume_command: "claude --continue",
        new_command: "claude",
    },
    CliPreset {
        id: "gemini",
        name: "Gemini",
        resume_command: "gemini --resume latest",
        new_command: "gemini",
    },
    CliPreset {
        id: "opencode",
        name: "OpenCode",
        resume_command: "opencode",
        new_command: "",
    },
];

pub struct AppModel {
    pub config_dir: PathBuf,
    pub config_path: PathBuf,
    pub backup_path: PathBuf,
    pub log_path: PathBuf,
    pub config: Config,
    pub notice: String,
    pub installed: BTreeMap<String, bool>,
}

impl AppModel {
    pub fn preset(&self, id: &str) -> Option<&'static CliPreset> {
        PRESETS.iter().find(|preset| preset.id == id)
    }

    pub fn project(&self, id: &str) -> Option<Project> {
        self.config
            .projects
            .iter()
            .find(|project| project.id == id)
            .cloned()
    }

    pub fn validate_commands(
        default_cli: &str,
        commands: &[String],
    ) -> Result<(String, Vec<String>), String> {
        let cleaned: Vec<String> = commands
            .iter()
            .map(|command| command.trim().to_string())
            .filter(|command| !command.is_empty())
            .collect();
        let normalized = if default_cli == "custom" {
            if cleaned.is_empty() {
                return Err("请输入自定义命令".to_string());
            }
            "custom:0".to_string()
        } else {
            default_cli.to_string()
        };
        if let Some(index_text) = normalized.strip_prefix("custom:") {
            let index = index_text
                .parse::<usize>()
                .map_err(|_| "默认自定义命令不存在".to_string())?;
            if index >= cleaned.len() {
                return Err("默认自定义命令不存在".to_string());
            }
        } else if !PRESETS.iter().any(|preset| preset.id == normalized) {
            return Err("请选择默认 CLI".to_string());
        }
        Ok((normalized, cleaned))
    }

    pub fn add_project(
        &mut self,
        path: &str,
        default_cli: &str,
        commands: &[String],
    ) -> Result<(), String> {
        let path = normalize_directory(path)?;
        let key = path_key(&path);
        if self
            .config
            .projects
            .iter()
            .any(|project| path_key(&project.path) == key)
        {
            return Err("该文件夹已经在项目列表中".to_string());
        }
        let (default_cli, custom_commands) = Self::validate_commands(default_cli, commands)?;
        self.config.projects.push(Project {
            id: new_id(),
            path,
            default_cli,
            custom_commands,
            added_at: now_rfc3339(),
            last_launched: String::new(),
        });
        self.save()?;
        self.log("INFO", "已添加项目");
        Ok(())
    }

    pub fn update_project(
        &mut self,
        id: &str,
        default_cli: &str,
        commands: &[String],
    ) -> Result<(), String> {
        let (default_cli, custom_commands) = Self::validate_commands(default_cli, commands)?;
        let project = self
            .config
            .projects
            .iter_mut()
            .find(|project| project.id == id)
            .ok_or_else(|| "项目不存在".to_string())?;
        project.default_cli = default_cli;
        project.custom_commands = custom_commands;
        self.save()?;
        self.log("INFO", "已更新项目配置");
        Ok(())
    }

    pub fn remove_project(&mut self, id: &str) -> Result<(), String> {
        let before = self.config.projects.len();
        self.config.projects.retain(|project| project.id != id);
        if self.config.projects.len() == before {
            return Err("项目不存在".to_string());
        }
        self.save()?;
        self.log("INFO", "已从启动器移除项目");
        Ok(())
    }

    pub fn repair_project(&mut self, id: &str, selected: &str) -> Result<(), String> {
        let path = normalize_directory(selected)?;
        let key = path_key(&path);
        if self
            .config
            .projects
            .iter()
            .any(|project| project.id != id && path_key(&project.path) == key)
        {
            return Err("该文件夹已经在项目列表中".to_string());
        }
        let project = self
            .config
            .projects
            .iter_mut()
            .find(|project| project.id == id)
            .ok_or_else(|| "项目不存在".to_string())?;
        project.path = path;
        self.save()?;
        Ok(())
    }

    pub fn mark_launched(&mut self, id: &str) {
        if let Some(project) = self
            .config
            .projects
            .iter_mut()
            .find(|project| project.id == id)
        {
            project.last_launched = now_rfc3339();
            let _ = self.save();
        }
    }
}

pub fn normalize_directory(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("文件夹路径不能为空".to_string());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_dir() {
        return Err("文件夹不存在".to_string());
    }
    let absolute = path
        .canonicalize()
        .map_err(|error| format!("无法读取文件夹：{error}"))?;
    Ok(absolute.to_string_lossy().into_owned())
}

fn path_key(path: &str) -> String {
    let normalized = PathBuf::from(path)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(path))
        .to_string_lossy()
        .into_owned();
    if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized
    }
}

pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn new_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos:024x}")
}

pub fn relative_time(value: &str) -> String {
    let Ok(when) = OffsetDateTime::parse(value, &Rfc3339) else {
        return "尚未启动".to_string();
    };
    let delta = OffsetDateTime::now_utc() - when;
    let seconds = delta.whole_seconds().max(0);
    if seconds < 60 {
        "刚刚".to_string()
    } else if seconds < 3600 {
        format!("{} 分钟前", seconds / 60)
    } else if seconds < 86_400 {
        format!("{} 小时前", seconds / 3600)
    } else {
        format!("{} 天前", seconds / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_existing_go_config_shape() {
        let json = r#"{
          "version": 1,
          "projects": [{
            "id": "abc", "path": "C:\\\\workspace", "defaultCli": "codex",
            "customCommands": [], "addedAt": "2026-08-08T00:00:00Z"
          }],
          "window": {"x": 1, "y": 2, "width": 1120, "height": 760, "maximized": false, "valid": true},
          "proxy": {"enabled": true, "host": "127.0.0.1", "port": 10808}
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.projects[0].default_cli, "codex");
        assert_eq!(config.proxy.port, 10808);
        assert_eq!(config.window.width, 1120);
    }

    #[test]
    fn custom_default_requires_a_command() {
        assert!(AppModel::validate_commands("custom", &[]).is_err());
        let (default_cli, commands) =
            AppModel::validate_commands("custom", &[" npm run dev ".to_string()]).unwrap();
        assert_eq!(default_cli, "custom:0");
        assert_eq!(commands, vec!["npm run dev"]);
    }
}
