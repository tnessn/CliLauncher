use crate::config::write_json;
use crate::model::{AppModel, ProxyConfig};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const HELPER_IDLE_SECONDS: u64 = 12 * 60 * 60;

#[derive(Debug, Clone, Default)]
pub struct ProxyRuntime {
    pub socks_url: String,
    pub http_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProxyHelperState {
    upstream: String,
    listen: String,
    pid: u32,
}

pub fn validate_proxy(mut config: ProxyConfig) -> Result<ProxyConfig, String> {
    config.host = config
        .host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    if config.host.is_empty() {
        config.host = "127.0.0.1".to_string();
    }
    if config
        .host
        .chars()
        .any(|character| character.is_whitespace() || "/\\".contains(character))
    {
        return Err("SOCKS5 代理主机格式不正确".to_string());
    }
    if config.port == 0 {
        config.port = 7890;
    }
    Ok(config)
}

pub fn test_proxy(config: &ProxyConfig) -> Result<(), String> {
    let config = validate_proxy(config.clone())?;
    let upstream = format_host_port(&config.host, config.port);
    let stream = connect_socks(&upstream, "example.com:443", Duration::from_secs(8))?;
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

pub fn runtime_for_launch(model: &AppModel) -> Result<ProxyRuntime, String> {
    if !model.config.proxy.enabled {
        return Ok(ProxyRuntime::default());
    }
    let config = validate_proxy(model.config.proxy.clone())?;
    let upstream = format_host_port(&config.host, config.port);
    probe_endpoint(&upstream, Duration::from_secs(2))?;
    let socks_url = format!("socks5://{upstream}");
    let state_path = model.config_dir.join("proxy-helper.json");
    if let Ok(state) = read_state(&state_path)
        && state.upstream == upstream
        && probe_endpoint(&state.listen, Duration::from_millis(400)).is_ok()
    {
        return Ok(ProxyRuntime {
            socks_url,
            http_url: format!("http://{}", state.listen),
        });
    }
    start_helper(&upstream, &socks_url, &state_path, &model.config_dir)
}

fn start_helper(
    upstream: &str,
    socks_url: &str,
    state_path: &Path,
    config_dir: &Path,
) -> Result<ProxyRuntime, String> {
    let ready_path = config_dir.join(format!(
        "proxy-helper-{}-{}.ready.json",
        std::process::id(),
        now_unix_nanos()
    ));
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command = Command::new(executable);
    command
        .args([
            "--proxy-helper",
            upstream,
            &ready_path.to_string_lossy(),
            &state_path.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动代理桥接：{error}"))?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if let Ok(state) = read_state(&ready_path)
            && state.upstream == upstream
            && probe_endpoint(&state.listen, Duration::from_millis(400)).is_ok()
        {
            let _ = fs::remove_file(&ready_path);
            return Ok(ProxyRuntime {
                socks_url: socks_url.to_string(),
                http_url: format!("http://{}", state.listen),
            });
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    Err("代理桥接启动超时".to_string())
}

pub fn run_helper_mode(arguments: &[String]) -> Option<Result<(), String>> {
    if arguments.first().map(String::as_str) != Some("--proxy-helper") {
        return None;
    }
    if arguments.len() != 4 {
        return Some(Err("代理桥接启动参数不完整".to_string()));
    }
    Some(serve_helper(
        &arguments[1],
        Path::new(&arguments[2]),
        Path::new(&arguments[3]),
    ))
}

fn serve_helper(upstream: &str, ready_path: &Path, state_path: &Path) -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let state = ProxyHelperState {
        upstream: upstream.to_string(),
        listen: listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .to_string(),
        pid: std::process::id(),
    };
    write_json(state_path, &state).map_err(|error| error.to_string())?;
    write_json(ready_path, &state).map_err(|error| error.to_string())?;
    let last_activity = Arc::new(AtomicU64::new(now_unix_seconds()));
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                last_activity.store(now_unix_seconds(), Ordering::Relaxed);
                let upstream = upstream.to_string();
                let activity = Arc::clone(&last_activity);
                thread::spawn(move || {
                    let _ = handle_http_proxy(stream, &upstream);
                    activity.store(now_unix_seconds(), Ordering::Relaxed);
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if now_unix_seconds().saturating_sub(last_activity.load(Ordering::Relaxed))
                    >= HELPER_IDLE_SECONDS
                {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    if read_state(state_path).is_ok_and(|current| current.pid == state.pid) {
        let _ = fs::remove_file(state_path);
    }
    Ok(())
}

fn handle_http_proxy(mut client: TcpStream, upstream: &str) -> Result<(), String> {
    client
        .set_nonblocking(false)
        .map_err(|error| error.to_string())?;
    client
        .set_read_timeout(Some(Duration::from_secs(20)))
        .map_err(|error| error.to_string())?;
    let mut request = Vec::with_capacity(4096);
    let mut buffer = [0u8; 4096];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = client
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Ok(());
        }
        request.extend_from_slice(&buffer[..count]);
        if request.len() > 64 * 1024 {
            return Err("HTTP 代理请求头过大".to_string());
        }
    }
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .ok_or_else(|| "HTTP 请求不完整".to_string())?;
    let header = String::from_utf8_lossy(&request[..header_end]).into_owned();
    let first_line = header.lines().next().unwrap_or_default();
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method.eq_ignore_ascii_case("CONNECT") {
        let mut destination = connect_socks(upstream, target, Duration::from_secs(15))?;
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .map_err(|error| error.to_string())?;
        if header_end < request.len() {
            destination
                .write_all(&request[header_end..])
                .map_err(|error| error.to_string())?;
        }
        client
            .set_read_timeout(None)
            .map_err(|error| error.to_string())?;
        client
            .set_write_timeout(None)
            .map_err(|error| error.to_string())?;
        relay(client, destination);
        return Ok(());
    }

    let (host, path) = parse_http_target(target, &header)?;
    let destination_target = if host.contains(':') {
        host.clone()
    } else {
        format!("{host}:80")
    };
    let mut destination = connect_socks(upstream, &destination_target, Duration::from_secs(15))?;
    let version = parts.next().unwrap_or("HTTP/1.1");
    let mut rewritten = format!("{method} {path} {version}\r\n");
    for line in header.lines().skip(1) {
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("proxy-connection:") || lower.starts_with("connection:") {
            continue;
        }
        rewritten.push_str(line);
        rewritten.push_str("\r\n");
    }
    rewritten.push_str("Connection: close\r\n\r\n");
    destination
        .write_all(rewritten.as_bytes())
        .map_err(|error| error.to_string())?;
    if header_end < request.len() {
        destination
            .write_all(&request[header_end..])
            .map_err(|error| error.to_string())?;
    }
    std::io::copy(&mut destination, &mut client).map_err(|error| error.to_string())?;
    Ok(())
}

fn parse_http_target(target: &str, header: &str) -> Result<(String, String), String> {
    if let Some(rest) = target.strip_prefix("http://") {
        let split = rest.find('/').unwrap_or(rest.len());
        let host = rest[..split].to_string();
        let path = if split < rest.len() {
            rest[split..].to_string()
        } else {
            "/".to_string()
        };
        return Ok((host, path));
    }
    let host = header
        .lines()
        .find_map(|line| {
            line.strip_prefix("Host:")
                .or_else(|| line.strip_prefix("host:"))
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "HTTP 请求缺少 Host".to_string())?;
    Ok((host.to_string(), target.to_string()))
}

fn relay(left: TcpStream, right: TcpStream) {
    let Ok(mut left_read) = left.try_clone() else {
        return;
    };
    let Ok(mut right_read) = right.try_clone() else {
        return;
    };
    let mut left_write = left;
    let mut right_write = right;
    let first = thread::spawn(move || {
        let _ = std::io::copy(&mut left_read, &mut right_write);
        let _ = right_write.shutdown(Shutdown::Write);
    });
    let second = thread::spawn(move || {
        let _ = std::io::copy(&mut right_read, &mut left_write);
        let _ = left_write.shutdown(Shutdown::Write);
    });
    let _ = first.join();
    let _ = second.join();
}

fn connect_socks(upstream: &str, target: &str, timeout: Duration) -> Result<TcpStream, String> {
    let address = resolve_one(upstream)?;
    let mut stream = TcpStream::connect_timeout(&address, timeout)
        .map_err(|error| format!("无法连接 SOCKS5 代理 {upstream}：{error}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    stream
        .write_all(&[5, 1, 0])
        .map_err(|error| error.to_string())?;
    let mut greeting = [0u8; 2];
    stream
        .read_exact(&mut greeting)
        .map_err(|error| error.to_string())?;
    if greeting != [5, 0] {
        return Err("SOCKS5 代理不支持无认证连接".to_string());
    }
    let (host, port) = split_target(target)?;
    let host_bytes = host.as_bytes();
    if host_bytes.len() > 255 {
        return Err("SOCKS5 目标主机过长".to_string());
    }
    let mut request = vec![5, 1, 0, 3, host_bytes.len() as u8];
    request.extend_from_slice(host_bytes);
    request.extend_from_slice(&port.to_be_bytes());
    stream
        .write_all(&request)
        .map_err(|error| error.to_string())?;
    let mut response = [0u8; 4];
    stream
        .read_exact(&mut response)
        .map_err(|error| error.to_string())?;
    if response[0] != 5 || response[1] != 0 {
        return Err(format!("SOCKS5 上游拒绝连接，状态码 {}", response[1]));
    }
    let remaining = match response[3] {
        1 => 4,
        3 => {
            let mut length = [0u8; 1];
            stream
                .read_exact(&mut length)
                .map_err(|error| error.to_string())?;
            length[0] as usize
        }
        4 => 16,
        _ => return Err("SOCKS5 返回了未知地址类型".to_string()),
    };
    let mut ignored = vec![0u8; remaining + 2];
    stream
        .read_exact(&mut ignored)
        .map_err(|error| error.to_string())?;
    stream.set_read_timeout(None).ok();
    stream.set_write_timeout(None).ok();
    Ok(stream)
}

fn split_target(target: &str) -> Result<(String, u16), String> {
    if let Some(rest) = target.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        let host = rest[..end].to_string();
        let port = rest[end + 1..]
            .strip_prefix(':')
            .ok_or_else(|| "目标端口缺失".to_string())?
            .parse::<u16>()
            .map_err(|_| "目标端口无效".to_string())?;
        return Ok((host, port));
    }
    let (host, port) = target
        .rsplit_once(':')
        .ok_or_else(|| "目标地址缺少端口".to_string())?;
    Ok((
        host.to_string(),
        port.parse::<u16>()
            .map_err(|_| "目标端口无效".to_string())?,
    ))
}

fn resolve_one(address: &str) -> Result<SocketAddr, String> {
    address
        .to_socket_addrs()
        .map_err(|error| error.to_string())?
        .next()
        .ok_or_else(|| "无法解析代理地址".to_string())
}

fn probe_endpoint(address: &str, timeout: Duration) -> Result<(), String> {
    let address = resolve_one(address)?;
    TcpStream::connect_timeout(&address, timeout)
        .map(|_| ())
        .map_err(|error| format!("无法连接代理：{error}"))
}

fn format_host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn read_state(path: &Path) -> Result<ProxyHelperState, String> {
    let data = fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&data).map_err(|error| error.to_string())
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_absolute_http_target() {
        let (host, path) = parse_http_target("http://example.com/a?b=1", "").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(path, "/a?b=1");
    }

    #[test]
    fn validates_proxy_defaults() {
        let proxy = validate_proxy(ProxyConfig {
            enabled: true,
            host: "".to_string(),
            port: 0,
        })
        .unwrap();
        assert_eq!(proxy.host, "127.0.0.1");
        assert_eq!(proxy.port, 7890);
    }

    #[test]
    fn relay_waits_for_delayed_tunnel_data() {
        fn tcp_pair() -> (TcpStream, TcpStream) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let client = TcpStream::connect(address).unwrap();
            let (server, _) = listener.accept().unwrap();
            (client, server)
        }

        let (mut client, relay_client) = tcp_pair();
        let (mut server, relay_server) = tcp_pair();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        relay_client.set_nonblocking(true).unwrap();
        relay_client.set_nonblocking(false).unwrap();
        relay_client
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        relay_client.set_read_timeout(None).unwrap();
        assert_eq!(relay_client.read_timeout().unwrap(), None);

        let relay_thread = thread::spawn(move || relay(relay_client, relay_server));
        thread::sleep(Duration::from_millis(75));

        client.write_all(b"client-to-server").unwrap();
        let mut request = [0u8; 16];
        server.read_exact(&mut request).unwrap();
        assert_eq!(&request, b"client-to-server");

        server.write_all(b"server-to-client").unwrap();
        let mut response = [0u8; 16];
        client.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"server-to-client");

        client.shutdown(Shutdown::Both).unwrap();
        server.shutdown(Shutdown::Both).unwrap();
        relay_thread.join().unwrap();
    }
}
