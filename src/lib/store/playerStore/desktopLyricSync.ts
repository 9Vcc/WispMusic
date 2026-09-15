/**
 * 桌面歌词悬浮控件的主窗口侧同步引擎（仅主窗口初始化）。
 *
 * 职责与 mini 播放器同构：桌面歌词是独立 webview，拿不到主窗口的内存态
 * store，一切数据经 Rust 定向转发（emit_to 链路已验证可靠，不走 webview
 * 间 JS 事件广播）。三类推送：
 *
 * - full      歌词全文快照：resolveLyricLines 解析好的歌词行 + 歌词偏移，
 *             在切歌 / 歌词源变化 / 偏移变化时整包推送；
 * - progress  播放进度锚点（currentTime + 墙钟 + 播放态），节流推送，
 *             桌面歌词窗口本地按 rAF 插值，逐帧驱动逐字动效；
 * - settings  控件设置（方向/动效/字号），settingStore 是唯一可信源。
 *
 * 反向链路：桌面歌词窗口的工具条操作经 desktop_lyric_command 转成
 * desktop-lyric-event 事件回到这里，落库到 settingStore 后再随 settings
 * 推送回流——单向数据流，控件自身不持有权威状态。
 *
 * 窗口几何（显示/隐藏/横竖切换缩放）也在这里统一驱动：桌面歌词窗口不
 * 管理自己的 bounds，避免两处计算漂移。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { shallow } from "zustand/shallow";
import {
	computeDesktopLyricGeometry,
	type DesktopLyricDirection,
	type DesktopLyricGeometry,
} from "@/lib/utils/desktop-lyric-geometry";
import type { ILyricLine } from "@/lib/utils/lyric-parser";
import { resolveLyricLines } from "@/lib/utils/lyric-source";
import {
	DESKTOP_LYRIC_FONT_LIMITS,
	type DesktopLyricSettings,
	type LyricAnimationStyle,
	useSettingStore,
} from "../settingStore/settingStore";
import { usePlayerStore } from "./playerStore";
import { selectActiveLocalLyric } from "./playerStoreSlice/createLocalLyricSlice";
import { selectCurrentLyricOffset } from "./playerStoreSlice/createLyricOffsetSlice";
import { selectActiveLyricVersionLines } from "./playerStoreSlice/createLyricVersionSlice";
import { useProgressStore } from "./progressStore";

export type { DesktopLyricGeometry };

const clampFontSize = (size: number) =>
	Math.min(
		DESKTOP_LYRIC_FONT_LIMITS.max,
		Math.max(DESKTOP_LYRIC_FONT_LIMITS.min, Math.round(size)),
	);

/** full 快照里歌词行的传输形态（ILyricLine 本身就是可 JSON 化的平面对象） */
interface FullSnapshot {
	type: "full";
	songKey: string;
	title: string;
	lines: ILyricLine[];
	offsetMs: number;
}

interface SettingsSnapshot {
	type: "settings";
	direction: DesktopLyricDirection;
	animationStyle: LyricAnimationStyle;
	fontSize: number;
}

interface ProgressSnapshot {
	type: "progress";
	currentTime: number;
	isPlaying: boolean;
	/** 主窗口墙钟（ms），桌面歌词窗口按本机时钟插值 */
	at: number;
}

type SyncSnapshot = FullSnapshot | SettingsSnapshot | ProgressSnapshot;

function pushState(state: SyncSnapshot) {
	invoke("desktop_lyric_push_state", { state }).catch(console.error);
}

export function initDesktopLyricSync() {
	const unlistenFns: Array<() => void> = [];
	const unsubscribes: Array<() => void> = [];

	// 桌面歌词窗口的可见性（近似记账：show 指令、shown/hidden 事件共同维护）
	let windowActive = false;

	const getDesktopLyricSettings = () => useSettingStore.getState().desktopLyric;

	// —— 快照构造与推送 ——

	function buildFullSnapshot(): FullSnapshot {
		const s = usePlayerStore.getState();
		// 与歌词页完全同一条解析链路（手动版本 > 本地歌词 > 在线逐字 > 逐行）
		const resolution = resolveLyricLines({
			manualLines: selectActiveLyricVersionLines(s),
			localLines: selectActiveLocalLyric(s)?.lines,
			verbatimSource: s.currentSongLyrics?.yrc?.lyric,
			plainSource: s.currentSongLyrics?.lrc?.lyric,
		});
		return {
			type: "full",
			songKey: s.currentSong ? String(s.currentSong.id) : "none",
			title: s.currentSong?.name || "",
			lines: resolution.lines,
			offsetMs: selectCurrentLyricOffset(s),
		};
	}

	function pushFull() {
		if (!windowActive) return;
		pushState(buildFullSnapshot());
	}

	function pushSettings() {
		if (!windowActive) return;
		const { direction, animationStyle, fontSize } = getDesktopLyricSettings();
		pushState({ type: "settings", direction, animationStyle, fontSize });
	}

	// 进度推送节流：桌面歌词本地插值，300ms 锚点已远超逐字动效的精度需求
	const PROGRESS_PUSH_INTERVAL_MS = 300;
	let lastProgressPush = 0;
	function pushProgress(force = false) {
		if (!windowActive) return;
		const now = Date.now();
		if (!force && now - lastProgressPush < PROGRESS_PUSH_INTERVAL_MS) return;
		lastProgressPush = now;
		const s = usePlayerStore.getState();
		pushState({
			type: "progress",
			currentTime: useProgressStore.getState().currentTime,
			isPlaying: s.isPlaying,
			at: now,
		});
	}

	/** 窗口可见时的一次性全量补给（shown 事件 / 控件 ready 回执） */
	function pushAll() {
		pushFull();
		pushSettings();
		pushProgress(true);
	}

	// —— 窗口几何 ——

	async function showWindow() {
		const { direction, fontSize } = getDesktopLyricSettings();
		const { width, height } = computeDesktopLyricGeometry(direction, fontSize);
		try {
			await invoke("desktop_lyric_show", { width, height });
			// Rust 侧 show 成功后会 emit desktop-lyric-shown，全量补给在
			// shown 监听里统一做（单点触发，避免双份歌词快照）
			windowActive = true;
		} catch (error) {
			console.error("[DesktopLyric] 显示窗口失败:", error);
		}
	}

	async function hideWindow() {
		windowActive = false;
		try {
			await invoke("desktop_lyric_hide");
		} catch (error) {
			console.error("[DesktopLyric] 隐藏窗口失败:", error);
		}
	}

	// 方向/字号变化：窗口可见时保持中心点原子缩放（Rust 侧钳制在屏内）
	function syncGeometry(next: DesktopLyricSettings) {
		if (!windowActive) return;
		const { width, height } = computeDesktopLyricGeometry(
			next.direction,
			next.fontSize,
		);
		invoke("desktop_lyric_resize", { width, height }).catch(console.error);
	}

	// —— 订阅 ——

	// 设置订阅：settingStore 未启用 subscribeWithSelector，用普通监听 +
	// 手动前后对比。enabled 驱动窗口显隐；方向/字号驱动几何；其余字段随
	// settings 推送回流。
	let prevDesktopLyric = useSettingStore.getState().desktopLyric;
	const unsubSettings = useSettingStore.subscribe((state) => {
		const next = state.desktopLyric;
		const prev = prevDesktopLyric;
		if (next === prev) return;
		prevDesktopLyric = next;

		if (next.enabled !== prev.enabled) {
			if (next.enabled) void showWindow();
			else void hideWindow();
			return;
		}
		if (next.direction !== prev.direction || next.fontSize !== prev.fontSize) {
			syncGeometry(next);
		}
		pushSettings();
	});
	unsubscribes.push(unsubSettings);

	// 歌曲与歌词源订阅：任一变化整包重推歌词快照（低频事件，整包最简单可靠）
	const unsubLyricSource = usePlayerStore.subscribe(
		(state) => ({
			currentSong: state.currentSong,
			currentSongLyrics: state.currentSongLyrics,
			lyricTimeOffsetMap: state.lyricTimeOffsetMap,
			lyricVersionLines: state.lyricVersionLines,
			localLyric: state.localLyric,
		}),
		() => pushFull(),
		{ equalityFn: shallow },
	);
	unsubscribes.push(unsubLyricSource);

	// 进度订阅（progressStore 由 corePlayer 高频写入，这里只做节流推送）
	const unsubProgress = useProgressStore.subscribe(() => pushProgress());
	unsubscribes.push(unsubProgress);

	// —— 桌面歌词窗口 → 主窗口的指令回传 ——
	void listen<{
		event: string;
		direction?: string;
		style?: string;
		delta?: number;
	}>("desktop-lyric-event", (e) => {
		const { payload } = e;
		const settings = useSettingStore.getState();
		switch (payload.event) {
			case "ready":
				// 控件窗口挂载（含 HMR 重挂载）后主动要一次全量快照
				if (windowActive) pushAll();
				break;
			case "close":
				// 唯一可信源仍是 settingStore；显隐订阅会随之隐藏窗口
				void settings.setDesktopLyricEnabled(false);
				break;
			case "setDirection":
				if (
					payload.direction === "horizontal" ||
					payload.direction === "vertical"
				) {
					void settings.setDesktopLyricDirection(payload.direction);
				}
				break;
			case "setAnimationStyle":
				if (payload.style === "default" || payload.style === "stroke") {
					void settings.setDesktopLyricAnimationStyle(payload.style);
				}
				break;
			case "setFontDelta": {
				if (typeof payload.delta === "number" && payload.delta !== 0) {
					const fontSize = clampFontSize(
						getDesktopLyricSettings().fontSize + payload.delta,
					);
					void settings.setDesktopLyricFontSize(fontSize);
				}
				break;
			}
		}
	}).then((unlisten) => unlistenFns.push(unlisten));

	// 窗口被 Alt+F4 等系统途径关闭：Rust 侧通知这里，把开关同步回 false
	void listen("desktop-lyric-hidden", () => {
		windowActive = false;
		if (getDesktopLyricSettings().enabled) {
			void useSettingStore.getState().setDesktopLyricEnabled(false);
		}
	}).then((unlisten) => unlistenFns.push(unlisten));

	// 窗口已显示（本模块发起 or 兜底）：记账并全量补给
	void listen("desktop-lyric-shown", () => {
		windowActive = true;
		pushAll();
	}).then((unlisten) => unlistenFns.push(unlisten));

	// 设置水合完成后再应用初始状态：上一次会话开着桌面歌词 → 本次启动恢复
	void (async () => {
		if (!useSettingStore.getState().hydrated) {
			await new Promise<void>((resolve) => {
				const unsubHydration = useSettingStore.subscribe((state) => {
					if (state.hydrated) {
						unsubHydration();
						resolve();
					}
				});
			});
		}
		if (getDesktopLyricSettings().enabled) {
			await showWindow();
		}
	})();

	return () => {
		unsubscribes.forEach((unsub) => {
			unsub();
		});
		unlistenFns.forEach((unlisten) => {
			unlisten();
		});
	};
}
