use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::api::ModelInfo;

#[derive(Debug)]
pub enum AppEvent {
    Quit,
    Clear,
    Submit(String),
    LoadModels,
    SelectModel(String),
    StreamToken(String),
    StreamDone,
    StreamError(String),
    ModelsLoaded(Vec<ModelInfo>),
    ModelsError(String),
    Resize,
}

#[derive(Clone)]
pub struct AppEventSender {
    tx: UnboundedSender<AppEvent>,
}

impl AppEventSender {
    pub fn new(tx: UnboundedSender<AppEvent>) -> Self {
        Self { tx }
    }

    pub fn send(&self, event: AppEvent) {
        let _ = self.tx.send(event);
    }
}

pub fn channel() -> (AppEventSender, UnboundedReceiver<AppEvent>) {
    let (tx, rx) = unbounded_channel();
    (AppEventSender::new(tx), rx)
}
