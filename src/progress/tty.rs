use std::collections::HashMap;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{FileStatus, SyncEvent};

pub async fn render(mut rx: UnboundedReceiver<SyncEvent>) {
    let multi = MultiProgress::new();
    let overall = multi.add(ProgressBar::new(0));
    let status = multi.add(ProgressBar::new(0));

    overall.set_style(
        ProgressStyle::with_template("{msg} [{bar:40}] {pos}/{len}")
            .unwrap()
            .progress_chars("##-"),
    );
    status.set_style(
        ProgressStyle::with_template("{msg}")
            .unwrap()
            .progress_chars("##-"),
    );

    let mut phase_totals: HashMap<&'static str, u64> = HashMap::new();
    let mut state = RenderState {
        overall: &overall,
        status: &status,
        phase_totals: &mut phase_totals,
    };
    process_events(&multi, &mut state, &mut rx).await;

    overall.finish();
    status.finish();
}

struct RenderState<'a> {
    overall: &'a ProgressBar,
    status: &'a ProgressBar,
    phase_totals: &'a mut HashMap<&'static str, u64>,
}

async fn process_events(
    multi: &MultiProgress,
    state: &mut RenderState<'_>,
    rx: &mut UnboundedReceiver<SyncEvent>,
) {
    while let Some(event) = rx.recv().await {
        match event {
            SyncEvent::PhaseStarted { phase, total } => {
                handle_phase_started(state, phase, total);
            }
            SyncEvent::PhaseProgress {
                phase,
                current,
                message,
            } => {
                handle_phase_progress(state, phase, current, message);
            }
            SyncEvent::PhaseFinished { phase, summary } => {
                handle_phase_finished(state, phase, summary);
            }
            SyncEvent::FileDone {
                filename,
                status: s,
                ..
            } => {
                handle_file_done(multi, filename, s);
            }
            SyncEvent::Error { message } | SyncEvent::Warning { message } => {
                multi.println(format!("! {message}")).ok();
            }
        }
    }
}

fn handle_phase_started(
    state: &mut RenderState<'_>,
    phase: &'static str,
    total: Option<u64>,
) {
    if let Some(t) = total {
        state.overall.set_length(t);
        state.phase_totals.insert(phase, t);
    } else {
        state.overall.set_length(0);
    }
    state.overall.set_position(0);
    state.overall.set_message(phase.to_string());
    state.status.set_message("准备中...".to_string());
}

fn handle_phase_progress(
    state: &RenderState<'_>,
    phase: &'static str,
    current: u64,
    message: String,
) {
    if let Some(&t) = state.phase_totals.get(phase) {
        state.overall.set_length(t);
    }
    state.overall.set_position(current);
    state.status.set_message(message);
}

fn handle_phase_finished(
    state: &RenderState<'_>,
    phase: &'static str,
    summary: String,
) {
    if let Some(&t) = state.phase_totals.get(phase) {
        state.overall.set_length(t);
        state.overall.set_position(t);
    }
    state.status.set_message(summary);
}

fn handle_file_done(
    multi: &MultiProgress,
    filename: String,
    status: FileStatus,
) {
    if let FileStatus::Failed(msg) = status {
        multi.println(format!("! {filename}: {msg}")).ok();
    }
}
