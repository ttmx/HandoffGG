use crate::models::{AudioEndpoint, AudioSession, ChatMixConfig, EndpointFlow};

pub trait AudioBackend: Send + Sync {
    fn endpoints(&self) -> anyhow::Result<Vec<AudioEndpoint>>;
    fn set_default(&self, endpoint_id: &str, flow: EndpointFlow) -> anyhow::Result<()>;
    fn render_sessions(&self, chatmix: &ChatMixConfig) -> anyhow::Result<Vec<AudioSession>>;
    fn set_session_volume(&self, session_id: &str, volume: f32) -> anyhow::Result<()>;
}

#[cfg(windows)]
pub use crate::windows_audio::WindowsAudioBackend as NativeAudioBackend;

#[cfg(not(windows))]
pub struct NativeAudioBackend;

#[cfg(not(windows))]
impl NativeAudioBackend {
    pub fn new() -> anyhow::Result<Self> {
        anyhow::bail!("HandoffGG V1 only implements the Windows audio backend");
    }
}
