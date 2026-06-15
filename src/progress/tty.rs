use std::collections::HashMap;

use console::Term;
use indicatif::{
    MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle,
};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{Duration, interval};

use super::{FileStatus, SyncEvent};

const MAX_FPS: u8 = 10;
const RESIZE_POLL_MS: u64 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    msg_width: usize,
    bar_width: usize,
}

const MIN_MSG_LEN: usize = 8;
const DEFAULT_BAR_WIDTH: usize = 25;
const MIN_BAR_WIDTH: usize = 5;
const POS_LEN_WIDTH: usize = 12;
const MIN_TOTAL_WIDTH: u16 = 30;

fn calculate_layout(total_cols: u16) -> Layout {
    if total_cols < MIN_TOTAL_WIDTH {
        return Layout {
            msg_width: MIN_MSG_LEN,
            bar_width: MIN_BAR_WIDTH,
        };
    }

    let total = total_cols as usize;
    let available = total.saturating_sub(POS_LEN_WIDTH);
    let msg_width =
        available.saturating_sub(DEFAULT_BAR_WIDTH).max(MIN_MSG_LEN);

    Layout {
        msg_width,
        bar_width: DEFAULT_BAR_WIDTH,
    }
}

fn apply_layout(bar: &ProgressBar, layout: Layout) -> Layout {
    bar.set_style(
        ProgressStyle::with_template(&format!(
            "{{msg:<{}}} [{{bar:{}}}] {{pos}}/{{len}}",
            layout.msg_width, layout.bar_width
        ))
        .unwrap()
        .progress_chars("##-"),
    );
    layout
}

pub async fn render(mut rx: UnboundedReceiver<SyncEvent>) {
    let multi = MultiProgress::with_draw_target(
        ProgressDrawTarget::stderr_with_hz(MAX_FPS),
    );
    super::set_progress_multi(multi.clone());
    let bar = multi.add(ProgressBar::new(0));

    let mut phase_totals: HashMap<&'static str, u64> = HashMap::new();
    let mut layout =
        apply_layout(&bar, calculate_layout(Term::stderr().size().1));
    let mut state = RenderState {
        bar: &bar,
        phase_totals: &mut phase_totals,
    };

    let term = Term::stderr();
    let mut resize_tick = interval(Duration::from_millis(RESIZE_POLL_MS));

    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Some(event) => {
                        process_event(&multi, &mut state, layout.msg_width, event);
                    }
                    None => break,
                }
            }
            _ = resize_tick.tick() => {
                let new_layout = calculate_layout(term.size().1);
                if new_layout != layout {
                    layout = apply_layout(&bar, new_layout);
                }
            }
        }
    }

    bar.finish();
}

struct RenderState<'a> {
    bar: &'a ProgressBar,
    phase_totals: &'a mut HashMap<&'static str, u64>,
}

fn process_event(
    multi: &MultiProgress,
    state: &mut RenderState<'_>,
    msg_width: usize,
    event: SyncEvent,
) {
    match event {
        SyncEvent::PhaseStarted { phase, total } => {
            handle_phase_started(state, msg_width, phase, total);
        }
        SyncEvent::PhaseProgress {
            phase,
            current,
            message,
        } => {
            handle_phase_progress(state, msg_width, phase, current, message);
        }
        SyncEvent::PhaseFinished { phase, summary } => {
            handle_phase_finished(state, msg_width, phase, summary);
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

fn handle_phase_started(
    state: &mut RenderState<'_>,
    msg_width: usize,
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
        .set_message(truncate(&format!("{phase}: 准备中..."), msg_width));
}

fn handle_phase_progress(
    state: &RenderState<'_>,
    msg_width: usize,
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
        .set_message(truncate(&format!("{phase}: {message}"), msg_width));
}

fn handle_phase_finished(
    state: &RenderState<'_>,
    msg_width: usize,
    phase: &'static str,
    summary: String,
) {
    if let Some(&t) = state.phase_totals.get(phase) {
        state.bar.set_length(t);
        state.bar.set_position(t);
    }
    state
        .bar
        .set_message(truncate(&format!("{phase}: {summary}"), msg_width));
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

fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        s.chars()
            .take(max_len.saturating_sub(1))
            .collect::<String>()
            + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_layout_wide() {
        let layout = calculate_layout(120);
        assert_eq!(layout.msg_width, 120 - POS_LEN_WIDTH - DEFAULT_BAR_WIDTH);
        assert_eq!(layout.bar_width, DEFAULT_BAR_WIDTH);
    }

    #[test]
    fn test_calculate_layout_normal() {
        let layout = calculate_layout(80);
        assert_eq!(layout.msg_width, 80 - POS_LEN_WIDTH - DEFAULT_BAR_WIDTH);
        assert_eq!(layout.bar_width, DEFAULT_BAR_WIDTH);
    }

    #[test]
    fn test_calculate_layout_narrow() {
        let layout = calculate_layout(40);
        assert_eq!(layout.msg_width, MIN_MSG_LEN);
        assert_eq!(layout.bar_width, DEFAULT_BAR_WIDTH);
    }

    #[test]
    fn test_calculate_layout_minimum() {
        let layout = calculate_layout(20);
        assert_eq!(layout.msg_width, MIN_MSG_LEN);
        assert_eq!(layout.bar_width, MIN_BAR_WIDTH);
    }

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate("hello", 8), "hello");
    }

    #[test]
    fn test_truncate_long() {
        assert_eq!(truncate("hello world", 8), "hello w…");
    }

    #[test]
    fn test_truncate_zero() {
        assert_eq!(truncate("hello", 0), "…");
    }

    #[test]
    fn test_resolve_total_sync_to_filtered() {
        use indicatif::ProgressDrawTarget;

        let multi =
            MultiProgress::with_draw_target(ProgressDrawTarget::hidden());
        let bar = ProgressBar::new(0);
        let mut phase_totals: HashMap<&'static str, u64> = HashMap::new();
        let mut state = RenderState {
            bar: &bar,
            phase_totals: &mut phase_totals,
        };

        process_event(
            &multi,
            &mut state,
            20,
            SyncEvent::PhaseStarted {
                phase: "resolve",
                total: Some(399),
            },
        );
        process_event(
            &multi,
            &mut state,
            20,
            SyncEvent::PhaseProgress {
                phase: "resolve",
                current: 310,
                message: "依赖求解中".into(),
            },
        );
        process_event(
            &multi,
            &mut state,
            20,
            SyncEvent::PhaseStarted {
                phase: "resolve",
                total: Some(310),
            },
        );
        process_event(
            &multi,
            &mut state,
            20,
            SyncEvent::PhaseProgress {
                phase: "resolve",
                current: 310,
                message: "依赖求解完成".into(),
            },
        );

        assert_eq!(bar.position(), 310);
        assert_eq!(bar.length(), Some(310));
    }
}
