use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use super::driver::DriverKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatMode {
    Stream,
    SessionFile,
}

pub struct Formatter {
    kind: DriverKind,
    mode: FormatMode,
    pi: PiFormatter,
}

impl Formatter {
    pub fn new(kind: DriverKind, mode: FormatMode) -> Self {
        Self {
            kind,
            mode,
            pi: PiFormatter::new(),
        }
    }

    pub fn render_line(&mut self, line: &str) -> Option<String> {
        match self.kind {
            DriverKind::Pi => self.pi.render_line(line, self.mode),
            _ => render_generic_line(line),
        }
    }
}

pub fn render_session_file(path: &Path, kind: DriverKind) -> Result<Option<String>> {
    match kind {
        DriverKind::Pi => Ok(Some(render_pi_session_file(path)?)),
        _ => Ok(None),
    }
}

fn render_generic_line(line: &str) -> Option<String> {
    if line.is_empty() {
        return None;
    }

    match serde_json::from_str::<Value>(line) {
        Ok(value) => {
            if let Some(text) = render_message(&value) {
                return Some(text);
            }
            if let Some(text) = render_tool(&value) {
                return Some(text);
            }
            Some(line.to_string())
        }
        Err(_) => Some(line.to_string()),
    }
}

fn render_message(value: &Value) -> Option<String> {
    if let Some(content) = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_array())
    {
        let mut out = String::new();
        for part in content {
            if part.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    out.push_str(text);
                }
            }
        }
        if !out.is_empty() {
            return Some(out);
        }
    }

    if let Some(content) = value.get("content").and_then(|content| content.as_array()) {
        let mut out = String::new();
        for part in content {
            if part.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    out.push_str(text);
                }
            }
        }
        if !out.is_empty() {
            return Some(out);
        }
    }

    if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }
    if let Some(text) = value.get("display").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }

    None
}

fn render_tool(value: &Value) -> Option<String> {
    let ty = value.get("type").and_then(|v| v.as_str())?;
    if !ty.starts_with("tool_") {
        return None;
    }

    let mut out = String::new();
    out.push_str("\n**tool**\n```");
    if let Some(text) = value.get("output").and_then(|v| v.as_str()) {
        out.push('\n');
        out.push_str(text);
    } else if let Some(text) = value.get("result").and_then(|v| v.as_str()) {
        out.push('\n');
        out.push_str(text);
    } else {
        out.push('\n');
        out.push_str(ty);
    }
    out.push_str("\n```\n");
    Some(out)
}

struct PiFormatter {
    tool_output_lens: HashMap<String, usize>,
    current_role: Option<String>,
    assistant_text: String,
    assistant_header_emitted: bool,
    assistant_last_delta: String,
}

impl PiFormatter {
    fn new() -> Self {
        Self {
            tool_output_lens: HashMap::new(),
            current_role: None,
            assistant_text: String::new(),
            assistant_header_emitted: false,
            assistant_last_delta: String::new(),
        }
    }

    fn render_line(&mut self, line: &str, mode: FormatMode) -> Option<String> {
        if line.is_empty() {
            return None;
        }

        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(_) => {
                if line.trim_start().starts_with('{') {
                    return None;
                }
                return Some(line.to_string());
            }
        };

        if let Some(text) = self.render_event(&value, mode) {
            return Some(text);
        }

        render_message(&value).or_else(|| render_tool(&value))
    }

    fn render_event(&mut self, value: &Value, mode: FormatMode) -> Option<String> {
        let ty = value.get("type").and_then(|v| v.as_str())?;
        if mode == FormatMode::SessionFile {
            return match ty {
                "message" => self.handle_message_record(value),
                _ => None,
            };
        }
        match ty {
            "agent_start" => None,
            "agent_end" => {
                self.assistant_header_emitted = false;
                Some("\n\n".to_string())
            }
            "message_start" => self.handle_message_start(value),
            "message_update" => self.handle_message_update(value),
            "message_end" => self.handle_message_end(value),
            "tool_execution_start" => self.handle_tool_start(value),
            "tool_execution_update" => self.handle_tool_update(value),
            "tool_execution_end" => self.handle_tool_end(value),
            "error" => self.handle_error_event(value),
            "extension_error" => self.handle_extension_error(value),
            "auto_retry_start" => self.handle_auto_retry_start(value),
            "auto_retry_end" => self.handle_auto_retry_end(value),
            "response" => self.handle_response(value),
            "message" => self.handle_message_record_stream(value),
            _ => None,
        }
    }

    fn handle_message_start(&mut self, value: &Value) -> Option<String> {
        let role = value
            .get("message")
            .and_then(|msg| msg.get("role"))
            .and_then(|v| v.as_str())?;
        self.current_role = Some(role.to_string());
        if role == "assistant" {
            self.assistant_text.clear();
            self.assistant_header_emitted = true;
            self.assistant_last_delta.clear();
        }
        role_header(role).map(|header| header.to_string())
    }

    fn handle_message_update(&mut self, value: &Value) -> Option<String> {
        let evt = value.get("assistantMessageEvent")?;
        let evt_type = evt.get("type").and_then(|v| v.as_str())?;
        match evt_type {
            "text_delta" => {
                let delta = evt.get("delta").and_then(|v| v.as_str())?;
                let text = self.maybe_append_assistant_delta(delta);
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            }
            "thinking_start" => Some("<thinking>\n".to_string()),
            "thinking_delta" => evt.get("delta").and_then(|v| v.as_str()).map(|s| s.to_string()),
            "thinking_end" => Some("\n</thinking>\n\n".to_string()),
            _ => None,
        }
    }

    fn handle_message_end(&mut self, value: &Value) -> Option<String> {
        let msg = value.get("message")?;
        let role = msg.get("role").and_then(|v| v.as_str())?;
        if role == "assistant" {
            self.current_role = None;
            self.assistant_header_emitted = false;
            self.assistant_last_delta.clear();
            return None;
        }
        if role != "user" {
            return None;
        }
        let content = msg.get("content")?;
        let parts = extract_text_parts(content);
        if parts.is_empty() {
            return None;
        }
        Some(format_text_parts(&parts))
    }

    fn handle_tool_start(&mut self, value: &Value) -> Option<String> {
        let tool_name = value
            .get("toolName")
            .and_then(|v| v.as_str())
            .unwrap_or("tool");
        Some(format!("\n**{}**\n```\n", tool_name))
    }

    fn handle_tool_update(&mut self, value: &Value) -> Option<String> {
        let tool_call_id = value.get("toolCallId").and_then(|v| v.as_str())?;
        let content = value
            .get("partialResult")
            .and_then(|v| v.get("content"))?;
        let text = extract_text(content);
        if text.is_empty() {
            return None;
        }
        let prev_len = *self.tool_output_lens.get(tool_call_id).unwrap_or(&0);
        let new_len = text.chars().count();
        if new_len <= prev_len {
            return None;
        }
        let delta = slice_from_char(&text, prev_len);
        self.tool_output_lens
            .insert(tool_call_id.to_string(), new_len);
        if delta.is_empty() {
            None
        } else {
            Some(delta.to_string())
        }
    }

    fn handle_tool_end(&mut self, value: &Value) -> Option<String> {
        let tool_call_id = value.get("toolCallId").and_then(|v| v.as_str());
        let content = value.get("result").and_then(|v| v.get("content"));
        let mut out = String::new();
        if let (Some(tool_call_id), Some(content)) = (tool_call_id, content) {
            let text = extract_text(content);
            if !text.is_empty() {
                let prev_len = *self.tool_output_lens.get(tool_call_id).unwrap_or(&0);
                let delta = slice_from_char(&text, prev_len);
                if !delta.is_empty() {
                    out.push_str(delta);
                }
            }
        }
        out.push_str("```\n\n");
        Some(out)
    }

    fn handle_error_event(&mut self, value: &Value) -> Option<String> {
        let reason = value.get("reason").and_then(|v| v.as_str());
        let error_msg = value.get("error").and_then(|v| v.as_str());
        Some(format!(
            "\n**Error**: {}\n\n",
            error_msg.or(reason).unwrap_or("Unknown error")
        ))
    }

    fn handle_extension_error(&mut self, value: &Value) -> Option<String> {
        let error_msg = value.get("error").and_then(|v| v.as_str()).unwrap_or("Unknown error");
        let ext_path = value.get("extensionPath").and_then(|v| v.as_str());
        if let Some(path) = ext_path {
            Some(format!("\n**Extension Error** ({path}): {error_msg}\n\n"))
        } else {
            Some(format!("\n**Extension Error**: {error_msg}\n\n"))
        }
    }

    fn handle_auto_retry_start(&mut self, value: &Value) -> Option<String> {
        let error_msg = value
            .get("errorMessage")
            .and_then(|v| v.as_str())
            .unwrap_or("transient error");
        let attempt = value
            .get("attempt")
            .and_then(|v| v.as_i64())
            .map(|v| v.to_string())
            .unwrap_or_else(|| "?".to_string());
        let max_attempts = value
            .get("maxAttempts")
            .and_then(|v| v.as_i64())
            .map(|v| v.to_string())
            .unwrap_or_else(|| "?".to_string());
        Some(format!("\n*Retrying ({attempt}/{max_attempts}): {error_msg}*\n"))
    }

    fn handle_auto_retry_end(&mut self, value: &Value) -> Option<String> {
        let success = value.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
        if success {
            return None;
        }
        let final_error = value
            .get("finalError")
            .and_then(|v| v.as_str())
            .unwrap_or("Max retries exceeded");
        Some(format!("\n**Retry Failed**: {final_error}\n\n"))
    }

    fn handle_response(&mut self, value: &Value) -> Option<String> {
        let success = value.get("success").and_then(|v| v.as_bool()).unwrap_or(true);
        let command = value.get("command").and_then(|v| v.as_str());
        if !success {
            let error_msg = value.get("error").and_then(|v| v.as_str()).unwrap_or("Unknown error");
            let suffix = command.map(|cmd| format!(" ({cmd})")).unwrap_or_default();
            return Some(format!("\n**Error{suffix}**: {error_msg}\n\n"));
        }

        if command == Some("get_state") {
            if let Some(data) = value.get("data") {
                let model = data
                    .get("model")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let thinking = data
                    .get("thinkingLevel")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let streaming = data
                    .get("isStreaming")
                    .and_then(|v| v.as_bool())
                    .map(|v| if v { "yes" } else { "no" })
                    .unwrap_or("?");
                let messages = data
                    .get("messageCount")
                    .and_then(|v| v.as_i64())
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "0".to_string());
                return Some(format!(
                    "\n**Status**\n- Model: {model}\n- Thinking: {thinking}\n- Streaming: {streaming}\n- Messages: {messages}\n\n"
                ));
            }
        }

        None
    }

    fn handle_message_record(&mut self, value: &Value) -> Option<String> {
        let msg = value.get("message")?;
        let role = msg.get("role").and_then(|v| v.as_str())?;
        let header = role_header(role)?;
        let content = msg.get("content")?;
        let parts = extract_text_parts(content);
        if parts.is_empty() {
            return None;
        }
        let mut out = String::new();
        out.push_str(header);
        out.push_str(&format_text_parts(&parts));
        Some(out)
    }

    fn handle_message_record_stream(&mut self, value: &Value) -> Option<String> {
        let msg = value.get("message")?;
        let role = msg.get("role").and_then(|v| v.as_str())?;
        if role != "assistant" {
            return None;
        }
        let content = msg.get("content")?;
        let parts = extract_text_parts(content);
        if parts.is_empty() {
            return None;
        }
        if !self.assistant_text.is_empty() {
            return None;
        }
        let joined = parts.join("");
        self.assistant_text.push_str(&joined);

        let mut out = String::new();
        if !self.assistant_header_emitted {
            if let Some(header) = role_header(role) {
                out.push_str(header);
            }
        }
        out.push_str(&format_text_parts(&parts));
        Some(out)
    }

    fn maybe_append_assistant_delta(&mut self, delta: &str) -> String {
        if self.current_role.as_deref() != Some("assistant") {
            return delta.to_string();
        }

        if delta == self.assistant_last_delta {
            return String::new();
        }
        self.assistant_last_delta.clear();
        self.assistant_last_delta.push_str(delta);

        let current = self.assistant_text.as_str();
        if delta.starts_with(current) {
            let appended = &delta[current.len()..];
            self.assistant_text.push_str(appended);
            return appended.to_string();
        }
        if current.starts_with(delta) {
            return String::new();
        }
        if let Some(pos) = delta.find(current) {
            let start = pos + current.len();
            let appended = &delta[start..];
            if appended.is_empty() {
                return String::new();
            }
            self.assistant_text.push_str(appended);
            return appended.to_string();
        }

        self.assistant_text.push_str(delta);
        delta.to_string()
    }
}

fn extract_text_parts(content: &Value) -> Vec<String> {
    if let Some(text) = content.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        return vec![trimmed.to_string()];
    }
    let Some(array) = content.as_array() else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|part| {
            if part.get("type").and_then(|v| v.as_str()) == Some("text") {
                part.get("text").and_then(|v| v.as_str()).map(|s| s.to_string())
            } else {
                None
            }
        })
        .filter(|text| !text.is_empty())
        .collect()
}

fn extract_text(content: &Value) -> String {
    let parts = extract_text_parts(content);
    if parts.is_empty() {
        String::new()
    } else {
        parts.join("")
    }
}

fn format_text_parts(parts: &[String]) -> String {
    let mut out = String::new();
    for text in parts {
        out.push_str(text);
        out.push_str("\n\n");
    }
    out
}

fn role_header(role: &str) -> Option<&'static str> {
    match role {
        "user" => Some("## You\n\n"),
        "assistant" => Some("## Assistant\n\n"),
        _ => None,
    }
}

fn slice_from_char(text: &str, start: usize) -> &str {
    if start == 0 {
        return text;
    }
    let mut iter = text.char_indices();
    let idx = match iter.nth(start) {
        Some((idx, _)) => idx,
        None => return "",
    };
    &text[idx..]
}

fn render_pi_session_file(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut out = String::new();
    for line in reader.lines() {
        let line = line?;
        let value = match serde_json::from_str::<Value>(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("message") {
            continue;
        }
        let msg = match value.get("message") {
            Some(msg) => msg,
            None => continue,
        };
        let role = match msg.get("role").and_then(|v| v.as_str()) {
            Some(role) => role,
            None => continue,
        };
        let header = match role_header(role) {
            Some(header) => header,
            None => continue,
        };
        let content = match msg.get("content") {
            Some(content) => content,
            None => continue,
        };
        let parts = extract_text_parts(content);
        if parts.is_empty() {
            continue;
        }
        out.push_str(header);
        out.push_str(&format_text_parts(&parts));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_message_content() {
        let line = r#"{"message":{"content":[{"type":"text","text":"hello"}]}}"#;
        let rendered = render_generic_line(line).unwrap();
        assert_eq!(rendered, "hello");
    }

    #[test]
    fn test_render_invalid_json_fallback() {
        let line = "not-json";
        let rendered = render_generic_line(line).unwrap();
        assert_eq!(rendered, "not-json");
    }

    #[test]
    fn test_render_tool_block() {
        let line = r#"{"type":"tool_execution_start","output":"running"}"#;
        let rendered = render_generic_line(line).unwrap();
        assert!(rendered.contains("**tool**"));
        assert!(rendered.contains("running"));
    }

    #[test]
    fn test_pi_message_start() {
        let mut formatter = Formatter::new(DriverKind::Pi, FormatMode::Stream);
        let line = r#"{"type":"message_start","message":{"role":"assistant"}}"#;
        let rendered = formatter.render_line(line).unwrap();
        assert!(rendered.contains("## Assistant"));
    }

    #[test]
    fn test_pi_message_update_text() {
        let mut formatter = Formatter::new(DriverKind::Pi, FormatMode::Stream);
        let line = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"hi"}}"#;
        let rendered = formatter.render_line(line).unwrap();
        assert_eq!(rendered, "hi");
    }

    #[test]
    fn test_pi_message_record_session_file() {
        let mut formatter = Formatter::new(DriverKind::Pi, FormatMode::SessionFile);
        let line = r#"{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#;
        let rendered = formatter.render_line(line).unwrap();
        assert!(rendered.contains("## Assistant"));
        assert!(rendered.contains("hello"));
    }

    #[test]
    fn test_pi_repeated_full_delta() {
        let mut formatter = Formatter::new(DriverKind::Pi, FormatMode::Stream);
        let start = r#"{"type":"message_start","message":{"role":"assistant"}}"#;
        let delta1 = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"Hello"}}"#;
        let delta2 =
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"Hello world"}}"#;
        assert!(formatter.render_line(start).is_some());
        assert_eq!(formatter.render_line(delta1).unwrap(), "Hello");
        assert_eq!(formatter.render_line(delta2).unwrap(), " world");
    }

    #[test]
    fn test_pi_duplicate_delta_ignored() {
        let mut formatter = Formatter::new(DriverKind::Pi, FormatMode::Stream);
        let start = r#"{"type":"message_start","message":{"role":"assistant"}}"#;
        let delta = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"Hello"}}"#;
        assert!(formatter.render_line(start).is_some());
        assert_eq!(formatter.render_line(delta).unwrap(), "Hello");
        assert!(formatter.render_line(delta).is_none());
    }

    #[test]
    fn test_pi_stream_message_record_fallback() {
        let mut formatter = Formatter::new(DriverKind::Pi, FormatMode::Stream);
        let line = r#"{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#;
        let rendered = formatter.render_line(line).unwrap();
        assert!(rendered.contains("## Assistant"));
        assert!(rendered.contains("hello"));
    }
}
