import { invoke } from "@tauri-apps/api/core";
import { useCloudStore } from "@/lib/store/cloudStore/cloudStore";
import { useDownloadStore } from "@/lib/store/downloadStore/downloadStore";
import { useLocalMusicStore } from "@/lib/store/localMusicStore/localMusicStore";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { useUserStore } from "@/lib/store/userStore/userStore";
import { getHanziCacheSize } from "@/lib/utils/hanzi-strokes";
import { getSwrCacheSnapshot } from "@/lib/utils/swr-cache";
import {
	getWebglCanvasSnapshot,
	type WebglCanvasSnapshot,
} from "./graphics-registry";
import {
	estimateBitmapBytes,
	estimateFramebufferBytes,
	measureJsonBytes,
} from "./memory-units";

export {
	byteLengthUtf8,
	estimateBitmapBytes,
	estimateFramebufferBytes,
	formatBytes,
	measureJsonBytes,
} from "./memory-units";

/**
 * WebView 内存归因（仅调试入口手动触发）。
 *
 * 明确定位：这里给的是**归因**而不是精确的堆快照。JS 侧能拿到的最准的数字是
 * V8 自己的 `performance.memory`（usedJSHeapSize），而它通常只占 WebView 进程
 * 的一小部分——剩下的是 Blink 的 DOM/样式、图像解码位图、WebGL 资源、以及
 * Chromium 自身的固定开销。所以面板同时给出：
 *   1. Rust 侧读到的**进程树真实工作集**（含 msedgewebview2.exe 的渲染/GPU 进程）；
 *   2. JS 侧的逐项估算（store 序列化体积、图像解码字节、WebGL 绘制缓冲、SWR 缓存）。
 * 用 1 做总量、用 2 做相对占比排序，就能判断下一刀该切哪里。
 *
 * 各 store 的体积用「序列化后的 UTF-8 字节数」表示：它是稳定的相对指标，
 * 但与真实堆占用不是 1:1（对象头、隐藏类、字符串内部表示都会额外膨胀），
 * 经验上真实堆约为序列化体积的 1.5~2 倍。
 */

/** Chromium 专有的堆信息（标准 Performance 接口里没有） */
interface ChromiumHeapInfo {
	usedJSHeapSize: number;
	totalJSHeapSize: number;
	jsHeapSizeLimit: number;
}

export interface MemorySample {
	name: string;
	bytes: number;
	detail?: string;
}

export interface ProcessMemoryEntry {
	pid: number;
	parentPid: number;
	name: string;
	/** Chromium 进程角色：browser / renderer / gpu / network / audio / utility */
	role: string;
	workingSet: number;
	privateBytes: number;
}

export interface ProcessMemoryReport {
	selfPid: number;
	/** 工作集之和：Chromium 各进程共享大量代码页，相加会重复计算共享部分，只看相对大小 */
	totalWorkingSet: number;
	/** 私有提交量之和：更接近「这套进程实际占了多少、关掉能还回多少」 */
	totalPrivateBytes: number;
	/** 本进程 Rust 堆存活字节数（Rust 全局分配器计数口径，不含 WebView2 渲染进程） */
	rustHeapBytes: number;
	/** Rust 堆历史峰值（同一口径）：稳态远低于峰值或只涨不落都能据此看出 */
	rustHeapPeakBytes: number;
	processes: ProcessMemoryEntry[];
}

export interface MemoryReport {
	heap: ChromiumHeapInfo | null;
	processes: ProcessMemoryReport | null;
	processError: string | null;
	dom: {
		nodes: number;
		canvases: number;
		images: number;
		imageBytes: number;
	};
	webgl: WebglCanvasSnapshot & {
		/** 单块画布的默认帧缓冲下限（仅 color，无 MSAA） */
		framebufferBytesMin: number;
		/** 上限：4×MSAA 的 color 附件 */
		framebufferBytesMax: number;
	};
	swr: {
		keys: number;
		totalBytes: number;
		entries: MemorySample[];
	};
	stores: MemorySample[];
	caches: MemorySample[];
	/** JS 侧各项估算之和（不含 V8 堆与进程基线） */
	attributedBytes: number;
}

function arrayDetail(value: unknown): string | undefined {
	if (Array.isArray(value)) return `${value.length} 项`;
	if (value instanceof Map) return `${value.size} 项`;
	if (value instanceof Set) return `${value.size} 项`;
	if (value && typeof value === "object") {
		return `${Object.keys(value as object).length} 键`;
	}
	return undefined;
}

function sample(name: string, value: unknown): MemorySample {
	return {
		name,
		bytes: measureJsonBytes(value),
		detail: arrayDetail(value),
	};
}

/** 遍历当前挂载的 <img>，累加其解码后的位图大小 */
function collectImages(): { count: number; bytes: number } {
	let count = 0;
	let bytes = 0;
	for (const image of Array.from(document.images)) {
		const width = image.naturalWidth;
		const height = image.naturalHeight;
		if (!image.complete || width === 0 || height === 0) continue;
		count += 1;
		bytes += estimateBitmapBytes(width, height);
	}
	return { count, bytes };
}

function collectStores(): MemorySample[] {
	const player = usePlayerStore.getState();
	const user = useUserStore.getState();
	const local = useLocalMusicStore.getState();
	const download = useDownloadStore.getState();
	const cloud = useCloudStore.getState();

	const samples: MemorySample[] = [
		sample("playerStore.playlist", player.playlist),
		sample("playerStore.currentSongLyrics", player.currentSongLyrics),
		sample("playerStore.lyricVersionLines", player.lyricVersionLines),
		sample("playerStore.lyricVersionMap", player.lyricVersionMap),
		sample("playerStore.lyricTimeOffsetMap", player.lyricTimeOffsetMap),
		sample("userStore.playlistList", user.playlistList),
		sample("userStore.artistList", user.artistList),
		sample("userStore.albumList", user.albumList),
		sample("userStore.likeList", user.likeList),
		sample("localMusicStore.tracks", local.tracks),
		sample("downloadStore.downloadedSongs", download.downloadedSongs),
	];

	if (cloud.uploadTasks.size > 0) {
		samples.push(sample("cloudStore.uploadTasks", [...cloud.uploadTasks]));
	}

	return samples.sort((a, b) => b.bytes - a.bytes);
}

export async function collectMemoryReport(): Promise<MemoryReport> {
	const heap =
		(performance as Performance & { memory?: ChromiumHeapInfo }).memory ??
		null;

	let processes: ProcessMemoryReport | null = null;
	let processError: string | null = null;
	try {
		processes = await invoke<ProcessMemoryReport>(
			"get_process_memory_report",
		);
	} catch (error) {
		processError =
			error instanceof Error ? error.message : String(error);
	}

	const images = collectImages();
	const webglSnapshot = getWebglCanvasSnapshot();
	const framebuffer = estimateFramebufferBytes(webglSnapshot.totalPixels);

	const swrEntries = getSwrCacheSnapshot()
		.map(({ key, data }) => ({
			name: key,
			bytes: measureJsonBytes(data),
		}))
		.sort((a, b) => b.bytes - a.bytes);

	const stores = collectStores();

	// 这两个缓存按条目数估算（笔画数据每字约 1~4KB，取 2KB；取色缓存每条含一张
	// 32×32 位图 = 4KB），界面上会标「估算」；合计里计入它们，但真正可信的是
	// 上面那些实测项，估算项只用来提示「这里还有一小块」。
	const hanziCount = getHanziCacheSize();
	const caches: MemorySample[] = [
		{
			name: "笔画字符缓存（LRU 800）",
			bytes: hanziCount * 2048,
			detail: `${hanziCount} 字 · 估算`,
		},
		{
			name: "封面取色缓存（上限 24）",
			bytes: 24 * 4096,
			detail: "每条含一张 32×32 位图 · 估算",
		},
	];

	const measuredTotal =
		stores.reduce((sum, item) => sum + item.bytes, 0) +
		swrEntries.reduce((sum, item) => sum + item.bytes, 0) +
		images.bytes +
		framebuffer.max +
		caches.reduce((sum, item) => sum + item.bytes, 0);

	return {
		heap,
		processes,
		processError,
		dom: {
			nodes: document.getElementsByTagName("*").length,
			canvases: document.querySelectorAll("canvas").length,
			images: images.count,
			imageBytes: images.bytes,
		},
		webgl: {
			...webglSnapshot,
			framebufferBytesMin: framebuffer.min,
			framebufferBytesMax: framebuffer.max,
		},
		swr: {
			keys: swrEntries.length,
			totalBytes: swrEntries.reduce((sum, item) => sum + item.bytes, 0),
			entries: swrEntries.slice(0, 8),
		},
		stores,
		caches,
		attributedBytes: measuredTotal,
	};
}
