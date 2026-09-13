import { getVersion } from "@tauri-apps/api/app";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { isVersionNewer } from "./versionCompare";

/**
 * 在线更新总开关（启动静默检查与设置页手动检查共用）。
 *
 * 更新链路：GitHub Actions 签名构建后自动发布到 CNB 公开更新仓库
 * （MR9vcc/wisp-updater，见 scripts/publish-cnb.mjs），客户端零凭证，
 * 公钥为本仓库自有密钥对。
 * 注意：若更新源尚未发布对应版本（update.json 不存在），check() 会抛错——
 * 启动静默检查直接忽略，设置页手动检查会 toast 报错，属预期。
 */
export const UPDATE_CHECK_ENABLED = true;

export interface AppUpdateInfo {
	/** plugin-updater 返回的更新对象，携带下载安装能力 */
	update: Update;
	/** 新版本号 */
	version: string;
	/** 更新说明（update.json 的 notes，可能为空字符串） */
	notes: string;
	/** 当前应用版本号 */
	currentVersion: string;
}

export interface DownloadProgress {
	downloaded: number;
	contentLength: number;
}

// "忽略本次更新"的会话级标记：只活在本次运行内，重启后自动失效
let dismissedForThisSession = false;

export function dismissUpdateForThisSession(): void {
	dismissedForThisSession = true;
}

export function isUpdateDismissedForThisSession(): boolean {
	return dismissedForThisSession;
}

/**
 * 检查更新。任何失败（网络异常、更新源缺失等）都返回 null 且只打 warn 日志，
 * 是否提示由调用方决定——启动静默检查应当完全不打扰用户。
 */
export async function checkForAppUpdate(): Promise<AppUpdateInfo | null> {
	if (!UPDATE_CHECK_ENABLED) return null;
	try {
		const [update, currentVersion] = await Promise.all([check(), getVersion()]);
		if (!update) return null;
		// 双保险：插件内部会比较版本，这里再防一次 update.json 配置了不高于
		// 当前版本的 version（发布脚本回滚、手动改坏）导致误弹窗
		if (!isVersionNewer(update.version, currentVersion)) {
			console.warn(
				`[appUpdate] update.json 版本（v${update.version}）不高于当前版本（v${currentVersion}），忽略本次更新`,
			);
			return null;
		}
		return {
			update,
			version: update.version,
			notes: (update.body ?? "").trim(),
			currentVersion,
		};
	} catch (error) {
		console.warn("[appUpdate] 检查更新失败（静默跳过）:", error);
		return null;
	}
}

/**
 * 下载并安装更新（启动弹窗与设置页卡片共用）。
 * 进度通过 onProgress 上报；contentLength 为 0 表示服务端未返回大小，
 * 此时调用方应隐藏百分比与进度条、只显示转圈。
 */
export async function downloadAndInstallUpdate(
	update: Update,
	onProgress?: (progress: DownloadProgress) => void,
): Promise<void> {
	let downloaded = 0;
	let contentLength = 0;
	await update.downloadAndInstall((event) => {
		switch (event.event) {
			case "Started":
				contentLength = event.data.contentLength || 0;
				onProgress?.({ downloaded, contentLength });
				break;
			case "Progress":
				downloaded += event.data.chunkLength || 0;
				onProgress?.({ downloaded, contentLength });
				break;
			case "Finished":
				break;
		}
	});
}
