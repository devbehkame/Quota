//! Google Antigravity per-model quotas.
//!
//! Antigravity (the app, or its extension inside VS Code and other editors) runs
//! a local language server that the editor asks for quota information. We find
//! that process, read the per-session CSRF token from its command line, and call
//! the same methods on 127.0.0.1. Nothing leaves the computer, and no Google
//! credentials are touched. When Antigravity isn't running, the app keeps
//! showing the last values it saw.
//!
//! The Antigravity CLI (`agy`) keeps its local server locked with a token it
//! doesn't share, so for it we run its own usage report instead:
//! `agy -p /usage --output-format json`, as the signed-in user.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::util::{num, timestamp};
use crate::{Meter, Unit, Usage, UsageError};

const SERVICE: &str = "/exa.language_server_pb.LanguageServerService/";
/// Newer builds answer the quota summary; older ones only the user status.
const METHODS: [&str; 2] = ["RetrieveUserQuotaSummary", "GetUserStatus"];

#[derive(Clone, Debug, PartialEq)]
struct Server {
    csrf: String,
    port: u16,
    tls: bool,
}

/// The last server that answered, so later refreshes skip discovery.
static LAST: Mutex<Option<Server>> = Mutex::new(None);

/// A running Antigravity language server.
#[derive(Debug)]
struct Candidate {
    pid: u32,
    csrf: Option<String>,
    ext_port: Option<u16>,
    ext_csrf: Option<String>,
}

/// Antigravity is installed (as the app or as an editor extension) or running.
pub fn detected() -> bool {
    installed() || !scan().0.is_empty()
}

fn installed() -> bool {
    let local = dirs::data_local_dir().map(|d| d.join("Programs").join("Antigravity"));
    let home = dirs::home_dir().map(|d| d.join(".antigravity"));
    [local, home].into_iter().flatten().any(|p| p.is_dir())
        || super::editor_extension_installed("antigravity")
        || agy_on_path().is_some()
}

pub async fn fetch() -> Result<Usage, UsageError> {
    let client = local_client();
    let cached = LAST.lock().unwrap().clone();
    if let Some(s) = cached {
        if let Some(u) = ask(&client, &s).await.and_then(|v| parse_any(&v)) {
            return Ok(u);
        }
        *LAST.lock().unwrap() = None;
    }

    let (candidates, related, running_agy) = scan_all();
    let agy = running_agy.into_iter().next().or_else(agy_on_path);
    if candidates.is_empty() {
        if let Some(agy) = &agy {
            return from_cli(agy).await;
        }
        let msg = if related.is_empty() {
            "Antigravity isn't running. Open Antigravity (or the editor it's installed in) to update these numbers."
                .to_string()
        } else {
            format!(
                "Antigravity is running ({}), but its language server wasn't found yet. Open a folder in it and start a chat once, then refresh.",
                related.join(", ")
            )
        };
        return Err(UsageError::Offline(msg));
    }

    let mut answered = false;
    let mut no_token = 0;
    for c in &candidates {
        let mut tries: Vec<Server> = Vec::new();
        if let Some(csrf) = &c.csrf {
            for port in listening_ports(c.pid).await {
                tries.push(Server { csrf: csrf.clone(), port, tls: true });
                tries.push(Server { csrf: csrf.clone(), port, tls: false });
            }
        } else {
            no_token += 1;
        }
        if let Some(port) = c.ext_port {
            if let Some(csrf) = c.ext_csrf.as_ref().or(c.csrf.as_ref()) {
                tries.push(Server { csrf: csrf.clone(), port, tls: false });
            }
        }
        for s in tries {
            if let Some(v) = ask(&client, &s).await {
                answered = true;
                if let Some(u) = parse_any(&v) {
                    *LAST.lock().unwrap() = Some(s);
                    return Ok(u);
                }
            }
        }
    }

    if let Some(agy) = &agy {
        return from_cli(agy).await;
    }
    Err(UsageError::Offline(if answered {
        "Connected to Antigravity, but it didn't report any quotas. Sign in inside Antigravity and send one message, then refresh."
            .into()
    } else if no_token == candidates.len() {
        "Found Antigravity's language server, but it didn't expose a local access token, so its quotas can't be read."
            .into()
    } else {
        "Found Antigravity's language server, but it didn't answer yet. Wait a few seconds after opening it, then refresh."
            .into()
    }))
}

/// Client for the editor's loopback server only: it uses a self-signed certificate,
/// so verification is off. It is never used for any other host.
fn local_client() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(4))
        .connect_timeout(Duration::from_millis(800))
        .build()
        .expect("TLS backend available")
}

/// Tries each method on one server; returns the first JSON reply.
async fn ask(client: &reqwest::Client, s: &Server) -> Option<Value> {
    let scheme = if s.tls { "https" } else { "http" };
    let body = json!({ "metadata": { "ideName": "antigravity", "extensionName": "antigravity", "locale": "en" } });
    for method in METHODS {
        let url = format!("{scheme}://127.0.0.1:{}{SERVICE}{method}", s.port);
        let resp = client
            .post(url)
            .header("content-type", "application/json")
            .header("connect-protocol-version", "1")
            .header("x-codeium-csrf-token", &s.csrf)
            .json(&body)
            .send()
            .await;
        let Ok(resp) = resp else { return None }; // nothing listening: skip other methods
        if resp.status().is_success() {
            if let Ok(v) = resp.json::<Value>().await {
                if parse_any(&v).is_some() {
                    return Some(v);
                }
                // Answered without quotas; let the caller know it is the right server.
                if method == METHODS[METHODS.len() - 1] {
                    return Some(v);
                }
            }
        }
    }
    None
}

fn parse_any(v: &Value) -> Option<Usage> {
    parse_summary(v).or_else(|| parse(v))
}

/// Finds Antigravity language servers, plus the names of other Antigravity
/// processes (for a helpful message when the server itself isn't up).
fn scan() -> (Vec<Candidate>, Vec<String>) {
    let (c, r, _) = scan_all();
    (c, r)
}

/// Also returns the paths of running `agy` (Antigravity CLI) executables.
fn scan_all() -> (Vec<Candidate>, Vec<String>, Vec<std::path::PathBuf>) {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always).with_exe(UpdateKind::Always),
    );
    let mut candidates = Vec::new();
    let mut related = Vec::new();
    let mut agy = Vec::new();
    for (pid, p) in sys.processes() {
        let name = p.name().to_string_lossy().to_string();
        if is_agy(&name) {
            if let Some(exe) = p.exe().filter(|e| e.is_file()) {
                if !agy.iter().any(|a: &std::path::PathBuf| a == exe) {
                    agy.push(exe.to_path_buf());
                }
            }
        }
        let mut args: Vec<String> = p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        // Inside another editor, the install path still names Antigravity.
        if let Some(exe) = p.exe() {
            args.push(exe.to_string_lossy().into_owned());
        }
        if !args.iter().any(|a| a.to_ascii_lowercase().contains("antigravity")) {
            continue;
        }
        let lname = name.to_ascii_lowercase();
        if lname.contains("language_server") || lname.contains("language-server") {
            candidates.push(candidate(pid.as_u32(), &args));
        } else if !related.contains(&name) && related.len() < 4 {
            related.push(name);
        }
    }
    (candidates, related, agy)
}

fn is_agy(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "agy" || n == "agy.exe"
}

/// `agy` on PATH, if the CLI is installed but not running.
fn agy_on_path() -> Option<std::path::PathBuf> {
    let exe = if cfg!(windows) { "agy.exe" } else { "agy" };
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join(exe)).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .find(|p| p.is_file())
}

fn candidate(pid: u32, args: &[String]) -> Candidate {
    let value = |flag: &str| -> Option<String> {
        args.iter()
            .enumerate()
            .find_map(|(i, a)| {
                if a == flag {
                    args.get(i + 1).cloned()
                } else {
                    a.strip_prefix(&format!("{flag}=")).map(str::to_owned)
                }
            })
            .filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_graphic()))
    };
    Candidate {
        pid,
        csrf: value("--csrf_token"),
        ext_port: value("--extension_server_port").and_then(|p| p.parse().ok()),
        ext_csrf: value("--extension_server_csrf_token"),
    }
}

/// Loopback TCP ports the process is listening on.
async fn listening_ports(pid: u32) -> Vec<u16> {
    #[cfg(windows)]
    {
        win_ports::listening(pid)
    }
    #[cfg(not(windows))]
    {
        let out = crate::util::run_hidden("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-a", "-p", &pid.to_string()]).await;
        out.map(|o| parse_lsof(&o)).unwrap_or_default()
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn parse_lsof(output: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in output.lines().filter(|l| l.contains("(LISTEN)")) {
        let Some(local) = line.split_whitespace().rev().nth(1) else { continue };
        let Some((host, port)) = local.rsplit_once(':') else { continue };
        let loopback = matches!(host, "127.0.0.1" | "0.0.0.0" | "*" | "[::]" | "[::1]" | "localhost");
        if let (true, Ok(p)) = (loopback, port.parse::<u16>()) {
            if p != 0 && !ports.contains(&p) {
                ports.push(p);
            }
        }
    }
    ports
}

/// Asks Windows directly for the process's listening sockets (no `netstat`,
/// whose output is translated on non-English Windows).
#[cfg(windows)]
mod win_ports {
    use std::ffi::c_void;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

    fn table(family: u16) -> Vec<u8> {
        let mut size: u32 = 0;
        // SAFETY: first call only reports the needed size; second fills our buffer of that size.
        unsafe {
            GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, family as u32, TCP_TABLE_OWNER_PID_LISTENER, 0);
            let mut buf = vec![0u8; size as usize + 1024];
            size = buf.len() as u32;
            if GetExtendedTcpTable(buf.as_mut_ptr() as *mut c_void, &mut size, 0, family as u32, TCP_TABLE_OWNER_PID_LISTENER, 0) != 0 {
                return Vec::new();
            }
            buf
        }
    }

    fn rows<T: Copy>(buf: &[u8]) -> Vec<T> {
        if buf.len() < 4 {
            return Vec::new();
        }
        let n = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        // Rows start after the u32 count, aligned to 4 bytes.
        let start = 4;
        let size = std::mem::size_of::<T>();
        (0..n)
            .filter_map(|i| {
                let off = start + i * size;
                // SAFETY: bounds checked; read_unaligned copes with any alignment.
                (off + size <= buf.len()).then(|| unsafe { std::ptr::read_unaligned(buf[off..].as_ptr() as *const T) })
            })
            .collect()
    }

    pub fn listening(pid: u32) -> Vec<u16> {
        let mut ports = Vec::new();
        let mut add = |raw: u32| {
            let p = u16::from_be((raw & 0xffff) as u16);
            if p != 0 && !ports.contains(&p) {
                ports.push(p);
            }
        };
        for r in rows::<MIB_TCPROW_OWNER_PID>(&table(AF_INET)) {
            // Only sockets reachable on 127.0.0.1: loopback or any-address.
            let loopback_or_any = r.dwLocalAddr == 0 || (r.dwLocalAddr & 0xff) == 127;
            if r.dwOwningPid == pid && loopback_or_any {
                add(r.dwLocalPort);
            }
        }
        for r in rows::<MIB_TCP6ROW_OWNER_PID>(&table(AF_INET6)) {
            if r.dwOwningPid == pid {
                add(r.dwLocalPort);
            }
        }
        ports
    }
}

/// Runs the CLI's own usage report. It takes a few seconds (agy starts its
/// backend), so it is only used when no editor server can answer.
async fn from_cli(agy: &std::path::Path) -> Result<Usage, UsageError> {
    if let Some((at, usage)) = LAST_CLI.lock().unwrap().clone() {
        if at.elapsed() < CLI_REUSE {
            return Ok(usage);
        }
    }
    let usage = run_cli(agy).await?;
    *LAST_CLI.lock().unwrap() = Some((std::time::Instant::now(), usage.clone()));
    Ok(usage)
}

/// Each CLI report starts agy's backend, so a fresh one is fetched at most this often.
const CLI_REUSE: Duration = Duration::from_secs(5 * 60);
static LAST_CLI: Mutex<Option<(std::time::Instant, Usage)>> = Mutex::new(None);

async fn run_cli(agy: &std::path::Path) -> Result<Usage, UsageError> {
    let program = agy.to_string_lossy().into_owned();
    let mut args = vec!["-p", "/usage", "--output-format", "json", "--print-timeout", "90s"];
    let mut out = crate::util::run_capture(&program, &args, Duration::from_secs(100)).await;
    // Older versions don't know --print-timeout.
    if out.as_ref().is_some_and(|o| !o.success && o.stderr.contains("print-timeout")) {
        args.truncate(4);
        out = crate::util::run_capture(&program, &args, Duration::from_secs(100)).await;
    }
    let Some(out) = out else {
        return Err(UsageError::Offline("The Antigravity CLI (agy) didn't answer in time. Try again later.".into()));
    };
    let combined = format!("{}\n{}", out.stdout, out.stderr).to_ascii_lowercase();
    if !out.success && (combined.contains("authentication required") || combined.contains("please log in")) {
        return Err(UsageError::Setup(
            "The Antigravity CLI isn't signed in. Run `agy` in a terminal and sign in, then refresh.".into(),
        ));
    }
    // The JSON report may be preceded by log lines.
    let json_start = out.stdout.find('{');
    let report = json_start.and_then(|i| serde_json::from_str::<Value>(&out.stdout[i..]).ok());
    match report.as_ref().and_then(parse_cli_report) {
        Some(u) => Ok(u),
        None => Err(UsageError::Offline(
            "The Antigravity CLI (agy) didn't return a usage report. Update agy to the latest version, then refresh."
                .into(),
        )),
    }
}

/// `agy -p /usage --output-format json`:
/// `{ status: "SUCCESS", command: { name: "usage", data: { groups: [{ name,
/// buckets: [{ name, window, remaining_fraction, reset_time, disabled }] }] } } }`.
pub(crate) fn parse_cli_report(v: &Value) -> Option<Usage> {
    if v["status"].as_str() != Some("SUCCESS") || v["command"]["name"].as_str() != Some("usage") {
        return None;
    }
    let mut meters = Vec::new();
    for g in v["command"]["data"]["groups"].as_array().into_iter().flatten() {
        let group = g["name"].as_str().unwrap_or_default().trim();
        for b in g["buckets"].as_array().into_iter().flatten() {
            if b["disabled"].as_bool() == Some(true) {
                continue;
            }
            let Some(fraction) = num(&b["remaining_fraction"]) else { continue };
            let window = match b["window"].as_str().unwrap_or_default() {
                "5h" => "5-hour".to_owned(),
                "weekly" => "Weekly".to_owned(),
                "daily" => "Daily".to_owned(),
                other if !other.is_empty() => other.to_owned(),
                _ => b["name"].as_str().unwrap_or("Limit").to_owned(),
            };
            let label = if group.is_empty() { window } else { format!("{group} · {window}") };
            let used = (1.0 - fraction.clamp(0.0, 1.0)) * 100.0;
            meters.push(Meter::percent(label, used, timestamp(&b["reset_time"])));
        }
    }
    (!meters.is_empty()).then(|| Usage { plan: None, meters, notes: vec!["From the Antigravity CLI (agy).".into()], signed_in_as: None, resets: None, tokens: None })
}

/// Newer builds: `{ response: { groups: [{ displayName, buckets: [{ displayName,
/// remaining: { remainingFraction }, description, resetTime? }] }] } }`.
pub(crate) fn parse_summary(v: &Value) -> Option<Usage> {
    let groups = [&v["response"]["groups"], &v["groups"], &v["summary"]["groups"]]
        .into_iter()
        .find_map(|g| g.as_array())?;
    let mut meters = Vec::new();
    for g in groups {
        let group = g["displayName"].as_str().unwrap_or_default().trim();
        for b in g["buckets"].as_array().into_iter().flatten() {
            let bucket = b["displayName"].as_str().or(b["bucketId"].as_str()).unwrap_or_default().trim();
            let label = match (group.is_empty(), bucket.is_empty()) {
                (false, false) => format!("{group} · {bucket}"),
                (false, true) => group.to_owned(),
                (true, false) => bucket.to_owned(),
                (true, true) => continue,
            };
            let reset = timestamp(&b["resetTime"]).or_else(|| timestamp(&b["remaining"]["resetTime"]));
            // Proto3 omits 0.0, so a missing fraction on a bucket means "used up".
            let fraction = num(&b["remaining"]["remainingFraction"]).unwrap_or(0.0).clamp(0.0, 1.0);
            let mut m = Meter::percent(label, (1.0 - fraction) * 100.0, reset);
            if let Some(d) = b["description"].as_str().filter(|d| !d.trim().is_empty()) {
                m = m.with_detail(d.trim());
            }
            meters.push(m);
        }
    }
    (!meters.is_empty()).then(|| Usage { plan: None, meters, notes: vec![], signed_in_as: None, resets: None, tokens: None })
}

/// Remaining fraction, reset time and the model labels sharing one quota.
type Pool = (f64, Option<DateTime<Utc>>, Vec<String>);

pub(crate) fn parse(v: &Value) -> Option<Usage> {
    let us = v.get("userStatus")?;
    let configs = us["cascadeModelConfigData"]["clientModelConfigs"].as_array();

    // Models that share a quota pool report the same fraction and reset time.
    let mut pools: BTreeMap<(String, i64), Pool> = BTreeMap::new();
    for c in configs.into_iter().flatten() {
        let Some(q) = c.get("quotaInfo") else { continue };
        let label = c["label"].as_str().unwrap_or_default().trim();
        if label.is_empty() {
            continue;
        }
        let reset_raw = q["resetTime"].as_str().unwrap_or_default();
        let reset = timestamp(&q["resetTime"]).filter(|t| t.timestamp() > 0);
        // Proto3 omits 0.0: missing means "full" if never used, otherwise "exhausted".
        let fraction = num(&q["remainingFraction"]).unwrap_or(if reset.is_some() { 0.0 } else { 1.0 });
        let key = (reset_raw.to_owned(), (fraction * 1000.0).round() as i64);
        pools.entry(key).or_insert((fraction, reset, Vec::new())).2.push(label.to_owned());
    }

    let mut meters: Vec<Meter> = pools
        .into_values()
        .map(|(fraction, reset, mut labels)| {
            labels.sort();
            let label = match labels.len() {
                1 => labels.remove(0),
                2 => labels.join(" / "),
                n => format!("{} + {} more", labels[0], n - 1),
            };
            let mut m = Meter::percent(label, (1.0 - fraction.clamp(0.0, 1.0)) * 100.0, reset);
            if labels.len() > 2 {
                m = m.with_detail(labels.join(", "));
            }
            m
        })
        .collect();
    meters.sort_by(|a, b| a.label.cmp(&b.label));

    let plan_status = &us["planStatus"];
    let plan_info = &plan_status["planInfo"];
    for (avail, monthly, label) in [
        ("availablePromptCredits", "monthlyPromptCredits", "Prompt credits"),
        ("availableFlowCredits", "monthlyFlowCredits", "Flow credits"),
    ] {
        if let (Some(left), Some(total)) = (num(&plan_status[avail]), num(&plan_info[monthly]).filter(|t| *t > 0.0)) {
            let mut m = Meter::spend(label, Unit::Count, (total - left).max(0.0), Some(total));
            m.remaining = Some(left.max(0.0));
            meters.push(m);
        }
    }

    if meters.is_empty() {
        return None;
    }
    let plan = us["userTier"]["name"]
        .as_str()
        .or(plan_info["planName"].as_str())
        .filter(|p| !p.is_empty())
        .map(str::to_owned);
    let email = us["email"].as_str().map(str::trim).filter(|e| e.contains('@')).map(str::to_owned);
    Some(Usage { plan, meters, notes: vec![], signed_in_as: email, resets: None, tokens: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_models_into_pools() {
        let v = json!({"userStatus": {
            "email": "me@gmail.com",
            "userTier": {"name": "Google AI Pro"},
            "planStatus": {"availablePromptCredits": 400, "planInfo": {"planName": "Pro", "monthlyPromptCredits": 500}},
            "cascadeModelConfigData": {"clientModelConfigs": [
                {"label": "Gemini 3 Pro (High)", "quotaInfo": {"remainingFraction": 0.6, "resetTime": "2026-10-13T15:00:00Z"}},
                {"label": "Gemini 3 Pro (Low)", "quotaInfo": {"remainingFraction": 0.6, "resetTime": "2026-10-13T15:00:00Z"}},
                {"label": "Claude Sonnet 4.5", "quotaInfo": {"resetTime": "2026-10-13T12:00:00Z"}},
                {"label": "Gemini 3 Flash", "quotaInfo": {"resetTime": "1970-01-01T00:00:00Z"}},
                {"label": "No quota model"}
            ]}
        }});
        let u = parse(&v).unwrap();
        assert_eq!(u.plan.as_deref(), Some("Google AI Pro"));
        assert_eq!(u.signed_in_as.as_deref(), Some("me@gmail.com"));
        let find = |l: &str| u.meters.iter().find(|m| m.label == l).unwrap();
        let pro = find("Gemini 3 Pro (High) / Gemini 3 Pro (Low)").remaining.unwrap();
        assert!((pro - 60.0).abs() < 1e-9);
        assert_eq!(find("Claude Sonnet 4.5").remaining, Some(0.0));
        assert_eq!(find("Gemini 3 Flash").remaining, Some(100.0));
        assert_eq!(find("Gemini 3 Flash").resets_at, None);
        assert_eq!(find("Prompt credits").remaining, Some(400.0));
        assert!(parse(&json!({"other": 1})).is_none());
    }

    #[test]
    fn reads_process_args() {
        let a = |s: &str| s.split(' ').map(str::to_owned).collect::<Vec<_>>();
        let c = candidate(1, &a("language_server_windows_x64.exe --app_data_dir antigravity-ide --csrf_token abc-123 --extension_server_port 52100 --extension_server_csrf_token e1"));
        assert_eq!(c.csrf.as_deref(), Some("abc-123"));
        assert_eq!(c.ext_port, Some(52100));
        assert_eq!(c.ext_csrf.as_deref(), Some("e1"));
        let c = candidate(1, &a("ls --app_data_dir=antigravity --csrf_token=tok"));
        assert_eq!(c.csrf.as_deref(), Some("tok"));
        assert_eq!(candidate(1, &a("ls --csrf_token")).csrf, None);
    }

    #[test]
    fn parses_quota_summary() {
        let v = json!({"response": {"groups": [
            {"displayName": "Gemini 3 Pro", "buckets": [
                {"bucketId": "5h", "displayName": "5-hour", "remaining": {"remainingFraction": 0.75}, "description": "Resets in 2h"},
                {"bucketId": "wk", "displayName": "Weekly", "remaining": {}}
            ]},
            {"displayName": "Claude", "buckets": [{"displayName": "5-hour", "remaining": {"remainingFraction": 1}}]}
        ]}});
        let u = parse_summary(&v).unwrap();
        assert_eq!(u.meters.len(), 3);
        assert_eq!(u.meters[0].label, "Gemini 3 Pro · 5-hour");
        assert_eq!(u.meters[0].remaining, Some(75.0));
        assert_eq!(u.meters[0].detail.as_deref(), Some("Resets in 2h"));
        assert_eq!(u.meters[1].remaining, Some(0.0));
        assert!(parse_summary(&json!({"userStatus": {}})).is_none());
    }

    #[test]
    fn parses_cli_report() {
        let v = json!({
            "status": "SUCCESS",
            "command": {"name": "usage", "data": {"groups": [
                {"name": "Gemini Models", "buckets": [
                    {"id": "gemini-weekly", "window": "weekly", "remaining_fraction": 0.86, "reset_time": "2026-09-17T18:40:27Z"},
                    {"id": "gemini-5h", "window": "5h", "remaining_fraction": 0.95, "reset_time": "2026-09-13T03:48:04Z"},
                    {"id": "off", "window": "5h", "remaining_fraction": 0.1, "disabled": true}
                ]},
                {"name": "Claude and GPT models", "buckets": [
                    {"id": "3p-5h", "window": "5h", "remaining_fraction": 1.0, "reset_time": "2026-09-13T05:29:19Z"},
                    {"id": "unknown", "window": "weekly"}
                ]}
            ]}}
        });
        let u = parse_cli_report(&v).unwrap();
        let labels: Vec<_> = u.meters.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["Gemini Models · Weekly", "Gemini Models · 5-hour", "Claude and GPT models · 5-hour"]);
        assert!((u.meters[0].remaining.unwrap() - 86.0).abs() < 1e-9);
        assert!(u.meters[0].resets_at.is_some());
        let mut bad = v.clone();
        bad["status"] = json!("ERROR");
        assert!(parse_cli_report(&bad).is_none());
    }

    #[test]
    fn parses_lsof() {
        let lsof = "COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\nlanguage 4242 me 12u IPv4 0x1 0t0 TCP 127.0.0.1:52101 (LISTEN)\nlanguage 4242 me 13u IPv4 0x1 0t0 TCP 10.0.0.2:7000 (LISTEN)\n";
        assert_eq!(parse_lsof(lsof), vec![52101]);
    }
}
