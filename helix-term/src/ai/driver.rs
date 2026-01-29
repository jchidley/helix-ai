use std::path::{Path, PathBuf};

use serde_json::json;

use super::config::{DriverConfig, TransportMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverKind {
    Pi,
    Codex,
    Claude,
    Other,
}

#[derive(Debug, Clone)]
pub struct Driver {
    pub name: String,
    pub kind: DriverKind,
    pub mode: TransportMode,
    pub cmd: Vec<String>,
    pub session_dir: PathBuf,
    pub session_glob: String,
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl Driver {
    pub fn from_config(name: &str, config: &DriverConfig) -> Self {
        let kind = match name {
            "pi" => DriverKind::Pi,
            "codex" => DriverKind::Codex,
            "claude" => DriverKind::Claude,
            _ => DriverKind::Other,
        };
        Self {
            name: name.to_string(),
            kind,
            mode: config.mode,
            cmd: config.cmd.clone(),
            session_dir: config.session_dir.clone(),
            session_glob: config.session_glob.clone(),
        }
    }

    pub fn requires_session(&self) -> bool {
        match self.kind {
            DriverKind::Codex => self.cmd.iter().any(|arg| arg == "resume"),
            DriverKind::Claude => self.cmd.iter().any(|arg| arg == "--resume"),
            _ => false,
        }
    }

    pub fn stream_command(&self, session_path: Option<&Path>) -> CommandSpec {
        let (program, mut args) = split_cmd(&self.cmd);
        if let Some(path) = session_path {
            args.push("--session".to_string());
            args.push(path.to_string_lossy().to_string());
        }
        CommandSpec { program, args }
    }

    pub fn tail_command(&self, session_id: Option<&str>, prompt: &str) -> CommandSpec {
        let (program, mut args) = split_cmd(&self.cmd);

        if let Some(session_id) = session_id {
            match self.kind {
                DriverKind::Codex => {
                    if !args.iter().any(|arg| arg == "resume") {
                        args.push("resume".to_string());
                    }
                    args.push(session_id.to_string());
                }
                DriverKind::Claude => {
                    if !args.iter().any(|arg| arg == "--resume") {
                        args.push("--resume".to_string());
                    }
                    args.push(session_id.to_string());
                }
                _ => {
                    args.push(session_id.to_string());
                }
            }
        }

        if !prompt.is_empty() {
            args.push(prompt.to_string());
        }

        CommandSpec { program, args }
    }

    pub fn stream_payload(&self, prompt: &str) -> String {
        match self.kind {
            DriverKind::Pi => json!({
                "type": "prompt",
                "message": prompt
            })
            .to_string(),
            _ => json!({ "type": "prompt", "text": prompt }).to_string(),
        }
    }
}

fn split_cmd(cmd: &[String]) -> (String, Vec<String>) {
    let mut iter = cmd.iter();
    let program = iter
        .next()
        .cloned()
        .unwrap_or_else(|| "sh".to_string());
    let args = iter.cloned().collect();
    (program, args)
}
