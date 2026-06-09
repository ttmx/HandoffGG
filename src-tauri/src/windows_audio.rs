use crate::audio::AudioBackend;
use crate::models::{AudioEndpoint, EndpointFlow, EndpointState};
use anyhow::Context;
use windows::core::{Interface, GUID, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, ERole, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE, DEVICE_STATE_DISABLED,
    DEVICE_STATE_NOTPRESENT, DEVICE_STATE_UNPLUGGED,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_APARTMENTTHREADED, STGM_READ,
};

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
