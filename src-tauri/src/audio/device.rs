use std::collections::HashMap;

use rodio::cpal::{
    self,
    traits::{DeviceTrait, HostTrait},
};
use rodio::{OutputStream, OutputStreamHandle};
use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceInfo {
    id: String,
    name: String,
    is_default: bool,
}

pub(super) fn normalize_device_id(device_id: Option<String>) -> Option<String> {
    device_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty() && id != "default")
}

fn selected_device_index(device_id: Option<&str>) -> Result<Option<usize>, String> {
    let Some(device_id) = device_id else {
        return Ok(None);
    };
    let Some(index) = device_id.strip_prefix("device:") else {
        return Err(format!("unknown audio output device id: {device_id}"));
    };

    index
        .parse::<usize>()
        .map(Some)
        .map_err(|_| format!("invalid audio output device id: {device_id}"))
}

pub(super) fn open_output(
    device_id: Option<&str>,
) -> Result<(OutputStream, OutputStreamHandle), String> {
    let Some(index) = selected_device_index(device_id)? else {
        return OutputStream::try_default()
            .map_err(|err| format!("open default audio output failed: {err}"));
    };

    let host = cpal::default_host();
    let mut devices = host
        .output_devices()
        .map_err(|err| format!("list audio output devices failed: {err}"))?;
    let device = devices
        .nth(index)
        .ok_or_else(|| format!("audio output device not found: device:{index}"))?;
    let device_name = device.name().unwrap_or_else(|_| format!("device:{index}"));

    OutputStream::try_from_device(&device)
        .map_err(|err| format!("open audio output {device_name} failed: {err}"))
}

/// Windows/WASAPI 设备名通常带适配器后缀，如 “主输出(2-ROUTIST R2)”。
/// 去掉末尾的括号段，只保留面向用户的友好名（“主输出”）。
fn simplify_device_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if let (Some(start), true) = (trimmed.rfind('('), trimmed.ends_with(')')) {
        let inner = &trimmed[start + 1..trimmed.len() - 1];
        if !inner.is_empty() && !inner.contains('(') && !inner.contains(')') {
            let simplified = trimmed[..start].trim();
            if !simplified.is_empty() {
                return simplified.to_string();
            }
        }
    }
    trimmed.to_string()
}

pub(super) fn list_output_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    let host = cpal::default_host();
    let default_device_name = host
        .default_output_device()
        .and_then(|device| device.name().ok());

    let mut raw_devices: Vec<(String, String)> = Vec::new();
    for (index, device) in host
        .output_devices()
        .map_err(|err| format!("list audio output devices failed: {err}"))?
        .enumerate()
    {
        let name = device
            .name()
            .unwrap_or_else(|_| format!("Output device {}", index + 1));
        raw_devices.push((format!("device:{index}"), name));
    }

    // 简化后同名冲突（如同一适配器的多个端口都叫“主输出”）时保留原始名称以便区分
    let mut simplified_counts: HashMap<String, usize> = HashMap::new();
    for (_, name) in &raw_devices {
        *simplified_counts
            .entry(simplify_device_name(name))
            .or_default() += 1;
    }

    let mut devices = vec![AudioDeviceInfo {
        id: "default".to_string(),
        name: "系统默认".to_string(),
        is_default: true,
    }];

    for (id, name) in raw_devices {
        let simplified = simplify_device_name(&name);
        let display = if simplified_counts[&simplified] > 1 {
            name.clone()
        } else {
            simplified
        };
        let is_default = default_device_name.as_deref() == Some(name.as_str());

        devices.push(AudioDeviceInfo {
            id,
            name: display,
            is_default,
        });
    }

    Ok(devices)
}
