use crate::model::relative_time;
use crate::platform;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone)]
pub struct AgentSession {
    pub id: String,
    pub label: String,
}

pub fn list(cli: &str, project_path: &str) -> Vec<AgentSession> {
    let project = Path::new(project_path);
    if !project.is_dir() {
        return Vec::new();
    }
    match cli {
        "codex" => {
            let home = codex_home();
            list_jsonl(&home.join("sessions"), project, Some(&codex_titles(&home)))
        }
        "claude" => list_jsonl(&claude_sessions_dir(), project, None),
        "gemini" => list_gemini(project),
        "opencode" => list_opencode(project),
        _ => Vec::new(),
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".codex"))
}

fn claude_sessions_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".claude"))
        .join("projects")
}

fn codex_titles(home: &Path) -> HashMap<String, String> {
    let Ok(file) = File::open(home.join("session_index.jsonl")) else {
        return HashMap::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| {
            let entry: Value = serde_json::from_str(&line).ok()?;
            Some((
                entry.get("id")?.as_str()?.to_string(),
                entry.get("thread_name")?.as_str()?.to_string(),
            ))
        })
        .collect()
}

fn list_jsonl(
    root: &Path,
    project: &Path,
    titles: Option<&HashMap<String, String>>,
) -> Vec<AgentSession> {
    let Ok(project) = project.canonicalize() else {
        return Vec::new();
    };
    let mut files = Vec::new();
    collect_jsonl(root, &mut files, 0);
    let mut sessions = files
        .into_iter()
        .filter_map(|path| {
            let mut line = String::new();
            BufReader::new(File::open(&path).ok()?)
                .read_line(&mut line)
                .ok()?;
            let entry: Value = serde_json::from_str(&line).ok()?;
            let cwd = entry
                .get("payload")
                .and_then(|payload| payload.get("cwd"))
                .or_else(|| entry.get("cwd"))
                .and_then(Value::as_str)?;
            if Path::new(cwd).canonicalize().ok()? != project {
                return None;
            }
            let id = entry
                .get("payload")
                .and_then(|payload| payload.get("session_id"))
                .or_else(|| entry.get("payload").and_then(|payload| payload.get("id")))
                .or_else(|| entry.get("sessionId"))
                .or_else(|| entry.get("session_id"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| path.file_stem()?.to_str().map(str::to_string))?;
            let title = titles
                .and_then(|titles| titles.get(&id))
                .filter(|title| !title.trim().is_empty())
                .cloned()
                .or_else(|| transcript_title(&path))
                .unwrap_or_default();
            Some((session(&id, &title), modified(&path)))
        })
        .collect::<Vec<_>>();
    sessions.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    let mut seen = HashSet::new();
    sessions.retain(|(session, _)| seen.insert(session.id.clone()));
    sessions.into_iter().map(|(session, _)| session).collect()
}

fn collect_jsonl(root: &Path, files: &mut Vec<PathBuf>, depth: usize) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, files, depth + 1);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            files.push(path);
        }
    }
}

fn list_gemini(project: &Path) -> Vec<AgentSession> {
    let Some(executable) = platform::command_path("gemini") else {
        return Vec::new();
    };
    let Ok(output) = Command::new(executable)
        .arg("--list-sessions")
        .current_dir(project)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut sessions = text
        .lines()
        .filter_map(|line| {
            let (label, id) = line.rsplit_once('[')?;
            let id = id.strip_suffix(']')?.trim();
            valid_id(id).then(|| session(id, label.trim()))
        })
        .collect::<Vec<_>>();
    sessions.dedup_by(|left, right| left.id == right.id);
    sessions
}

fn list_opencode(project: &Path) -> Vec<AgentSession> {
    let Some(executable) = platform::command_path("opencode") else {
        return Vec::new();
    };
    let Ok(output) = Command::new(executable)
        .args(["session", "list", "--max-count", "50", "--format", "json"])
        .current_dir(project)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) else {
        return Vec::new();
    };
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let id = entry.get("id")?.as_str()?;
            if !valid_id(id) {
                return None;
            }
            let directory = entry
                .get("directory")
                .or_else(|| entry.get("cwd"))
                .and_then(Value::as_str);
            if directory.is_some_and(|directory| {
                Path::new(directory).canonicalize().ok() != project.canonicalize().ok()
            }) {
                return None;
            }
            let title = entry.get("title").and_then(Value::as_str).unwrap_or("");
            Some(session(id, title))
        })
        .collect()
}

fn session(id: &str, detail: &str) -> AgentSession {
    let title = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = if title.len() >= 20 && title.as_bytes().get(10) == Some(&b'T') {
        relative_time(&title)
    } else {
        title.chars().take(44).collect::<String>()
    };
    let label = if title.is_empty() {
        "未命名会话".to_string()
    } else {
        title
    };
    AgentSession {
        id: id.to_string(),
        label,
    }
}

fn transcript_title(path: &Path) -> Option<String> {
    let tail = read_jsonl_window(path, true);
    for entry in tail.iter().rev() {
        if let Some(title) = entry
            .get("customTitle")
            .or_else(|| entry.get("aiTitle"))
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty())
        {
            return Some(title.to_string());
        }
    }
    read_jsonl_window(path, false)
        .iter()
        .find_map(first_user_prompt)
}

fn read_jsonl_window(path: &Path, from_end: bool) -> Vec<Value> {
    const WINDOW: u64 = 64 * 1024;
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let Ok(length) = file.metadata().map(|metadata| metadata.len()) else {
        return Vec::new();
    };
    let offset = if from_end {
        length.saturating_sub(WINDOW)
    } else {
        0
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::with_capacity(WINDOW as usize);
    if file.take(WINDOW).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    text.lines()
        .skip(usize::from(from_end && offset > 0))
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn first_user_prompt(entry: &Value) -> Option<String> {
    let is_user = entry.get("type")?.as_str()? == "user"
        || (entry.get("type")?.as_str()? == "response_item"
            && entry.pointer("/payload/item/role").and_then(Value::as_str) == Some("user"));
    if !is_user {
        return None;
    }
    let content = entry
        .pointer("/message/content")
        .or_else(|| entry.pointer("/payload/item/content"))?;
    let prompt = if let Some(text) = content.as_str() {
        text.to_string()
    } else {
        content
            .as_array()?
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let prompt = prompt.trim();
    (!prompt.is_empty()).then(|| prompt.to_string())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character))
}

fn modified(path: &Path) -> u64 {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
