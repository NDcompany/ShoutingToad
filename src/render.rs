//! Optional JavaScript rendering with the Chrome, Chromium or Edge already installed on the
//! user's machine. Nothing is downloaded.
//!
//! The browser is driven over the Chrome DevTools Protocol by a small client written here: a
//! client-side WebSocket (RFC 6455, unencrypted, to 127.0.0.1 only) on a std `TcpStream`, one
//! socket per tab, synchronous request/response. It needs no new dependency (the maintained
//! sync crates pull in a websocket stack, a generated protocol crate of ~100k lines and a
//! browser fetcher we would have to switch off), builds on every platform and keeps the MSRV.
//!
//! Each rendered page yields a compact [`Rendered`] summary next to the raw-HTML data: what a
//! browser (and Google) sees after JavaScript, versus what non-rendering crawlers (most AI
//! crawlers) see in the raw HTML. With rendering on, the same pass measures text contrast,
//! keyboard focus visibility and mouse-only click handlers for the accessibility checks.

use crate::model::{HtmlData, Link};
use crate::parse::parse_html;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use url::Url;

fn is_false(b: &bool) -> bool {
    !*b
}

/// What the page looks like after JavaScript ran, kept small: the raw-HTML data stays the
/// source of truth for every other check.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Rendered {
    /// URL after JavaScript redirects.
    pub final_url: String,
    pub render_ms: u64,
    #[serde(skip_serializing_if = "is_false")]
    pub timed_out: bool,
    /// The page could not be rendered at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub word_count: usize,
    /// Links in the rendered DOM.
    pub links: usize,
    /// Link targets found only in the rendered DOM (capped at 200).
    pub new_links: Vec<String>,
    pub new_links_total: usize,
    pub title: String,
    pub h1: String,
    pub h1_count: usize,
    pub canonical: String,
    pub meta_robots: String,
    /// Start of the rendered visible text.
    pub text_sample: String,
    /// Uncaught exceptions and console.error() calls (capped at 20).
    pub js_errors: Vec<String>,
    /// Contrast, focus and keyboard probes ran on this page.
    #[serde(skip_serializing_if = "is_false")]
    pub a11y_tested: bool,
    /// Text elements whose contrast could be measured.
    pub contrast_checked: usize,
    /// Text elements below the WCAG 1.4.3 minimum (all of them; `contrast` lists 10 groups).
    pub contrast_fail_count: usize,
    pub contrast: Vec<ContrastIssue>,
    /// Elements reached with the Tab key.
    pub focus_tested: usize,
    /// Of those, elements with no visible change on keyboard focus (capped at 10).
    pub focus_not_visible: Vec<String>,
    /// Elements with mouse click handlers that keyboard users cannot reach (capped at 10).
    pub click_no_keyboard: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ContrastIssue {
    pub text: String,
    pub fg: String,
    pub bg: String,
    pub ratio: f32,
    pub required: f32,
    /// Elements on the page with this colour pair and text size.
    pub count: usize,
}

// ---------------------------------------------------------------- browser detection

/// Standard install locations of Chrome, Chromium, Edge and Brave, most preferred first.
pub fn browser_candidates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        let roots: Vec<PathBuf> = ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"]
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .collect();
        for rel in [
            r"Google\Chrome\Application\chrome.exe",
            r"Chromium\Application\chrome.exe",
            r"Microsoft\Edge\Application\msedge.exe",
            r"BraveSoftware\Brave-Browser\Application\brave.exe",
            r"Google\Chrome Beta\Application\chrome.exe",
            r"Google\Chrome SxS\Application\chrome.exe",
        ] {
            v.extend(roots.iter().map(|r| r.join(rel)));
        }
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        for app in [
            "Google Chrome",
            "Chromium",
            "Microsoft Edge",
            "Brave Browser",
            "Google Chrome Canary",
        ] {
            let rel = format!("{app}.app/Contents/MacOS/{app}");
            v.push(PathBuf::from("/Applications").join(&rel));
            if let Some(h) = &home {
                v.push(h.join("Applications").join(&rel));
            }
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        for d in ["/usr/bin", "/usr/local/bin", "/snap/bin"] {
            dirs.push(d.into());
        }
        for name in [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
            "microsoft-edge-stable",
            "brave-browser",
        ] {
            v.extend(dirs.iter().map(|d| d.join(name)));
        }
        v.push("/opt/google/chrome/chrome".into());
    }
    v
}

/// The browser to render with: the explicit path, else `CHROME_PATH`, else the first standard
/// install found.
pub fn find_browser(explicit: &str) -> Result<PathBuf, String> {
    let explicit = explicit.trim().trim_matches('"');
    if !explicit.is_empty() {
        let p = PathBuf::from(explicit);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!(
                "Browser not found at {explicit}. JavaScript rendering is off for this crawl."
            ))
        };
    }
    if let Some(p) = std::env::var_os("CHROME_PATH").map(PathBuf::from) {
        if p.is_file() {
            return Ok(p);
        }
    }
    browser_candidates().into_iter().find(|p| p.is_file()).ok_or_else(|| {
        "No Chrome, Chromium or Edge found on this computer, so JavaScript rendering is off for \
         this crawl. Install one, or set its path (--chrome <path> or Spider settings)."
            .to_string()
    })
}

// ---------------------------------------------------------------- websocket

/// Client frame: FIN set, masked as RFC 6455 requires.
fn encode_frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut f = Vec::with_capacity(payload.len() + 14);
    f.push(0x80 | opcode);
    match payload.len() {
        n if n < 126 => f.push(0x80 | n as u8),
        n if n <= 0xffff => {
            f.push(0x80 | 126);
            f.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            f.push(0x80 | 127);
            f.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    f.extend_from_slice(&mask);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

/// One complete frame at the start of `buf`: (fin, opcode, payload, bytes used).
fn decode_frame(buf: &[u8]) -> Option<(bool, u8, Vec<u8>, usize)> {
    let (&b0, &b1) = (buf.first()?, buf.get(1)?);
    let (masked, len7) = (b1 & 0x80 != 0, (b1 & 0x7f) as usize);
    let (len, mut pos) = match len7 {
        126 => (
            u16::from_be_bytes(buf.get(2..4)?.try_into().ok()?) as usize,
            4,
        ),
        127 => (
            usize::try_from(u64::from_be_bytes(buf.get(2..10)?.try_into().ok()?)).ok()?,
            10,
        ),
        n => (n, 2),
    };
    let mask: Option<[u8; 4]> = if masked {
        let m = buf.get(pos..pos + 4)?.try_into().ok()?;
        pos += 4;
        Some(m)
    } else {
        None
    };
    let end = pos.checked_add(len)?;
    let mut payload = buf.get(pos..end)?.to_vec();
    if let Some(m) = mask {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= m[i % 4];
        }
    }
    Some((b0 & 0x80 != 0, b0 & 0x0f, payload, end))
}

struct Ws {
    s: TcpStream,
    buf: Vec<u8>,
    msg: Vec<u8>,
    mask: u32,
}

const STOPPED: &str = "Stopped";
const TIMEOUT: &str = "timeout";

impl Ws {
    fn connect(port: u16, path: &str) -> Result<Ws, String> {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
            .map_err(|e| format!("DevTools connection failed: {e}"))?;
        let _ = s.set_nodelay(true);
        s.set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| e.to_string())?;
        // Chrome only checks that the key is present; the RFC's sample nonce is a valid key.
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            match s.read(&mut byte) {
                Ok(1) => head.push(byte[0]),
                _ => return Err("DevTools handshake failed".into()),
            }
            if head.len() > 8192 {
                return Err("DevTools handshake failed".into());
            }
        }
        if !head.starts_with(b"HTTP/1.1 101") {
            let line = String::from_utf8_lossy(&head);
            return Err(format!(
                "DevTools refused the connection: {}",
                line.lines().next().unwrap_or("")
            ));
        }
        Ok(Ws {
            s,
            buf: Vec::new(),
            msg: Vec::new(),
            mask: 0x9e37_79b9,
        })
    }

    fn send_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), String> {
        self.mask = self
            .mask
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        self.s
            .write_all(&encode_frame(opcode, payload, self.mask.to_le_bytes()))
            .map_err(|e| format!("DevTools connection lost: {e}"))
    }

    fn send(&mut self, text: &str) -> Result<(), String> {
        self.send_frame(1, text.as_bytes())
    }

    /// Next text message. Wakes every 100 ms to honour `deadline` and `stop`.
    fn recv(&mut self, deadline: Instant, stop: &AtomicBool) -> Result<String, String> {
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            while let Some((fin, op, payload, used)) = decode_frame(&self.buf) {
                self.buf.drain(..used);
                match op {
                    0x9 => self.send_frame(0xA, &payload)?,
                    0x8 => return Err("DevTools connection closed".into()),
                    0x0..=0x2 => {
                        self.msg.extend_from_slice(&payload);
                        if fin {
                            let m = std::mem::take(&mut self.msg);
                            return Ok(String::from_utf8_lossy(&m).into_owned());
                        }
                    }
                    _ => {}
                }
            }
            if stop.load(Ordering::Relaxed) {
                return Err(STOPPED.into());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(TIMEOUT.into());
            }
            let wait = (deadline - now).clamp(Duration::from_millis(1), Duration::from_millis(100));
            let _ = self.s.set_read_timeout(Some(wait));
            match self.s.read(&mut chunk) {
                Ok(0) => return Err("DevTools connection closed".into()),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => return Err(format!("DevTools connection lost: {e}")),
            }
        }
    }
}

// ---------------------------------------------------------------- one tab

struct Tab {
    ws: Ws,
    target: String,
    next: u64,
    loaded: bool,
    dom_ready: Option<Instant>,
    crashed: bool,
    errors: Vec<String>,
}

impl Tab {
    fn call(
        &mut self,
        method: &str,
        params: Value,
        deadline: Instant,
        stop: &AtomicBool,
    ) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        self.ws
            .send(&json!({"id": id, "method": method, "params": params}).to_string())?;
        loop {
            let m = self.ws.recv(deadline, stop)?;
            let Ok(v) = serde_json::from_str::<Value>(&m) else {
                continue;
            };
            if v["id"].as_u64() == Some(id) {
                return match v.get("error") {
                    Some(e) => Err(e["message"].as_str().unwrap_or("CDP error").to_string()),
                    None => Ok(v["result"].clone()),
                };
            }
            if let Some(ev) = v["method"].as_str() {
                self.event(ev, &v["params"]);
            }
            if self.crashed {
                return Err("The page crashed".into());
            }
        }
    }

    /// Handle events until `until` (or until `done` says so).
    fn pump(
        &mut self,
        until: Instant,
        stop: &AtomicBool,
        done: impl Fn(&Tab) -> bool,
    ) -> Result<(), String> {
        while !done(self) {
            match self.ws.recv(until, stop) {
                Ok(m) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&m) {
                        if let Some(ev) = v["method"].as_str() {
                            self.event(ev, &v["params"]);
                        }
                    }
                }
                Err(e) if e == TIMEOUT => return Ok(()),
                Err(e) => return Err(e),
            }
            if self.crashed {
                return Err("The page crashed".into());
            }
        }
        Ok(())
    }

    fn event(&mut self, method: &str, p: &Value) {
        match method {
            "Page.loadEventFired" => self.loaded = true,
            "Page.domContentEventFired" => self.dom_ready = Some(Instant::now()),
            // a new document: drop events that belonged to the previous one
            "Runtime.executionContextsCleared" => {
                self.errors.clear();
                self.loaded = false;
                self.dom_ready = None;
            }
            "Runtime.exceptionThrown" => {
                let d = &p["exceptionDetails"];
                let msg = d["exception"]["description"]
                    .as_str()
                    .or(d["text"].as_str())
                    .unwrap_or("Uncaught exception");
                self.push_error(msg);
            }
            "Runtime.consoleAPICalled" if p["type"] == "error" => {
                let msg: Vec<String> = p["args"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|x| match &x["value"] {
                                Value::String(s) => s.clone(),
                                Value::Null => x["description"].as_str().unwrap_or("").to_string(),
                                v => v.to_string(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.push_error(&format!("console.error: {}", msg.join(" ")));
            }
            "Page.javascriptDialogOpening" => {
                // alert()/confirm() would block the page: accept and move on
                self.next += 1;
                let _ = self.ws.send(
                    &json!({"id": self.next, "method": "Page.handleJavaScriptDialog", "params": {"accept": true}})
                        .to_string(),
                );
            }
            "Inspector.targetCrashed" | "Inspector.detached" => self.crashed = true,
            _ => {}
        }
    }

    fn push_error(&mut self, msg: &str) {
        if self.errors.len() < 20 {
            let first = msg.lines().next().unwrap_or("").trim();
            self.errors.push(first.chars().take(300).collect());
        }
    }

    fn eval(
        &mut self,
        expr: &str,
        command_line_api: bool,
        deadline: Instant,
        stop: &AtomicBool,
    ) -> Result<Value, String> {
        let r = self.call(
            "Runtime.evaluate",
            json!({"expression": expr, "returnByValue": true, "includeCommandLineAPI": command_line_api}),
            deadline,
            stop,
        )?;
        if let Some(e) = r.get("exceptionDetails") {
            return Err(format!(
                "Script error: {}",
                e["exception"]["description"]
                    .as_str()
                    .or(e["text"].as_str())
                    .unwrap_or("")
            ));
        }
        Ok(r["result"]["value"].clone())
    }
}

// ---------------------------------------------------------------- browser + tab pool

static LIVE_BROWSERS: AtomicUsize = AtomicUsize::new(0);

/// Browser processes started by this process that are still running (for tests).
pub fn live_browsers() -> usize {
    LIVE_BROWSERS.load(Ordering::SeqCst)
}

pub struct Pool {
    /// Display name of the browser executable.
    pub browser: String,
    child: Mutex<Option<Child>>,
    #[cfg(windows)]
    job: Option<job::Job>,
    port: u16,
    control: Mutex<Option<Ws>>,
    control_id: AtomicUsize,
    idle: Mutex<Vec<Tab>>,
    cv: Condvar,
    dir: PathBuf,
    closed: AtomicBool,
    user_agent: String,
    timeout: Duration,
}

impl Pool {
    /// Start the browser and open `tabs` tabs. Gives up when `stop` is set.
    pub fn launch(
        explicit: &str,
        tabs: usize,
        user_agent: &str,
        timeout_secs: u64,
        stop: &AtomicBool,
    ) -> Result<Pool, String> {
        let exe = find_browser(explicit)?;
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "shoutingtoad-chrome-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        sweep_stale_profiles();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {dir:?}: {e}"))?;
        let mut cmd = Command::new(&exe);
        cmd.args([
            "--headless=new",
            "--remote-debugging-port=0",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-default-apps",
            "--disable-sync",
            "--disable-features=Translate,OptimizationHints,MediaRouter,InterestFeedContentSuggestions",
            "--metrics-recording-only",
            "--mute-audio",
            "--hide-scrollbars",
            "--password-store=basic",
            "--use-mock-keychain",
            // images do not change the DOM; skipping them saves most of the bandwidth
            "--blink-settings=imagesEnabled=false",
            "--window-size=1366,900",
        ])
        .arg(format!("--user-data-dir={}", dir.display()))
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        #[cfg(target_os = "linux")]
        {
            // Chrome refuses to start as root without this (CI containers).
            if std::env::var("USER").as_deref() == Ok("root") {
                cmd.arg("--no-sandbox");
            }
            use std::os::unix::process::CommandExt;
            extern "C" {
                fn prctl(option: std::ffi::c_int, ...) -> std::ffi::c_int;
            }
            // SAFETY: prctl(PR_SET_PDEATHSIG, SIGKILL) is async-signal-safe; it makes the kernel
            // kill the browser if this process dies without cleaning up.
            unsafe {
                cmd.pre_exec(|| {
                    prctl(1, 9 as std::ffi::c_ulong);
                    Ok(())
                });
            }
        }
        let child = cmd
            .spawn()
            .map_err(|e| format!("Could not start {}: {e}", exe.display()))?;
        LIVE_BROWSERS.fetch_add(1, Ordering::SeqCst);
        let mut pool = Pool {
            browser: exe.display().to_string(),
            #[cfg(windows)]
            job: job::Job::new(&child),
            child: Mutex::new(Some(child)),
            port: 0,
            control: Mutex::new(None),
            control_id: AtomicUsize::new(10),
            idle: Mutex::new(Vec::new()),
            cv: Condvar::new(),
            dir,
            closed: AtomicBool::new(false),
            user_agent: user_agent.to_string(),
            timeout: Duration::from_secs(timeout_secs.max(5)),
        };
        // From here on, dropping `pool` on an error kills the browser.
        let port_file = pool.dir.join("DevToolsActivePort");
        let started = Instant::now();
        let (port, path) = loop {
            if let Ok(s) = std::fs::read_to_string(&port_file) {
                let mut l = s.lines();
                if let (Some(port), Some(path)) = (l.next(), l.next()) {
                    if let Ok(port) = port.trim().parse::<u16>() {
                        break (port, path.trim().to_string());
                    }
                }
            }
            if stop.load(Ordering::Relaxed) {
                return Err(STOPPED.into());
            }
            if let Some(c) = pool.child.lock().unwrap().as_mut() {
                if let Ok(Some(status)) = c.try_wait() {
                    return Err(format!(
                        "{} exited at start ({status}); JavaScript rendering is off.",
                        pool.browser
                    ));
                }
            }
            if started.elapsed() > Duration::from_secs(30) {
                return Err(format!(
                    "{} did not start within 30s; JavaScript rendering is off.",
                    pool.browser
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        pool.port = port;
        *pool.control.lock().unwrap() = Some(Ws::connect(port, &path)?);
        for _ in 0..tabs.clamp(1, 16) {
            let t = pool.new_tab(stop)?;
            pool.idle.lock().unwrap().push(t);
        }
        Ok(pool)
    }

    fn browser_call(
        &self,
        method: &str,
        params: Value,
        stop: &AtomicBool,
    ) -> Result<Value, String> {
        let mut g = self.control.lock().unwrap();
        let ws = g.as_mut().ok_or("The browser is closed")?;
        let id = self.control_id.fetch_add(1, Ordering::Relaxed);
        ws.send(&json!({"id": id, "method": method, "params": params}).to_string())?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let v: Value = serde_json::from_str(&ws.recv(deadline, stop)?).unwrap_or_default();
            if v["id"].as_u64() == Some(id as u64) {
                return match v.get("error") {
                    Some(e) => Err(e["message"].as_str().unwrap_or("CDP error").to_string()),
                    None => Ok(v["result"].clone()),
                };
            }
        }
    }

    fn new_tab(&self, stop: &AtomicBool) -> Result<Tab, String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("The browser is closed".into());
        }
        let r = self.browser_call("Target.createTarget", json!({"url": "about:blank"}), stop)?;
        let target = r["targetId"].as_str().ok_or("No target id")?.to_string();
        let ws = Ws::connect(self.port, &format!("/devtools/page/{target}"))?;
        let mut t = Tab {
            ws,
            target,
            next: 0,
            loaded: false,
            dom_ready: None,
            crashed: false,
            errors: Vec::new(),
        };
        let d = Instant::now() + Duration::from_secs(15);
        t.call("Page.enable", json!({}), d, stop)?;
        t.call("Runtime.enable", json!({}), d, stop)?;
        t.call(
            "Emulation.setFocusEmulationEnabled",
            json!({"enabled": true}),
            d,
            stop,
        )?;
        if !self.user_agent.is_empty() {
            t.call(
                "Emulation.setUserAgentOverride",
                json!({"userAgent": self.user_agent}),
                d,
                stop,
            )?;
        }
        Ok(t)
    }

    fn acquire(&self, stop: &AtomicBool) -> Option<Tab> {
        let mut idle = self.idle.lock().unwrap();
        loop {
            if stop.load(Ordering::Relaxed) || self.closed.load(Ordering::SeqCst) {
                return None;
            }
            if let Some(t) = idle.pop() {
                return Some(t);
            }
            idle = self
                .cv
                .wait_timeout(idle, Duration::from_millis(100))
                .unwrap()
                .0;
        }
    }

    fn release(&self, t: Tab, healthy: bool, stop: &AtomicBool) {
        let t = if healthy {
            Some(t)
        } else {
            // A hung or crashed tab is replaced (closing it also kills a busy renderer).
            let _ = self.browser_call("Target.closeTarget", json!({"targetId": t.target}), stop);
            drop(t);
            self.new_tab(stop).ok()
        };
        if let Some(t) = t {
            self.idle.lock().unwrap().push(t);
        }
        self.cv.notify_one();
    }

    /// Render `url` and compare it with its raw-HTML parse. Returns the summary and the links
    /// that exist only in the rendered DOM.
    pub fn render(&self, url: &str, raw: &HtmlData, stop: &AtomicBool) -> (Rendered, Vec<Link>) {
        let t0 = Instant::now();
        let Some(mut tab) = self.acquire(stop) else {
            return (
                Rendered {
                    error: Some(STOPPED.into()),
                    ..Default::default()
                },
                Vec::new(),
            );
        };
        let res = render_in(&mut tab, url, self.timeout, stop);
        let healthy = res.as_ref().map(|s| !s.timed_out).unwrap_or(false) && !tab.crashed;
        let errors = std::mem::take(&mut tab.errors);
        self.release(tab, healthy, stop);
        match res {
            Ok(snap) => {
                let (mut r, links) = summarize(raw, &snap, url);
                r.js_errors = errors;
                r.render_ms = t0.elapsed().as_millis() as u64;
                (r, links)
            }
            Err(e) => (
                Rendered {
                    final_url: url.to_string(),
                    timed_out: e == TIMEOUT,
                    error: Some(if e == TIMEOUT {
                        format!("Rendering timed out after {}s", self.timeout.as_secs())
                    } else {
                        e
                    }),
                    js_errors: errors,
                    render_ms: t0.elapsed().as_millis() as u64,
                    ..Default::default()
                },
                Vec::new(),
            ),
        }
    }

    /// Close the browser and every process it started. Safe to call more than once.
    pub fn shutdown(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.cv.notify_all();
        // The profile is temporary, so there is nothing to flush: kill straight away, which
        // keeps Stop prompt.
        if let Ok(mut g) = self.control.try_lock() {
            *g = None;
        }
        #[cfg(windows)]
        if let Some(j) = &self.job {
            j.kill();
        }
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
            LIVE_BROWSERS.fetch_sub(1, Ordering::SeqCst);
        }
        self.idle.lock().unwrap().clear();
        // Helper processes can hold profile files for a moment after the browser exits: try for
        // up to 1 s here, then keep trying in the background.
        let dir = self.dir.clone();
        let remove = move |tries: usize| {
            for _ in 0..tries {
                if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            false
        };
        if !remove(10) {
            std::thread::spawn(move || remove(100));
        }
    }
}

/// Remove temporary profiles left by a run that was killed (the browser itself died with it).
/// Only profiles untouched for an hour, and on Linux only when their process is gone.
fn sweep_stale_profiles() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    let me = std::process::id().to_string();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = name
            .strip_prefix("shoutingtoad-chrome-")
            .and_then(|r| r.split('-').next())
        else {
            continue;
        };
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(3600));
        let alive = cfg!(target_os = "linux") && Path::new("/proc").join(pid).exists();
        if pid != me && old && !alive {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Windows: the browser and every process it starts live in a job object that is killed when
/// the crawl ends, and by Windows when this process exits for any reason (crash, task kill).
#[cfg(windows)]
mod job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimit {
        per_process_user_time: i64,
        per_job_user_time: i64,
        limit_flags: u32,
        min_working_set: usize,
        max_working_set: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimit {
        basic: BasicLimit,
        io: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }

    extern "system" {
        fn CreateJobObjectW(attrs: *mut c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: i32,
            info: *mut c_void,
            len: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn TerminateJobObject(job: *mut c_void, code: u32) -> i32;
        fn CloseHandle(h: *mut c_void) -> i32;
    }

    pub struct Job(*mut c_void);
    // SAFETY: a job handle is a kernel object handle, usable from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new(child: &std::process::Child) -> Option<Job> {
            // SAFETY: plain Win32 calls on handles we own; the struct layout matches
            // JOBOBJECT_EXTENDED_LIMIT_INFORMATION.
            unsafe {
                let h = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if h.is_null() {
                    return None;
                }
                let job = Job(h);
                let mut info = ExtendedLimit::default();
                info.basic.limit_flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                let ok = SetInformationJobObject(
                    h,
                    9, // JobObjectExtendedLimitInformation
                    &mut info as *mut ExtendedLimit as *mut c_void,
                    std::mem::size_of::<ExtendedLimit>() as u32,
                ) != 0
                    && AssignProcessToJobObject(h, child.as_raw_handle()) != 0;
                ok.then_some(job)
            }
        }

        pub fn kill(&self) {
            // SAFETY: valid job handle until drop.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: closing our own handle once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

// ---------------------------------------------------------------- one render

struct Snapshot {
    final_url: String,
    html: String,
    timed_out: bool,
    a11y: Option<A11yRaw>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ContrastGroup {
    fg: String,
    layers: Vec<String>,
    size: f64,
    weight: f64,
    count: usize,
    sample: String,
    /// The page script confirmed the background is really behind the text.
    v: bool,
}

#[derive(Default)]
struct A11yRaw {
    contrast: Vec<ContrastGroup>,
    focus_tested: usize,
    focus_missing: Vec<String>,
    click_only: Vec<String>,
}

fn render_in(
    tab: &mut Tab,
    url: &str,
    timeout: Duration,
    stop: &AtomicBool,
) -> Result<Snapshot, String> {
    let start = Instant::now();
    let deadline = start + timeout;
    tab.loaded = false;
    tab.dom_ready = None;
    tab.errors.clear();
    let nav = tab.call("Page.navigate", json!({"url": url}), deadline, stop)?;
    if let Some(e) = nav["errorText"].as_str().filter(|e| !e.is_empty()) {
        return Err(format!("Navigation failed: {e}"));
    }
    // Wait for the load event, but not for ever: trackers and embeds can hold it back for
    // seconds after the document is ready. DOMContentLoaded + 5 s is enough for scripts to run.
    while !tab.loaded
        && Instant::now() < deadline
        && tab
            .dom_ready
            .is_none_or(|d| d.elapsed() < Duration::from_secs(5))
    {
        let slice = (Instant::now() + Duration::from_millis(250)).min(deadline);
        tab.pump(slice, stop, |t| t.loaded)?;
    }
    let timed_out = tab.dom_ready.is_none() && !tab.loaded;
    // After a timeout, stop loading and read whatever DOM there is, with a short grace period.
    let grace = Instant::now() + Duration::from_secs(5);
    if timed_out {
        let _ = tab.call("Page.stopLoading", json!({}), grace, stop);
    }
    // Let client-side rendering settle: the element count stops changing (max ~3 s).
    let settle_until = Instant::now() + Duration::from_secs(if timed_out { 1 } else { 3 });
    let mut last = -1i64;
    while Instant::now() < settle_until {
        let n = tab
            .eval(
                "document.getElementsByTagName('*').length",
                false,
                Instant::now() + Duration::from_secs(5),
                stop,
            )?
            .as_i64()
            .unwrap_or(0);
        if n == last {
            break;
        }
        last = n;
        tab.pump(
            (Instant::now() + Duration::from_millis(300)).min(settle_until),
            stop,
            |_| false,
        )?;
    }
    let snap_deadline = Instant::now() + Duration::from_secs(10);
    let v = tab.eval(SNAPSHOT_JS, false, snap_deadline, stop)?;
    let s: Value = serde_json::from_str(v.as_str().unwrap_or("{}")).unwrap_or_default();
    let mut a11y = A11yRaw {
        contrast: serde_json::from_value(s["contrast"].clone()).unwrap_or_default(),
        ..Default::default()
    };
    // Keyboard probes. Failures here only lose the a11y data, not the render.
    let probe_deadline = Instant::now() + Duration::from_secs(8);
    let probed = focus_probe(tab, &mut a11y, probe_deadline, stop).and_then(|_| {
        let v = tab.eval(CLICK_JS, true, probe_deadline, stop)?;
        a11y.click_only = serde_json::from_value(v).unwrap_or_default();
        Ok(())
    });
    if let Err(e) = &probed {
        if e == STOPPED {
            return Err(e.clone());
        }
    }
    Ok(Snapshot {
        final_url: s["url"].as_str().unwrap_or(url).to_string(),
        html: s["html"].as_str().unwrap_or("").to_string(),
        timed_out,
        a11y: probed.is_ok().then_some(a11y),
    })
}

fn focus_probe(
    tab: &mut Tab,
    a: &mut A11yRaw,
    deadline: Instant,
    stop: &AtomicBool,
) -> Result<(), String> {
    let mut seen = HashSet::new();
    for _ in 0..15 {
        for kind in ["rawKeyDown", "keyUp"] {
            tab.call(
                "Input.dispatchKeyEvent",
                json!({"type": kind, "key": "Tab", "code": "Tab", "windowsVirtualKeyCode": 9, "nativeVirtualKeyCode": 9}),
                deadline,
                stop,
            )?;
        }
        let v = tab.eval(FOCUS_JS, false, deadline, stop)?;
        let Some(label) = v["label"].as_str() else {
            break; // focus left the page
        };
        if !seen.insert(v["sig"].as_str().unwrap_or(label).to_string()) {
            break; // wrapped around
        }
        a.focus_tested += 1;
        if v["visible"] == false && a.focus_missing.len() < 10 {
            a.focus_missing.push(label.to_string());
        }
    }
    Ok(())
}

/// Compare the rendered DOM with the raw HTML parse. Also returns the links that exist only in
/// the rendered DOM, for the crawler to follow.
fn summarize(raw: &HtmlData, snap: &Snapshot, url: &str) -> (Rendered, Vec<Link>) {
    let final_url = Url::parse(&snap.final_url)
        .or_else(|_| Url::parse(url))
        .expect("rendered URL");
    let r = parse_html(&snap.html, &final_url);
    let raw_links: HashSet<&str> = raw.links.iter().map(|l| l.url.as_str()).collect();
    let mut seen = HashSet::new();
    let new: Vec<&Link> = r
        .links
        .iter()
        .filter(|l| !raw_links.contains(l.url.as_str()) && seen.insert(l.url.as_str()))
        .collect();
    let mut out = Rendered {
        final_url: snap.final_url.clone(),
        timed_out: snap.timed_out,
        word_count: r.word_count,
        links: r.links.len(),
        new_links: new.iter().take(200).map(|l| l.url.clone()).collect(),
        new_links_total: new.len(),
        title: r.titles.first().cloned().unwrap_or_default(),
        h1: r.h1.first().cloned().unwrap_or_default(),
        h1_count: r.h1.len(),
        canonical: r.canonicals.first().cloned().unwrap_or_default(),
        meta_robots: r.meta_robots.join(", "),
        text_sample: r.text.chars().take(400).collect(),
        ..Default::default()
    };
    if let Some(a) = &snap.a11y {
        out.a11y_tested = true;
        out.focus_tested = a.focus_tested;
        out.focus_not_visible = a.focus_missing.clone();
        out.click_no_keyboard = a.click_only.iter().take(10).cloned().collect();
        let mut fails: Vec<ContrastIssue> = Vec::new();
        for g in &a.contrast {
            let Some(c) = evaluate_contrast(g) else {
                continue;
            };
            out.contrast_checked += g.count;
            if c.ratio < c.required && g.v {
                out.contrast_fail_count += g.count;
                fails.push(c);
            }
        }
        fails.sort_by(|a, b| a.ratio.total_cmp(&b.ratio));
        fails.truncate(10);
        out.contrast = fails;
    }
    let links = new.into_iter().cloned().collect();
    (out, links)
}

// ---------------------------------------------------------------- contrast maths (WCAG 2.x)

/// `rgb(1, 2, 3)` / `rgba(1, 2, 3, 0.5)` / `rgb(1 2 3 / 50%)` as [r, g, b, alpha 0..1].
/// Other colour syntaxes (oklch(), color()) return None and are not measured.
pub fn parse_css_color(s: &str) -> Option<[f64; 4]> {
    let s = s.trim();
    let inner = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<&str> = inner
        .split([',', ' ', '/'])
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() < 3 {
        return None;
    }
    let num = |p: &str, scale: f64| -> Option<f64> {
        match p.strip_suffix('%') {
            Some(pct) => pct.parse::<f64>().ok().map(|v| v / 100.0 * scale),
            None => p.parse::<f64>().ok(),
        }
    };
    let alpha = match parts.get(3) {
        Some(a) => num(a, 1.0)?,
        None => 1.0,
    };
    Some([
        num(parts[0], 255.0)?.clamp(0.0, 255.0),
        num(parts[1], 255.0)?.clamp(0.0, 255.0),
        num(parts[2], 255.0)?.clamp(0.0, 255.0),
        alpha.clamp(0.0, 1.0),
    ])
}

/// WCAG relative luminance of an sRGB colour (0..255 channels).
pub fn relative_luminance(c: [f64; 3]) -> f64 {
    let f = |v: f64| {
        let v = v / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
}

/// WCAG contrast ratio, 1..21.
pub fn contrast_ratio(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (x, y) = (relative_luminance(a), relative_luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `top` (with alpha) painted over the opaque `bottom`.
pub fn blend(top: [f64; 4], bottom: [f64; 3]) -> [f64; 3] {
    let a = top[3];
    [
        top[0] * a + bottom[0] * (1.0 - a),
        top[1] * a + bottom[1] * (1.0 - a),
        top[2] * a + bottom[2] * (1.0 - a),
    ]
}

/// Background behind the text: background colours from the element outwards, painted over the
/// white page canvas.
pub fn effective_background(layers: &[[f64; 4]]) -> [f64; 3] {
    layers
        .iter()
        .rev()
        .fold([255.0, 255.0, 255.0], |under, l| blend(*l, under))
}

/// WCAG "large text": at least 18pt (24px), or 14pt (18.66px) bold.
pub fn is_large_text(size_px: f64, weight: f64) -> bool {
    size_px >= 24.0 || (size_px >= 18.66 && weight >= 700.0)
}

/// Minimum contrast for WCAG 1.4.3 (AA).
pub fn required_ratio(size_px: f64, weight: f64) -> f64 {
    if is_large_text(size_px, weight) {
        3.0
    } else {
        4.5
    }
}

fn hex(c: [f64; 3]) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        c[0].round() as u8,
        c[1].round() as u8,
        c[2].round() as u8
    )
}

fn evaluate_contrast(g: &ContrastGroup) -> Option<ContrastIssue> {
    let fg = parse_css_color(&g.fg)?;
    let layers: Vec<[f64; 4]> = g
        .layers
        .iter()
        .map(|l| parse_css_color(l))
        .collect::<Option<_>>()?;
    let bg = effective_background(&layers);
    let fg = blend(fg, bg);
    let ratio = contrast_ratio(fg, bg);
    Some(ContrastIssue {
        text: g.sample.chars().take(80).collect(),
        fg: hex(fg),
        bg: hex(bg),
        ratio: ratio as f32,
        required: required_ratio(g.size, g.weight) as f32,
        count: g.count,
    })
}

// ---------------------------------------------------------------- page scripts

/// Serialised DOM plus colour data of every visible text element, grouped by colours and size.
/// Elements whose background cannot be known from colours alone (background images, opacity,
/// blend modes, text over sibling images) are skipped rather than guessed. Groups that look
/// below the minimum are confirmed with elementsFromPoint (`v`).
const SNAPSHOT_JS: &str = r#"(() => {
  const out = {url: location.href, html: (document.doctype ? '<!DOCTYPE html>' : '') + document.documentElement.outerHTML, contrast: []};
  try {
    const parse = c => { const m = /^rgba?\(([^)]*)\)$/.exec(c || ''); if (!m) return null; const p = m[1].split(/[\s,\/]+/).filter(Boolean).map(x => x.endsWith('%') ? parseFloat(x) / 100 : Number(x)); return p.length >= 3 ? [p[0], p[1], p[2], p.length > 3 ? p[3] : 1] : null; };
    const lin = v => { v /= 255; return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); };
    const lum = c => 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]);
    const over = (t, b) => [t[0] * t[3] + b[0] * (1 - t[3]), t[1] * t[3] + b[1] * (1 - t[3]), t[2] * t[3] + b[2] * (1 - t[3])];
    const ratio = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
    const opaque = s => { const b = parse(s.backgroundColor); return s.backgroundImage !== 'none' || (b && b[3] > 0); };
    const media = e => /^(IMG|VIDEO|CANVAS|PICTURE|IFRAME|svg|OBJECT|EMBED)$/.test(e.tagName);
    const verify = el => {
      el.scrollIntoView({block: 'center', inline: 'center'});
      const r = el.getBoundingClientRect(), x = r.left + Math.min(r.width / 2, 40), y = r.top + r.height / 2;
      if (x < 0 || y < 0 || x >= innerWidth || y >= innerHeight) return false;
      const stack = document.elementsFromPoint(x, y), i = stack.indexOf(el);
      if (i < 0) return false;
      for (const e of stack.slice(0, i)) if (!el.contains(e) && (media(e) || opaque(getComputedStyle(e)))) return false;
      for (const e of stack.slice(i + 1)) { if (e.contains(el)) continue; if (media(e) || opaque(getComputedStyle(e))) return false; }
      return true;
    };
    const groups = new Map();
    const walker = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT);
    const done = new Set();
    let node, budget = 4000, verifyBudget = 60;
    while ((node = walker.nextNode()) && budget > 0) {
      if (!/\S/.test(node.nodeValue)) continue;
      const el = node.parentElement;
      if (!el || done.has(el)) continue;
      done.add(el); budget--;
      if (el.closest('script,style,noscript,template,svg,math,select,option,[aria-hidden="true"],[hidden],:disabled,[aria-disabled="true"]')) continue;
      if (el.checkVisibility && !el.checkVisibility({opacityProperty: true, visibilityProperty: true})) continue;
      const rect = el.getBoundingClientRect();
      if (rect.width < 4 || rect.height < 4) continue;
      const cs = getComputedStyle(el);
      if (parseFloat(cs.textIndent) < -500 || cs.textShadow !== 'none') continue;
      const fg = parse(cs.color);
      if (!fg || fg[3] === 0) continue;
      if (cs.webkitTextFillColor && cs.webkitTextFillColor !== cs.color) continue;
      const layers = [];
      let e = el, unknown = false;
      while (e) {
        const s = e === el ? cs : getComputedStyle(e);
        if (s.backgroundImage !== 'none' || parseFloat(s.opacity) < 1 || s.mixBlendMode !== 'normal' || s.filter !== 'none' || (s.backdropFilter && s.backdropFilter !== 'none')) { unknown = true; break; }
        const b = parse(s.backgroundColor);
        if (!b) { unknown = true; break; }
        if (b[3] > 0) layers.push(s.backgroundColor);
        if (b[3] >= 1) break;
        e = e.parentElement;
      }
      if (unknown) continue;
      const size = parseFloat(cs.fontSize), weight = parseFloat(cs.fontWeight) || 400;
      let bg = [255, 255, 255];
      for (const l of layers.slice().reverse()) bg = over(parse(l), bg);
      const large = size >= 24 || (size >= 18.66 && weight >= 700);
      const fails = ratio(over(fg, bg), bg) < (large ? 3 : 4.5);
      let v = false;
      if (fails) { if (verifyBudget-- <= 0) continue; if (!verify(el)) continue; v = true; }
      const key = cs.color + '|' + layers.join(';') + '|' + large + '|' + v;
      let g = groups.get(key);
      if (!g) { g = {fg: cs.color, layers, size, weight, count: 0, sample: node.nodeValue.trim().replace(/\s+/g, ' ').slice(0, 80), v}; groups.set(key, g); }
      g.count++;
    }
    scrollTo(0, 0);
    out.contrast = [...groups.values()].slice(0, 300);
  } catch (e) {}
  return JSON.stringify(out);
})()"#;

/// The focused element, and whether its look changes between focused and blurred (outline,
/// shadow, border, colours, position, pseudo-elements, its parent and its first children).
const FOCUS_JS: &str = r#"(() => {
  const el = document.activeElement;
  if (!el || el === document.body || el === document.documentElement) return null;
  // only what is painted: an outline of style none has a width and offset, but shows nothing
  const sty = e => { if (!e) return ''; const s = getComputedStyle(e);
    const outline = s.outlineStyle === 'none' || parseFloat(s.outlineWidth) === 0 ? 'none' : [s.outlineStyle, s.outlineWidth, s.outlineColor, s.outlineOffset].join(' ');
    const border = ['Top', 'Right', 'Bottom', 'Left'].map(k => s['border' + k + 'Style'] === 'none' || parseFloat(s['border' + k + 'Width']) === 0 ? '-' : s['border' + k + 'Width'] + ' ' + s['border' + k + 'Color']).join(' ');
    const deco = s.textDecorationLine === 'none' ? 'none' : s.textDecorationLine + ' ' + s.textDecorationColor;
    return [outline, border, deco, s.boxShadow, s.backgroundColor, s.backgroundImage, s.color, s.transform, s.filter, s.opacity, s.fontWeight].join('|'); };
  const ps = (e, p) => { const s = getComputedStyle(e, p); return [s.content, s.backgroundColor, s.borderBottomColor, s.outlineStyle, s.boxShadow, s.transform, s.opacity, s.width].join('|'); };
  const fin = () => { try { (el.parentElement || el).getAnimations({subtree: true}).forEach(a => { try { a.finish(); } catch (_) {} }); } catch (_) {} };
  const snap = () => { fin(); const r = el.getBoundingClientRect(); return [sty(el), ps(el, '::before'), ps(el, '::after'), sty(el.parentElement), Array.from(el.querySelectorAll('*')).slice(0, 8).map(sty).join('#'), Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)].join('~'); };
  const r = el.getBoundingClientRect();
  const name = (el.getAttribute('aria-label') || el.innerText || el.value || el.getAttribute('title') || '').trim().replace(/\s+/g, ' ').slice(0, 50);
  const label = '<' + el.tagName.toLowerCase() + (el.id ? ' id="' + el.id + '"' : '') + '> ' + name;
  const sig = label + '@' + Math.round(r.left + scrollX) + ',' + Math.round(r.top + scrollY);
  const focused = snap();
  el.blur();
  const plain = snap();
  el.focus({preventScroll: true});
  return {label, sig, visible: focused !== plain};
})()"#;

/// Elements with mouse handlers that are not keyboard reachable (needs the DevTools
/// command-line API for getEventListeners). Frameworks that delegate events to the root are
/// not seen, so this under-reports rather than guessing.
const CLICK_JS: &str = r#"(() => {
  if (typeof getEventListeners !== 'function') return [];
  const focusable = 'a[href],area[href],button,input,select,textarea,summary,label,iframe,video,audio,[tabindex],[contenteditable]';
  const out = [];
  let n = 0;
  for (const el of (document.body ? document.body.querySelectorAll('*') : [])) {
    if (++n > 4000 || out.length >= 10) break;
    if (el.closest(focusable) || el.querySelector(focusable) || el.closest('[aria-hidden="true"]')) continue;
    const l = getEventListeners(el);
    if (!(l.click || l.mousedown || l.mouseup || l.pointerdown || l.pointerup)) continue;
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) continue;
    // overlays and modal backdrops (click outside to close) and bare decorations such as custom
    // scrollbar tracks are mouse conveniences, not controls: need text, an icon or a label
    if (r.width * r.height > innerWidth * innerHeight * 0.5) continue;
    if (getComputedStyle(el).pointerEvents === 'none' || (el.checkVisibility && !el.checkVisibility({opacityProperty: true, visibilityProperty: true}))) continue;
    if (!(el.innerText || '').trim() && !el.querySelector('svg,img,[role=img]') && !el.getAttribute('aria-label') && !el.getAttribute('title')) continue;
    out.push('<' + el.tagName.toLowerCase() + (el.id ? ' id="' + el.id + '"' : '') + (el.className && typeof el.className === 'string' ? ' class="' + el.className.slice(0, 40) + '"' : '') + '> ' + (el.innerText || '').trim().replace(/\s+/g, ' ').slice(0, 50));
  }
  return out;
})()"#;

// ---------------------------------------------------------------- reports

/// Section for report.html comparing raw and rendered pages; empty when nothing was rendered.
pub fn report_section(crawl: &crate::model::Crawl) -> String {
    use crate::report::esc;
    use std::fmt::Write;
    let rendered: Vec<&crate::model::Page> = crawl
        .pages
        .iter()
        .filter(|p| p.rendered.is_some())
        .collect();
    if rendered.is_empty() {
        return String::new();
    }
    let (mut raw_words, mut js_words, mut js_links, mut errors, mut timeouts) = (0, 0, 0, 0, 0);
    let mut rows: Vec<(usize, String)> = Vec::new();
    for p in &rendered {
        let r = p.rendered.as_deref().unwrap();
        let raw = p.word_count();
        raw_words += raw;
        js_words += r.word_count;
        js_links += r.new_links_total;
        errors += usize::from(!r.js_errors.is_empty());
        timeouts += usize::from(r.timed_out);
        let gap = r.word_count.saturating_sub(raw);
        if gap > 0 || r.new_links_total > 0 {
            rows.push((
                gap,
                format!(
                    "<tr><td><a href=\"{u}\">{u}</a></td><td class=num>{raw}</td><td class=num>{}</td><td class=num>{}</td></tr>",
                    r.word_count,
                    r.new_links_total,
                    u = esc(&p.url)
                ),
            ));
        }
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    let mut table = String::new();
    for (_, r) in rows.iter().take(25) {
        table.push_str(r);
    }
    let mut s = String::new();
    let _ = write!(
        s,
        "<h2>JavaScript rendering: raw HTML vs rendered</h2>\
         <p class=muted>{n} pages were rendered in a real browser. Raw HTML is what crawlers that do not run JavaScript see (most AI crawlers); the rendered page is what browsers and Google see.</p>\
         <div class=cards><div class=card><div class=label>Words in raw HTML</div><div class=value>{raw_words}</div></div>\
         <div class=card><div class=label>Words after rendering</div><div class=value>{js_words}</div></div>\
         <div class=card><div class=label>Links only after rendering</div><div class=value>{js_links}</div></div>\
         <div class=card><div class=label>Pages with JS errors</div><div class=value>{errors}</div></div>\
         <div class=card><div class=label>Render timeouts</div><div class=value>{timeouts}</div></div></div>",
        n = rendered.len()
    );
    if !table.is_empty() {
        let _ = write!(s, "<h3>Pages that depend most on JavaScript</h3><table><thead><tr><th>URL</th><th class=num>Raw words</th><th class=num>Rendered words</th><th class=num>JS-only links</th></tr></thead><tbody>{table}</tbody></table>");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_frames_round_trip() {
        for len in [0usize, 5, 125, 126, 65535, 65536, 200_000] {
            let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let f = encode_frame(1, &payload, [1, 2, 3, 4]);
            let (fin, op, back, used) = decode_frame(&f).unwrap();
            assert!(fin && op == 1 && used == f.len());
            assert_eq!(back, payload, "len {len}");
            assert!(decode_frame(&f[..f.len() - 1]).is_none() || len == 0);
        }
        // unmasked server frame with a 16-bit length
        let mut f = vec![0x81, 126, 0, 200];
        f.extend(std::iter::repeat_n(b'x', 200));
        let (_, _, p, used) = decode_frame(&f).unwrap();
        assert_eq!((p.len(), used), (200, 204));
    }

    #[test]
    fn contrast_maths_matches_wcag() {
        let black = [0.0, 0.0, 0.0];
        let white = [255.0, 255.0, 255.0];
        assert!((contrast_ratio(black, white) - 21.0).abs() < 1e-9);
        assert!((contrast_ratio(white, white) - 1.0).abs() < 1e-9);
        // #767676 on white is the classic 4.54:1 pass, #777777 fails at 4.48:1
        let ratio = |g: f64| contrast_ratio([g, g, g], white);
        assert!(ratio(118.0) >= 4.5 && ratio(119.0) < 4.5);
        // #999 on white: 2.85
        assert!((ratio(153.0) - 2.85).abs() < 0.01);
        assert!(is_large_text(24.0, 400.0) && is_large_text(18.7, 700.0));
        assert!(!is_large_text(18.7, 400.0) && !is_large_text(16.0, 700.0));
        assert_eq!(required_ratio(16.0, 400.0), 4.5);
        assert_eq!(required_ratio(32.0, 400.0), 3.0);
    }

    #[test]
    fn css_colours_and_layers() {
        assert_eq!(parse_css_color("rgb(1, 2, 3)"), Some([1.0, 2.0, 3.0, 1.0]));
        assert_eq!(
            parse_css_color("rgba(0, 0, 0, 0.5)"),
            Some([0.0, 0.0, 0.0, 0.5])
        );
        assert_eq!(
            parse_css_color("rgb(255 0 0 / 50%)"),
            Some([255.0, 0.0, 0.0, 0.5])
        );
        assert_eq!(parse_css_color("oklch(0.5 0.1 200)"), None);
        assert_eq!(parse_css_color("rgba(0, 0, 0, 0)").unwrap()[3], 0.0);
        // no layers: the white canvas
        assert_eq!(effective_background(&[]), [255.0, 255.0, 255.0]);
        // half-transparent black over white is mid grey
        let g = effective_background(&[[0.0, 0.0, 0.0, 0.5]]);
        assert!((g[0] - 127.5).abs() < 1e-9);
        // inner layer is painted over the outer one
        let bg = effective_background(&[[255.0, 0.0, 0.0, 1.0], [0.0, 0.0, 255.0, 1.0]]);
        assert_eq!(bg, [255.0, 0.0, 0.0]);
        let low = ContrastGroup {
            fg: "rgb(153, 153, 153)".into(),
            layers: vec!["rgb(255, 255, 255)".into()],
            size: 16.0,
            weight: 400.0,
            count: 3,
            sample: "faint".into(),
            v: true,
        };
        let c = evaluate_contrast(&low).unwrap();
        assert!(c.ratio < c.required && c.fg == "#999999" && c.bg == "#ffffff");
    }

    #[test]
    fn browser_detection() {
        // an explicit path must exist
        let me = std::env::current_exe().unwrap();
        assert_eq!(find_browser(me.to_str().unwrap()).unwrap(), me);
        let missing = find_browser("/definitely/not/a/browser");
        assert!(missing.unwrap_err().contains("not found"));
        // every platform has standard locations to look in
        assert!(!browser_candidates().is_empty());
        assert!(browser_candidates().iter().all(|p| p.is_absolute()));
    }

    #[test]
    fn rendered_differences_are_summarized() {
        let u = Url::parse("https://e.com/").unwrap();
        let raw = parse_html(
            r#"<title>Raw</title><link rel=canonical href="https://e.com/"><a href=/a>a</a>"#,
            &u,
        );
        let snap = Snapshot {
            final_url: "https://e.com/".into(),
            html: r#"<title>JS</title><link rel=canonical href="https://e.com/x"><meta name=robots content=noindex><a href=/a>a</a><a href=/b>b</a><a href=/b>again</a>"#.into(),
            timed_out: false,
            a11y: None,
        };
        let (r, links) = summarize(&raw, &snap, "https://e.com/");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://e.com/b");
        assert_eq!((r.links, r.new_links_total), (3, 1));
        assert_eq!(r.title, "JS");
        assert_eq!(r.canonical, "https://e.com/x");
        assert_eq!(r.meta_robots, "noindex");
        assert!(!r.a11y_tested);
    }
}
