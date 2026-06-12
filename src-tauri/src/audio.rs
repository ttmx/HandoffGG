use crate::models::{AudioEndpoint, AudioSession, ChatMixConfig, EndpointFlow};

pub trait AudioBackend: Send + Sync {
    fn endpoints(&self) -> anyhow::Result<Vec<AudioEndpoint>>;
    fn set_default(&self, endpoint_id: &str, flow: EndpointFlow) -> anyhow::Result<()>;
    fn render_sessions(&self, chatmix: &ChatMixConfig) -> anyhow::Result<Vec<AudioSession>>;
    fn set_session_volume(&self, session_id: &str, volume: f32) -> anyhow::Result<()>;

    /// Backends that can push device-change notifications (e.g. the PipeWire registry)
    /// hand out a receiver here, once, for the audio-device monitor to drive
    /// re-evaluation. Backends without such a channel (Windows uses its own
    /// `IMMNotificationClient` listener) return `None`.
    fn take_change_receiver(&self) -> Option<std::sync::mpsc::Receiver<()>> {
        None
    }
}

#[cfg(windows)]
pub use crate::windows_audio::WindowsAudioBackend as NativeAudioBackend;

#[cfg(target_os = "linux")]
pub use crate::pipewire_audio::PipewireAudioBackend as NativeAudioBackend;

#[cfg(not(any(windows, target_os = "linux")))]
pub struct NativeAudioBackend;

#[cfg(not(any(windows, target_os = "linux")))]
impl NativeAudioBackend {
    pub fn new() -> anyhow::Result<Self> {
        anyhow::bail!("HandoffGG implements audio backends for Windows and Linux (PipeWire) only");
    }
}
