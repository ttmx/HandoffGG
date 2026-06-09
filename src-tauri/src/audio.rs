use crate::models::{AudioEndpoint, EndpointFlow};

pub trait AudioBackend: Send + Sync {
    fn endpoints(&self) -> anyhow::Result<Vec<AudioEndpoint>>;
    fn set_default(&self, endpoint_id: &str, flow: EndpointFlow) -> anyhow::Result<()>;
}

#[cfg(windows)]
pub use crate::windows_audio::WindowsAudioBackend as NativeAudioBackend;

#[cfg(not(windows))]
pub struct NativeAudioBackend;

#[cfg(not(windows))]
impl NativeAudioBackend {
    pub fn new() -> anyhow::Result<Self> {
        anyhow::bail!("Autoswapper V1 only implements the Windows audio backend");
    }
}
