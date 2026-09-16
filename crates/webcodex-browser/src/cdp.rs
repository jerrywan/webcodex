use crate::types::{
    clip_bytes, BrowserError, BrowserKey, BrowserResult, LAUNCH_TIMEOUT, MAX_NODE_TEXT_BYTES,
    MAX_PAGES_PER_BROWSER, MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_NODES, REQUEST_TIMEOUT,
};
use serde_json::{json, Value};
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tungstenite::{client::client_with_config, protocol::WebSocketConfig, Message, WebSocket};
use url::Url;
use webcodex_process::ManagedChild;

const MAX_CDP_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_CDP_LIST_BYTES: usize = 256 * 1024;

pub(crate) trait BackendFactory: Send + Sync {
    fn available(&self) -> bool;
    fn launch(&self) -> BrowserResult<Box<dyn BrowserBackend>>;
}

pub(crate) trait BrowserBackend: Send {
    fn pages(&mut self) -> BrowserResult<Vec<BackendPage>>;
    fn new_page(&mut self) -> BrowserResult<String>;
    fn snapshot(&mut self, target_id: &str) -> BrowserResult<BackendSnapshot>;
    fn screenshot(&mut self, target_id: &str) -> BrowserResult<BackendScreenshot>;
    fn navigate(&mut self, target_id: &str, url: &str) -> BrowserResult<()>;
    fn click(&mut self, target_id: &str, backend_node_id: i64) -> BrowserResult<()>;
    fn input_text(
        &mut self,
        target_id: &str,
        backend_node_id: i64,
        text: &str,
    ) -> BrowserResult<()>;
    fn key(&mut self, target_id: &str, key: BrowserKey) -> BrowserResult<()>;
    fn close_page(&mut self, target_id: &str) -> BrowserResult<()>;
    fn shutdown(&mut self, timeout: Duration) -> BrowserResult<()>;
}

#[derive(Debug, Clone)]
pub(crate) struct BackendPage {
    pub(crate) target_id: String,
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) document_id: String,
}

#[derive(Debug)]
pub(crate) struct BackendNode {
    pub(crate) role: String,
    pub(crate) name: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) backend_node_id: Option<i64>,
    pub(crate) actionable: bool,
}

#[derive(Debug)]
pub(crate) struct BackendSnapshot {
    pub(crate) document_id: String,
    pub(crate) nodes: Vec<BackendNode>,
    pub(crate) truncated: bool,
}

#[derive(Debug)]
pub(crate) struct BackendScreenshot {
    pub(crate) data: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

pub(crate) struct ChromiumFactory;

impl BackendFactory for ChromiumFactory {
    fn available(&self) -> bool {
        discover_chromium_executable().is_some()
    }

    fn launch(&self) -> BrowserResult<Box<dyn BrowserBackend>> {
        let executable = discover_chromium_executable().ok_or_else(|| {
            BrowserError::not_started(
                "browser_unavailable",
                "no supported Chromium-family browser is installed",
            )
        })?;
        CdpBackend::launch(&executable).map(|backend| Box::new(backend) as Box<dyn BrowserBackend>)
    }
}

pub fn discover_chromium_executable() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let mut candidates = Vec::new();
        for variable in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(variable) {
                let base = PathBuf::from(base);
                candidates.push(base.join("Microsoft/Edge/Application/msedge.exe"));
                candidates.push(base.join("Google/Chrome/Application/chrome.exe"));
            }
        }
        for candidate in candidates {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        for executable in ["msedge.exe", "chrome.exe"] {
            if let Some(path) = find_in_path(executable) {
                return Some(path);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        for candidate in [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ] {
            let path = PathBuf::from(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for executable in [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
        ] {
            if let Some(path) = find_in_path(executable) {
                return Some(path);
            }
        }
    }

    None
}

fn parse_devtools_active_port(contents: &str) -> Option<(u16, String)> {
    let mut lines = contents.lines();
    let port = lines.next()?.parse::<u16>().ok()?;
    let path = lines.next()?;
    if port == 0 || !path.starts_with("/devtools/browser/") || path.contains(['\r', '\n']) {
        return None;
    }
    Some((port, path.to_string()))
}

fn find_in_path(executable: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|path| path.join(executable))
        .find(|path| path.is_file())
}

fn cleanup_failed_launch(child: &mut ManagedChild) {
    const FAILED_LAUNCH_CLEANUP_TIMEOUT: Duration = Duration::from_millis(500);
    let _ = child.terminate_tree();
    let _ = child.wait_tree_exit(FAILED_LAUNCH_CLEANUP_TIMEOUT);
    let _ = child.try_wait();
}

struct CdpBackend {
    child: ManagedChild,
    _profile: TempDir,
    endpoint: Url,
    next_id: u64,
}

impl CdpBackend {
    fn launch(executable: &Path) -> BrowserResult<Self> {
        let profile = tempfile::Builder::new()
            .prefix("webcodex-browser-")
            .tempdir()
            .map_err(|error| {
                BrowserError::not_started(
                    "profile_create_failed",
                    format!("could not create ephemeral Browser profile: {error}"),
                )
            })?;
        let mut command = Command::new(executable);
        command
            .arg("--headless=new")
            .arg("--remote-debugging-address=127.0.0.1")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-background-networking")
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = ManagedChild::spawn(&mut command).map_err(|error| {
            BrowserError::not_started(
                "launch_failed",
                format!("could not start Browser runtime: {error}"),
            )
        })?;
        let active_port = profile.path().join("DevToolsActivePort");
        let deadline = Instant::now() + LAUNCH_TIMEOUT;
        let (port, websocket_path) = loop {
            if let Ok(contents) = std::fs::read_to_string(&active_port) {
                if let Some(endpoint) = parse_devtools_active_port(&contents) {
                    break endpoint;
                }
            }
            if child.try_wait().ok().flatten().is_some() {
                cleanup_failed_launch(&mut child);
                return Err(BrowserError::not_started(
                    "launch_failed",
                    "Browser exited before loopback CDP became ready",
                ));
            }
            if Instant::now() >= deadline {
                cleanup_failed_launch(&mut child);
                return Err(BrowserError::not_started(
                    "launch_timeout",
                    "Browser did not expose loopback CDP before launch timeout",
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let endpoint = match Url::parse(&format!("ws://127.0.0.1:{port}{websocket_path}")) {
            Ok(endpoint) => endpoint,
            Err(_) => {
                cleanup_failed_launch(&mut child);
                return Err(BrowserError::not_started(
                    "cdp_endpoint_invalid",
                    "Browser returned an invalid loopback CDP endpoint",
                ));
            }
        };
        if endpoint.host_str() != Some("127.0.0.1") {
            cleanup_failed_launch(&mut child);
            return Err(BrowserError::not_started(
                "cdp_endpoint_invalid",
                "Browser CDP endpoint was not loopback",
            ));
        }
        Ok(Self {
            child,
            _profile: profile,
            endpoint,
            next_id: 1,
        })
    }

    fn browser_call(&mut self, method: &str, params: Value, effect: bool) -> BrowserResult<Value> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        self.browser_call_until(method, params, effect, deadline)
    }

    fn browser_call_until(
        &mut self,
        method: &str,
        params: Value,
        effect: bool,
        deadline: Instant,
    ) -> BrowserResult<Value> {
        cdp_call_until(
            &self.endpoint,
            &mut self.next_id,
            method,
            params,
            effect,
            deadline,
        )
    }

    fn page_descriptors_until(&self, deadline: Instant) -> BrowserResult<Vec<Value>> {
        fetch_page_descriptors(self.endpoint.port().unwrap_or_default(), deadline)
    }

    fn page_endpoint_until(&self, target_id: &str, deadline: Instant) -> BrowserResult<Url> {
        let list = self.page_descriptors_until(deadline)?;
        let websocket_url = list
            .into_iter()
            .find(|value| value.get("id").and_then(Value::as_str) == Some(target_id))
            .and_then(|value| {
                value
                    .get("webSocketDebuggerUrl")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .ok_or_else(|| {
                BrowserError::not_started("stale_page", "page target is no longer available")
            })?;
        let endpoint = Url::parse(&websocket_url).map_err(|_| {
            BrowserError::observed("cdp_endpoint_invalid", "page CDP endpoint is invalid", None)
        })?;
        if !matches!(endpoint.host_str(), Some("127.0.0.1") | Some("localhost"))
            || endpoint.scheme() != "ws"
            || endpoint.port().is_none()
        {
            return Err(BrowserError::observed(
                "cdp_endpoint_invalid",
                "page CDP endpoint is not a bounded loopback websocket",
                None,
            ));
        }
        Ok(endpoint)
    }

    fn page_call_until(
        &mut self,
        target_id: &str,
        method: &str,
        params: Value,
        effect: bool,
        deadline: Instant,
    ) -> BrowserResult<Value> {
        let endpoint = self
            .page_endpoint_until(target_id, deadline)
            .map_err(|error| {
                if effect {
                    pre_dispatch_error(error)
                } else {
                    error
                }
            })?;
        cdp_call_until(
            &endpoint,
            &mut self.next_id,
            method,
            params,
            effect,
            deadline,
        )
    }

    fn page_list_until(&self, deadline: Instant) -> BrowserResult<Vec<BackendPage>> {
        let list = self.page_descriptors_until(deadline)?;
        let mut pages = Vec::new();
        for value in list {
            if value.get("type").and_then(Value::as_str) != Some("page") {
                continue;
            }
            let Some(target_id) = value.get("id").and_then(Value::as_str) else {
                continue;
            };
            pages.push(BackendPage {
                target_id: target_id.to_string(),
                title: value
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                url: value
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                document_id: value
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("about:blank")
                    .to_string(),
            });
            if pages.len() >= MAX_PAGES_PER_BROWSER {
                break;
            }
        }
        Ok(pages)
    }
}

impl BrowserBackend for CdpBackend {
    fn pages(&mut self) -> BrowserResult<Vec<BackendPage>> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let mut pages = self.page_list_until(deadline)?;
        for page in &mut pages {
            if Instant::now() >= deadline {
                break;
            }
            if let Ok(tree) = self.page_call_until(
                &page.target_id,
                "Page.getFrameTree",
                json!({}),
                false,
                deadline,
            ) {
                if let Some(loader_id) = tree
                    .pointer("/frameTree/frame/loaderId")
                    .and_then(Value::as_str)
                {
                    page.document_id = loader_id.to_string();
                }
            }
        }
        Ok(pages)
    }

    fn new_page(&mut self) -> BrowserResult<String> {
        let result =
            self.browser_call("Target.createTarget", json!({ "url": "about:blank" }), true)?;
        result
            .get("targetId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                BrowserError::uncertain(
                    "create_page_result_invalid",
                    "CDP did not return targetId after page creation",
                    "pages",
                )
            })
    }

    fn snapshot(&mut self, target_id: &str) -> BrowserResult<BackendSnapshot> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let frame_tree =
            self.page_call_until(target_id, "Page.getFrameTree", json!({}), false, deadline)?;
        let document_id = frame_tree
            .pointer("/frameTree/frame/loaderId")
            .and_then(Value::as_str)
            .unwrap_or("unknown-document")
            .to_string();
        let accessibility = self.page_call_until(
            target_id,
            "Accessibility.getFullAXTree",
            json!({ "depth": 32 }),
            false,
            deadline,
        )?;
        let raw_nodes = accessibility
            .get("nodes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut nodes = Vec::new();
        let mut truncated = raw_nodes.len() > MAX_SNAPSHOT_NODES;
        let mut estimated_bytes = 0usize;
        for raw in raw_nodes.into_iter().take(MAX_SNAPSHOT_NODES) {
            let role = ax_value(&raw, "role").unwrap_or_else(|| "generic".to_string());
            if role == "RootWebArea" {
                continue;
            }
            let name = ax_value(&raw, "name");
            let value = ax_value(&raw, "value");
            let backend_node_id = raw.get("backendDOMNodeId").and_then(Value::as_i64);
            let actionable = is_actionable(&role) && backend_node_id.is_some();
            estimated_bytes = estimated_bytes
                .saturating_add(role.len())
                .saturating_add(name.as_deref().map(str::len).unwrap_or(0))
                .saturating_add(value.as_deref().map(str::len).unwrap_or(0))
                .saturating_add(64);
            if estimated_bytes > MAX_SNAPSHOT_BYTES {
                truncated = true;
                break;
            }
            nodes.push(BackendNode {
                role,
                name,
                value,
                backend_node_id,
                actionable,
            });
        }
        Ok(BackendSnapshot {
            document_id,
            nodes,
            truncated,
        })
    }

    fn screenshot(&mut self, target_id: &str) -> BrowserResult<BackendScreenshot> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let metrics = self
            .page_call_until(
                target_id,
                "Page.getLayoutMetrics",
                json!({}),
                false,
                deadline,
            )
            .unwrap_or_else(|_| json!({}));
        let width = viewport_dimension(&metrics, "/cssVisualViewport/clientWidth");
        let height = viewport_dimension(&metrics, "/cssVisualViewport/clientHeight");
        let result = self.page_call_until(
            target_id,
            "Page.captureScreenshot",
            json!({
                "format": "png",
                "fromSurface": true,
                "captureBeyondViewport": false
            }),
            false,
            deadline,
        )?;
        let data = result
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BrowserError::observed("invalid_image", "CDP screenshot response had no data", None)
            })?
            .to_string();
        Ok(BackendScreenshot {
            data,
            width,
            height,
        })
    }

    fn navigate(&mut self, target_id: &str, url: &str) -> BrowserResult<()> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        self.page_call_until(
            target_id,
            "Page.navigate",
            json!({ "url": url }),
            true,
            deadline,
        )
        .map(|_| ())
    }

    fn click(&mut self, target_id: &str, backend_node_id: i64) -> BrowserResult<()> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let model = self
            .page_call_until(
                target_id,
                "DOM.getBoxModel",
                json!({ "backendNodeId": backend_node_id }),
                false,
                deadline,
            )
            .map_err(pre_dispatch_error)?;
        let quad = model
            .pointer("/model/content")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                BrowserError::not_started(
                    "element_not_actionable",
                    "element has no current box model",
                )
            })?;
        let coordinates = quad.iter().filter_map(Value::as_f64).collect::<Vec<_>>();
        if coordinates.len() < 8 {
            return Err(BrowserError::not_started(
                "element_not_actionable",
                "element box model is invalid",
            ));
        }
        let x = (coordinates[0] + coordinates[2] + coordinates[4] + coordinates[6]) / 4.0;
        let y = (coordinates[1] + coordinates[3] + coordinates[5] + coordinates[7]) / 4.0;
        self.page_call_until(
            target_id,
            "Input.dispatchMouseEvent",
            json!({
                "type": "mousePressed",
                "x": x,
                "y": y,
                "button": "left",
                "clickCount": 1
            }),
            true,
            deadline,
        )?;
        self.page_call_until(
            target_id,
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseReleased",
                "x": x,
                "y": y,
                "button": "left",
                "clickCount": 1
            }),
            true,
            deadline,
        )
        .map(|_| ())
        .map_err(|error| post_effect_error(error, "snapshot"))
    }

    fn input_text(
        &mut self,
        target_id: &str,
        backend_node_id: i64,
        text: &str,
    ) -> BrowserResult<()> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        self.page_call_until(
            target_id,
            "DOM.focus",
            json!({ "backendNodeId": backend_node_id }),
            true,
            deadline,
        )?;
        self.page_call_until(
            target_id,
            "Input.insertText",
            json!({ "text": text }),
            true,
            deadline,
        )
        .map(|_| ())
        .map_err(|error| post_effect_error(error, "snapshot"))
    }

    fn key(&mut self, target_id: &str, key: BrowserKey) -> BrowserResult<()> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let (key_name, code, text) = key.cdp();
        self.page_call_until(
            target_id,
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyDown",
                "key": key_name,
                "code": code,
                "text": text
            }),
            true,
            deadline,
        )?;
        self.page_call_until(
            target_id,
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp",
                "key": key_name,
                "code": code
            }),
            true,
            deadline,
        )
        .map(|_| ())
        .map_err(|error| post_effect_error(error, "snapshot"))
    }

    fn close_page(&mut self, target_id: &str) -> BrowserResult<()> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        self.browser_call_until(
            "Target.closeTarget",
            json!({ "targetId": target_id }),
            true,
            deadline,
        )
        .map(|_| ())
    }

    fn shutdown(&mut self, timeout: Duration) -> BrowserResult<()> {
        let deadline = Instant::now() + timeout;
        if !timeout.is_zero() {
            let _ = self.browser_call_until("Browser.close", json!({}), true, deadline);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let graceful = self.child.wait_tree_exit(remaining).unwrap_or(false);
        if !graceful {
            self.child.terminate_tree().map_err(|error| {
                BrowserError::uncertain(
                    "browser_shutdown_failed",
                    format!("could not terminate owned Browser process tree: {error}"),
                    "browsers",
                )
            })?;
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(1));
            let _ = self.child.wait_tree_exit(remaining);
        }
        let _ = self.child.try_wait();
        Ok(())
    }
}

fn pre_dispatch_error(mut error: BrowserError) -> BrowserError {
    error.execution_state = crate::types::ExecutionState::NotStarted;
    error.recovery_action = None;
    error
}

fn post_effect_error(error: BrowserError, recovery_action: &'static str) -> BrowserError {
    if error.execution_state == crate::types::ExecutionState::OutcomeUnknown {
        error
    } else {
        BrowserError::uncertain(
            "partial_effect_outcome_unknown",
            format!(
                "Browser effect partially completed before a later step failed: {}",
                error.message
            ),
            recovery_action,
        )
    }
}

fn viewport_dimension(metrics: &Value, pointer: &str) -> u32 {
    metrics
        .pointer(pointer)
        .and_then(Value::as_f64)
        .unwrap_or(1.0)
        .round()
        .clamp(1.0, 4096.0) as u32
}

fn ax_value(node: &Value, key: &str) -> Option<String> {
    node.get(key)?
        .get("value")?
        .as_str()
        .map(|value| clip_bytes(value, MAX_NODE_TEXT_BYTES))
}

fn is_actionable(role: &str) -> bool {
    matches!(
        role,
        "button"
            | "link"
            | "textbox"
            | "searchbox"
            | "combobox"
            | "checkbox"
            | "radio"
            | "switch"
            | "menuitem"
            | "tab"
            | "option"
    )
}

fn fetch_page_descriptors(port: u16, deadline: Instant) -> BrowserResult<Vec<Value>> {
    let remaining = remaining_before_dispatch(deadline)?;
    let http = format!("http://127.0.0.1:{port}/json/list");
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(remaining)
        .build()
        .map_err(|error| BrowserError::observed("cdp_connect_failed", error.to_string(), None))?;
    let mut response = client
        .get(http)
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|error| {
            BrowserError::observed(
                "cdp_list_failed",
                format!("could not observe CDP pages: {error}"),
                None,
            )
        })?;
    let mut body = Vec::new();
    response
        .by_ref()
        .take((MAX_CDP_LIST_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|error| {
            BrowserError::observed(
                "cdp_list_failed",
                format!("could not read bounded CDP page list: {error}"),
                None,
            )
        })?;
    if body.len() > MAX_CDP_LIST_BYTES {
        return Err(BrowserError::observed(
            "cdp_list_too_large",
            "CDP page list exceeded the bounded response size",
            None,
        ));
    }
    serde_json::from_slice(&body).map_err(|error| {
        BrowserError::observed(
            "cdp_list_failed",
            format!("could not decode CDP page list: {error}"),
            None,
        )
    })
}

fn remaining_before_dispatch(deadline: Instant) -> BrowserResult<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(BrowserError::not_started(
            "cdp_timeout",
            "Browser CDP request exceeded its absolute deadline before dispatch",
        ))
    } else {
        Ok(remaining)
    }
}

fn configure_socket_timeout(
    websocket: &mut WebSocket<TcpStream>,
    timeout: Duration,
) -> BrowserResult<()> {
    websocket
        .get_mut()
        .set_read_timeout(Some(timeout))
        .map_err(|error| {
            BrowserError::not_started(
                "cdp_timeout_config_failed",
                format!("could not bound CDP read timeout: {error}"),
            )
        })?;
    websocket
        .get_mut()
        .set_write_timeout(Some(timeout))
        .map_err(|error| {
            BrowserError::not_started(
                "cdp_timeout_config_failed",
                format!("could not bound CDP write timeout: {error}"),
            )
        })?;
    Ok(())
}

fn open_loopback_websocket(
    endpoint: &Url,
    deadline: Instant,
) -> BrowserResult<WebSocket<TcpStream>> {
    if !matches!(endpoint.host_str(), Some("127.0.0.1") | Some("localhost"))
        || endpoint.scheme() != "ws"
    {
        return Err(BrowserError::not_started(
            "cdp_endpoint_invalid",
            "Browser CDP transport must be a plain loopback websocket",
        ));
    }
    let port = endpoint.port().ok_or_else(|| {
        BrowserError::not_started("cdp_endpoint_invalid", "Browser CDP endpoint has no port")
    })?;
    let remaining = remaining_before_dispatch(deadline)?;
    let stream =
        TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), remaining)
            .map_err(|error| {
                BrowserError::not_started(
                    "cdp_connect_failed",
                    format!("could not connect to loopback CDP: {error}"),
                )
            })?;
    let _ = stream.set_nodelay(true);
    stream.set_read_timeout(Some(remaining)).map_err(|error| {
        BrowserError::not_started(
            "cdp_timeout_config_failed",
            format!("could not bound CDP handshake read timeout: {error}"),
        )
    })?;
    stream.set_write_timeout(Some(remaining)).map_err(|error| {
        BrowserError::not_started(
            "cdp_timeout_config_failed",
            format!("could not bound CDP handshake write timeout: {error}"),
        )
    })?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_CDP_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_CDP_MESSAGE_BYTES));
    let (mut websocket, _) =
        client_with_config(endpoint.as_str(), stream, Some(config)).map_err(|error| {
            BrowserError::not_started(
                "cdp_connect_failed",
                format!("could not complete loopback CDP handshake: {error}"),
            )
        })?;
    configure_socket_timeout(&mut websocket, remaining_before_dispatch(deadline)?)?;
    Ok(websocket)
}

#[cfg(test)]
fn cdp_call(
    endpoint: &Url,
    next_id: &mut u64,
    method: &str,
    params: Value,
    effect: bool,
) -> BrowserResult<Value> {
    cdp_call_until(
        endpoint,
        next_id,
        method,
        params,
        effect,
        Instant::now() + REQUEST_TIMEOUT,
    )
}

fn cdp_call_until(
    endpoint: &Url,
    next_id: &mut u64,
    method: &str,
    params: Value,
    effect: bool,
    deadline: Instant,
) -> BrowserResult<Value> {
    let id = *next_id;
    *next_id = next_id.saturating_add(1);
    let mut websocket = open_loopback_websocket(endpoint, deadline)?;
    let request = json!({ "id": id, "method": method, "params": params }).to_string();
    configure_socket_timeout(&mut websocket, remaining_before_dispatch(deadline)?)?;
    websocket
        .send(Message::Text(request.into()))
        .map_err(|error| {
            if effect {
                BrowserError::uncertain(
                    "cdp_delivery_uncertain",
                    format!("effect delivery became uncertain: {error}"),
                    "snapshot",
                )
            } else {
                BrowserError::observed("cdp_send_failed", error.to_string(), None)
            }
        })?;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(if effect {
                BrowserError::uncertain(
                    "cdp_receive_timeout",
                    "effect result exceeded the Browser CDP absolute deadline",
                    "snapshot",
                )
            } else {
                BrowserError::observed(
                    "cdp_receive_timeout",
                    "Browser CDP observation exceeded its absolute deadline",
                    None,
                )
            });
        }
        configure_socket_timeout(&mut websocket, remaining)?;
        match websocket.read() {
            Ok(Message::Text(text)) => {
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if value.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = value.get("error") {
                    return Err(if effect {
                        BrowserError::uncertain(
                            "cdp_effect_error",
                            format!(
                                "CDP effect returned an error after dispatch: {}",
                                clip_bytes(&error.to_string(), 256)
                            ),
                            "snapshot",
                        )
                    } else {
                        BrowserError::observed(
                            "cdp_error",
                            format!(
                                "CDP observation returned an error: {}",
                                clip_bytes(&error.to_string(), 256)
                            ),
                            None,
                        )
                    });
                }
                return Ok(value.get("result").cloned().unwrap_or_else(|| json!({})));
            }
            Ok(Message::Close(_)) => {
                return Err(if effect {
                    BrowserError::uncertain(
                        "cdp_closed",
                        "CDP connection closed after effect dispatch",
                        "snapshot",
                    )
                } else {
                    BrowserError::observed("cdp_closed", "CDP connection closed", None)
                })
            }
            Ok(_) => {}
            Err(error) => {
                return Err(if effect {
                    BrowserError::uncertain(
                        "cdp_receive_uncertain",
                        format!("effect result became uncertain: {error}"),
                        "snapshot",
                    )
                } else {
                    BrowserError::observed("cdp_receive_failed", error.to_string(), None)
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::net::TcpListener;
    use std::thread;
    use tungstenite::{accept, Message};

    fn fake_cdp_server(reply: Option<Value>) -> (Url, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut websocket = accept(stream).unwrap();
            let request = websocket.read().unwrap();
            let Message::Text(text) = request else {
                panic!("expected CDP text request")
            };
            let parsed: Value = serde_json::from_str(&text).unwrap();
            if let Some(result) = reply {
                websocket
                    .send(Message::Text(
                        json!({"id": parsed["id"], "result": result})
                            .to_string()
                            .into(),
                    ))
                    .unwrap();
            } else {
                websocket.close(None).unwrap();
            }
        });
        (
            Url::parse(&format!(
                "ws://127.0.0.1:{}/devtools/page/fake",
                address.port()
            ))
            .unwrap(),
            handle,
        )
    }

    fn fake_cdp_event_flood(events: usize, interval: Duration) -> (Url, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut websocket = accept(stream).unwrap();
            let request = websocket.read().unwrap();
            assert!(matches!(request, Message::Text(_)));
            for sequence in 0..events {
                thread::sleep(interval);
                if websocket
                    .send(Message::Text(
                        json!({"method": "Runtime.fixtureEvent", "params": {"sequence": sequence}})
                            .to_string()
                            .into(),
                    ))
                    .is_err()
                {
                    break;
                }
            }
        });
        (
            Url::parse(&format!(
                "ws://127.0.0.1:{}/devtools/page/flood",
                address.port()
            ))
            .unwrap(),
            handle,
        )
    }

    fn fake_http_body(body_len: usize) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            let header = format!(
                "HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: {body_len}
Connection: close

"
            );
            use std::io::Write;
            stream.write_all(header.as_bytes()).unwrap();
            let chunk = vec![b' '; body_len.min(8192)];
            let mut remaining = body_len;
            while remaining > 0 {
                let count = remaining.min(chunk.len());
                if stream.write_all(&chunk[..count]).is_err() {
                    break;
                }
                remaining -= count;
            }
        });
        (address.port(), handle)
    }

    #[test]
    fn devtools_active_port_is_parsed_only_as_loopback_browser_path_input() {
        assert_eq!(
            parse_devtools_active_port("9222\n/devtools/browser/opaque\n"),
            Some((9222, "/devtools/browser/opaque".to_string()))
        );
        assert!(parse_devtools_active_port("0\n/devtools/browser/x\n").is_none());
        assert!(
            parse_devtools_active_port("9222\nws://evil.example/devtools/browser/x\n").is_none()
        );
        assert!(parse_devtools_active_port("9222\n/devtools/page/x\n").is_none());
    }

    #[test]
    fn effect_composition_preserves_pre_dispatch_and_partial_effect_certainty() {
        let pre = pre_dispatch_error(BrowserError::observed("probe_failed", "probe", None));
        assert_eq!(
            pre.execution_state,
            crate::types::ExecutionState::NotStarted
        );
        assert_eq!(pre.recovery_action, None);

        let partial = post_effect_error(
            BrowserError::not_started("second_stage_not_started", "second stage"),
            "snapshot",
        );
        assert_eq!(
            partial.execution_state,
            crate::types::ExecutionState::OutcomeUnknown
        );
        assert_eq!(partial.recovery_action, Some("snapshot"));
    }

    #[test]
    fn irrelevant_cdp_events_do_not_rearm_the_absolute_deadline() {
        let (endpoint, handle) = fake_cdp_event_flood(40, Duration::from_millis(20));
        let mut next_id = 1;
        let started = Instant::now();
        let error = cdp_call_until(
            &endpoint,
            &mut next_id,
            "Page.getFrameTree",
            json!({}),
            false,
            started + Duration::from_millis(120),
        )
        .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_millis(600),
            "non-matching CDP events must not renew the request deadline"
        );
        assert!(matches!(
            error.kind,
            "cdp_receive_timeout" | "cdp_receive_failed"
        ));
        handle.join().unwrap();
    }

    #[test]
    fn oversized_cdp_message_fails_before_unbounded_materialization() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut websocket = accept(stream).unwrap();
            let _ = websocket.read().unwrap();
            let oversized = "x".repeat(MAX_CDP_MESSAGE_BYTES + 1);
            let _ = websocket.send(Message::Text(oversized.into()));
        });
        let endpoint = Url::parse(&format!(
            "ws://127.0.0.1:{}/devtools/page/oversized",
            address.port()
        ))
        .unwrap();
        let mut next_id = 1;
        let error = cdp_call_until(
            &endpoint,
            &mut next_id,
            "Page.getFrameTree",
            json!({}),
            false,
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap_err();
        assert_eq!(
            error.execution_state,
            crate::types::ExecutionState::Completed
        );
        assert_eq!(error.kind, "cdp_receive_failed");
        handle.join().unwrap();
    }

    #[test]
    fn oversized_cdp_page_list_is_rejected_at_the_transport_bound() {
        let (port, handle) = fake_http_body(MAX_CDP_LIST_BYTES + 1);
        let error =
            fetch_page_descriptors(port, Instant::now() + Duration::from_secs(1)).unwrap_err();
        assert_eq!(error.kind, "cdp_list_too_large");
        assert_eq!(
            error.execution_state,
            crate::types::ExecutionState::Completed
        );
        handle.join().unwrap();
    }

    #[test]
    fn fake_cdp_observation_round_trip_is_deterministic() {
        let (endpoint, handle) =
            fake_cdp_server(Some(json!({"frameTree": {"frame": {"loaderId": "doc"}}})));
        let mut next_id = 1;
        let result = cdp_call(
            &endpoint,
            &mut next_id,
            "Page.getFrameTree",
            json!({}),
            false,
        )
        .expect("fake CDP observation");
        assert_eq!(result["frameTree"]["frame"]["loaderId"], "doc");
        assert_eq!(next_id, 2);
        handle.join().unwrap();
    }

    #[test]
    fn fake_cdp_close_after_effect_dispatch_is_outcome_unknown() {
        let (endpoint, handle) = fake_cdp_server(None);
        let mut next_id = 7;
        let error = cdp_call(
            &endpoint,
            &mut next_id,
            "Page.navigate",
            json!({"url": "https://example.test/"}),
            true,
        )
        .unwrap_err();
        assert_eq!(
            error.execution_state,
            crate::types::ExecutionState::OutcomeUnknown
        );
        assert_eq!(error.recovery_action, Some("snapshot"));
        handle.join().unwrap();
    }
}
