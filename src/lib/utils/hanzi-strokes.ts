/**
 * 汉字笔画数据加载（Make Me a Hanzi / hanzi-writer-data 开源笔画库）。
 *
 * 每个汉字一条记录：{ strokes: string[] }，strokes 为 SVG 路径字符串，
 * 每笔画一条独立路径，可严格按笔画拆分做逐笔动画。坐标系与 hanzi-writer
 * 一致：x 0..1024、y -124..900（y 轴朝上），渲染到 SVG（y 轴朝下）时需
 * 翻转：translate(0, 900) scale(1, -1)。
 *
 * 数据体积原因（全库约 20MB）不打包进应用，按字从 CDN 拉取并内存缓存，
 * 两个 CDN 互为容灾（与 test/hanzi-stroke-flyin.html 的加载策略一致）。
 * 非 CJK 字符（字母 / 数字 / 标点 / 空白）直接短路返回 null，由调用方
 * 以静态字兜底显示，绝不阻塞歌词主流程。
 */

import { createLruCache } from "./lru";

const DATA_SOURCES = [
	(ch: string) =>
		`https://cdn.jsdelivr.net/npm/hanzi-writer-data@2.0.1/${encodeURIComponent(ch)}.json`,
	(ch: string) =>
		`https://unpkg.com/hanzi-writer-data@2.0.1/${encodeURIComponent(ch)}.json`,
];

export interface HanziCharData {
	strokes: string[];
}

/**
 * 笔画字符缓存。Map 只增不减时，笔画歌词模式下每听一首中文歌都会永久留下新字，
 * 长期使用单调增长（全库体量约 20MB）。这里加 LRU 上限：命中即刷新，超出上限
 * 从最久未用淘汰。
 *
 * 上限 800：单曲工作集是 100~300 个不重复汉字，800 字约等于「最近三首」的容量，
 * 上一首/下一首往返仍在命中范围内；每字约 1~4KB 路径串，常驻量因此压在 ~3MB 内
 * （原先 2000 字上限下约 6MB）。
 */
const CHAR_CACHE_LIMIT = 800;
const charDataCache = createLruCache<string, Promise<HanziCharData | null>>(
	CHAR_CACHE_LIMIT,
);

/** 当前缓存的字符条数（供测试断言上限行为） */
export function getHanziCacheSize(): number {
	return charDataCache.size;
}

/** 笔画库覆盖的区段：CJK 部件与扩展 A、统一表意文字、兼容表意文字 */
const STROKABLE_CHAR_RE =
	/[\u2E80-\u2EFF\u31C0-\u31EF\u3400-\u4DBF\u4E00-\u9FFF\uF900-\uFAFF]/;

/** 是否为可能存在笔画数据的汉字（不含拉丁字母、数字、标点等） */
export function isStrokableChar(ch: string): boolean {
	return STROKABLE_CHAR_RE.test(ch);
}

/**
 * 加载单字笔画数据（带缓存）。网络失败或无数据时 resolve null，
 * 调用方必须兜底为静态字显示。
 */
export function loadHanziCharData(ch: string): Promise<HanziCharData | null> {
	if (!isStrokableChar(ch)) return Promise.resolve(null);

	let cached = charDataCache.get(ch);
	if (!cached) {
		cached = (async () => {
			for (const makeUrl of DATA_SOURCES) {
				try {
					const res = await fetch(makeUrl(ch));
					if (!res.ok) continue;
					const data = (await res.json()) as {
						strokes?: unknown[];
					};
					if (data?.strokes && Array.isArray(data.strokes)) {
						// 兼容字符串 / { path } 对象两种条目格式
						const strokes = data.strokes
							.map((s) =>
								typeof s === "string"
									? s
									: String((s as { path?: string }).path ?? ""),
							)
							.filter(Boolean);
						if (strokes.length) return { strokes };
					}
				} catch {
					// 网络失败时尝试下一个数据源
				}
			}
			return null;
		})();
		charDataCache.set(ch, cached);
	}
	return cached;
}

/** 预取一段文本中所有可拆笔画汉字的数据（如下一句歌词），失败静默 */
export function prefetchHanziChars(text: string): void {
	for (const ch of text) {
		if (isStrokableChar(ch) && !charDataCache.has(ch)) {
			void loadHanziCharData(ch);
		}
	}
}
