use tokio::sync::mpsc::UnboundedReceiver;

use super::SyncEvent;

pub async fn render(mut _rx: UnboundedReceiver<SyncEvent>) {}
