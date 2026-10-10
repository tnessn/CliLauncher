use crate::model::{AppModel, CONFIG_VERSION, Config};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

const MAX_LOG_SIZE: u64 = 1024 * 1024;

impl AppModel {
    pub fn load() -> Self {
        let config_dir = crate::platform::config_dir();
        let config_path = config_dir.join("config.json");
        let backup_path = config_dir.join("config.backup.json");
        let log_path = config_dir.join("CliLauncher.log");
        let _ = fs::create_dir_all(&config_dir);

        let (mut config, notice) = match read_config(&config_path) {
            Ok(config) => (config, String::new()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (Config::default(), String::new())
            }
            Err(_) => match read_config(&backup_path) {
                Ok(config) => {
                    let _ = write_json(&config_path, &config);
                    (config, "配置文件损坏，已自动恢复最近的有效备份".to_string())
                }
                Err(_) => (
                    Config::default(),
                    "配置文件及备份均无法读取，已创建空配置".to_string(),
                ),
            },
        };
        config
            .projects
            .sort_by(|left, right| left.added_at.cmp(&right.added_at));
        Self {
            config_dir,
            config_path,
            backup_path,
            log_path,
            config,
            notice,
            installed: BTreeMap::new(),
        }
    }

    pub fn save(&mut self) -> Result<(), String> {
        fs::create_dir_all(&self.config_dir)
            .map_err(|error| format!("无法创建配置目录：{error}"))?;
        self.config.version = CONFIG_VERSION;
        let temp_path = self.config_path.with_extension("json.tmp");
        write_json(&temp_path, &self.config).map_err(|error| format!("无法写入配置：{error}"))?;
        if self.config_path.exists() {
            fs::copy(&self.config_path, &self.backup_path)
                .map_err(|error| format!("无法创建配置备份：{error}"))?;
            fs::remove_file(&self.config_path)
                .map_err(|error| format!("无法替换旧配置：{error}"))?;
        }
        fs::rename(&temp_path, &self.config_path)
            .map_err(|error| format!("无法启用新配置：{error}"))?;
        if !self.backup_path.exists() {
            let _ = fs::copy(&self.config_path, &self.backup_path);
        }
        Ok(())
    }

    pub fn log(&self, level: &str, message: &str) {
        if let Ok(metadata) = fs::metadata(&self.log_path)
            && metadata.len() >= MAX_LOG_SIZE
        {
            let rotated = self.log_path.with_extension("log.1");
            let _ = fs::remove_file(&rotated);
            let _ = fs::rename(&self.log_path, rotated);
        }
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
        {
            let _ = writeln!(
                file,
                "{} [{}] {}",
                crate::model::now_rfc3339(),
                level,
                message
            );
        }
    }
}

fn read_config(path: &Path) -> Result<Config, std::io::Error> {
    let data = fs::read(path)?;
    let config: Config = serde_json::from_slice(&data)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if config.version != CONFIG_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("不支持的配置版本：{}", config.version),
        ));
    }
    Ok(config)
}

pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), std::io::Error> {
    let mut data = serde_json::to_vec_pretty(value)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    data.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    file.write_all(&data)?;
    file.sync_all()
}
