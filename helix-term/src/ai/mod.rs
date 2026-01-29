use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;

use crate::compositor;
use crate::job;
use crate::ui::{overlay::overlaid, Picker, PickerColumn};
use helix_view::tree::{Dimension, Resize};

pub mod config;
pub mod driver;
pub mod format;
pub mod session;
pub mod stream;
pub mod ui;

use config::AiConfig;
use driver::Driver;
use session::SessionEntry;

static AI_STATE: Lazy<Mutex<AiState>> = Lazy::new(|| Mutex::new(AiState::default()));

#[derive(Default)]
struct AiState {
    config: Option<AiConfig>,
    buffers: Option<ui::AiBuffers>,
    session: Option<AiSession>,
}

enum AiSession {
    Stream(StreamSessionState),
    Tail(TailSessionState),
}

struct StreamSessionState {
    driver: Driver,
    handle: stream::StreamHandle,
}

#[derive(Clone)]
struct TailSessionState {
    driver: Driver,
    session_path: Option<std::path::PathBuf>,
    session_id: Option<String>,
}

pub fn start(cx: &mut compositor::Context, driver_name: Option<&str>) -> Result<()> {
    let config = AiConfig::load()?;
    let driver_name = driver_name.unwrap_or(&config.general.default_driver);
    let driver_cfg = config::require_driver(&config, driver_name)?;
    let driver = Driver::from_config(driver_name, driver_cfg);

    let buffers = ui::ensure_buffers(cx.editor, &config)?;

    let previous = {
        let mut state = AI_STATE.lock().unwrap();
        state.config = Some(config);
        state.buffers = Some(buffers.clone());
        state.session.take()
    };

    if let Some(session) = previous {
        stop_session(cx, session);
    }

    match driver.mode {
        config::TransportMode::Stream => {
            spawn_stream_session(cx, driver, None, buffers.output_doc);
        }
        config::TransportMode::Tail => {
            let mut state = AI_STATE.lock().unwrap();
            state.session = Some(AiSession::Tail(TailSessionState {
                driver,
                session_path: None,
                session_id: None,
            }));
            cx.editor
                .set_status("ai: tail mode ready (use :ai-resume to pick a session)".to_string());
        }
    }

    Ok(())
}

pub fn resume(cx: &mut compositor::Context, driver_name: Option<&str>) -> Result<()> {
    let config = AiConfig::load()?;
    let driver_name = driver_name.unwrap_or(&config.general.default_driver);
    let driver_cfg = config::require_driver(&config, driver_name)?;
    let driver = Driver::from_config(driver_name, driver_cfg);

    let buffers = ui::ensure_buffers(cx.editor, &config)?;

    let previous = {
        let mut state = AI_STATE.lock().unwrap();
        state.config = Some(config);
        state.buffers = Some(buffers);
        state.session.take()
    };

    if let Some(session) = previous {
        stop_session(cx, session);
    }

    let sessions = session::list_sessions(&driver)?;
    if sessions.is_empty() {
        return Err(anyhow!("no sessions found for {}", driver.name));
    }

    let driver_clone = driver.clone();
    let callback = async move {
        let call: job::Callback = job::Callback::EditorCompositor(Box::new(
            move |_editor, compositor| {
                let columns = [PickerColumn::new("session", |entry: &SessionEntry, _| {
                    entry.display.clone().into()
                })];
                let driver_for_picker = driver_clone.clone();
                let picker = Picker::new(columns, 0, sessions, (), move |cx, entry, _action| {
                    if let Err(err) = activate_session(cx, &driver_for_picker, entry.clone()) {
                        cx.editor.set_error(err.to_string());
                    }
                });
                compositor.push(Box::new(overlaid(picker)));
            },
        ));
        Ok(call)
    };
    cx.jobs.callback(callback);
    Ok(())
}

pub fn send(cx: &mut compositor::Context, prompt: Option<String>) -> Result<()> {
    enum SendTarget {
        Stream {
            stdin: Arc<tokio::sync::Mutex<tokio::process::ChildStdin>>,
            driver: Driver,
        },
        Tail(TailSessionState),
    }

    let (buffers, target) = {
        let state = AI_STATE.lock().unwrap();
        let buffers = state
            .buffers
            .clone()
            .ok_or_else(|| anyhow!("ai buffers are not initialized"))?;
        let target = match state.session.as_ref() {
            Some(AiSession::Stream(session)) => SendTarget::Stream {
                stdin: session.handle.stdin.clone(),
                driver: session.driver.clone(),
            },
            Some(AiSession::Tail(session)) => SendTarget::Tail(session.clone()),
            None => return Err(anyhow!("ai session is not active")),
        };
        (buffers, target)
    };

    let prompt = match prompt {
        Some(prompt) => prompt,
        None => ui::take_prompt(cx.editor, buffers.input_doc)?,
    };
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return Err(anyhow!("ai prompt is empty"));
    }

    match target {
        SendTarget::Stream { stdin, driver } => {
            let payload = driver.stream_payload(&prompt);
            cx.jobs.spawn(async move { stream::send_stream(stdin, payload).await });
        }
        SendTarget::Tail(session) => {
            send_tail_prompt(cx, session, prompt, buffers.output_doc)?;
        }
    }

    Ok(())
}

pub fn quit(cx: &mut compositor::Context) -> Result<()> {
    let session = {
        let mut state = AI_STATE.lock().unwrap();
        state.session.take()
    };

    if let Some(session) = session {
        stop_session(cx, session);
    }
    cx.editor.set_status("ai: stopped".to_string());
    Ok(())
}

pub fn resize(cx: &mut compositor::Context, resize: Resize) -> Result<()> {
    let target_view = {
        let state = AI_STATE.lock().unwrap();
        if let Some(buffers) = &state.buffers {
            let current_doc = view!(cx.editor).doc;
            if current_doc == buffers.output_doc || current_doc == buffers.input_doc {
                view!(cx.editor).id
            } else {
                ui::find_view_for_doc(cx.editor, buffers.input_doc)
                    .or_else(|| ui::find_view_for_doc(cx.editor, buffers.output_doc))
                    .unwrap_or_else(|| view!(cx.editor).id)
            }
        } else {
            view!(cx.editor).id
        }
    };

    cx.editor
        .resize_buffer(target_view, resize, Dimension::Fixed(5));
    Ok(())
}

fn stop_session(cx: &mut compositor::Context, session: AiSession) {
    if let AiSession::Stream(mut stream_session) = session {
        cx.jobs.spawn(async move {
            let _ = stream_session.handle.child.kill().await;
            let _ = stream_session.handle.child.wait().await;
            Ok(())
        });
    }
}

fn append_session_history(
    cx: &mut compositor::Context,
    driver: &Driver,
    output_doc: helix_view::DocumentId,
    path: &Path,
) -> Result<()> {
    if let Some(rendered) = format::render_session_file(path, driver.kind)? {
        if !rendered.is_empty() {
            ui::append_output(cx.editor, output_doc, &rendered);
        }
    }
    Ok(())
}

fn activate_session(
    cx: &mut compositor::Context,
    driver: &Driver,
    entry: SessionEntry,
) -> Result<()> {
    let buffers = {
        let state = AI_STATE.lock().unwrap();
        state
            .buffers
            .clone()
            .ok_or_else(|| anyhow!("ai buffers are not initialized"))?
    };

    match driver.mode {
        config::TransportMode::Stream => {
            if let Err(err) = append_session_history(cx, driver, buffers.output_doc, &entry.path) {
                cx.editor.set_error(format!("ai history error: {}", err));
            }
            spawn_stream_session(cx, driver.clone(), Some(entry.path.clone()), buffers.output_doc);
        }
        config::TransportMode::Tail => {
            if driver.requires_session() && entry.session_id.is_none() {
                return Err(anyhow!("session id missing for {}", driver.name));
            }
            if let Err(err) = append_session_history(cx, driver, buffers.output_doc, &entry.path) {
                cx.editor.set_error(format!("ai history error: {}", err));
            }
            let mut state = AI_STATE.lock().unwrap();
            state.session = Some(AiSession::Tail(TailSessionState {
                driver: driver.clone(),
                session_path: Some(entry.path),
                session_id: entry.session_id,
            }));
            cx.editor
                .set_status(format!("ai: resumed {}", driver.name));
        }
    }

    Ok(())
}

fn spawn_stream_session(
    cx: &mut compositor::Context,
    driver: Driver,
    session_path: Option<std::path::PathBuf>,
    output_doc: helix_view::DocumentId,
) {
    let spec = driver.stream_command(session_path.as_deref());
    cx.jobs.spawn(async move {
        let handle = stream::spawn_stream(spec, output_doc, driver.kind).await?;
        job::dispatch(move |editor, _| {
            let mut state = AI_STATE.lock().unwrap();
            state.session = Some(AiSession::Stream(StreamSessionState { driver, handle }));
            editor.set_status("ai: stream session ready".to_string());
        })
        .await;
        Ok(())
    });
}

fn send_tail_prompt(
    cx: &mut compositor::Context,
    session: TailSessionState,
    prompt: String,
    output_doc: helix_view::DocumentId,
) -> Result<()> {
    if session.driver.requires_session() && session.session_id.is_none() {
        return Err(anyhow!("ai session not selected; use :ai-resume"));
    }

    let driver = session.driver.clone();
    let session_path = session.session_path.clone();
    let session_id = session.session_id.clone();

    cx.jobs.spawn(async move {
        let start_time = SystemTime::now();
        let start_offset = session_path
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|meta| meta.len())
            .unwrap_or(0);

        let spec = driver.tail_command(session_id.as_deref(), &prompt);
        let mut child = stream::spawn_child(spec)?;

        let session_entry = resolve_tail_session(&driver, session_path, start_time).await?;
        let stop = Arc::new(AtomicBool::new(false));
        let tail_path = session_entry.path.clone();
        let tail_stop = stop.clone();

        tokio::spawn(async move {
            let _ = stream::tail_file(tail_path, start_offset, output_doc, tail_stop, driver.kind).await;
        });

        let status = child.wait().await;
        stop.store(true, Ordering::Relaxed);

        job::dispatch(move |editor, _| {
            let mut state = AI_STATE.lock().unwrap();
            if let Some(AiSession::Tail(tail)) = state.session.as_mut() {
                tail.session_path = Some(session_entry.path);
                if session_entry.session_id.is_some() {
                    tail.session_id = session_entry.session_id;
                }
            }
            if let Err(err) = status {
                editor.set_error(format!("ai driver failed: {}", err));
            }
        })
        .await;

        Ok(())
    });

    Ok(())
}

async fn resolve_tail_session(
    driver: &Driver,
    existing: Option<std::path::PathBuf>,
    start_time: SystemTime,
) -> Result<SessionEntry> {
    if let Some(path) = existing {
        return Ok(SessionEntry {
            path: path.clone(),
            display: path.to_string_lossy().to_string(),
            session_id: None,
            modified: start_time,
        });
    }

    let mut last = None;
    for _ in 0..20 {
        let sessions = session::list_sessions(driver)?;
        if let Some(entry) = sessions.into_iter().next() {
            if entry.modified >= start_time {
                return Ok(entry);
            }
            last = Some(entry);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    last.ok_or_else(|| anyhow!("unable to locate ai session file"))
}
