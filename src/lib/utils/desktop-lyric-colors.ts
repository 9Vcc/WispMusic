/**
 * 桌面歌词配色工具。
 *
 * 桌面歌词浮在任意壁纸上，可读性本来就靠「填充色 + 深色描边背衬」两层
 * 保证；这里把这两层的颜色与窗口背景透明度抽成一份可自定义、可持久化、
 * 可一键恢复默认的配色方案。
 *
 * 关键约定：
 * - **默认值与内置外观逐像素一致**（白字 + 深色描边 + 全透明底），旧用户
 *   不动设置就看不到任何变化；
 * - 各状态的**相对**透明度（当前行 / 下一句预览 / 描边 / 阴影）是固定
 *   层级常量，改配色只换色相不改层级——用户随便挑什么颜色都不会丢掉
 *   「当前句比下一句亮」这一信息；
 * - 颜色一律规范化成 `#rrggbb`，落库前就非法值回落默认（手改配置 / 旧版
 *   残留不会让窗口变透明或全黑）。
 *
 * 变更配色只需改根节点的 CSS 变量（见 buildDesktopLyricColorVars）：变量
 * 沿继承链传播，不动任何动画组件的 props 与状态，因此换色**不会重挂载
 * 歌词行、不会打断逐字填充与进度插值**。
 */

/** 桌面歌词配色方案（三项颜色 + 背景不透明度） */
export interface DesktopLyricColors {
	/** 普通歌词文字颜色：下一句预览与未高亮的行 */
	textColor: string;
	/** 高亮 / 当前行颜色：当前演唱句正文与已唱逐字填充 */
	highlightColor: string;
	/** 描边与阴影颜色：浮在壁纸上的可读性背衬层 */
	strokeColor: string;
	/** 窗口背景不透明度（百分比，0 = 全透明，仅悬浮时显示深色背板） */
	backgroundOpacity: number;
}

/**
 * 默认配色：与内置外观一致。
 * 描边取 `#080a0e`（原 `rgb(8 10 14 / 0.52)` 的基色），不是纯黑。
 */
export const DESKTOP_LYRIC_DEFAULT_COLORS: DesktopLyricColors = {
	textColor: "#ffffff",
	highlightColor: "#ffffff",
	strokeColor: "#080a0e",
	backgroundOpacity: 0,
};

/** 背景不透明度可调范围（百分比） */
export const DESKTOP_LYRIC_BACKGROUND_OPACITY_LIMITS = {
	min: 0,
	max: 100,
} as const;

/** 悬浮时的背景不透明度下限：工具条胶囊要有足够背衬才可读（原 hover:bg-black/60） */
export const DESKTOP_LYRIC_HOVER_BACKGROUND_OPACITY = 60;

/**
 * 各状态的相对透明度——取值与改造前的硬编码一一对应，是外观基线的一部分，
 * 不要为了「更亮/更透」随手调：它们共同承担「当前行 > 预览行 > 描边阴影」
 * 的层级表达。例外：笔画字格两项（strokeGlyph/strokeGlyphSoft）按需求定为
 * 纯色不透明——笔画合并为整字后不保留任何透明成分（歌词页同步）。
 */
export const DESKTOP_LYRIC_ALPHA = {
	/** 当前行正文（无逐字数据时整行用它） */
	current: 0.88,
	/** 下一句预览 */
	preview: 0.45,
	/** 逐字填充基色（逐字遮罩再乘 0.32~0.85，故基色取 1） */
	wordFill: 1,
	/** 描边（-webkit-text-stroke） */
	stroke: 0.52,
	/** 背衬层大半径阴影 */
	shadowSoft: 0.45,
	/** 背衬层小半径描边阴影 */
	shadowStrong: 0.55,
	/** 填充层自身投影 */
	halo: 0.32,
	/** 笔画字格填充（合并为整字后纯色不透明，不保留透明度） */
	strokeGlyph: 1,
	/** 笔画无数据兜底字（宋体/楷体静态字，与整字同为纯色不透明） */
	strokeGlyphSoft: 1,
	/** 笔画字格外投影 */
	strokeDrop: 0.5,
} as const;

/** 一套命名配色方案：色板一键应用，之后仍可单独微调 */
export interface DesktopLyricColorScheme {
	name: string;
	colors: DesktopLyricColors;
}

/**
 * 内置配色方案：白字基线之上按「深色描边 + 高饱和高亮」展开，
 * 每种都以能在亮/暗壁纸上保持可读为准（描边色一律取同色系深色）。
 */
export const DESKTOP_LYRIC_COLOR_SCHEMES: readonly DesktopLyricColorScheme[] = [
	{ name: "经典白", colors: { ...DESKTOP_LYRIC_DEFAULT_COLORS } },
	{
		name: "暖金",
		colors: {
			textColor: "#f3dcae",
			highlightColor: "#ffb733",
			strokeColor: "#2a1a04",
			backgroundOpacity: 0,
		},
	},
	{
		name: "薄荷",
		colors: {
			textColor: "#cdf5e4",
			highlightColor: "#34e0a1",
			strokeColor: "#062a20",
			backgroundOpacity: 0,
		},
	},
	{
		name: "樱花",
		colors: {
			textColor: "#ffd9e6",
			highlightColor: "#ff7bac",
			strokeColor: "#33101f",
			backgroundOpacity: 0,
		},
	},
	{
		name: "霓虹",
		colors: {
			textColor: "#cfe6ff",
			highlightColor: "#54c8ff",
			strokeColor: "#04182b",
			backgroundOpacity: 0,
		},
	},
	{
		name: "暗夜黑底",
		colors: {
			textColor: "#e8ecf5",
			highlightColor: "#ffffff",
			strokeColor: "#000000",
			backgroundOpacity: 42,
		},
	},
];

const HEX_COLOR_REGEX = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i;

/**
 * 规范化颜色输入：接受 `#rgb` / `#rrggbb`（可省略 #、大小写不限），
 * 非法输入回落 `fallback`（与主题色取色同款宽松策略）。
 */
export function normalizeLyricColor(input: unknown, fallback: string): string {
	if (typeof input !== "string") return fallback;

	const matched = HEX_COLOR_REGEX.exec(input.trim());
	if (!matched) return fallback;

	let value = matched[1].toLowerCase();
	if (value.length === 3) {
		value = value
			.split("")
			.map((char) => char + char)
			.join("");
	}
	return `#${value}`;
}

/** 尝试规范化：非法返回 null（供输入框校验；与带 fallback 的版本同源） */
export function tryNormalizeLyricColor(input: unknown): string | null {
	const normalized = normalizeLyricColor(input, "");
	return normalized ? normalized : null;
}

/** 规范化背景不透明度：按 1% 取整并钳在 0~100，非法值回落 `fallback` */
export function clampBackgroundOpacity(
	input: unknown,
	fallback: number,
): number {
	if (typeof input !== "number" || !Number.isFinite(input)) return fallback;
	return Math.min(
		DESKTOP_LYRIC_BACKGROUND_OPACITY_LIMITS.max,
		Math.max(DESKTOP_LYRIC_BACKGROUND_OPACITY_LIMITS.min, Math.round(input)),
	);
}

/**
 * 规范化整份配色（水合与动作入口共用）：逐字段校验，任何脏数据只影响
 * 它自己那一项，其余仍按用户设置生效。
 */
export function normalizeDesktopLyricColors(
	saved?: Partial<DesktopLyricColors> | null,
): DesktopLyricColors {
	return {
		textColor: normalizeLyricColor(
			saved?.textColor,
			DESKTOP_LYRIC_DEFAULT_COLORS.textColor,
		),
		highlightColor: normalizeLyricColor(
			saved?.highlightColor,
			DESKTOP_LYRIC_DEFAULT_COLORS.highlightColor,
		),
		strokeColor: normalizeLyricColor(
			saved?.strokeColor,
			DESKTOP_LYRIC_DEFAULT_COLORS.strokeColor,
		),
		backgroundOpacity: clampBackgroundOpacity(
			saved?.backgroundOpacity,
			DESKTOP_LYRIC_DEFAULT_COLORS.backgroundOpacity,
		),
	};
}

/** `#rrggbb` → RGB 通道（供拼装 `rgb(r g b / a)`） */
export function hexToRgbChannels(hex: string): [number, number, number] {
	const normalized = normalizeLyricColor(
		hex,
		DESKTOP_LYRIC_DEFAULT_COLORS.highlightColor,
	);
	return [
		Number.parseInt(normalized.slice(1, 3), 16),
		Number.parseInt(normalized.slice(3, 5), 16),
		Number.parseInt(normalized.slice(5, 7), 16),
	];
}

/** hex + 相对透明度 → CSS 颜色；alpha 钳在 0~1，保留三位小数避免长尾 */
export function withAlpha(hex: string, alpha: number): string {
	const [red, green, blue] = hexToRgbChannels(hex);
	const clamped = Math.min(1, Math.max(0, alpha));
	return `rgb(${red} ${green} ${blue} / ${Number(clamped.toFixed(3))})`;
}

/** 是否仍为出厂配色（三项颜色默认 + 背景全透明），用于禁用「恢复默认」 */
export function isDefaultDesktopLyricColors(
	colors: DesktopLyricColors,
): boolean {
	return isSameDesktopLyricColors(colors, DESKTOP_LYRIC_DEFAULT_COLORS);
}

/** 两份配色是否等价（规范化后逐字段比较，供预设选中态判定） */
export function isSameDesktopLyricColors(
	left: DesktopLyricColors,
	right: DesktopLyricColors,
): boolean {
	const a = normalizeDesktopLyricColors(left);
	const b = normalizeDesktopLyricColors(right);
	return (
		a.textColor === b.textColor &&
		a.highlightColor === b.highlightColor &&
		a.strokeColor === b.strokeColor &&
		a.backgroundOpacity === b.backgroundOpacity
	);
}

/**
 * 生成桌面歌词窗口的配色 CSS 变量表（挂根节点，整窗继承）。
 *
 * 变量名按消费方划分，避免「同一个变量被两处以不同 alpha 使用」而互相
 * 干扰：
 * - `--dl-text-color`        下一句预览 / 非当前行
 * - `--dl-current-color`     当前行正文（无逐字数据时整行）
 * - `--dl-word-fill-color`   逐字填充基色（默认动效）
 * - `--dl-stroke-color`      描边背衬层主色
 * - `--dl-shadow-soft/strong/halo` 各层阴影
 * - `--dl-glyph-fill-color` / `--dl-glyph-fill-soft` 笔画字格填充（笔画视图
 *   会按「是否当前句」在行上覆盖这两个变量）
 * - `--dl-bg-alpha` / `--dl-bg-alpha-hover` 背景不透明度与悬浮兜底
 */
export function buildDesktopLyricColorVars(
	colors: DesktopLyricColors,
): Record<string, string> {
	const normalized = normalizeDesktopLyricColors(colors);
	const opacity = normalized.backgroundOpacity / 100;
	const hoverOpacity = Math.max(
		opacity,
		DESKTOP_LYRIC_HOVER_BACKGROUND_OPACITY / 100,
	);

	return {
		"--dl-text-color": withAlpha(
			normalized.textColor,
			DESKTOP_LYRIC_ALPHA.preview,
		),
		"--dl-current-color": withAlpha(
			normalized.highlightColor,
			DESKTOP_LYRIC_ALPHA.current,
		),
		"--dl-word-fill-color": withAlpha(
			normalized.highlightColor,
			DESKTOP_LYRIC_ALPHA.wordFill,
		),
		"--dl-stroke-color": withAlpha(
			normalized.strokeColor,
			DESKTOP_LYRIC_ALPHA.stroke,
		),
		"--dl-shadow-soft": withAlpha(
			normalized.strokeColor,
			DESKTOP_LYRIC_ALPHA.shadowSoft,
		),
		"--dl-shadow-strong": withAlpha(
			normalized.strokeColor,
			DESKTOP_LYRIC_ALPHA.shadowStrong,
		),
		"--dl-halo-shadow": withAlpha(
			normalized.strokeColor,
			DESKTOP_LYRIC_ALPHA.halo,
		),
		"--dl-stroke-drop-shadow": withAlpha(
			normalized.strokeColor,
			DESKTOP_LYRIC_ALPHA.strokeDrop,
		),
		"--dl-glyph-fill-color": withAlpha(
			normalized.highlightColor,
			DESKTOP_LYRIC_ALPHA.strokeGlyph,
		),
		"--dl-glyph-fill-soft": withAlpha(
			normalized.highlightColor,
			DESKTOP_LYRIC_ALPHA.strokeGlyphSoft,
		),
		"--dl-bg-alpha": String(opacity),
		"--dl-bg-alpha-hover": String(hoverOpacity),
	};
}

/** 笔画视图按「是否当前句」覆盖字格填充色：当前句高亮，其余用普通文字色 */
export function buildStrokeRowColorVars(
	colors: DesktopLyricColors,
	isCurrent: boolean,
): Record<string, string> {
	const color = isCurrent ? colors.highlightColor : colors.textColor;
	return {
		"--dl-glyph-fill-color": withAlpha(color, DESKTOP_LYRIC_ALPHA.strokeGlyph),
		"--dl-glyph-fill-soft": withAlpha(
			color,
			DESKTOP_LYRIC_ALPHA.strokeGlyphSoft,
		),
	};
}
