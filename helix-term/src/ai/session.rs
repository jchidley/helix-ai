use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use globset::{GlobBuilder, GlobSetBuilder};
use ignore::WalkBuilder;
use serde_json::Value;

use super::driver::{Driver, DriverKind};

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub path: PathBuf,
    pub display: String,
    pub session_id: Option<String>,
    pub modified: SystemTime,
}

pub fn list_sessions(driver: &Driver) -> Result<Vec<SessionEntry>> {
    if !driver.session_dir.exists() {
        return Ok(Vec::new());
    }

    let glob = GlobBuilder::new(&driver.session_glob)
        .literal_separator(true)
        .build()
        .with_context(|| format!("invalid session_glob for {}", driver.name))?;

    let mut builder = GlobSetBuilder::new();
    builder.add(glob);
    let matcher = builder
        .build()
        .with_context(|| format!("failed to build glob matcher for {}", driver.name))?;

    let walker = WalkBuilder::new(&driver.session_dir)
        .hidden(false)
        .follow_links(true)
        .build();

    let mut entries = Vec::new();
    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }

        let path = entry.path().to_path_buf();
        let relative = path.strip_prefix(&driver.session_dir).unwrap_or(&path);
        if !matcher.is_match(relative) {
            continue;
        }

        let modified = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let display = relative.to_string_lossy().to_string();
        let session_id = session_id_for_driver(driver, &path).ok();

        entries.push(SessionEntry {
            path,
            display,
            session_id,
            modified,
        });
    }

    entries.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(entries)
}

fn session_id_for_driver(driver: &Driver, path: &Path) -> Result<String> {
    match driver.kind {
        DriverKind::Codex => extract_uuid_suffix(path)
            .ok_or_else(|| anyhow::anyhow!("unable to parse codex session id")),
        DriverKind::Claude => claude_session_id(path),
        _ => Ok(path.to_string_lossy().to_string()),
    }
}

fn extract_uuid_suffix(path: &Path) -> Option<String> {
    let name = path.file_stem()?.to_string_lossy();
    let text = name.as_ref();
    if text.len() < 36 {
        return None;
    }
    let candidate = &text[text.len() - 36..];
    if is_uuid(candidate) {
        Some(candidate.to_string())
    } else {
        None
    }
}

fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    let hyphens = [8, 13, 18, 23];
    for (idx, &b) in bytes.iter().enumerate() {
        if hyphens.contains(&idx) {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn claude_session_id(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let reader = BufReader::new(file);

    for line in reader.lines().take(10) {
        let line = line?;
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            if let Some(id) = value.get("sessionId").and_then(|v| v.as_str()) {
                return Ok(id.to_string());
            }
            if let Some(id) = value.get("session_id").and_then(|v| v.as_str()) {
                return Ok(id.to_string());
            }
        }
    }

    Err(anyhow::anyhow!("missing sessionId in claude session"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use super::super::driver::{Driver, DriverKind};
    use crate::ai::config::TransportMode;
    use tempfile::TempDir;

    #[test]
    fn test_extract_uuid_suffix() {
        let uuid = "123e4567-e89b-12d3-a456-426614174000";
        let name = format!("session-{uuid}");
        let path = PathBuf::from(name);
        assert_eq!(extract_uuid_suffix(&path), Some(uuid.to_string()));

        let path = PathBuf::from("nope.jsonl");
        assert_eq!(extract_uuid_suffix(&path), None);
    }

    #[test]
    fn test_claude_session_id_from_jsonl() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("agent-1.jsonl");
        std::fs::write(
            &path,
            "{\"sessionId\":\"abc-123\",\"type\":\"session\"}\n{\"message\":{\"content\":[]}}",
        )
        .expect("write");

        let id = claude_session_id(&path).expect("session id");
        assert_eq!(id, "abc-123");
    }

    #[test]
    fn test_list_sessions_filters_glob() {
        let dir = TempDir::new().expect("temp dir");
        let root = dir.path();
        let keep = root.join("keep").join("a.jsonl");
        let drop = root.join("drop").join("b.txt");
        std::fs::create_dir_all(keep.parent().unwrap()).expect("mkdir keep");
        std::fs::create_dir_all(drop.parent().unwrap()).expect("mkdir drop");
        std::fs::write(&keep, "{}").expect("write keep");
        std::fs::write(&drop, "{}").expect("write drop");

        let driver = Driver {
            name: "codex".to_string(),
            kind: DriverKind::Codex,
            mode: TransportMode::Tail,
            cmd: vec![],
            session_dir: root.to_path_buf(),
            session_glob: "**/*.jsonl".to_string(),
        };

        let sessions = list_sessions(&driver).expect("list sessions");
        assert!(sessions.iter().any(|entry| entry.path == keep));
        assert!(!sessions.iter().any(|entry| entry.path == drop));
    }
}
