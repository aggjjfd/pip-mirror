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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_phase_started_with_total() {
        let line = format_phase_started("download", Some(100));
        assert_eq!(line, "[progress] 开始 download (共 100)");
    }

    #[test]
    fn test_format_phase_started_without_total() {
        let line = format_phase_started("import", None);
        assert_eq!(line, "[progress] 开始 import (共 ?)");
    }

    #[test]
    fn test_format_line_progress() {
        let line = format_line(SyncEvent::PhaseProgress {
            phase: "download",
            current: 7,
            message: "a.whl".to_string(),
        });
        assert_eq!(line, Some("[progress] download 7 a.whl".to_string()));
    }

    #[test]
    fn test_format_line_finished() {
        let line = format_line(SyncEvent::PhaseFinished {
            phase: "download",
            summary: "10/10".to_string(),
        });
        assert_eq!(line, Some("[progress] download 完成 10/10".to_string()));
    }

    #[test]
    fn test_format_line_error() {
        let line = format_line(SyncEvent::Error {
            message: "network error".to_string(),
        });
        assert_eq!(line, Some("[error] network error".to_string()));
    }

    #[test]
    fn test_format_file_done_failed() {
        let line = format_file_done(
            "bad.whl".to_string(),
            FileStatus::Failed("hash mismatch".to_string()),
        );
        assert_eq!(line, Some("[error] bad.whl: hash mismatch".to_string()));
    }

    #[test]
    fn test_format_file_done_downloaded_is_silent() {
        let line =
            format_file_done("ok.whl".to_string(), FileStatus::Downloaded);
        assert_eq!(line, None);
    }
}
