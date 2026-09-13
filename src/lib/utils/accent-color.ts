/**
 * 主题色（强调色）工具。
 *
 * 约束来源：`--primary` 在本项目里既是"背景"（按钮、进度条、选中态）又是
 * "文字"（侧栏激活图标、歌名高亮、hover 提示），所以 index.css 给深浅两套主题
 * 分别定了明度——浅色 `oklch(0.55 0.22 26.5)` 偏暗，深色 `oklch(0.72 0.18 26.5)`
 * 偏亮。用户自定义时若把所选颜色原样写进两套主题，浅色下选浅色会糊在白底上、
 * 深色下选深色会看不见。因此这里只取所选颜色的**色相与彩度**，明度一律套用
 * 既有的两档，`--primary-foreground` 也就无需另算（白 / 近黑在两档下都成立）。
 *
 * 彩度不做钳制：超出 sRGB 色域时交给浏览器按 CSS Color 4 做色域映射
 * （保持色相、降低彩度），比按固定上限硬截更接近用户预期。
 *
 * 换算公式为 Björn Ottosson 的 sRGB → OKLab → OKLCH，已用公开参考值交叉验证
 * （`#ff0000` → 0.6279/0.2577/29.23、`#3b82f6` → 0.6231/0.1880/259.81）。
 */

/** 浅色 / 深色主题下 `--primary` 的明度档位，与 index.css 内置值一致 */
export const ACCENT_LIGHTNESS = {
	light: 0.55,
	dark: 0.72,
} as const;

export type AccentTheme = keyof typeof ACCENT_LIGHTNESS;

/**
 * 内置主题色的 sRGB 近似值：`oklch(0.55 0.22 26.5)` 经浏览器 canvas 取色得到。
 * 只用于取色器初值与"当前是内置配色"时的色块展示，不参与 `--primary` 的计算。
 */
export const BUILT_IN_ACCENT_HEX = "#d40b1d";

/** 预设色板的一项：名称仅用于 title / aria-label */
export interface AccentColorPreset {
	name: string;
	value: string;
}

/** 预设色板：按色相环铺开，深浅主题的明度差异由 ACCENT_LIGHTNESS 统一校正 */
export const ACCENT_COLOR_PRESETS: readonly AccentColorPreset[] = [
	{ name: "默认红", value: "#d40b1d" },
	{ name: "橙", value: "#f97316" },
	{ name: "琥珀", value: "#f59e0b" },
	{ name: "草绿", value: "#84cc16" },
	{ name: "绿", value: "#22c55e" },
	{ name: "青", value: "#14b8a6" },
	{ name: "天蓝", value: "#0ea5e9" },
	{ name: "蓝", value: "#3b82f6" },
	{ name: "靛", value: "#6366f1" },
	{ name: "紫", value: "#a855f7" },
	{ name: "品红", value: "#ec4899" },
	{ name: "灰", value: "#64748b" },
];

const HEX_COLOR_REGEX = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i;

/** 规范化用户输入：接受 `#rgb` / `#rrggbb`（可省略 #、大小写不限），非法返回 null */
export function normalizeAccentColor(
	input: string | null | undefined,
): string | null {
	if (!input) return null;

	const matched = HEX_COLOR_REGEX.exec(input.trim());
	if (!matched) return null;

	let value = matched[1].toLowerCase();
	if (value.length === 3) {
		value = value
			.split("")
			.map((char) => char + char)
			.join("");
	}

	return `#${value}`;
}

function srgbChannelToLinear(channel: number): number {
	return channel <= 0.04045
		? channel / 12.92
		: ((channel + 0.055) / 1.055) ** 2.4;
}

/** hex → OKLCH；非法输入返回 null。灰色的色相无意义（彩度为 0），返回 0 */
export function hexToOklch(
	hex: string,
): { l: number; c: number; h: number } | null {
	const normalized = normalizeAccentColor(hex);
	if (!normalized) return null;

	const r = srgbChannelToLinear(parseInt(normalized.slice(1, 3), 16) / 255);
	const g = srgbChannelToLinear(parseInt(normalized.slice(3, 5), 16) / 255);
	const b = srgbChannelToLinear(parseInt(normalized.slice(5, 7), 16) / 255);

	const l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
	const m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
	const s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;

	const lRoot = Math.cbrt(l);
	const mRoot = Math.cbrt(m);
	const sRoot = Math.cbrt(s);

	const lightness =
		0.2104542553 * lRoot + 0.793617785 * mRoot - 0.0040720468 * sRoot;
	const axisA =
		1.9779984951 * lRoot - 2.428592205 * mRoot + 0.4505937099 * sRoot;
	const axisB =
		0.0259040371 * lRoot + 0.7827717662 * mRoot - 0.808675766 * sRoot;

	const chroma = Math.sqrt(axisA * axisA + axisB * axisB);
	const hue =
		chroma < 1e-6
			? 0
			: ((Math.atan2(axisB, axisA) * 180) / Math.PI + 360) % 360;

	return { l: lightness, c: chroma, h: hue };
}

/**
 * 生成要写进 `--primary` 的取值：保留色相与彩度，明度按主题取档。
 * 传入 null 或非法颜色时返回 null，调用方据此撤销覆写、回落到 index.css 的内置值。
 */
export function buildAccentPrimary(
	color: string | null | undefined,
	theme: AccentTheme,
): string | null {
	const oklch = hexToOklch(color ?? "");
	if (!oklch) return null;

	const lightness = ACCENT_LIGHTNESS[theme];
	const chroma = Number(oklch.c.toFixed(4));
	const hue = Number(oklch.h.toFixed(2));

	return `oklch(${lightness} ${chroma} ${hue})`;
}
