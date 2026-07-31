use std::{path::PathBuf, time::Duration};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Local;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use serde_json::Value;
use tauri::AppHandle;

use super::config::{load_legacy_config, AppConfig};

const SCHEMA_VERSION: &str = "1";
const MAX_ASSET_BYTES: usize = 10 * 1024 * 1024;
const ALLOWED_ASSET_MIMES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/webp",
    "image/gif",
];

#[derive(Debug, Serialize)]
pub struct SaveConfigResponse {
    pub config: AppConfig,
    pub revision: u64,
}

fn db_path() -> Result<PathBuf, String> {
    let mut path = dirs::home_dir().ok_or_else(|| "home directory is unavailable".to_string())?;
    path.push(".pocket-agent");
    std::fs::create_dir_all(&path).map_err(|e| format!("create settings directory: {e}"))?;
    path.push("settings.db");
    Ok(path)
}

fn open() -> Result<Connection, String> {
    let path = db_path()?;
    let conn = Connection::open(path).map_err(|e| format!("open settings database: {e}"))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| format!("configure settings database: {e}"))?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| format!("enable settings WAL: {e}"))?;
    initialize_schema(&conn)?;
    Ok(conn)
}

fn initialize_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value_json TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS assets (
            key TEXT PRIMARY KEY,
            mime_type TEXT NOT NULL,
            data BLOB NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
    .map_err(|e| format!("initialize settings schema: {e}"))?;
    Ok(())
}

pub fn initialize(app: &AppHandle) -> Result<(), String> {
    let mut conn = open()?;
    let migrated = conn
        .query_row(
            "SELECT value FROM metadata WHERE key='migration_completed'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .as_deref()
        == Some("1");

    if !migrated {
        let legacy = load_legacy_config(app);
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        write_config(&tx, &legacy)?;
        tx.execute(
            "INSERT OR REPLACE INTO metadata(key,value) VALUES('schema_version',?1)",
            [SCHEMA_VERSION],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR REPLACE INTO metadata(key,value) VALUES('migration_completed','1')",
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR IGNORE INTO metadata(key,value) VALUES('revision','1')",
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| format!("commit settings migration: {e}"))?;
        return Ok(());
    }

    // Normalize existing databases on every schema-compatible startup. This
    // fills keys introduced by newer releases without returning to JSON.
    let config = load_from_connection(&conn)?;
    validate(&config)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_config(&tx, &config)?;
    tx.commit().map_err(|e| e.to_string())
}

pub fn load() -> Result<AppConfig, String> {
    let conn = open()?;
    let config = load_from_connection(&conn)?;
    validate(&config)?;
    Ok(config)
}

pub fn save(config: &AppConfig) -> Result<SaveConfigResponse, String> {
    validate(config)?;
    let mut conn = open()?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_config(&tx, config)?;
    let revision = current_revision(&tx)?.saturating_add(1);
    tx.execute(
        "INSERT OR REPLACE INTO metadata(key,value) VALUES('revision',?1)",
        [revision.to_string()],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| format!("commit settings: {e}"))?;
    Ok(SaveConfigResponse {
        config: load()?,
        revision,
    })
}

pub fn save_window_position(x: f64, y: f64) -> Result<(), String> {
    if !x.is_finite() || !y.is_finite() {
        return Err("window position must be finite".into());
    }
    let mut conn = open()?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let now = Local::now().to_rfc3339();
    for (key, value) in [("window_x", x), ("window_y", y)] {
        tx.execute(
            "INSERT OR REPLACE INTO settings(key,value_json,updated_at) VALUES(?1,?2,?3)",
            params![key, value.to_string(), now],
        )
        .map_err(|e| e.to_string())?;
    }
    let revision = current_revision(&tx)?.saturating_add(1);
    tx.execute(
        "INSERT OR REPLACE INTO metadata(key,value) VALUES('revision',?1)",
        [revision.to_string()],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

pub fn get_asset(key: &str) -> Result<Option<String>, String> {
    ensure_asset_key(key)?;
    let conn = open()?;
    read_asset(&conn, key)
}

pub fn save_asset(key: &str, data_uri: &str) -> Result<u64, String> {
    ensure_asset_key(key)?;
    let (mime, data) = decode_data_uri(data_uri)?;
    let conn = open()?;
    conn.execute(
        "INSERT OR REPLACE INTO assets(key,mime_type,data,updated_at) VALUES(?1,?2,?3,?4)",
        params![key, mime, data, Local::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    bump_revision(&conn)
}

pub fn delete_asset(key: &str) -> Result<u64, String> {
    ensure_asset_key(key)?;
    let conn = open()?;
    conn.execute("DELETE FROM assets WHERE key=?1", [key])
        .map_err(|e| e.to_string())?;
    bump_revision(&conn)
}

fn load_from_connection(conn: &Connection) -> Result<AppConfig, String> {
    let defaults = serde_json::to_value(AppConfig::default()).map_err(|e| e.to_string())?;
    let mut object = defaults
        .as_object()
        .cloned()
        .ok_or_else(|| "invalid AppConfig defaults".to_string())?;

    let mut stmt = conn
        .prepare("SELECT key,value_json FROM settings")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (key, raw) = row.map_err(|e| e.to_string())?;
        if object.contains_key(&key) {
            let value: Value =
                serde_json::from_str(&raw).map_err(|e| format!("invalid setting {key}: {e}"))?;
            object.insert(key, value);
        }
    }

    object.insert(
        "avatar_image".into(),
        read_asset(conn, "avatar_image")?.map(Value::String).unwrap_or(Value::Null),
    );
    object.insert(
        "avatar_gif".into(),
        read_asset(conn, "avatar_gif")?.map(Value::String).unwrap_or(Value::Null),
    );
    serde_json::from_value(Value::Object(object))
        .map_err(|e| format!("decode settings database: {e}"))
}

fn write_config(tx: &Transaction<'_>, config: &AppConfig) -> Result<(), String> {
    validate(config)?;
    let now = Local::now().to_rfc3339();
    let mut object = serde_json::to_value(config)
        .map_err(|e| e.to_string())?
        .as_object()
        .cloned()
        .ok_or_else(|| "encode AppConfig failed".to_string())?;
    let avatar_image = object.remove("avatar_image");
    let avatar_gif = object.remove("avatar_gif");

    for (key, value) in object {
        tx.execute(
            "INSERT OR REPLACE INTO settings(key,value_json,updated_at) VALUES(?1,?2,?3)",
            params![key, value.to_string(), now],
        )
        .map_err(|e| format!("write setting: {e}"))?;
    }
    write_optional_asset(tx, "avatar_image", avatar_image.as_ref(), &now)?;
    write_optional_asset(tx, "avatar_gif", avatar_gif.as_ref(), &now)?;
    Ok(())
}

fn write_optional_asset(
    tx: &Transaction<'_>,
    key: &str,
    value: Option<&Value>,
    now: &str,
) -> Result<(), String> {
    match value.and_then(Value::as_str) {
        Some(data_uri) if !data_uri.is_empty() => {
            let (mime, data) = decode_data_uri(data_uri)?;
            tx.execute(
                "INSERT OR REPLACE INTO assets(key,mime_type,data,updated_at) VALUES(?1,?2,?3,?4)",
                params![key, mime, data, now],
            )
            .map_err(|e| e.to_string())?;
        }
        _ => {
            tx.execute("DELETE FROM assets WHERE key=?1", [key])
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn decode_data_uri(value: &str) -> Result<(String, Vec<u8>), String> {
    let (header, encoded) = value
        .split_once(',')
        .ok_or_else(|| "asset must be a data URI".to_string())?;
    let mime = header
        .strip_prefix("data:")
        .and_then(|h| h.strip_suffix(";base64"))
        .ok_or_else(|| "asset must be base64 encoded".to_string())?;
    if !ALLOWED_ASSET_MIMES.contains(&mime) {
        return Err(format!("unsupported asset type: {mime}"));
    }
    let data = STANDARD
        .decode(encoded)
        .map_err(|e| format!("invalid asset data: {e}"))?;
    if data.len() > MAX_ASSET_BYTES {
        return Err("asset exceeds the 10 MB limit".to_string());
    }
    Ok((mime.to_string(), data))
}

fn read_asset(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT mime_type,data FROM assets WHERE key=?1",
        [key],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
    )
    .optional()
    .map_err(|e| e.to_string())
    .map(|asset| asset.map(|(mime, data)| format!("data:{mime};base64,{}", STANDARD.encode(data))))
}

fn ensure_asset_key(key: &str) -> Result<(), String> {
    if matches!(key, "avatar_image" | "avatar_gif") {
        Ok(())
    } else {
        Err("unknown settings asset".to_string())
    }
}

fn current_revision(conn: &Connection) -> Result<u64, String> {
    Ok(conn
        .query_row(
            "SELECT value FROM metadata WHERE key='revision'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

fn bump_revision(conn: &Connection) -> Result<u64, String> {
    let revision = current_revision(conn)?.saturating_add(1);
    conn.execute(
        "INSERT OR REPLACE INTO metadata(key,value) VALUES('revision',?1)",
        [revision.to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok(revision)
}

pub fn validate(config: &AppConfig) -> Result<(), String> {
    if !(0.0..=1.0).contains(&config.volume) {
        return Err("volume must be between 0 and 1".into());
    }
    if !matches!(config.character_skin.as_str(), "default-css" | "rive" | "lottie") {
        return Err("invalid character skin".into());
    }
    if !matches!(config.dialog_style.as_str(), "bubble" | "tv" | "terminal") {
        return Err("invalid dialog style".into());
    }
    if !matches!(config.tts_format.as_str(), "wav" | "mp3") {
        return Err("invalid TTS format".into());
    }
    if config.tts_primary_voice.trim().is_empty() {
        return Err("primary voice is required".into());
    }
    if !matches!(config.fixed_lang.as_str(), "" | "primary" | "aux1" | "aux2") {
        return Err("invalid fixed language selection".into());
    }
    if !matches!(config.ui_lang.as_str(), "zh" | "en" | "ja" | "ko" | "fr" | "de" | "es") {
        return Err("invalid UI language".into());
    }
    if !(0..=255).contains(&config.hotkey_code) || config.hotkey_name.len() > 64 {
        return Err("invalid hotkey".into());
    }
    if !(3..=10).contains(&config.silence_timeout_secs)
        || !(500..=5000).contains(&config.pause_tolerance_ms)
        || !(0.003..=0.020).contains(&config.speech_rms_threshold)
        || !(0.02..=0.15).contains(&config.barge_in_rms_threshold)
        || !(0.30..=0.90).contains(&config.wake_word_threshold)
    {
        return Err("one or more voice thresholds are out of range".into());
    }
    if config.last_enrolled_speaker.len() > 32
        || (!config.last_enrolled_speaker.is_empty()
            && !config
                .last_enrolled_speaker
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    {
        return Err("invalid enrolled speaker name".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_defaults_and_rejects_bad_ranges() {
        assert!(validate(&AppConfig::default()).is_ok());
        let mut config = AppConfig::default();
        config.volume = 2.0;
        assert!(validate(&config).is_err());
    }

    #[test]
    fn accepts_supported_asset_and_rejects_large_or_unknown_data() {
        let png = "data:image/png;base64,iVBORw0KGgo=";
        assert!(decode_data_uri(png).is_ok());
        assert!(decode_data_uri("data:text/plain;base64,SGk=").is_err());
    }

    #[test]
    fn round_trips_config_and_assets_in_sqlite() {
        let mut conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        let mut config = AppConfig::default();
        config.ui_lang = "fr".into();
        config.volume = 0.55;
        config.avatar_image = Some("data:image/png;base64,iVBORw0KGgo=".into());
        let tx = conn.transaction().unwrap();
        write_config(&tx, &config).unwrap();
        tx.commit().unwrap();

        let loaded = load_from_connection(&conn).unwrap();
        assert_eq!(loaded.ui_lang, "fr");
        assert!((loaded.volume - 0.55).abs() < f32::EPSILON);
        assert_eq!(loaded.avatar_image, config.avatar_image);
    }

    #[test]
    fn missing_keys_use_defaults_and_corrupt_json_is_reported() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO settings(key,value_json,updated_at) VALUES('volume','0.4','now')",
            [],
        )
        .unwrap();
        let loaded = load_from_connection(&conn).unwrap();
        assert_eq!(loaded.ui_lang, AppConfig::default().ui_lang);

        conn.execute(
            "UPDATE settings SET value_json='not-json' WHERE key='volume'",
            [],
        )
        .unwrap();
        assert!(load_from_connection(&conn).is_err());
    }
}
