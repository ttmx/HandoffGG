use crate::audio::AudioBackend;
use crate::chatmix::{app_id_for_session, route_for_app};
use crate::models::{AudioEndpoint, AudioSession, ChatMixConfig, EndpointFlow, EndpointState};
use anyhow::Context;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;
use windows::core::{Interface, GUID, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, PROPERTYKEY};
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, ERole,
    IAudioSessionControl, IAudioSessionControl2, IAudioSessionManager2, IMMDevice,
    IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl, ISimpleAudioVolume,
    MMDeviceEnumerator, DEVICE_STATE, DEVICE_STATE_ACTIVE, DEVICE_STATE_DISABLED,
    DEVICE_STATE_NOTPRESENT, DEVICE_STATE_UNPLUGGED,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_core::BOOL;
use windows_implement::implement;

pub struct WindowsAudioBackend;

impl WindowsAudioBackend {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self)
    }
}

impl AudioBackend for WindowsAudioBackend {
    fn endpoints(&self) -> anyhow::Result<Vec<AudioEndpoint>> {
        with_com(|| {
            let enumerator = device_enumerator()?;
            let mut endpoints = Vec::new();
            endpoints.extend(enumerate_flow(&enumerator, EndpointFlow::Render)?);
            endpoints.extend(enumerate_flow(&enumerator, EndpointFlow::Capture)?);
            endpoints.retain(|endpoint| {
                !endpoint
                    .name
                    .to_ascii_lowercase()
                    .contains("steelseries sonar")
            });
            Ok(endpoints)
        })
    }

    fn set_default(&self, endpoint_id: &str, _flow: EndpointFlow) -> anyhow::Result<()> {
        with_com(|| {
            let policy: IPolicyConfig = unsafe {
                CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)
                    .context("failed to create IPolicyConfig")?
            };
            let endpoint_id = widestring(endpoint_id);
            for role in [eConsole, eMultimedia, eCommunications] {
                unsafe {
                    (Interface::vtable(&policy).SetDefaultEndpoint)(
                        Interface::as_raw(&policy),
                        PCWSTR(endpoint_id.as_ptr()),
                        role,
                    )
                    .ok()
                    .context("failed to set default endpoint")?;
                }
            }
            Ok(())
        })
    }

    fn render_sessions(&self, chatmix: &ChatMixConfig) -> anyhow::Result<Vec<AudioSession>> {
        with_com(|| {
            let enumerator = device_enumerator()?;
            let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) }
                .context("failed to get default render endpoint")?;
            enumerate_render_sessions(&device, chatmix)
        })
    }

    fn set_session_volume(&self, session_id: &str, volume: f32) -> anyhow::Result<()> {
        with_com(|| {
            let enumerator = device_enumerator()?;
            let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) }
                .context("failed to get default render endpoint")?;
            let mut found = false;
            for_each_session(&device, |control, control2| {
                if session_instance_id(control2)? != session_id {
                    return Ok(false);
                }
                let volume_control: ISimpleAudioVolume = control
                    .cast()
                    .context("failed to query session volume control")?;
                unsafe {
                    volume_control
                        .SetMasterVolume(volume.clamp(0.0, 1.0), std::ptr::null())
                        .context("failed to set session volume")?;
                }
                found = true;
                Ok(true)
            })?;
            anyhow::ensure!(found, "Audio session was not found");
            Ok(())
        })
    }
}

pub fn start_endpoint_notification_listener(tx: Sender<()>) {
    thread::spawn(move || {
        if let Err(error) = endpoint_notification_loop(tx) {
            eprintln!("audio endpoint notification listener stopped: {error}");
        }
    });
}

fn endpoint_notification_loop(tx: Sender<()>) -> anyhow::Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    }
    let result = (|| {
        let enumerator = device_enumerator()?;
        let callback: IMMNotificationClient = EndpointNotificationClient { tx }.into();
        unsafe {
            enumerator
                .RegisterEndpointNotificationCallback(&callback)
                .context("failed to register endpoint notification callback")?;
        }

        loop {
            thread::park_timeout(Duration::from_secs(3600));
        }
    })();
    unsafe {
        CoUninitialize();
    }
    result
}

#[implement(IMMNotificationClient)]
struct EndpointNotificationClient {
    tx: Sender<()>,
}

#[allow(non_snake_case)]
impl EndpointNotificationClient {
    fn notify(&self) {
        let _ = self.tx.send(());
    }
}

#[allow(non_snake_case)]
impl IMMNotificationClient_Impl for EndpointNotificationClient_Impl {
    fn OnDeviceStateChanged(
        &self,
        _pwstrdeviceid: &PCWSTR,
        _dwnewstate: DEVICE_STATE,
    ) -> windows::core::Result<()> {
        windows_core::IUnknownImpl::get_impl(self).notify();
        Ok(())
    }

    fn OnDeviceAdded(&self, _pwstrdeviceid: &PCWSTR) -> windows::core::Result<()> {
        windows_core::IUnknownImpl::get_impl(self).notify();
        Ok(())
    }

    fn OnDeviceRemoved(&self, _pwstrdeviceid: &PCWSTR) -> windows::core::Result<()> {
        windows_core::IUnknownImpl::get_impl(self).notify();
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        _flow: EDataFlow,
        _role: ERole,
        _pwstrdefaultdeviceid: &PCWSTR,
    ) -> windows::core::Result<()> {
        windows_core::IUnknownImpl::get_impl(self).notify();
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _pwstrdeviceid: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> windows::core::Result<()> {
        windows_core::IUnknownImpl::get_impl(self).notify();
        Ok(())
    }
}

/// Walk every audio session on `device`, yielding each session's control interfaces.
/// Sessions whose `IAudioSessionControl2` cast fails are skipped. The closure returns
/// `true` to stop the walk early.
fn for_each_session(
    device: &IMMDevice,
    mut visit: impl FnMut(&IAudioSessionControl, &IAudioSessionControl2) -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    let manager: IAudioSessionManager2 = unsafe { device.Activate(CLSCTX_ALL, None) }
        .context("failed to activate audio session manager")?;
    let sessions =
        unsafe { manager.GetSessionEnumerator() }.context("failed to enumerate audio sessions")?;
    let count = unsafe { sessions.GetCount() }.context("failed to get session count")?;
    for index in 0..count {
        let control =
            unsafe { sessions.GetSession(index) }.context("failed to get audio session")?;
        let Ok(control2) = control.cast::<IAudioSessionControl2>() else {
            continue;
        };
        if visit(&control, &control2)? {
            break;
        }
    }
    Ok(())
}

fn enumerate_render_sessions(
    device: &IMMDevice,
    chatmix: &ChatMixConfig,
) -> anyhow::Result<Vec<AudioSession>> {
    let mut result = Vec::new();
    for_each_session(device, |control, control2| {
        if unsafe { control2.IsSystemSoundsSession() }.0 == 0 {
            return Ok(false);
        }
        let Ok(volume_control) = control.cast::<ISimpleAudioVolume>() else {
            return Ok(false);
        };

        let id = session_instance_id(control2)?;
        let process_id = unsafe { control2.GetProcessId() }.unwrap_or_default();
        let executable_path = process_image_path(process_id);
        let raw_display = unsafe { control.GetDisplayName() }
            .ok()
            .and_then(|name| pwstr_to_string_and_free(name).ok())
            .filter(|value| !value.trim().is_empty());
        let display_name = raw_display
            .or_else(|| {
                executable_path
                    .as_deref()
                    .and_then(file_name)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| format!("Process {process_id}"));
        let app_id = app_id_for_session(executable_path.as_deref(), &display_name, process_id);
        let (route, route_source) =
            route_for_app(&app_id, &display_name, executable_path.as_deref(), chatmix);
        let volume = unsafe { volume_control.GetMasterVolume() }.unwrap_or(1.0);
        let muted = unsafe { volume_control.GetMute() }
            .unwrap_or(BOOL(0))
            .as_bool();

        result.push(AudioSession {
            id,
            app_id,
            display_name,
            executable_path,
            process_id,
            route,
            route_source,
            volume,
            muted,
        });
        Ok(false)
    })?;

    result.sort_by(|a, b| {
        a.display_name
            .cmp(&b.display_name)
            .then(a.app_id.cmp(&b.app_id))
            .then(a.id.cmp(&b.id))
    });
    Ok(result)
}

fn enumerate_flow(
    enumerator: &IMMDeviceEnumerator,
    flow: EndpointFlow,
) -> anyhow::Result<Vec<AudioEndpoint>> {
    let data_flow = match flow {
        EndpointFlow::Render => eRender,
        EndpointFlow::Capture => eCapture,
    };
    let collection = unsafe {
        enumerator.EnumAudioEndpoints(
            data_flow,
            windows::Win32::Media::Audio::DEVICE_STATE(
                DEVICE_STATE_ACTIVE.0
                    | DEVICE_STATE_DISABLED.0
                    | DEVICE_STATE_NOTPRESENT.0
                    | DEVICE_STATE_UNPLUGGED.0,
            ),
        )
    }
    .context("failed to enumerate audio endpoints")?;

    let count = unsafe { collection.GetCount() }.context("failed to read endpoint count")?;
    let defaults = DefaultIds {
        console: default_id(enumerator, data_flow, eConsole),
        multimedia: default_id(enumerator, data_flow, eMultimedia),
        communications: default_id(enumerator, data_flow, eCommunications),
    };

    let mut endpoints = Vec::new();
    for index in 0..count {
        let device = unsafe { collection.Item(index) }.context("failed to read endpoint")?;
        let id = device_id(&device)?;
        let name = device_name(&device).unwrap_or_else(|_| id.clone());
        let state = unsafe { device.GetState() }
            .map(map_state)
            .unwrap_or(EndpointState::Unknown);

        endpoints.push(AudioEndpoint {
            is_default_console: defaults.console.as_deref() == Some(id.as_str()),
            is_default_multimedia: defaults.multimedia.as_deref() == Some(id.as_str()),
            is_default_communications: defaults.communications.as_deref() == Some(id.as_str()),
            is_presence_tracked: is_presence_tracked(&name),
            id,
            name,
            flow,
            state,
        });
    }

    endpoints.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(endpoints)
}

fn device_enumerator() -> anyhow::Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .context("failed to create MMDeviceEnumerator")
}

fn default_id(enumerator: &IMMDeviceEnumerator, flow: EDataFlow, role: ERole) -> Option<String> {
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(flow, role) }.ok()?;
    device_id(&device).ok()
}

fn device_id(device: &IMMDevice) -> anyhow::Result<String> {
    let id = unsafe { device.GetId() }.context("failed to read endpoint id")?;
    pwstr_to_string_and_free(id)
}

fn device_name(device: &IMMDevice) -> anyhow::Result<String> {
    let store =
        unsafe { device.OpenPropertyStore(STGM_READ) }.context("failed to open property store")?;
    let mut value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }
        .context("failed to read friendly name")?;
    let name = value.to_string();
    unsafe {
        let _ = PropVariantClear(&mut value);
    }
    Ok(name)
}

fn pwstr_to_string_and_free(value: PWSTR) -> anyhow::Result<String> {
    let result = unsafe { value.to_string() }.context("failed to convert Windows string")?;
    unsafe {
        CoTaskMemFree(Some(value.0.cast()));
    }
    Ok(result)
}

fn session_instance_id(control: &IAudioSessionControl2) -> anyhow::Result<String> {
    let id = unsafe { control.GetSessionInstanceIdentifier() }
        .context("failed to read session instance id")?;
    pwstr_to_string_and_free(id)
}

fn process_image_path(process_id: u32) -> Option<String> {
    if process_id == 0 {
        return None;
    }

    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }.ok()?;
    let mut buffer = vec![0_u16; 32_768];
    let mut size = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
    };
    unsafe {
        let _ = CloseHandle(handle);
    }
    result.ok()?;
    Some(String::from_utf16_lossy(&buffer[..size as usize]))
}

fn file_name(path: &str) -> Option<&str> {
    path.rsplit(['\\', '/'])
        .next()
        .filter(|value| !value.is_empty())
}

/// SteelSeries/Arctis endpoints expose a USB dongle that stays present while the
/// wireless headset is powered off, so their availability is driven by HID presence.
fn is_presence_tracked(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("arctis") || lower.contains("steelseries")
}

fn map_state(state: windows::Win32::Media::Audio::DEVICE_STATE) -> EndpointState {
    if state.0 == DEVICE_STATE_ACTIVE.0 {
        EndpointState::Active
    } else if state.0 == DEVICE_STATE_DISABLED.0 {
        EndpointState::Disabled
    } else if state.0 == DEVICE_STATE_NOTPRESENT.0 {
        EndpointState::NotPresent
    } else if state.0 == DEVICE_STATE_UNPLUGGED.0 {
        EndpointState::Unplugged
    } else {
        EndpointState::Unknown
    }
}

fn with_com<T>(f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }
    let result = f();
    unsafe {
        CoUninitialize();
    }
    result
}

fn widestring(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

struct DefaultIds {
    console: Option<String>,
    multimedia: Option<String>,
    communications: Option<String>,
}

#[repr(transparent)]
#[derive(Clone, PartialEq, Eq)]
struct IPolicyConfig(windows::core::IUnknown);

unsafe impl Interface for IPolicyConfig {
    type Vtable = IPolicyConfig_Vtbl;
    const IID: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
}

#[repr(C)]
#[allow(non_snake_case)]
struct IPolicyConfig_Vtbl {
    pub base__: windows::core::IUnknown_Vtbl,
    pub GetMixFormat: usize,
    pub GetDeviceFormat: usize,
    pub ResetDeviceFormat: usize,
    pub SetDeviceFormat: usize,
    pub GetProcessingPeriod: usize,
    pub SetProcessingPeriod: usize,
    pub GetShareMode: usize,
    pub SetShareMode: usize,
    pub GetPropertyValue: usize,
    pub SetPropertyValue: usize,
    pub SetDefaultEndpoint:
        unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR, ERole) -> windows::core::HRESULT,
    pub SetEndpointVisibility: usize,
}

const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
