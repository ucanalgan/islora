//! On a single-monitor system, WMI can control an active integrated panel.
//! Multi-monitor mapping is deliberately left to the Windows settings fallback.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICEW, EnumDisplayDevicesW, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MONITORINFOEXW, MonitorFromWindow,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoSetProxyBlanket, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL,
    RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VT_I4, VT_UI1, VT_UI4, VariantClear};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemLocator, IWbemServices, WBEM_FLAG_FORWARD_ONLY,
    WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_GENERIC_FLAG_TYPE, WbemLocator,
};
use windows::core::{BSTR, PCWSTR, Result, w};

pub(super) fn brightness(hwnd: HWND, requested: Option<u8>) -> Option<u8> {
    let device_id = monitor_id(hwnd)?;
    match unsafe { read_or_set(&device_id, requested) } {
        Ok(value) => value,
        Err(error) => {
            eprintln!("integrated display brightness unavailable: {error}");
            None
        }
    }
}

fn monitor_id(hwnd: HWND) -> Option<String> {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(
            monitor,
            (&mut info as *mut MONITORINFOEXW).cast::<MONITORINFO>(),
        )
        .as_bool()
        {
            return None;
        }
        let mut device = DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // EDD_GET_DEVICE_INTERFACE_NAME yields the monitor interface path,
        // including the PnP instance used by the WMI brightness provider.
        if !EnumDisplayDevicesW(PCWSTR(info.szDevice.as_ptr()), 0, &mut device, 1).as_bool() {
            return None;
        }
        let end = device
            .DeviceID
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(device.DeviceID.len());
        Some(String::from_utf16_lossy(&device.DeviceID[..end]))
    }
}

unsafe fn read_or_set(device_id: &str, requested: Option<u8>) -> Result<Option<u8>> {
    unsafe {
        let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
        let empty = BSTR::new();
        let services = locator.ConnectServer(
            &BSTR::from("ROOT\\WMI"),
            &empty,
            &empty,
            &empty,
            0,
            &empty,
            None,
        )?;
        // RPC_C_AUTHN_WINNT and RPC_C_AUTHZ_NONE, respectively.
        CoSetProxyBlanket(
            &services,
            10,
            0,
            PCWSTR::null(),
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )?;
        let Some(current) = one_active(&services, "WmiMonitorBrightness")? else {
            return Ok(None);
        };
        let Some(current_name) = string_property(&current, w!("InstanceName"))? else {
            return Ok(None);
        };
        // Require the actual display instance, not just a matching model. Some
        // drivers expose different identifiers; those use the settings fallback.
        if !same_display(device_id, &current_name.to_string()) {
            return Ok(None);
        }
        if let Some(percent) = requested {
            let Some(method) = one_active(&services, "WmiMonitorBrightnessMethods")? else {
                return Ok(None);
            };
            let Some(method_name) = string_property(&method, w!("InstanceName"))? else {
                return Ok(None);
            };
            if current_name != method_name {
                return Ok(None);
            }
            let Some(path) = string_property(&method, w!("__PATH"))? else {
                return Ok(None);
            };
            let mut class = None;
            services.GetObject(
                &BSTR::from("WmiMonitorBrightnessMethods"),
                WBEM_GENERIC_FLAG_TYPE(0),
                None,
                Some(&mut class),
                None,
            )?;
            let Some(class) = class else { return Ok(None) };
            let mut signature = None;
            class.GetMethod(
                w!("WmiSetBrightness"),
                0,
                &mut signature,
                std::ptr::null_mut(),
            )?;
            let Some(signature) = signature else {
                return Ok(None);
            };
            let input = signature.SpawnInstance(0)?;
            let mut level = VARIANT::default();
            (*level.Anonymous.Anonymous).vt = VT_UI1;
            (*level.Anonymous.Anonymous).Anonymous.bVal = percent;
            input.Put(w!("Brightness"), 0, &level, 0)?;
            let mut timeout = VARIANT::default();
            // WMI represents CIM uint32 as VT_I4 in Automation variants.
            (*timeout.Anonymous.Anonymous).vt = VT_I4;
            (*timeout.Anonymous.Anonymous).Anonymous.lVal = 0;
            input.Put(w!("Timeout"), 0, &timeout, 0)?;
            let mut output = None;
            services.ExecMethod(
                &path,
                &BSTR::from("WmiSetBrightness"),
                WBEM_GENERIC_FLAG_TYPE(0),
                None,
                &input,
                Some(&mut output),
                None,
            )?;
            let Some(output) = output else {
                return Ok(None);
            };
            if number_property(&output, w!("ReturnValue"))? != Some(0) {
                return Ok(None);
            }
            let Some(updated) = one_active(&services, "WmiMonitorBrightness")? else {
                return Ok(None);
            };
            return Ok(number_property(&updated, w!("CurrentBrightness"))?
                .and_then(|value| u8::try_from(value).ok())
                .filter(|value| *value <= 100));
        }
        Ok(number_property(&current, w!("CurrentBrightness"))?
            .and_then(|value| u8::try_from(value).ok())
            .filter(|value| *value <= 100))
    }
}

fn same_display(device_id: &str, instance_name: &str) -> bool {
    let normalized_device = device_id.to_ascii_uppercase();
    let mut parts = normalized_device.split('#');
    if parts.next() != Some("\\\\?\\DISPLAY") {
        return false;
    }
    let (Some(model), Some(instance), Some(interface)) = (parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if model.is_empty() || instance.is_empty() || interface.is_empty() || parts.next().is_some() {
        return false;
    }
    let normalized_device = format!("DISPLAY\\{model}\\{instance}");
    let normalized_wmi = instance_name.to_ascii_uppercase();
    normalized_wmi
        .rsplit_once('_')
        .is_some_and(|(name, suffix)| {
            !suffix.is_empty()
                && suffix.bytes().all(|byte| byte.is_ascii_digit())
                && name == normalized_device
        })
}

unsafe fn one_active(services: &IWbemServices, class: &str) -> Result<Option<IWbemClassObject>> {
    unsafe {
        let query = BSTR::from(format!("SELECT * FROM {class} WHERE Active = TRUE"));
        let flags =
            WBEM_GENERIC_FLAG_TYPE(WBEM_FLAG_FORWARD_ONLY.0 | WBEM_FLAG_RETURN_IMMEDIATELY.0);
        let items = services.ExecQuery(&BSTR::from("WQL"), &query, flags, None)?;
        let mut objects = [None, None];
        let mut count = 0;
        let status = items.Next(5000, &mut objects, &mut count);
        status.ok()?;
        // S_FALSE confirms enumeration ended; a timeout cannot prove uniqueness.
        Ok(if status.0 == 1 && count == 1 {
            objects[0].take()
        } else {
            None
        })
    }
}

unsafe fn number_property(object: &IWbemClassObject, name: PCWSTR) -> Result<Option<u32>> {
    unsafe {
        let mut value = VARIANT::default();
        object.Get(name, 0, &mut value, None, None)?;
        let data = &value.Anonymous.Anonymous;
        let result = if data.vt == VT_UI1 {
            Some(u32::from(data.Anonymous.bVal))
        } else if data.vt == VT_UI4 {
            Some(data.Anonymous.ulVal)
        } else if data.vt == VT_I4 {
            u32::try_from(data.Anonymous.lVal).ok()
        } else {
            None
        };
        VariantClear(&mut value)?;
        Ok(result)
    }
}

unsafe fn string_property(object: &IWbemClassObject, name: PCWSTR) -> Result<Option<BSTR>> {
    unsafe {
        let mut value = VARIANT::default();
        object.Get(name, 0, &mut value, None, None)?;
        let data = &value.Anonymous.Anonymous;
        let result =
            (data.vt == VT_BSTR).then(|| BSTR::from(data.Anonymous.bstrVal.to_string().as_str()));
        VariantClear(&mut value)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::same_display;

    #[test]
    fn requires_matching_instance_not_just_monitor_model() {
        assert!(same_display(
            "\\\\?\\DISPLAY#ABC#instance#{guid}",
            "DISPLAY\\ABC\\instance_0"
        ));
        assert!(!same_display(
            "\\\\?\\DISPLAY#ABC#first#{guid}",
            "DISPLAY\\ABC\\second_0"
        ));
        assert!(!same_display(
            "\\\\?\\DISPLAY#ABC#first#{guid}",
            "DISPLAY\\ABC\\first_"
        ));
        assert!(!same_display("", "_0"));
    }
}
