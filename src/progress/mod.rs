use std::io::IsTerminal;

use tokio::sync::mpsc;

pub mod events;
mod plain;
mod tty;

pub use events::{FileStatus, SyncEvent};

#[derive(Clone)]
pub struct ProgressHandle {
    tx: mpsc::UnboundedSender<SyncEvent>,
}

impl ProgressHandle {
    pub fn emit(&self, event: SyncEvent) {
        let _ = self.tx.send(event);
    }
}

pub async fn run_with_progress<F, Fut, T>(
    _verbose: bool,
    f: F,
) -> Result<T, Box<dyn std::error::Error>>
where
    F: FnOnce(ProgressHandle) -> Fut,
    Fut: std::future::Future<Output = Result<T, Box<dyn std::error::Error>>>,
{
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = ProgressHandle { tx };

    let renderer = if std::io::stdout().is_terminal() {
        tokio::spawn(async move { tty::render(rx).await })
    } else {
        tokio::spawn(async move { plain::render(rx).await })
    };

    let result = f(handle).await;
    let render_result = renderer.await;

    match (result, render_result) {
        (Ok(v), Ok(_)) => Ok(v),
        (Err(e), Ok(_)) => Err(e),
        (Ok(_), Err(e)) => Err(format!("进度渲染任务异常: {e}").into()),
        (Err(e), Err(re)) => {
            Err(format!("业务失败: {e}; 渲染任务异常: {re}").into())
        }
    }
}
