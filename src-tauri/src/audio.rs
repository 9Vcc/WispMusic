mod command;
mod controller;
mod device;
mod dsp;
mod engine;
mod event;
mod mp4_decoder;
mod pipeline;
mod snapshot;
mod source;
mod state;
mod streaming;
mod tempo;
mod types;

use tauri::{AppHandle, Manager, State};

pub use controller::AudioState;
use types::{AudioSnapshot, PreloadResult};

/// 同步命令的 send_command 会阻塞等音频线程回包（seek 期间还包含 demuxer
/// 定位耗时），统一放到 spawn_blocking 执行，避免卡住 Tauri 的调用线程。
async fn run_audio_command<F>(app: AppHandle, command: F) -> Result<AudioSnapshot, String>
where
    F: FnOnce(AppHandle, &AudioState) -> Result<AudioSnapshot, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AudioState>();
        command(app.clone(), &state)
    })
    .await
    .map_err(|err| format!("audio command task failed: {err}"))?
}

#[tauri::command]
pub async fn player_load(
    app: AppHandle,
    state: State<'_, AudioState>,
    source: String,
    autoplay: bool,
    request_id: u64,
) -> Result<AudioSnapshot, String> {
    controller::player_load(app, &state, source, autoplay, request_id).await
}

#[tauri::command]
pub async fn player_preload(
    app: AppHandle,
    state: State<'_, AudioState>,
    source: String,
    request_id: u64,
) -> Result<PreloadResult, String> {
    controller::player_preload(app, &state, source, request_id).await
}

#[tauri::command]
pub fn get_native_audio_cache_size(app: AppHandle) -> Result<u64, String> {
    source::native_audio_cache_size(&app)
}

#[tauri::command]
pub fn clear_native_audio_cache(app: AppHandle) -> Result<(), String> {
    source::clear_native_audio_cache(&app)
}

#[tauri::command]
pub fn enforce_native_audio_cache_limit(app: AppHandle, max_bytes: u64) -> Result<(), String> {
    source::enforce_native_audio_cache_limit(&app, max_bytes)
}

#[tauri::command]
pub fn list_audio_output_devices() -> Result<Vec<device::AudioDeviceInfo>, String> {
    device::list_output_devices()
}

#[tauri::command]
pub async fn player_set_output_device(
    app: AppHandle,
    device_id: Option<String>,
) -> Result<AudioSnapshot, String> {
    run_audio_command(app, move |app, state| {
        controller::player_set_output_device(app, state, device_id)
    })
    .await
}

#[tauri::command]
pub async fn player_set_replay_gain(
    app: AppHandle,
    enabled: bool,
    preamp_db: f32,
) -> Result<AudioSnapshot, String> {
    run_audio_command(app, move |app, state| {
        controller::player_set_replay_gain(app, state, enabled, preamp_db)
    })
    .await
}

#[tauri::command]
pub async fn player_set_equalizer(
    app: AppHandle,
    enabled: bool,
    gains_db: Vec<f32>,
) -> Result<AudioSnapshot, String> {
    run_audio_command(app, move |app, state| {
        controller::player_set_equalizer(app, state, enabled, gains_db)
    })
    .await
}

#[tauri::command]
pub async fn player_set_crossfade(
    app: AppHandle,
    duration_secs: f64,
) -> Result<AudioSnapshot, String> {
    run_audio_command(app, move |app, state| {
        controller::player_set_crossfade(app, state, duration_secs)
    })
    .await
}

#[tauri::command]
pub async fn player_play(app: AppHandle) -> Result<AudioSnapshot, String> {
    run_audio_command(app, controller::player_play).await
}

#[tauri::command]
pub async fn player_pause(app: AppHandle) -> Result<AudioSnapshot, String> {
    run_audio_command(app, controller::player_pause).await
}

#[tauri::command]
pub async fn player_seek(app: AppHandle, position_secs: f64) -> Result<AudioSnapshot, String> {
    run_audio_command(app, move |app, state| {
        controller::player_seek(app, state, position_secs)
    })
    .await
}

#[tauri::command]
pub async fn player_set_volume(app: AppHandle, volume: f32) -> Result<AudioSnapshot, String> {
    run_audio_command(app, move |app, state| {
        controller::player_set_volume(app, state, volume)
    })
    .await
}

#[tauri::command]
pub async fn player_set_speed(app: AppHandle, speed: f32) -> Result<AudioSnapshot, String> {
    run_audio_command(app, move |app, state| {
        controller::player_set_speed(app, state, speed)
    })
    .await
}

#[tauri::command]
pub async fn player_stop(app: AppHandle) -> Result<AudioSnapshot, String> {
    run_audio_command(app, controller::player_stop).await
}

#[tauri::command]
pub fn player_get_state(state: State<'_, AudioState>) -> Result<AudioSnapshot, String> {
    controller::player_get_state(&state)
}
