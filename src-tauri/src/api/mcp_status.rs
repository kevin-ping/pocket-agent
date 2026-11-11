// mcp_status.rs — Can Hermes use PA's /mcp right now? (BUG-MCP1 / BUG-MCP2)
//
// Hermes asks /mcp for its tools only when the gateway starts, so a PA that
// restarted under a running gateway sees no request even though the tools
// work. Three sources, in order:
// 1. the local server: did the port bind;
// 2. this run's requests on /mcp: authorized ("seen") or a wrong bearer
//    ("rejected"), last one wins; nothing from the request is kept;
// 3. the Hermes profile PA talks to (local gateway only): its config.yaml
//    registers `mcp_servers.pocket_agent`, enabled, pointing at PA's port.
// A definite "not registered" from the config beats an earlier "seen": the
// user may have removed the registration and restarted the gateway since.
//
// Security (SEC-MCP1): the profile's .env and config.yaml hold keys. They are
// only read, only the fields below are looked at, and failures come back as
// fixed categories — never the parser's message, which can quote the line.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use yaml_rust2::{Yaml, YamlLoader};

/// Hermes' api_server port when the profile's .env doesn't set one.
const HERMES_DEFAULT_PORT: u16 = 8642;

pub const REMOTE_GATEWAY: &str = "remote_gateway";
pub const NOT_HERMES: &str = "not_hermes";
pub const NO_PROFILE: &str = "no_profile";
pub const MANY_PROFILES: &str = "many_profiles";
pub const ENV_UNREADABLE: &str = "env_unreadable";
pub const CONFIG_UNREADABLE: &str = "config_unreadable";
pub const NOT_LOCAL_URL: &str = "not_local_url";
pub const SERVER_STARTING: &str = "server_starting";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum McpStatus {
    Ready,
    /// `config`: the profile's config.yaml to add `mcp_servers.pocket_agent` to.
    NotRegistered { config: String },
    /// /mcp answered 401 to a wrong bearer (API_SERVER_KEY differs).
    Rejected,
    ServerDown { port: u16 },
    /// `reason`: one of the category constants above.
    Unknown { reason: &'static str },
}

#[derive(Clone, Copy)]
enum Server {
    Starting,
    Up(u16),
    Down(u16),
}

static SERVER: Mutex<Server> = Mutex::new(Server::Starting);

const NO_EVIDENCE: u8 = 0;
pub(crate) const SEEN: u8 = 1;
pub(crate) const REJECTED: u8 = 2;
static EVIDENCE: AtomicU8 = AtomicU8::new(NO_EVIDENCE);
/// Tests that touch EVIDENCE.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn evidence() -> u8 {
    EVIDENCE.load(Ordering::Acquire)
}

pub fn server_bound(port: u16) {
    *SERVER.lock().unwrap_or_else(|p| p.into_inner()) = Server::Up(port);
}

pub fn server_bind_failed(port: u16) {
    *SERVER.lock().unwrap_or_else(|p| p.into_inner()) = Server::Down(port);
}

/// A request on /mcp. `wrong_key`: an unauthorized request that counts as
/// Hermes using the wrong key (api/mcp.rs decides which do).
pub fn record_request(authorized: bool, wrong_key: bool) {
    if authorized {
        EVIDENCE.store(SEEN, Ordering::Release);
    } else if wrong_key {
        EVIDENCE.store(REJECTED, Ordering::Release);
    }
}

pub fn mcp_status() -> McpStatus {
    use crate::commands::config::{get_api_agent, get_api_url};
    let pa_port = match *SERVER.lock().unwrap_or_else(|p| p.into_inner()) {
        Server::Down(port) => return McpStatus::ServerDown { port },
        Server::Starting => return McpStatus::Unknown { reason: SERVER_STARTING },
        Server::Up(port) => port,
    };
    let config = if get_api_agent().is_some() {
        Config::Unknown(NOT_HERMES)
    } else {
        profile_check(&crate::platform::hermes_home(), &get_api_url(), pa_port)
    };
    decide(EVIDENCE.load(Ordering::Acquire), config)
}

pub fn mcp_ready() -> bool {
    mcp_status() == McpStatus::Ready
}

/// Async: it reads files, so it stays off the main thread.
#[tauri::command]
pub async fn get_mcp_status() -> McpStatus {
    mcp_status()
}

#[derive(Debug, PartialEq, Eq)]
enum Config {
    Registered,
    NotRegistered(PathBuf),
    Unknown(&'static str),
}

fn decide(evidence: u8, config: Config) -> McpStatus {
    match (config, evidence) {
        (Config::NotRegistered(path), _) => McpStatus::NotRegistered { config: path.display().to_string() },
        (_, REJECTED) => McpStatus::Rejected,
        (_, SEEN) | (Config::Registered, _) => McpStatus::Ready,
        (Config::Unknown(reason), _) => McpStatus::Unknown { reason },
    }
}

/// The profile PA talks to is the one (default home or `profiles/*`) whose
/// api_server is on — Hermes starts it only with an API_SERVER_KEY in that
/// profile's .env — at the gateway URL's port. Exactly one must match.
fn profile_check(home: &Path, gateway_url: &str, pa_port: u16) -> Config {
    if !crate::commands::computer::is_local_gateway(gateway_url) {
        return Config::Unknown(REMOTE_GATEWAY);
    }
    let Some(gateway_port) = reqwest::Url::parse(gateway_url).ok().and_then(|u| u.port_or_known_default()) else {
        return Config::Unknown(REMOTE_GATEWAY);
    };
    let mut dirs = vec![home.to_path_buf()];
    if let Ok(entries) = std::fs::read_dir(home.join("profiles")) {
        dirs.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    let mut matched = Vec::new();
    for dir in dirs {
        match api_server_port(&dir.join(".env")) {
            Ok(Some(port)) if port == gateway_port => matched.push(dir),
            Ok(_) => {}
            Err(reason) => return Config::Unknown(reason),
        }
    }
    match matched.as_slice() {
        [profile] => registration(&profile.join("config.yaml"), pa_port),
        [] => Config::Unknown(NO_PROFILE),
        _ => Config::Unknown(MANY_PROFILES),
    }
}

/// The api_server port of a profile, None when its api_server is off (no .env
/// or no API_SERVER_KEY). Reads no other key; the key itself is dropped.
fn api_server_port(env: &Path) -> Result<Option<u16>, &'static str> {
    let items = match dotenvy::from_path_iter(env) {
        Ok(items) => items,
        Err(e) if e.not_found() => return Ok(None),
        Err(_) => return Err(ENV_UNREADABLE),
    };
    let (mut enabled, mut port) = (false, HERMES_DEFAULT_PORT);
    for item in items {
        let (name, value) = item.map_err(|_| ENV_UNREADABLE)?;
        match name.as_str() {
            "API_SERVER_KEY" => enabled = !value.trim().is_empty(),
            "API_SERVER_PORT" => port = value.trim().parse().map_err(|_| ENV_UNREADABLE)?,
            _ => {}
        }
    }
    Ok(enabled.then_some(port))
}

/// `mcp_servers.pocket_agent`: present, enabled, url at PA's /mcp on this machine.
fn registration(config: &Path, pa_port: u16) -> Config {
    let not_registered = || Config::NotRegistered(config.to_path_buf());
    let text = match std::fs::read_to_string(config) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return not_registered(),
        Err(_) => return Config::Unknown(CONFIG_UNREADABLE),
    };
    let Ok(docs) = YamlLoader::load_from_str(&text) else {
        return Config::Unknown(CONFIG_UNREADABLE);
    };
    let Some(doc) = docs.first() else {
        return not_registered();
    };
    let entry = &doc["mcp_servers"]["pocket_agent"];
    if !matches!(entry, Yaml::Hash(_)) {
        return not_registered();
    }
    match enabled_flag(&entry["enabled"]) {
        Some(true) => {}
        Some(false) => return not_registered(),
        None => return Config::Unknown(CONFIG_UNREADABLE),
    }
    let Some(url) = entry["url"].as_str().and_then(|u| reqwest::Url::parse(u).ok()) else {
        return not_registered();
    };
    // Another machine may well serve /mcp there; only a local url can be judged.
    // PA listens on IPv4 only (server.rs binds 0.0.0.0), so [::1] never reaches it.
    match url.host_str() {
        Some("localhost" | "127.0.0.1") => {}
        Some("[::1]") => return not_registered(),
        _ => return Config::Unknown(NOT_LOCAL_URL),
    }
    // PA serves plain HTTP on exactly /mcp.
    if url.scheme() != "http" || url.port_or_known_default() != Some(pa_port) || url.path() != "/mcp" {
        return not_registered();
    }
    Config::Registered
}

/// `enabled` the way Hermes reads it (`mcp_tool_common._parse_boolish`):
/// missing / null is on, numbers by zero, and the words below in any case —
/// Hermes' YAML 1.1 loader turns `off` / `no` into booleans, ours keeps them
/// as strings. None for anything else, where Hermes falls back with a warning.
fn enabled_flag(value: &Yaml) -> Option<bool> {
    match value {
        Yaml::BadValue | Yaml::Null => Some(true),
        Yaml::Boolean(b) => Some(*b),
        Yaml::Integer(i) => Some(*i != 0),
        Yaml::Real(r) => r.parse::<f64>().ok().map(|r| r != 0.0),
        Yaml::String(s) => match s.trim().to_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "sk-live-0123456789abcdef";

    fn profile(dir: &Path, env: &str, config: Option<&str>) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(".env"), env).unwrap();
        if let Some(config) = config {
            std::fs::write(dir.join("config.yaml"), config).unwrap();
        }
    }

    fn registered(enabled: &str, port: u16) -> String {
        format!(
            "model: x\nmcp_servers:\n  pocket_agent:\n    url: http://127.0.0.1:{port}/mcp\n    headers:\n      Authorization: Bearer {SECRET}\n{enabled}    trust: untrusted\n"
        )
    }

    const GW: &str = "http://localhost:8642";

    #[test]
    fn finds_the_profile_with_the_api_server_on_at_the_gateway_port() {
        let home = tempfile::tempdir().unwrap();
        // Default home: api_server off (no key), so it is not a candidate even on 8642.
        profile(home.path(), "GLM_API_KEY=x\n", None);
        profile(&home.path().join("profiles/xingli"), &format!("API_SERVER_KEY={SECRET}\nAPI_SERVER_PORT=8642\n"), Some(&registered("", 8650)));
        profile(&home.path().join("profiles/other"), &format!("API_SERVER_KEY={SECRET}\nAPI_SERVER_PORT=8643\n"), Some("mcp_servers: {}\n"));
        assert_eq!(profile_check(home.path(), GW, 8650), Config::Registered);
        assert_eq!(profile_check(home.path(), "http://127.0.0.1:8643", 8650), Config::NotRegistered(home.path().join("profiles/other/config.yaml")));
        assert_eq!(profile_check(home.path(), "http://127.0.0.1:9999", 8650), Config::Unknown(NO_PROFILE));
        assert_eq!(profile_check(home.path(), "http://192.168.1.2:8642", 8650), Config::Unknown(REMOTE_GATEWAY));
        // Unset port means 8642: a second profile there makes it ambiguous.
        profile(&home.path().join("profiles/third"), &format!("API_SERVER_KEY={SECRET}\n"), None);
        assert_eq!(profile_check(home.path(), GW, 8650), Config::Unknown(MANY_PROFILES));
    }

    #[test]
    fn registration_needs_the_entry_enabled_and_at_pas_port() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.yaml");
        let check = |yaml: &str| {
            std::fs::write(&config, yaml).unwrap();
            registration(&config, 8650)
        };
        let no = Config::NotRegistered(config.clone());
        assert_eq!(check(&registered("", 8650)), Config::Registered, "enabled defaults to true");
        assert_eq!(check(&registered("    enabled: true\n", 8650)), Config::Registered);
        assert_eq!(check(&registered("    enabled: false\n", 8650)), no);
        assert_eq!(check(&registered("", 8651)), no, "points at another port");
        assert_eq!(check("mcp_servers:\n  rea:\n    command: npx\n"), no);
        assert_eq!(check("model: x\n"), no);
        assert_eq!(check(""), no);
        assert_eq!(check("mcp_servers:\n  pocket_agent:\n    command: x\n"), no, "no url");
        // Off the way Hermes reads `enabled`, not only YAML `false`.
        for off in ["0", "0.0", "\"false\"", "off", "Off", "no", "NO", "\" 0 \""] {
            assert_eq!(check(&registered(&format!("    enabled: {off}\n"), 8650)), no, "enabled: {off}");
        }
        for on in ["", "1", "on", "yes", "\"true\"", "~"] {
            assert_eq!(check(&registered(&format!("    enabled: {on}\n"), 8650)), Config::Registered, "enabled: {on}");
        }
        assert_eq!(check(&registered("    enabled: maybe\n", 8650)), Config::Unknown(CONFIG_UNREADABLE));
        assert_eq!(check(&registered("    enabled: [false]\n", 8650)), Config::Unknown(CONFIG_UNREADABLE));
        // The url must be PA's /mcp on this machine.
        let at = |url: &str| check(&format!("mcp_servers:\n  pocket_agent:\n    url: {url}\n"));
        assert_eq!(at("http://localhost:8650/mcp"), Config::Registered);
        assert_eq!(at("http://127.0.0.1:8650/mcp"), Config::Registered);
        assert_eq!(at("http://[::1]:8650/mcp"), no, "PA listens on IPv4 only");
        assert_eq!(at("http://127.0.0.1:8650/not-mcp"), no, "wrong path");
        assert_eq!(at("http://127.0.0.1:8650/mcp/"), no, "wrong path");
        assert_eq!(at("https://127.0.0.1:8650/mcp"), no, "PA has no TLS");
        assert_eq!(at("not a url"), no);
        assert_eq!(at("http://192.0.2.1:8650/mcp"), Config::Unknown(NOT_LOCAL_URL));
        assert_eq!(at("http://my-mac.local:8650/mcp"), Config::Unknown(NOT_LOCAL_URL));
        std::fs::remove_file(&config).unwrap();
        assert_eq!(registration(&config, 8650), no, "no config.yaml registers nothing");
    }

    #[test]
    fn unreadable_files_give_a_category_and_never_their_contents() {
        let home = tempfile::tempdir().unwrap();
        let xingli = home.path().join("profiles/xingli");
        // Broken YAML on the line that carries the key.
        profile(&xingli, &format!("API_SERVER_KEY={SECRET}\n"), Some(&format!("mcp_servers:\n  pocket_agent: [\n  Authorization: \"Bearer {SECRET}\n")));
        let broken_yaml = profile_check(home.path(), GW, 8650);
        assert_eq!(broken_yaml, Config::Unknown(CONFIG_UNREADABLE));
        // A .env line dotenvy cannot parse, also next to the key.
        profile(&xingli, &format!("API_SERVER_KEY={SECRET}\nBROKEN LINE {SECRET}\n"), None);
        let broken_env = profile_check(home.path(), GW, 8650);
        assert_eq!(broken_env, Config::Unknown(ENV_UNREADABLE));
        profile(&xingli, &format!("API_SERVER_KEY={SECRET}\nAPI_SERVER_PORT=abc\n"), None);
        assert_eq!(profile_check(home.path(), GW, 8650), Config::Unknown(ENV_UNREADABLE));
        // Every outward form — the status (Tauri command / JSON) and its Debug — is key-free.
        profile(&xingli, &format!("API_SERVER_KEY={SECRET}\n"), Some(&registered("    enabled: false\n", 8650)));
        let not_registered = profile_check(home.path(), GW, 8650);
        for config in [broken_yaml, broken_env, not_registered, Config::Registered] {
            for evidence in [NO_EVIDENCE, SEEN, REJECTED] {
                let status = decide(evidence, match &config {
                    Config::NotRegistered(p) => Config::NotRegistered(p.clone()),
                    Config::Registered => Config::Registered,
                    Config::Unknown(r) => Config::Unknown(r),
                });
                let out = format!("{} {status:?}", serde_json::to_string(&status).unwrap());
                assert!(!out.contains(SECRET) && !out.contains("Bearer"), "{out}");
            }
        }
    }

    #[test]
    fn config_reading_is_read_only() {
        let home = tempfile::tempdir().unwrap();
        let xingli = home.path().join("profiles/xingli");
        profile(&xingli, &format!("API_SERVER_KEY={SECRET}\n"), Some(&registered("", 8650)));
        let before: Vec<_> = ["profiles/xingli/.env", "profiles/xingli/config.yaml"]
            .iter()
            .map(|f| std::fs::read(home.path().join(f)).unwrap())
            .collect();
        profile_check(home.path(), GW, 8650);
        let after: Vec<_> = ["profiles/xingli/.env", "profiles/xingli/config.yaml"]
            .iter()
            .map(|f| std::fs::read(home.path().join(f)).unwrap())
            .collect();
        assert_eq!(before, after);
        let mut names: Vec<_> = std::fs::read_dir(&xingli).unwrap().map(|e| e.unwrap().file_name()).collect();
        names.sort();
        assert_eq!(names, [".env", "config.yaml"], "nothing written next to them");
    }

    #[test]
    fn config_beats_seen_and_seen_covers_what_the_config_cannot_tell() {
        let path = PathBuf::from("/h/config.yaml");
        let not_registered = McpStatus::NotRegistered { config: "/h/config.yaml".into() };
        // Registration removed and the gateway restarted while PA kept running.
        assert_eq!(decide(SEEN, Config::NotRegistered(path.clone())), not_registered);
        assert_eq!(decide(REJECTED, Config::NotRegistered(path)), not_registered);
        // PA restarted under a running gateway: no request this run, the config says yes.
        assert_eq!(decide(NO_EVIDENCE, Config::Registered), McpStatus::Ready);
        assert_eq!(decide(REJECTED, Config::Registered), McpStatus::Rejected);
        assert_eq!(decide(SEEN, Config::Unknown(REMOTE_GATEWAY)), McpStatus::Ready);
        assert_eq!(decide(NO_EVIDENCE, Config::Unknown(MANY_PROFILES)), McpStatus::Unknown { reason: MANY_PROFILES });
    }
}
