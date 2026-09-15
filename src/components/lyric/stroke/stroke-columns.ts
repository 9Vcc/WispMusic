/**
 * 笔画歌词动效的纯逻辑：把一行歌词摊平成逐字时序，并拆成 1~3 列。
 *
 * 进入动画（逐字按笔画飞入拼合）由 triggerAt 驱动——跟播放进度逐字
 * 出现；急促连唱的行（逐字间隔低于预算）免飞入、整句直接显示。句切换
 * 由整句状态机接管（旧句散开结束才进新句）。歌词偏移只进时钟、不改
 * 歌词时间戳的约定不变。
 */
import type { ILyricLine } from "@/lib/utils/lyric-parser";

/** 句尾散开动画时长（ms）：整句笔画四散飞开并渐隐，结束后切下一句 */
export const STROKE_LINE_SCATTER_MS = 320;

/** 双列单元格尺寸（px）上下限（上限收敛到 46，字号更细腻柔和；下限兜底长句可读） */
export const STROKE_CELL_MIN_PX = 26;
export const STROKE_CELL_MAX_PX = 46;

/** 两列合计可占舞台高度的比例（留出副行与呼吸空间） */
export const STROKE_COLUMN_HEIGHT_RATIO = 0.68;

/**
 * 双列上下安全区（占舞台高度比例）：靠上列从安全区下缘开始排、靠下列
 * 贴安全区上缘收尾，给翻译/罗马音副行与画面呼吸留出空间。
 */
export const STROKE_STAGE_SAFE_RATIO = 0.1;

/** 逐行歌词（lrc）逐字兜底触发的步长范围（ms） */
const LRC_STAGGER_MIN_MS = 140;
const LRC_STAGGER_MAX_MS = 420;

/** 逐行歌词行时长未知时的默认单字跨度（ms） */
const LRC_DEFAULT_CHAR_SPAN_MS = 320;

export interface StrokeCharTiming {
	/** 单个字符（空白已被滤除） */
	char: string;
	/** 该字的逐字演唱时刻（原始歌词时间 ms），整句直接显示模式下仅作数据保留 */
	triggerAt: number;
	/** 该字的演唱跨度（ms），仅作时序数据保留 */
	spanMs: number;
}

/**
 * 把一行歌词摊平成逐字触发时间：
 * - 逐字歌词（yrc）：词元内逐字摊平，触发点在词元时长内均分——中文一词一字
 *   即逐字卡拉OK；英文/多字词元也会逐字错开而不是整词同闪。
 * - 逐行歌词（lrc）：无逐字时间，从 lineStart 起按行内均分步进。
 * 空白字符不产生触发点（不渲染单元格）。
 */
export function flattenLineChars(line: ILyricLine): StrokeCharTiming[] {
	const timings: StrokeCharTiming[] = [];

	if (line.words && line.words.length > 0) {
		for (const word of line.words) {
			const chars = [...word.char].filter((c) => c.trim());
			if (chars.length === 0) continue;
			const span = Math.max(0, word.duration) / chars.length;
			chars.forEach((char, i) => {
				timings.push({
					char,
					triggerAt: word.startTime + span * i,
					spanMs: span,
				});
			});
		}
		return timings;
	}

	const chars = [...line.lineText].filter((c) => c.trim());
	if (chars.length === 0) return timings;
	const lineDuration =
		line.lineEnd !== undefined
			? Math.max(0, line.lineEnd - line.lineStart)
			: chars.length * LRC_DEFAULT_CHAR_SPAN_MS;
	const stagger = Math.min(
		LRC_STAGGER_MAX_MS,
		Math.max(LRC_STAGGER_MIN_MS, lineDuration / chars.length),
	);
	chars.forEach((char, i) => {
		timings.push({
			char,
			triggerAt: line.lineStart + stagger * i,
			spanMs: stagger,
		});
	});
	return timings;
}

/**
 * 快歌免飞入的逐字间隔预算（ms/字）：低于它视为急促连唱——飞入跟不上
 * 演唱（字还没拼完就唱过去了），整句直接显示。250 ≈ 4 字/秒，中速歌
 * （一字一拍 ≈ 300~700ms/字）都能正常飞入。
 */
export const STROKE_INSTANT_CHAR_BUDGET_MS = 250;

/**
 * 行内逐字演唱间隔（ms/字）：
 * - 逐字歌词：末字与首字触发点之差 ÷（字数−1），是真实的演唱速度；
 * - 逐行歌词：triggerAt 由 stagger 均摊，结果即均摊步长；
 * - 退化保护：单字行取该字 span；triggerAt 全行同刻（数据异常）退回
 *   span 均值；空行返回 Infinity（永不判快）。
 */
export function computeCharIntervalMs(
	timings: readonly StrokeCharTiming[],
): number {
	if (timings.length === 0) return Number.POSITIVE_INFINITY;
	if (timings.length === 1) {
		return timings[0].spanMs > 0 ? timings[0].spanMs : Number.POSITIVE_INFINITY;
	}
	const interval =
		(timings[timings.length - 1].triggerAt - timings[0].triggerAt) /
		(timings.length - 1);
	if (interval > 0) return interval;
	const spans = timings.map((t) => t.spanMs).filter((s) => s > 0);
	if (spans.length === 0) return Number.POSITIVE_INFINITY;
	return spans.reduce((sum, s) => sum + s, 0) / spans.length;
}

/**
 * 该行是否免飞入、整句直接显示：逐字间隔低于预算即急促连唱
 * （>4 字/秒），飞入动画跟不上演唱节奏，直接呈现完成态。
 */
export function shouldUseInstantDisplay(
	timings: readonly StrokeCharTiming[],
): boolean {
	return computeCharIntervalMs(timings) < STROKE_INSTANT_CHAR_BUDGET_MS;
}

/**
 * 飞入时长两段式结构（单字自然窗口恒定，不随笔画数变化）：
 * 提速部分（起飞错峰空间 + 单笔飞行）+ 减速收尾时长。句级预算不足时
 * 整句统一压缩提速部分，收尾时长保底——减速由 easeOutCubic 单段曲线
 * 的长尾连续承担（keyframe 不分段、无拼接速度跳变），整句节奏均匀，
 * 不会出现句末字单独加速的观感割裂。
 */
/** 起飞错峰空间（ms，自然时间线）：单字内各笔起飞时刻的分布范围 */
export const FLY_STAGGER_SPACE_MS = 300;
/** 单笔飞行自然时长（ms，页外起点 → 定妆位的主行程） */
export const FLY_PATH_NATURAL_MS = 940;
/** 减速收尾时长（ms）：并入单笔总时长，保障缓动长尾的减速时间 */
export const FLY_TAIL_MS = 260;
/** 收尾压缩保底（ms）：提速部分任意压缩，收尾时长不低于此——落位自然 */
export const FLY_TAIL_MIN_MS = 180;
/** 时间线压缩下限：极快歌兜底时宁可稍快，也不让飞入变成看不清的闪电 */
export const FLY_SCALE_MIN = 0.35;

/** 单字飞入自然总窗口 = 提速段（错峰 + 飞行）+ 收尾段 */
export const STROKE_FLY_WINDOW_MS =
	FLY_STAGGER_SPACE_MS + FLY_PATH_NATURAL_MS + FLY_TAIL_MS;

/** 句级飞入时间线参数（fitFlyTimeline 的输出，整句所有字共享） */
export interface FlyTimeline {
	/** 整体时间线缩放系数（句级统一；预算充足 = 1） */
	scale: number;
	/** 提速段总量（错峰 + 飞行，已乘 scale） */
	fastMs: number;
	/** 收尾段时长（保底减速尾巴） */
	tailMs: number;
}

/**
 * 句级飞入时间线适配：以整句为基准统一压缩（预算 = 句窗口 − 尾部余量
 * − 末字触发偏移，由 view 层算好后传入），整句所有字共享同一缩放——
 * 各字速度一致、过渡平滑，不再按字独立压缩导致句末字单独加速。
 *
 * 压缩策略两段式：提速部分（错峰 + 飞行）按比例吃压缩；收尾时长触到
 * FLY_TAIL_MIN_MS 保底后固定不变、提速部分吃剩余预算——飞入阶段加快，
 * 减速长尾保有充足时间，落定丝滑无顿挫。两段解在等比分界处连续
 * （总时长贴合预算无跳变），极端小预算钳在 FLY_SCALE_MIN 下限
 * （此时散开动画会从当前位姿平滑接续兜底）。
 */
export function fitFlyTimeline(budgetMs: number): FlyTimeline {
	const fastNaturalMs = FLY_STAGGER_SPACE_MS + FLY_PATH_NATURAL_MS;
	if (budgetMs >= STROKE_FLY_WINDOW_MS) {
		return { scale: 1, fastMs: fastNaturalMs, tailMs: FLY_TAIL_MS };
	}
	// 等比解：提速段与收尾段同比例，直到收尾触到保底
	const uniform = budgetMs / STROKE_FLY_WINDOW_MS;
	// 收尾保底解：收尾固定 FLY_TAIL_MIN_MS，提速段吃剩余预算
	const tailPinned = (budgetMs - FLY_TAIL_MIN_MS) / fastNaturalMs;
	// 分段选择：未触保底走等比解；触保底后走保底解（两解在分界处连续）
	const tailFloor = FLY_TAIL_MIN_MS / FLY_TAIL_MS;
	const scale = Math.max(
		FLY_SCALE_MIN,
		uniform >= tailFloor ? uniform : tailPinned,
	);
	return {
		scale,
		fastMs: fastNaturalMs * scale,
		tailMs: Math.max(FLY_TAIL_MIN_MS, FLY_TAIL_MS * scale),
	};
}

/**
 * 从 (cx,cy) 沿单位方向 (dx,dy) 飞到页面矩形边界所需的距离（屏幕像素）。
 * 射线对 x/y 两轴的两侧平面分别求交，取正的最小 t（即先撞上的那道边）；
 * 起点在页面外朝外飞（全轴无正交点）或方向退化时返回 600 兜底，
 * 保证飞入起点永远落在页面之外。
 */
export function exitDistancePx(
	cx: number,
	cy: number,
	dx: number,
	dy: number,
	left: number,
	top: number,
	right: number,
	bottom: number,
): number {
	let t = Number.POSITIVE_INFINITY;
	if (Math.abs(dx) > 1e-6) {
		const tLeft = (left - cx) / dx;
		const tRight = (right - cx) / dx;
		if (tLeft > 1e-6) t = Math.min(t, tLeft);
		if (tRight > 1e-6) t = Math.min(t, tRight);
	}
	if (Math.abs(dy) > 1e-6) {
		const tTop = (top - cy) / dy;
		const tBottom = (bottom - cy) / dy;
		if (tTop > 1e-6) t = Math.min(t, tTop);
		if (tBottom > 1e-6) t = Math.min(t, tBottom);
	}
	return Number.isFinite(t) ? t : 600;
}

/** 单列对齐：靠上 / 居中（单列）/ 靠下，多列时上下交错成对联式构图 */
export type StrokeColumnAlign = "start" | "center" | "end";

export interface StrokeColumn {
	items: StrokeCharTiming[];
	align: StrokeColumnAlign;
}

/** 列数上限：竖排歌词超过三列横向过于拥挤 */
export const STROKE_MAX_COLUMNS = 3;

/**
 * 按字数决定列数：短句单列居中，中句两列、长句三列（每列约 ≤7 字），
 * 单元格随之保持可读——长句不再被压到极限小字。
 */
export function computeStrokeColumnCount(charCount: number): number {
	if (charCount <= 0) return 0;
	if (charCount <= 6) return 1;
	if (charCount <= 12) return 2;
	return STROKE_MAX_COLUMNS;
}

/**
 * 拆列（构图需要，与时序无关）：均分进 1~3 列，多列时按"靠上/靠下"
 * 交错对齐成对联式构图；前面列多一字，保证先唱到的部分靠前。
 */
export function splitStrokeColumns(chars: StrokeCharTiming[]): StrokeColumn[] {
	const count = computeStrokeColumnCount(chars.length);
	if (count === 0) return [];
	const per = Math.ceil(chars.length / count);
	const columns: StrokeColumn[] = [];
	for (let i = 0; i < count; i++) {
		const items = chars.slice(i * per, (i + 1) * per);
		if (items.length === 0) break;
		columns.push({
			items,
			align: count === 1 ? "center" : i % 2 === 0 ? "start" : "end",
		});
	}
	return columns;
}

/**
 * 依据舞台可用高度与较长列的字数计算单元格边长（px）：
 * 两列合计占舞台高度的比例上限内均分，并夹在上下限之间。
 */
export function computeStrokeCellSize(
	stageHeight: number,
	columnLength: number,
): number {
	if (stageHeight <= 0 || columnLength <= 0) return STROKE_CELL_MAX_PX;
	const available = stageHeight * STROKE_COLUMN_HEIGHT_RATIO;
	return Math.min(
		STROKE_CELL_MAX_PX,
		Math.max(STROKE_CELL_MIN_PX, Math.floor(available / columnLength)),
	);
}
