/**
 * 内存归因用到的纯换算与格式化。
 *
 * 与 memory-stats.ts 分开：那个文件要在运行时读 store 与 Tauri 命令（依赖窗口
 * 环境），这里的换算与格式化没有任何副作用依赖，可以被 node 环境的单测直接覆盖。
 */

export function formatBytes(bytes: number): string {
	if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
	const units = ["B", "KB", "MB", "GB"];
	let value = bytes;
	let unit = 0;
	while (value >= 1024 && unit < units.length - 1) {
		value /= 1024;
		unit += 1;
	}
	const digits = value >= 100 || unit === 0 ? 0 : 1;
	return `${value.toFixed(digits)} ${units[unit]}`;
}

/** 解码后的位图字节数：宽 × 高 × 4（RGBA） */
export function estimateBitmapBytes(width: number, height: number): number {
	if (!Number.isFinite(width) || !Number.isFinite(height)) return 0;
	return Math.max(0, Math.round(width)) * Math.max(0, Math.round(height)) * 4;
}

/**
 * 绘制缓冲字节区间。默认帧缓冲至少有一份 color 附件（4 字节/像素）；
 * 开 MSAA 时 color 要按样本数翻倍（本项目取 4×）。深度/模板附件另算，
 * 所以给区间而非单值——够用来判断量级。
 */
export function estimateFramebufferBytes(totalPixels: number): {
	min: number;
	max: number;
} {
	const pixels = Number.isFinite(totalPixels) ? Math.max(0, totalPixels) : 0;
	return { min: pixels * 4, max: pixels * 4 * 4 };
}

export function byteLengthUtf8(text: string): number {
	return new TextEncoder().encode(text).length;
}

/**
 * 序列化后的字节数。结构里可能有循环引用（store 中互相引用的对象很常见），
 * 直接 JSON.stringify 会抛错——归因面板绝不能因此整个崩掉，故吞掉错误返回 0。
 */
export function measureJsonBytes(value: unknown): number {
	if (value === null || value === undefined) return 0;
	try {
		return byteLengthUtf8(JSON.stringify(value) ?? "");
	} catch {
		return 0;
	}
}
