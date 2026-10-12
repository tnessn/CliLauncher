#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod config;
mod model;
mod platform;
mod proxy;
mod sessions;

use fs2::FileExt;
use model::{AppModel, PRESETS, Project, ProxyConfig, relative_time};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

slint::include_modules!();

struct State {
    model: AppModel,
    visible_ids: Vec<String>,
    sessions: std::collections::HashMap<String, Vec<sessions::AgentSession>>,
    selected_sessions: std::collections::HashMap<String, String>,
}

const INSTANCE_ACTIVATE_COMMAND: &[u8] = b"activate-window";

fn main() {
    if let Err(error) = run() {
        show_error(&error);
    }
}

fn run() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = proxy::run_helper_mode(&arguments) {
        return result;
    }

    let mut model = AppModel::load();
    model.installed = platform::detect_installed_clis();
    model.log("INFO", "跨平台应用启动");

    let lock_path = model.config_dir.join("cli-launcher.lock");
    let endpoint_path = model.config_dir.join("cli-launcher.instance");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|error| format!("无法创建单实例锁：{error}"))?;
    if lock.try_lock_exclusive().is_err() {
        notify_existing_instance(&endpoint_path)?;
        return Ok(());
    }
    let activation_listener = publish_instance_endpoint(&endpoint_path)?;

    let window = AppWindow::new().map_err(|error| error.to_string())?;
    let state = Rc::new(RefCell::new(State {
        model,
        visible_ids: Vec::new(),
        sessions: std::collections::HashMap::new(),
        selected_sessions: std::collections::HashMap::new(),
    }));
    install_callbacks(&window, &state);
    install_file_drop(&window, &state);
    install_activation_listener(&window, activation_listener);
    reload_all_sessions(&state);
    refresh_ui(&window, &state, None);
    let run_result = window.run().map_err(|error| error.to_string());
    let _ = fs::remove_file(&endpoint_path);
    run_result?;
    state.borrow().model.log("INFO", "跨平台应用退出");
    drop(lock);
    Ok(())
}

fn publish_instance_endpoint(endpoint_path: &Path) -> Result<TcpListener, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("无法创建窗口激活通道：{error}"))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("无法读取窗口激活地址：{error}"))?;
    fs::write(endpoint_path, address.to_string())
        .map_err(|error| format!("无法保存窗口激活地址：{error}"))?;
    Ok(listener)
}

fn notify_existing_instance(endpoint_path: &Path) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut last_error = "已有窗口尚未准备好".to_string();
    loop {
        match fs::read_to_string(endpoint_path) {
            Ok(endpoint) => {
                if let Ok(address) = endpoint.trim().parse::<SocketAddr>() {
                    match notify_instance(address) {
                        Ok(()) => return Ok(()),
                        Err(error) => last_error = error,
                    }
                }
            }
            Err(error) => last_error = error.to_string(),
        }
        if Instant::now() >= deadline {
            return Err(format!("无法打开已运行的 CLI 启动器窗口：{last_error}"));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn notify_instance(address: SocketAddr) -> Result<(), String> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(400))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_millis(400)))
        .map_err(|error| error.to_string())?;
    stream
        .write_all(INSTANCE_ACTIVATE_COMMAND)
        .map_err(|error| error.to_string())
}

fn install_activation_listener(window: &AppWindow, listener: TcpListener) {
    let weak = window.as_weak();
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut stream) = connection else {
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut command = [0u8; 32];
            let Ok(count) = stream.read(&mut command) else {
                continue;
            };
            if &command[..count] != INSTANCE_ACTIVATE_COMMAND {
                continue;
            }
            let _ = weak.upgrade_in_event_loop(|window| {
                let slint_window = window.window();
                let _ = slint_window.show();
                slint_window.set_minimized(false);
                let _ = slint_window.with_winit_window(|winit_window| {
                    winit_window.set_minimized(false);
                    winit_window.focus_window();
                    winit_window.request_user_attention(Some(
                        winit::window::UserAttentionType::Informational,
                    ));
                });
            });
        }
    });
}

fn install_callbacks(window: &AppWindow, state: &Rc<RefCell<State>>) {
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_search_changed(move |_| {
            if let Some(window) = weak.upgrade() {
                refresh_ui(&window, &state, None);
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_project_command_changed(move |row_index, command_index| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let Some((project_id, command_id)) = ({
                let state = state.borrow();
                state
                    .visible_ids
                    .get(row_index.max(0) as usize)
                    .and_then(|project_id| {
                        state.model.project(project_id).and_then(|project| {
                            command_id_at(&project, command_index)
                                .map(|command_id| (project_id.clone(), command_id))
                        })
                    })
            }) else {
                return;
            };
            let result = {
                let mut state = state.borrow_mut();
                if let Some(project) = state
                    .model
                    .config
                    .projects
                    .iter_mut()
                    .find(|project| project.id == project_id)
                {
                    project.default_cli = command_id;
                    state.model.save()
                } else {
                    Err("未找到项目".to_string())
                }
            };
            match result {
                Ok(()) => {
                    reload_project_sessions(&state, &project_id);
                    refresh_ui(&window, &state, Some(&project_id));
                }
                Err(error) => show_error(&error),
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_project_session_changed(move |row_index, session_index| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            let Some(project_id) = state.visible_ids.get(row_index.max(0) as usize).cloned() else {
                return;
            };
            if session_index <= 0 {
                state.selected_sessions.remove(&project_id);
            } else if let Some(session_id) = state
                .sessions
                .get(&project_id)
                .and_then(|sessions| sessions.get(session_index as usize - 1))
                .map(|session| session.id.clone())
            {
                state.selected_sessions.insert(project_id, session_id);
            }
            drop(state);
            window.set_status_text("启动时将使用所选会话".into());
        });
    }
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_refresh_data(move || {
            if let Some(window) = weak.upgrade() {
                state.borrow_mut().model.installed = platform::detect_installed_clis();
                reload_all_sessions(&state);
                refresh_ui(&window, &state, None);
                window.set_status_text("已刷新 CLI、会话与项目状态".into());
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_add_project(move || {
            if let Some(window) = weak.upgrade()
                && let Some(path) = pick_folder(None)
            {
                let installed = &state.borrow().model.installed;
                let initial = PRESETS
                    .iter()
                    .position(|preset| installed.get(preset.id).copied().unwrap_or(false))
                    .unwrap_or(0) as i32;
                window.set_dialog_title("添加项目".into());
                window.set_dialog_edit_id("".into());
                window.set_dialog_path(path.to_string_lossy().into_owned().into());
                window.set_dialog_cli_index(initial);
                window.set_dialog_command_one("".into());
                window.set_dialog_command_two("".into());
                window.set_project_dialog_open(true);
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_browse_dialog_path(move || {
            if let Some(window) = weak.upgrade() {
                let current = window.get_dialog_path().to_string();
                if let Some(path) = pick_folder((!current.is_empty()).then_some(current.as_str())) {
                    window.set_dialog_path(path.to_string_lossy().into_owned().into());
                }
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_save_project(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let path = window.get_dialog_path().to_string();
            let edit_id = window.get_dialog_edit_id().to_string();
            let cli_index = window.get_dialog_cli_index().clamp(0, PRESETS.len() as i32);
            let default_cli = if cli_index as usize == PRESETS.len() {
                "custom"
            } else {
                PRESETS[cli_index as usize].id
            };
            let commands = [
                window.get_dialog_command_one().to_string(),
                window.get_dialog_command_two().to_string(),
            ];
            let result = if edit_id.is_empty() {
                state
                    .borrow_mut()
                    .model
                    .add_project(&path, default_cli, &commands)
            } else {
                state
                    .borrow_mut()
                    .model
                    .update_project(&edit_id, default_cli, &commands)
            };
            match result {
                Ok(()) => {
                    window.set_project_dialog_open(false);
                    window.set_status_text(
                        if edit_id.is_empty() {
                            "项目已添加"
                        } else {
                            "项目配置已更新"
                        }
                        .into(),
                    );
                    reload_all_sessions(&state);
                    refresh_ui(
                        &window,
                        &state,
                        (!edit_id.is_empty()).then_some(edit_id.as_str()),
                    );
                }
                Err(error) => show_error(&error),
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_edit_project(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let Some(project) = selected_project(&window, &state.borrow()) else {
                return;
            };
            let cli_index = PRESETS
                .iter()
                .position(|preset| preset.id == project.default_cli)
                .unwrap_or(PRESETS.len()) as i32;
            window.set_dialog_title("编辑项目".into());
            window.set_dialog_edit_id(project.id.into());
            window.set_dialog_path(project.path.into());
            window.set_dialog_cli_index(cli_index);
            window.set_dialog_command_one(
                project
                    .custom_commands
                    .first()
                    .cloned()
                    .unwrap_or_default()
                    .into(),
            );
            window.set_dialog_command_two(
                project
                    .custom_commands
                    .get(1)
                    .cloned()
                    .unwrap_or_default()
                    .into(),
            );
            window.set_project_dialog_open(true);
        });
    }
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_repair_project(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let Some(project) = selected_project(&window, &state.borrow()) else {
                return;
            };
            if let Some(path) = pick_folder(Some(&project.path)) {
                let result = state
                    .borrow_mut()
                    .model
                    .repair_project(&project.id, &path.to_string_lossy());
                match result {
                    Ok(()) => {
                        window.set_status_text("项目路径已修复".into());
                        reload_project_sessions(&state, &project.id);
                        refresh_ui(&window, &state, Some(&project.id));
                    }
                    Err(error) => show_error(&error),
                }
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_remove_project(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let Some(project) = selected_project(&window, &state.borrow()) else {
                return;
            };
            window.set_remove_project_name(project.name().into());
            window.set_remove_project_id(project.id.into());
            window.set_remove_dialog_open(true);
        });
    }
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_confirm_remove(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let project_id = window.get_remove_project_id().to_string();
            if project_id.is_empty() {
                window.set_remove_dialog_open(false);
                return;
            }
            let result = state.borrow_mut().model.remove_project(&project_id);
            match result {
                Ok(()) => {
                    window.set_remove_dialog_open(false);
                    window.set_remove_project_id("".into());
                    window.set_remove_project_name("".into());
                    window.set_status_text("项目已移除".into());
                    {
                        let mut state = state.borrow_mut();
                        state.sessions.remove(&project_id);
                        state.selected_sessions.remove(&project_id);
                    }
                    refresh_ui(&window, &state, None);
                }
                Err(error) => show_error(&error),
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_launch_project(move |index| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut state_ref = state.borrow_mut();
            let Some(project_id) = state_ref.visible_ids.get(index.max(0) as usize).cloned() else {
                return;
            };
            let Some(project) = state_ref.model.project(&project_id) else {
                return;
            };
            let command_id = project.default_cli.clone();
            let session_id = state_ref.selected_sessions.get(&project_id).cloned();
            let result = proxy::runtime_for_launch(&state_ref.model).and_then(|runtime| {
                platform::launch_project(
                    &state_ref.model,
                    &project,
                    &command_id,
                    session_id.as_deref(),
                    &runtime,
                )
            });
            match result {
                Ok(()) => {
                    state_ref.model.mark_launched(&project.id);
                    drop(state_ref);
                    window.set_status_text(format!("已启动 {}", project.name()).into());
                    refresh_ui(&window, &state, Some(&project.id));
                }
                Err(error) => show_error(&error),
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_terminal_selected(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if let Some(project) = selected_project(&window, &state.borrow()) {
                match platform::open_plain_terminal(&project) {
                    Ok(()) => window.set_status_text("终端已打开".into()),
                    Err(error) => show_error(&error),
                }
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_folder_selected(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if let Some(project) = selected_project(&window, &state.borrow()) {
                match platform::open_folder(&project) {
                    Ok(()) => window.set_status_text("文件管理器已打开".into()),
                    Err(error) => show_error(&error),
                }
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_open_proxy(move || {
            if let Some(window) = weak.upgrade() {
                let proxy = state.borrow().model.config.proxy.clone();
                window.set_proxy_enabled(proxy.enabled);
                window.set_proxy_host(proxy.host.into());
                window.set_proxy_port(proxy.port.to_string().into());
                window.set_proxy_dialog_open(true);
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_test_proxy(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let port = match window.get_proxy_port().parse::<u16>() {
                Ok(port) => port,
                Err(_) => {
                    show_error("代理端口必须是 1–65535 的整数");
                    return;
                }
            };
            let config = ProxyConfig {
                enabled: true,
                host: window.get_proxy_host().to_string(),
                port,
            };
            match proxy::test_proxy(&config) {
                Ok(()) => window.set_status_text("代理连接测试成功".into()),
                Err(error) => show_error(&error),
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = Rc::clone(state);
        window.on_save_proxy(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let port = match window.get_proxy_port().parse::<u16>() {
                Ok(port) => port,
                Err(_) => {
                    show_error("代理端口必须是 1–65535 的整数");
                    return;
                }
            };
            let config = ProxyConfig {
                enabled: window.get_proxy_enabled(),
                host: window.get_proxy_host().to_string(),
                port,
            };
            match proxy::validate_proxy(config) {
                Ok(config) => {
                    let mut state = state.borrow_mut();
                    state.model.config.proxy = config;
                    match state.model.save() {
                        Ok(()) => {
                            let label = platform::proxy_label(&state.model.config.proxy);
                            drop(state);
                            window.set_proxy_dialog_open(false);
                            window.set_status_text(format!("代理设置已保存 · {label}").into());
                        }
                        Err(error) => show_error(&error),
                    }
                }
                Err(error) => show_error(&error),
            }
        });
    }
}

fn install_file_drop(window: &AppWindow, state: &Rc<RefCell<State>>) {
    let weak = window.as_weak();
    let state = Rc::clone(state);
    window
        .window()
        .on_winit_window_event(move |_window, event| {
            let Some(window) = weak.upgrade() else {
                return EventResult::Propagate;
            };
            match event {
                winit::event::WindowEvent::HoveredFile(path) => {
                    window.set_drag_active(true);
                    window.set_drag_status(path.to_string_lossy().into_owned().into());
                }
                winit::event::WindowEvent::HoveredFileCancelled => {
                    window.set_drag_active(false);
                }
                winit::event::WindowEvent::DroppedFile(path) => {
                    window.set_drag_active(false);
                    add_dropped_project(&window, &state, path);
                }
                _ => {}
            }
            EventResult::Propagate
        });
}

fn add_dropped_project(window: &AppWindow, state: &Rc<RefCell<State>>, path: &std::path::Path) {
    if !path.is_dir() {
        window.set_status_text("拖拽添加仅支持文件夹".into());
        return;
    }
    let path_text = path.to_string_lossy().into_owned();
    let default_cli = {
        let state = state.borrow();
        PRESETS
            .iter()
            .find(|preset| {
                state
                    .model
                    .installed
                    .get(preset.id)
                    .copied()
                    .unwrap_or(false)
            })
            .unwrap_or(&PRESETS[0])
            .id
    };
    let result = state
        .borrow_mut()
        .model
        .add_project(&path_text, default_cli, &[]);
    match result {
        Ok(()) => {
            reload_all_sessions(state);
            refresh_ui(window, state, None);
            window.set_status_text(format!("已拖拽添加：{}", path.display()).into());
        }
        Err(error) => window.set_status_text(error.into()),
    }
}

fn reload_all_sessions(state: &Rc<RefCell<State>>) {
    let projects = state.borrow().model.config.projects.clone();
    let sessions = projects
        .iter()
        .map(|project| {
            (
                project.id.clone(),
                sessions::list(&project.default_cli, &project.path),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut state = state.borrow_mut();
    state.sessions = sessions;
    let valid = state
        .sessions
        .iter()
        .flat_map(|(project_id, sessions)| {
            sessions
                .iter()
                .map(move |session| (project_id.clone(), session.id.clone()))
        })
        .collect::<std::collections::HashSet<_>>();
    state
        .selected_sessions
        .retain(|project_id, session_id| valid.contains(&(project_id.clone(), session_id.clone())));
}

fn reload_project_sessions(state: &Rc<RefCell<State>>, project_id: &str) {
    let project = state.borrow().model.project(project_id);
    let Some(project) = project else {
        return;
    };
    let sessions = sessions::list(&project.default_cli, &project.path);
    let mut state = state.borrow_mut();
    state.sessions.insert(project_id.to_string(), sessions);
    state.selected_sessions.remove(project_id);
}

fn refresh_ui(window: &AppWindow, state: &Rc<RefCell<State>>, preferred_id: Option<&str>) {
    let search = window.get_search_text().to_string().to_lowercase();
    let current_id = {
        let state = state.borrow();
        selected_id(window, &state)
    };
    let preserve = preferred_id.map(str::to_string).or(current_id);
    let (rows, status, selected_index) = {
        let mut state = state.borrow_mut();
        let mut projects = state.model.config.projects.clone();
        projects.sort_by(|left, right| left.added_at.cmp(&right.added_at));
        projects.retain(|project| {
            search.trim().is_empty()
                || project.name().to_lowercase().contains(search.trim())
                || project.path.to_lowercase().contains(search.trim())
        });
        state.visible_ids = projects.iter().map(|project| project.id.clone()).collect();
        let selected_index = preserve
            .as_ref()
            .and_then(|id| {
                state
                    .visible_ids
                    .iter()
                    .position(|candidate| candidate == id)
            })
            .or((!projects.is_empty()).then_some(0))
            .map(|index| index as i32)
            .unwrap_or(-1);
        let rows = projects
            .iter()
            .map(|project| {
                let (command_options, command_index) =
                    project_command_choices(&state.model, project);
                let project_sessions = state.sessions.get(&project.id).cloned().unwrap_or_default();
                let mut session_options = vec![SharedString::from("新建会话")];
                session_options.extend(
                    project_sessions
                        .iter()
                        .map(|session| SharedString::from(session.label.as_str())),
                );
                let session_index = state
                    .selected_sessions
                    .get(&project.id)
                    .and_then(|selected| {
                        project_sessions
                            .iter()
                            .position(|session| &session.id == selected)
                    })
                    .map(|index| index as i32 + 1)
                    .unwrap_or(0);
                ProjectRow {
                    id: project.id.clone().into(),
                    name: project.name().into(),
                    path: project.path.clone().into(),
                    cli_id: project.default_cli.clone().into(),
                    cli: command_label(&state.model, project, &project.default_cli).into(),
                    command_options,
                    command_index,
                    session_options: ModelRc::new(VecModel::from(session_options)),
                    session_index,
                    recent: if project.last_launched.is_empty() {
                        "尚未启动".into()
                    } else {
                        relative_time(&project.last_launched).into()
                    },
                    available: project.path_available(),
                    launchable: project.path_available()
                        && command_available(&state.model, project, &project.default_cli),
                }
            })
            .collect::<Vec<_>>();
        let installed = state
            .model
            .installed
            .values()
            .filter(|value| **value)
            .count();
        let mut status = format!(
            "{} 个项目 · {}/{} 个 CLI 可用 · {}",
            projects.len(),
            installed,
            PRESETS.len(),
            platform::proxy_label(&state.model.config.proxy)
        );
        if !state.model.notice.is_empty() {
            status.push_str(" · ");
            status.push_str(&state.model.notice);
        }
        (rows, status, selected_index)
    };
    window.set_projects(ModelRc::new(VecModel::from(rows)));
    window.set_selected_index(selected_index);
    window.set_status_text(status.into());
}

fn selected_id(window: &AppWindow, state: &State) -> Option<String> {
    state
        .visible_ids
        .get(window.get_selected_index().max(0) as usize)
        .cloned()
}

fn selected_project(window: &AppWindow, state: &State) -> Option<Project> {
    selected_id(window, state).and_then(|id| state.model.project(&id))
}

fn command_label(model: &AppModel, project: &Project, id: &str) -> String {
    if let Some(index) = id.strip_prefix("custom:")
        && let Ok(index) = index.parse::<usize>()
    {
        return project
            .custom_commands
            .get(index)
            .map(|command| format!("自定义：{}", truncate(command, 24)))
            .unwrap_or_else(|| "自定义命令".to_string());
    }
    model
        .preset(id)
        .map(|preset| preset.name.to_string())
        .unwrap_or_else(|| id.to_string())
}

fn project_command_choices(model: &AppModel, project: &Project) -> (ModelRc<SharedString>, i32) {
    let mut labels = Vec::new();
    let mut selected = 0;
    for (index, preset) in PRESETS.iter().enumerate() {
        let available = model.installed.get(preset.id).copied().unwrap_or(false);
        labels.push(
            if available {
                preset.name.to_string()
            } else {
                format!("{}（未安装）", preset.name)
            }
            .into(),
        );
        if project.default_cli == preset.id {
            selected = index as i32;
        }
    }
    for (index, command) in project.custom_commands.iter().enumerate() {
        labels.push(format!("自定义 {}：{}", index + 1, truncate(command, 24)).into());
        if project.default_cli == format!("custom:{index}") {
            selected = (PRESETS.len() + index) as i32;
        }
    }
    (ModelRc::new(VecModel::from(labels)), selected)
}

fn command_id_at(project: &Project, index: i32) -> Option<String> {
    let index = usize::try_from(index).ok()?;
    if let Some(preset) = PRESETS.get(index) {
        return Some(preset.id.to_string());
    }
    let custom_index = index.checked_sub(PRESETS.len())?;
    project
        .custom_commands
        .get(custom_index)
        .map(|_| format!("custom:{custom_index}"))
}

fn command_available(model: &AppModel, project: &Project, id: &str) -> bool {
    if let Some(index) = id.strip_prefix("custom:")
        && let Ok(index) = index.parse::<usize>()
    {
        return project
            .custom_commands
            .get(index)
            .is_some_and(|command| !command.trim().is_empty());
    }
    model.installed.get(id).copied().unwrap_or(false)
}

fn pick_folder(current: Option<&str>) -> Option<std::path::PathBuf> {
    let mut dialog = FileDialog::new();
    if let Some(current) = current
        && std::path::Path::new(current).is_dir()
    {
        dialog = dialog.set_directory(current);
    }
    dialog.pick_folder()
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    format!(
        "{}…",
        value
            .chars()
            .take(max.saturating_sub(1))
            .collect::<String>()
    )
}

fn show_error(message: &str) {
    let _ = MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title("CLI 启动器")
        .set_description(message)
        .set_buttons(MessageButtons::Ok)
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifies_an_existing_instance() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let receiver = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut command = vec![0u8; INSTANCE_ACTIVATE_COMMAND.len()];
            stream.read_exact(&mut command).unwrap();
            command
        });

        notify_instance(address).unwrap();
        assert_eq!(receiver.join().unwrap(), INSTANCE_ACTIVATE_COMMAND);
    }
}
