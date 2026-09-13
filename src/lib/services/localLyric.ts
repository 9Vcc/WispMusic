import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

/**
 * 本地歌词文件访问。
 * 读文件与同目录查找都放在 Rust 侧（`src-tauri/src/local_lyric.rs`）：
 * Rust 侧能按 BOM / UTF-8 / GB18030 依次探测编码，避免 GBK 歌词整篇乱码。
 */

/** 打开系统文件选择器，返回选中文件的绝对路径；用户取消时返回 null */
export async function pickLyricFile(): Promise<string | null> {
	const selected = await open({
		multiple: false,
		directory: false,
		title: "选择歌词文件",
		filters: [
			{ name: "歌词文件", extensions: ["lrc", "yrc", "txt"] },
			{ name: "所有文件", extensions: ["*"] },
		],
	});

	return typeof selected === "string" ? selected : null;
}

/** 读取文本文件（自动处理 BOM 与 GBK/GB18030 编码） */
export async function readLyricFile(filePath: string): Promise<string> {
	return invoke<string>("read_text_file", { path: filePath });
}

/** 查找与音频文件同目录、同名的歌词文件；找不到返回 null */
export async function findSiblingLyricFile(
	audioFilePath: string,
): Promise<string | null> {
	return invoke<string | null>("find_sibling_lyric", {
		audioPath: audioFilePath,
	});
}

/** 从完整路径中取文件名，用于界面展示 */
export function getBaseName(filePath: string): string {
	const normalized = filePath.replace(/\\/g, "/");
	const index = normalized.lastIndexOf("/");
	return index >= 0 ? normalized.slice(index + 1) : filePath;
}
