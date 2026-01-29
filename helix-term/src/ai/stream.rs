use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::time::{sleep, Duration};

use helix_view::DocumentId;

use super::driver::{CommandSpec, DriverKind};
use super::format::FormatMode;
use super::{format, ui};
use crate::job;

pub struct StreamHandle {
    pub child: Child,
    pub stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
}

pub async fn spawn_stream(
    spec: CommandSpec,
    output_doc: DocumentId,
    driver_kind: DriverKind,
) -> Result<StreamHandle> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = command.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("failed to capture ai stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed to capture ai stdout"))?;
    let stderr = child.stderr.take();

    tokio::spawn(read_lines(stdout, output_doc, None, driver_kind));
    if let Some(stderr) = stderr {
        tokio::spawn(read_lines(stderr, output_doc, Some("stderr"), DriverKind::Other));
    }

    Ok(StreamHandle {
        child,
        stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
    })
}

pub fn spawn_child(spec: CommandSpec) -> Result<Child> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    Ok(command.spawn()?)
}

pub async fn send_stream(stdin: Arc<tokio::sync::Mutex<ChildStdin>>, payload: String) -> Result<()> {
    let mut stdin = stdin.lock().await;
    stdin.write_all(payload.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;
    Ok(())
}

pub async fn tail_file(
    path: PathBuf,
    start_offset: u64,
    output_doc: DocumentId,
    stop: Arc<AtomicBool>,
    driver_kind: DriverKind,
) -> Result<()> {
    let mut file = loop {
        match File::open(&path).await {
            Ok(file) => break file,
            Err(err) => {
                if stop.load(Ordering::Relaxed) {
                    return Ok(());
                }
                if err.kind() != std::io::ErrorKind::NotFound {
                    return Err(err.into());
                }
                sleep(Duration::from_millis(100)).await;
            }
        }
    };

    file.seek(std::io::SeekFrom::Start(start_offset)).await?;
    let mut reader = BufReader::new(file);
    let mut buf = String::new();

    let mut formatter = format::Formatter::new(driver_kind, FormatMode::SessionFile);
    loop {
        buf.clear();
        let read = reader.read_line(&mut buf).await?;
        if read == 0 {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            sleep(Duration::from_millis(100)).await;
            continue;
        }
        let line = buf.trim_end_matches(|c| c == '\n' || c == '\r');
        if let Some(rendered) = formatter.render_line(line) {
            job::dispatch(move |editor, _| {
                if driver_kind == DriverKind::Pi {
                    ui::append_output_raw(editor, output_doc, &rendered);
                } else {
                    ui::append_output(editor, output_doc, &rendered);
                }
            })
            .await;
        }
    }

    Ok(())
}

async fn read_lines<R>(
    reader: R,
    output_doc: DocumentId,
    label: Option<&'static str>,
    driver_kind: DriverKind,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut formatter = format::Formatter::new(driver_kind, FormatMode::Stream);
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        if let Some(mut rendered) = formatter.render_line(&line) {
            if let Some(label) = label {
                rendered = format!("[{}] {}", label, rendered);
            }
            job::dispatch(move |editor, _| {
                if driver_kind == DriverKind::Pi {
                    ui::append_output_raw(editor, output_doc, &rendered);
                } else {
                    ui::append_output(editor, output_doc, &rendered);
                }
            })
            .await;
        }
    }
    Ok(())
}
