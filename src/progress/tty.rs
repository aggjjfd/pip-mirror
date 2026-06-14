use std::collections::HashMap;

use indicatif::{
    MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle,
};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{FileStatus, SyncEvent};

const MAX_MSG_LEN: usize = 30;
const BAR_WIDTH: usize = 25;
const MAX_FPS: u8 = 10;

pub async fn render(mut rx: UnboundedReceiver<SyncEvent>) {
    let multi = MultiProgress::with_draw_target(
        ProgressDrawTarget::stderr_with_hz(MAX_FPS),
    );
    super::set_progress_multi(multi.clone());
    let bar = multi.add(ProgressBar::new(0));

    bar.set_style(
        ProgressStyle::with_template(&format!(
            "{{msg:<{MAX_MSG_LEN}}} [{{bar:{BAR_WIDTH}}}] {{pos}}/{{len}}"
        ))
        .unwrap()
        .progress_chars("##-"),
    );

    let mut phase_totals: HashMap<&'static str, u64> = HashMap::new();
    let mut state = RenderState {
        bar: &bar,
        phase_totals: &mut phase_totals,
    };
    process_events(&multi, &mut state, &mut rx).await;

    bar.finish();
}

struct RenderState<'a> {
    bar: &'a ProgressBar,
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
        state.bar.set_length(t);
        state.phase_totals.insert(phase, t);
    } else {
        state.bar.set_length(0);
    }
    state.bar.set_position(0);
    state
        .bar
        .set_message(truncate(&format!("{phase}: 准备中...")));
}

fn handle_phase_progress(
    state: &RenderState<'_>,
    phase: &'static str,
    current: u64,
    message: String,
) {
    if let Some(&t) = state.phase_totals.get(phase) {
        state.bar.set_length(t);
    }
    state.bar.set_position(current);
    state
        .bar
        .set_message(truncate(&format!("{phase}: {message}")));
}

fn handle_phase_finished(
    state: &RenderState<'_>,
    phase: &'static str,
    summary: String,
) {
    if let Some(&t) = state.phase_totals.get(phase) {
        state.bar.set_length(t);
        state.bar.set_position(t);
    }
    state
        .bar
        .set_message(truncate(&format!("{phase}: {summary}")));
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

fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_MSG_LEN {
        s.to_string()
    } else {
        s.chars().take(MAX_MSG_LEN - 1).collect::<String>() + "…"
    }
}
