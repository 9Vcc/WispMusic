import { invoke, isTauri } from "@tauri-apps/api/core";

/** 缓存目录信息快照（与 Rust `cache::AudioCacheDirInfo` 字段一一对应） */
export interface AudioCacheDirInfo {
	/** 当前生效的缓存根目录 */
	path: string;
	/** 系统默认缓存根目录 */
	defaultPath: string;
	/** 是否已切换到用户自定义目录 */
	isCustom: boolean;
	/** 当前缓存占用总字节数 */
	totalSize: number;
}

/** 读取当前缓存目录信息；非 Tauri 环境返回 null。 */
export async function getAudioCacheDir(): Promise<AudioCacheDirInfo | null> {
	if (!isTauri()) return null;
	return invoke<AudioCacheDirInfo>("get_audio_cache_dir");
}

/** 切换缓存目录：原生侧会校验可写并尽力迁移已有缓存。 */
export async function setAudioCacheDir(
	path: string,
): Promise<AudioCacheDirInfo> {
	return invoke<AudioCacheDirInfo>("set_audio_cache_dir", { path });
}

/** 恢复系统默认缓存目录。 */
export async function resetAudioCacheDir(): Promise<AudioCacheDirInfo> {
	return invoke<AudioCacheDirInfo>("reset_audio_cache_dir");
}

/** 一次性清理全部缓存，返回释放的字节数。 */
export async function clearAllAudioCache(): Promise<number> {
	return invoke<number>("clear_all_audio_cache");
}
