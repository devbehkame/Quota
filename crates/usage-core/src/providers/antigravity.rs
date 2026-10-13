//! Google Antigravity per-model quotas.
//!
//! Antigravity runs a local language server that the IDE itself asks for quota
//! information. We find that process, read the per-session CSRF token from its
//! command line, and call the same `GetUserStatus` method on 127.0.0.1. Nothing
//! leaves the computer, and no Google credentials are touched. The IDE has to be
//! running; when it isn't, the app keeps showing the last values it saw.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::util::{num, timestamp};
use crate::{Meter, Unit, Usage, UsageError};

const METHOD_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUserStatus";

#[derive(Clone, Debug, PartialEq)]
struct Server {
    pid: u32,
    csrf: String,
    port: u16,
    tls: bool,
}

/// The last server that answered, so later refreshes skip discovery.
static LAST: Mutex<Option<Server>> = Mutex::new(None);

pub fn detected() -> bool {
    !find_processes().is_empty()
}

pub async fn fetch() -> Result<Usage, UsageError> {
    let client = local_client();
    let cached = LAST.lock().unwrap().clone();
    if let Some(s) = cached {
        if let Ok(v) = call(&client, &s).await {
            return parse(&v).ok_or_else(|| UsageError::Parse("no quota data in the response".into()));
        }
        *LAST.lock().unwrap() = None;
    }

    let procs = find_processes();
    if procs.is_empty() {
        return Err(UsageError::Offline(
            "Antigravity isn't running. Open it to update these numbers.".into(),
        ));
    }
    for (pid, csrf, hint_port) in procs {
        let mut ports = listening_ports(pid).await;
        if let Some(p) = hint_port {
            for cand in [p.saturating_add(1), p] {
                if !ports.contains(&cand) {
                    ports.push(cand);
                }
            }
        }
        for port in ports {
            for tls in [true, false] {
                let s = Server { pid, csrf: csrf.clone(), port, tls };
                if let Ok(v) = call(&client, &s).await {
                    if let Some(u) = parse(&v) {
                        *LAST.lock().unwrap() = Some(s);
                        return Ok(u);
                    }
                }
            }
        }
    }
    Err(UsageError::Offline(
        "Antigravity is open but not ready yet. Sign in inside Antigravity, then refresh.".into(),
    ))
}

/// Client for the IDE's loopback server only: it uses a self-signed certificate,
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

async fn call(client: &reqwest::Client, s: &Server) -> Result<Value, UsageError> {
    let scheme = if s.tls { "https" } else { "http" };
    let url = format!("{scheme}://127.0.0.1:{}{METHOD_PATH}", s.port);
    let resp = client
        .post(url)
        .header("content-type", "application/json")
        .header("connect-protocol-version", "1")
        .header("x-codeium-csrf-token", &s.csrf)
        .json(&json!({ "metadata": { "ideName": "antigravity", "extensionName": "antigravity" } }))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(UsageError::Http { status: resp.status().as_u16(), message: "language server refused".into() });
    }
    Ok(resp.json().await?)
}

/// Antigravity language-server processes: (pid, csrf token, extension port hint).
fn find_processes() -> Vec<(u32, String, Option<u16>)> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    sys.processes()
        .iter()
        .filter_map(|(pid, p)| {
            let name = p.name().to_string_lossy().to_ascii_lowercase();
            if !name.starts_with("language_server") {
                return None;
            }
            let args: Vec<String> = p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect();
            let (csrf, port) = parse_args(&args)?;
            Some((pid.as_u32(), csrf, port))
        })
        .collect()
}

/// Accepts the process only if it belongs to Antigravity (not another Codeium-based IDE).
fn parse_args(args: &[String]) -> Option<(String, Option<u16>)> {
    if !args.iter().any(|a| a.to_ascii_lowercase().contains("antigravity")) {
        return None;
    }
    let value = |flag: &str| -> Option<String> {
        args.iter().enumerate().find_map(|(i, a)| {
            if a == flag {
                args.get(i + 1).cloned()
            } else {
                a.strip_prefix(&format!("{flag}=")).map(str::to_owned)
            }
        })
    };
    let csrf = value("--csrf_token").filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_graphic()))?;
    let port = value("--extension_server_port").and_then(|p| p.parse().ok());
    Some((csrf, port))
}

/// Loopback TCP ports the process is listening on.
async fn listening_ports(pid: u32) -> Vec<u16> {
    #[cfg(windows)]
    let out = {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        crate::util::run_hidden(&format!("{root}\\System32\\NETSTAT.EXE"), &["-ano", "-p", "TCP"]).await
    };
    #[cfg(not(windows))]
    let out = crate::util::run_hidden("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-a", "-p", &pid.to_string()]).await;
    out.map(|o| parse_ports(&o, pid)).unwrap_or_default()
}

/// Parses `netstat -ano` (Windows) or `lsof` output for listening ports owned by `pid`.
fn parse_ports(output: &str, pid: u32) -> Vec<u16> {
    let pid_s = pid.to_string();
    let mut ports = Vec::new();
    for line in output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        let is_netstat = cols.len() >= 5 && cols[0].eq_ignore_ascii_case("TCP");
        let local = if is_netstat {
            if !cols[3].eq_ignore_ascii_case("LISTENING") || cols[4] != pid_s {
                continue;
            }
            cols[1]
        } else if line.contains("(LISTEN)") {
            match cols.iter().rev().nth(1) {
                Some(c) => c,
                None => continue,
            }
        } else {
            continue;
        };
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
    Some(Usage { plan, meters, notes: vec![] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_models_into_pools() {
        let v = json!({"userStatus": {
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
        let (t, p) = parse_args(&a("language_server_windows_x64.exe --app_data_dir antigravity --csrf_token abc-123 --extension_server_port 52100")).unwrap();
        assert_eq!((t.as_str(), p), ("abc-123", Some(52100)));
        assert!(parse_args(&a("language_server --app_data_dir windsurf --csrf_token x")).is_none());
        assert_eq!(parse_args(&a("ls --app_data_dir=antigravity --csrf_token=tok")).unwrap().0, "tok");
    }

    #[test]
    fn parses_netstat_and_lsof() {
        let netstat = "\n  Proto  Local Address          Foreign Address        State           PID\n  TCP    127.0.0.1:52101        0.0.0.0:0              LISTENING       4242\n  TCP    127.0.0.1:52102        0.0.0.0:0              LISTENING       14242\n  TCP    0.0.0.0:52103          0.0.0.0:0              LISTENING       4242\n  TCP    127.0.0.1:52101        127.0.0.1:60000        ESTABLISHED     4242\n  TCP    192.168.1.5:7000       0.0.0.0:0              LISTENING       4242\n";
        assert_eq!(parse_ports(netstat, 4242), vec![52101, 52103]);
        let lsof = "COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\nlanguage 4242 me 12u IPv4 0x1 0t0 TCP 127.0.0.1:52101 (LISTEN)\n";
        assert_eq!(parse_ports(lsof, 4242), vec![52101]);
    }
}
