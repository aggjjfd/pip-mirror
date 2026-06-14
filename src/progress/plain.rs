use tokio::sync::mpsc::UnboundedReceiver;

use super::{FileStatus, SyncEvent};

pub async fn render(mut rx: UnboundedReceiver<SyncEvent>) {
    while let Some(event) = rx.recv().await {
        if let Some(line) = format_line(event) {
            println!("{line}");
        }
    }
}

fn format_line(event: SyncEvent) -> Option<String> {
    match event {
        SyncEvent::PhaseStarted { phase, total } => {
            Some(format_phase_started(phase, total))
        }
        SyncEvent::PhaseProgress {
            phase,
            current,
            message,
        } => Some(format!("[progress] {phase} {current} {message}")),
        SyncEvent::PhaseFinished { phase, summary } => {
            Some(format!("[progress] {phase} 完成 {summary}"))
        }
        SyncEvent::FileDone {
            filename, status, ..
        } => format_file_done(filename, status),
        SyncEvent::Error { message } => Some(format!("[error] {message}")),
        SyncEvent::Warning { message } => Some(format!("[warn] {message}")),
    }
}

fn format_phase_started(phase: &str, total: Option<u64>) -> String {
    let total_str = total
        .map(|n| n.to_string())
        .unwrap_or_else(|| "?".to_string());
    format!("[progress] 开始 {phase} (共 {total_str})")
}

fn format_file_done(filename: String, status: FileStatus) -> Option<String> {
    if let FileStatus::Failed(msg) = status {
        Some(format!("[error] {filename}: {msg}"))
    } else {
        None
    }
}
