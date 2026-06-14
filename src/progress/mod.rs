use std::io::{IsTerminal, Write};
use std::sync::OnceLock;

use indicatif::MultiProgress;
use tokio::sync::mpsc;

pub mod events;
mod plain;
mod tty;

pub use events::{FileStatus, SyncEvent};

static PROGRESS_MULTI: OnceLock<MultiProgress> = OnceLock::new();

pub fn set_progress_multi(multi: MultiProgress) {
    let _ = PROGRESS_MULTI.set(multi);
}

pub struct ProgressWriterMaker;

impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a>
    for ProgressWriterMaker
{
    type Writer = ProgressWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ProgressWriter
    }
}

pub struct ProgressWriter;

impl Write for ProgressWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        let Some(multi) = PROGRESS_MULTI.get() else {
            return std::io::stderr().write_all(buf).map(|_| buf.len());
        };
        for line in text.lines() {
            multi.println(line).ok();
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stderr().flush()
    }
}

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
