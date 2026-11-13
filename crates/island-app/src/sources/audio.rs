//! Reports volume changes on the default output device and mute on the default
//! microphone, and follows both when the user switches to another device.

use island_core::Indicator;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{HWND, LPARAM, PROPERTYKEY, WPARAM};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator,
    IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator, eCapture, eConsole,
    eRender,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToBSTR;
use windows::Win32::System::Com::{CLSCTX_ALL, CLSCTX_INPROC_SERVER, CoCreateInstance, STGM_READ};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{Error, PCWSTR, Result, implement};

use super::IndicatorSink;

/// `wParam` says which device: `DEVICE_OUTPUT` or `DEVICE_MICROPHONE`.
pub const MSG_AUDIO_DEVICE: u32 = WM_APP + 6;
pub const DEVICE_OUTPUT: usize = 0;
pub const DEVICE_MICROPHONE: usize = 1;

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback {
    sink: IndicatorSink,
}

#[implement(IAudioEndpointVolumeCallback)]
struct MicrophoneCallback {
    sink: IndicatorSink,
}

impl IAudioEndpointVolumeCallback_Impl for MicrophoneCallback_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        let data = unsafe { data.as_ref() }.ok_or_else(Error::empty)?;
        self.sink.send(Indicator::Microphone {
            muted: data.bMuted.as_bool(),
        });
        Ok(())
    }
}

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        let data = unsafe { data.as_ref() }.ok_or_else(Error::empty)?;
        let percent = (data.fMasterVolume * 100.0).round() as u8;
        self.sink.send(Indicator::Volume {
            percent,
            muted: data.bMuted.as_bool(),
        });
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceClient {
    window: isize,
}

impl IMMNotificationClient_Impl for DeviceClient_Impl {
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _device: &PCWSTR) -> Result<()> {
        if role == eConsole {
            let device = if flow == eRender {
                DEVICE_OUTPUT
            } else {
                DEVICE_MICROPHONE
            };
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.window as _)),
                    MSG_AUDIO_DEVICE,
                    WPARAM(device),
                    LPARAM(0),
                );
            }
        }
        Ok(())
    }

    fn OnDeviceStateChanged(&self, _device: &PCWSTR, _state: DEVICE_STATE) -> Result<()> {
        Ok(())
    }

    fn OnDeviceAdded(&self, _device: &PCWSTR) -> Result<()> {
        Ok(())
    }

    fn OnDeviceRemoved(&self, _device: &PCWSTR) -> Result<()> {
        Ok(())
    }

    fn OnPropertyValueChanged(&self, _device: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
        Ok(())
    }
}

pub struct AudioWatch {
    enumerator: IMMDeviceEnumerator,
    device_client: IMMNotificationClient,
    callback: IAudioEndpointVolumeCallback,
    endpoint: Option<IAudioEndpointVolume>,
    microphone_callback: IAudioEndpointVolumeCallback,
    microphone: Option<IAudioEndpointVolume>,
    sink: IndicatorSink,
}

impl AudioWatch {
    pub fn start(hwnd: HWND, sink: IndicatorSink) -> Result<Self> {
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_INPROC_SERVER)? };
        let device_client: IMMNotificationClient = DeviceClient {
            window: hwnd.0 as isize,
        }
        .into();
        unsafe { enumerator.RegisterEndpointNotificationCallback(&device_client)? };

        let mut watch = Self {
            enumerator,
            device_client,
            callback: VolumeCallback { sink: sink.clone() }.into(),
            endpoint: None,
            microphone_callback: MicrophoneCallback { sink: sink.clone() }.into(),
            microphone: None,
            sink,
        };
        watch.follow_default_microphone();
        Ok(watch)
    }

    /// Returns the device's name. A machine with no output device is fine: there
    /// is just nothing to watch.
    pub fn follow_default_device(&mut self) -> Option<String> {
        detach(&mut self.endpoint, &self.callback);
        let (endpoint, name) = self.attach(eRender, &self.callback).ok()?;
        self.endpoint = Some(endpoint);
        name
    }

    /// Read the actual output state for the controls panel.
    pub fn output_level(&self) -> Result<Option<(u8, bool)>> {
        let Some(endpoint) = &self.endpoint else {
            return Ok(None);
        };
        let level = unsafe { endpoint.GetMasterVolumeLevelScalar()? };
        let muted = unsafe { endpoint.GetMute()? }.as_bool();
        Ok(Some((
            (level * 100.0).round().clamp(0.0, 100.0) as u8,
            muted,
        )))
    }

    pub fn set_output_level(&self, percent: u8) -> Result<Option<(u8, bool)>> {
        let Some(endpoint) = &self.endpoint else {
            return Ok(None);
        };
        unsafe {
            endpoint.SetMasterVolumeLevelScalar(
                f32::from(percent.min(100)) / 100.0,
                std::ptr::null(),
            )?;
        }
        self.output_level()
    }

    pub fn toggle_output_mute(&self) -> Result<Option<(u8, bool)>> {
        let Some(endpoint) = &self.endpoint else {
            return Ok(None);
        };
        let muted = unsafe { endpoint.GetMute()? }.as_bool();
        unsafe {
            endpoint.SetMute(!muted, std::ptr::null())?;
        }
        self.output_level()
    }

    /// Reports its mute state right away, which is the starting point for
    /// telling a change.
    pub fn follow_default_microphone(&mut self) {
        detach(&mut self.microphone, &self.microphone_callback);
        let Ok((endpoint, _)) = self.attach(eCapture, &self.microphone_callback) else {
            return;
        };
        if let Ok(muted) = unsafe { endpoint.GetMute() } {
            self.sink.send(Indicator::Microphone {
                muted: muted.as_bool(),
            });
        }
        self.microphone = Some(endpoint);
    }

    fn attach(
        &self,
        flow: EDataFlow,
        callback: &IAudioEndpointVolumeCallback,
    ) -> Result<(IAudioEndpointVolume, Option<String>)> {
        unsafe {
            let device = self.enumerator.GetDefaultAudioEndpoint(flow, eConsole)?;
            let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
            endpoint.RegisterControlChangeNotify(callback)?;
            Ok((endpoint, device_name(&device)))
        }
    }
}

fn device_name(device: &IMMDevice) -> Option<String> {
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ).ok()?;
        let name = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        PropVariantToBSTR(&name).ok().map(|name| name.to_string())
    }
}

fn detach(endpoint: &mut Option<IAudioEndpointVolume>, callback: &IAudioEndpointVolumeCallback) {
    if let Some(endpoint) = endpoint.take() {
        unsafe {
            let _ = endpoint.UnregisterControlChangeNotify(callback);
        }
    }
}

impl Drop for AudioWatch {
    fn drop(&mut self) {
        detach(&mut self.endpoint, &self.callback);
        detach(&mut self.microphone, &self.microphone_callback);
        unsafe {
            let _ = self
                .enumerator
                .UnregisterEndpointNotificationCallback(&self.device_client);
        }
    }
}
