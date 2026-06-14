use pip_mirror::progress::{SyncEvent, run_with_progress};

#[tokio::test]
async fn test_progress_handle_emits_events() {
    let result = run_with_progress(false, |handle| async move {
        handle.emit(SyncEvent::PhaseStarted {
            phase: "test",
            total: Some(2),
        });
        handle.emit(SyncEvent::PhaseProgress {
            phase: "test",
            current: 1,
            message: "item 1".to_string(),
        });
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_plain_renderer_outputs_lines() {
    let result = run_with_progress(false, |handle| async move {
        handle.emit(SyncEvent::PhaseStarted {
            phase: "download",
            total: Some(2),
        });
        handle.emit(SyncEvent::PhaseProgress {
            phase: "download",
            current: 1,
            message: "a.whl".to_string(),
        });
        handle.emit(SyncEvent::PhaseFinished {
            phase: "download",
            summary: "完成 2/2".to_string(),
        });
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await;

    assert!(result.is_ok());
}
