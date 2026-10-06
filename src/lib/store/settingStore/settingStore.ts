import { invoke, isTauri } from "@tauri-apps/api/core";
import { Effect, getCurrentWindow } from "@tauri-apps/api/window";
import { create } from "zustand";
import {
	setApiBaseUrl as applyApiBaseUrl,
	normalizeApiBaseUrl,
} from "@/lib/api";
import {
	DEFAULT_AUDIO_OUTPUT_DEVICE_ID,
	DEFAULT_AUDIO_OUTPUT_DEVICE_NAME,
} from "@/lib/constants/audio-output-devices";
import {
	defaultShortcutBindings,
	type ShortcutAction,
	type ShortcutBindings,
} from "@/lib/constants/shortcuts";
import {
	resetAudioCacheDir,
	setAudioCacheDir,
} from "@/lib/player/audio-cache-dir";
import { type AudioDeviceInfo, corePlayer } from "@/lib/player/corePlayer";
import {
	type AccentTheme,
	buildAccentPrimary,
	normalizeAccentColor,
} from "@/lib/utils/accent-color";
import {
	DESKTOP_LYRIC_DEFAULT_COLORS,
	type DesktopLyricColors,
	normalizeDesktopLyricColors,
} from "@/lib/utils/desktop-lyric-colors";
import {
	applyMinWindowSize,
	captureCurrentBounds,
	initMainWindowState,
	setWindowBoundsTracking,
} from "@/lib/window/windowState";
import type { QualityKey } from "../../constants/song";
import { getSettingsStore } from "./settings.persistence";

const APPEARANCE_SYNC_EVENT = "settings-appearance-changed";
const BYTES_PER_GIB = 1024 * 1024 * 1024;

export interface FontSettings {
	interfaceFontStr: string;
	lyricFontStr: string;
	lyricFontWeight: number;
	lyricFontSize: number;
}

export interface AppearanceSettings {
	theme: "light" | "dark" | "system";
	/** 窗口材质：mica/acrylic 为系统特效，solid 为不透明纯色兜底，none 为透明 */
	material: "acrylic" | "mica" | "none" | "solid";
	/**
	 * 自定义主题色，规范化为 `#rrggbb`。
	 * null 表示使用 index.css 的内置配色（此时不覆写 --primary）。
	 */
	accentColor: string | null;
	font: FontSettings;
}

/** 歌词页动效风格：default 为项目默认滚动逐字动效，stroke 为笔画双列动效 */
export type LyricAnimationStyle = "default" | "stroke";

export interface LyricSettings {
	/** 歌词页：非当前行是否按距离做模糊处理（关闭则全部行保持清晰） */
	blurEnabled: boolean;
	/** 播放本地音乐时，是否自动加载音频同目录下的同名 .lrc 歌词文件 */
	autoLoadLocalLyric: boolean;
	/** 歌词页动效：default=默认滚动逐字；stroke=笔画双列（一列靠上一列靠下） */
	animationStyle: LyricAnimationStyle;
}

/** 桌面歌词显示方向：horizontal=横向单行；vertical=竖排（传统右起直书） */
export type DesktopLyricDirection = "horizontal" | "vertical";

export interface DesktopLyricSettings {
	/** 桌面歌词悬浮控件是否启用（窗口可见性以此为准，主窗口是唯一写入方） */
	enabled: boolean;
	/** 显示方向：横向 / 竖向，可自由切换（窗口尺寸随方向变化） */
	direction: DesktopLyricDirection;
	/** 桌面歌词动效（独立于歌词页）：default=逐字填充；stroke=笔画飞入 */
	animationStyle: LyricAnimationStyle;
	/** 歌词字号（逻辑像素，横向单行/竖排单字共用） */
	fontSize: number;
	/** 配色方案：文字 / 高亮 / 描边阴影颜色 + 背景不透明度 */
	colors: DesktopLyricColors;
}

/** 桌面歌词字号可调范围（逻辑像素） */
export const DESKTOP_LYRIC_FONT_LIMITS = { min: 24, max: 64 } as const;

export interface OutputDeviceProfile {
	id: string;
	displayName?: string;
	iconKey?: string;
	lastKnownName: string;
	firstSeenAt: number;
	lastSeenAt: number;
}

export type OutputDeviceProfiles = Record<string, OutputDeviceProfile>;

export interface AudioSettings {
	preferQuality: QualityKey;
	maxCacheSize: number;
	/** 自定义缓存目录（空字符串表示使用系统默认位置） */
	cacheDir: string;
	outputDeviceId: string | null;
	outputDeviceProfiles: OutputDeviceProfiles;
	replayGainEnabled: boolean;
	replayGainPreampDb: number;
	equalizerEnabled: boolean;
	equalizerGainsDb: number[];
	crossfadeDuration: number;
}

export interface NetworkSettings {
	/** 自定义 API 服务地址，空字符串表示使用出厂默认地址 */
	apiBaseUrl: string;
	/** 灰色歌曲解灰开关 */
	unblockEnabled: boolean;
	/** 解灰音源，逗号分隔的自由文本，空字符串表示自动（unm 默认音源 + HTTP 音源依次尝试） */
	unblockSource: string;
	/** 解灰启用 FLAC（与 VutronMusic 一致） */
	unblockEnableFlac: boolean;
	/** 解灰音源搜索模式：true 顺序优先 / false 速度优先（与 VutronMusic 一致） */
	unblockOrderFirst: boolean;
	/** QQ 音源引擎 Cookie */
	unblockQqCookie: string;
	/** JOOX 音源引擎 Cookie */
	unblockJooxCookie: string;
	/** 解灰设置是否已在设置页解锁（连续点击 5 次版本号后为 true，持久化） */
	unblockSettingsVisible: boolean;
}

export interface WindowSettings {
	/** 记住窗口大小和位置 */
	rememberWindowBounds: boolean;
	/** 主窗口最小宽度（逻辑像素） */
	minWindowWidth: number;
	/** 主窗口最小高度（逻辑像素） */
	minWindowHeight: number;
}

/** 可隐藏的功能侧栏入口 key（与 AppSidebar 的入口一一对应） */
export const SIDEBAR_ENTRY_KEYS = [
	"explore",
	"recent",
	"download",
	"local",
	"cloud",
	"favorite",
	"playlists",
] as const;

export type SidebarEntryKey = (typeof SIDEBAR_ENTRY_KEYS)[number];

export interface SidebarSettings {
	/** 已隐藏的侧栏入口 */
	hiddenEntries: SidebarEntryKey[];
}

export interface DebugSettings {
	/** 调试模式入口是否已解锁（设置页「关于」标题 2 秒内连点 5 次后为 true，持久化） */
	visible: boolean;
	/** 调试模式：开启后可在设置页手动触发更新弹窗预览 */
	enabled: boolean;
}

export const UNBLOCK_SOURCE_AUTO = "auto";

export const MIN_WINDOW_WIDTH_LIMITS = { min: 800, max: 2560 } as const;
export const MIN_WINDOW_HEIGHT_LIMITS = { min: 480, max: 1440 } as const;

const defaultAppearanceSettings: AppearanceSettings = {
	theme: "dark",
	material: "mica",
	accentColor: null,
	font: {
		interfaceFontStr: "",
		lyricFontStr: "",
		lyricFontWeight: 400,
		lyricFontSize: 46,
	},
};

const defaultLyricSettings: LyricSettings = {
	blurEnabled: true,
	autoLoadLocalLyric: true,
	animationStyle: "default",
};

const defaultDesktopLyricSettings: DesktopLyricSettings = {
	enabled: false,
	direction: "horizontal",
	animationStyle: "default",
	fontSize: 40,
	colors: { ...DESKTOP_LYRIC_DEFAULT_COLORS },
};

const defaultOutputDeviceProfiles: OutputDeviceProfiles = {
	[DEFAULT_AUDIO_OUTPUT_DEVICE_ID]: {
		id: DEFAULT_AUDIO_OUTPUT_DEVICE_ID,
		lastKnownName: DEFAULT_AUDIO_OUTPUT_DEVICE_NAME,
		firstSeenAt: 0,
		lastSeenAt: 0,
	},
};

const defaultAudioSettings: AudioSettings = {
	preferQuality: "l",
	maxCacheSize: 10,
	cacheDir: "",
	outputDeviceId: null,
	outputDeviceProfiles: defaultOutputDeviceProfiles,
	replayGainEnabled: true,
	replayGainPreampDb: 0,
	equalizerEnabled: false,
	equalizerGainsDb: [0, 0, 0, 0, 0],
	crossfadeDuration: 0,
};

const defaultNetworkSettings: NetworkSettings = {
	apiBaseUrl: "",
	unblockEnabled: true,
	unblockSource: "",
	unblockEnableFlac: true,
	unblockOrderFirst: true,
	unblockQqCookie: "",
	unblockJooxCookie: "",
	unblockSettingsVisible: false,
};

const defaultWindowSettings: WindowSettings = {
	rememberWindowBounds: true,
	minWindowWidth: 1000,
	minWindowHeight: 700,
};

const defaultSidebarSettings: SidebarSettings = {
	hiddenEntries: [],
};

const defaultDebugSettings: DebugSettings = {
	visible: false,
	enabled: false,
};

export interface ShortcutSettings {
	bindings: ShortcutBindings;
}

const defaultShortcutSettings: ShortcutSettings = {
	bindings: defaultShortcutBindings,
};

type OutputDeviceProfilePatch = Partial<
	Pick<OutputDeviceProfile, "displayName" | "iconKey">
>;

interface UpdateAudioEngineOptions {
	throwOnApplyError?: boolean;
}

type SettingStore = {
	appearance: AppearanceSettings;
	lyric: LyricSettings;
	desktopLyric: DesktopLyricSettings;
	audio: AudioSettings;
	network: NetworkSettings;
	window: WindowSettings;
	sidebar: SidebarSettings;
	debug: DebugSettings;
	shortcuts: ShortcutSettings;
	hydrated: boolean;

	setTheme: (theme: AppearanceSettings["theme"]) => void;
	setMaterial: (material: AppearanceSettings["material"]) => void;
	/** 传 null 表示恢复内置配色；非法取值会被规范化丢弃 */
	setAccentColor: (color: string | null) => void;
	updateFont: (patch: Partial<FontSettings>) => void;

	loadSettings: () => Promise<void>;
	saveSettings: () => Promise<void>;
	resetAppearance: () => Promise<void>;

	setLyricBlurEnabled: (enabled: boolean) => Promise<void>;
	setAutoLoadLocalLyric: (enabled: boolean) => Promise<void>;
	setLyricAnimationStyle: (style: LyricAnimationStyle) => Promise<void>;
	setDesktopLyricEnabled: (enabled: boolean) => Promise<void>;
	setDesktopLyricDirection: (direction: DesktopLyricDirection) => Promise<void>;
	setDesktopLyricAnimationStyle: (style: LyricAnimationStyle) => Promise<void>;
	setDesktopLyricFontSize: (size: number) => Promise<void>;
	/** 局部更新配色（保存并即时推送），颜色走 hex 规范化、透明度钳制 */
	setDesktopLyricColors: (colors: Partial<DesktopLyricColors>) => Promise<void>;
	/** 恢复出厂配色（白字 + 深色描边 + 全透明底） */
	resetDesktopLyricColors: () => Promise<void>;
	setPreferQuality: (quality: QualityKey) => Promise<void>;
	setMaxCacheSize: (size: number) => Promise<void>;
	/** 切换缓存目录（原生侧校验可写并迁移旧缓存），成功后写回设置 */
	setCacheDir: (dir: string) => Promise<void>;
	/** 恢复缓存到系统默认目录 */
	resetCacheDir: () => Promise<void>;
	upsertOutputDeviceProfiles: (devices: AudioDeviceInfo[]) => Promise<void>;
	updateOutputDeviceProfile: (
		id: string,
		patch: OutputDeviceProfilePatch,
	) => Promise<void>;
	setApiBaseUrl: (url: string) => Promise<void>;
	setUnblockEnabled: (enabled: boolean) => Promise<void>;
	setUnblockSource: (source: string) => Promise<void>;
	setUnblockEnableFlac: (enabled: boolean) => Promise<void>;
	setUnblockOrderFirst: (orderFirst: boolean) => Promise<void>;
	setUnblockQqCookie: (cookie: string) => Promise<void>;
	setUnblockJooxCookie: (cookie: string) => Promise<void>;
	setUnblockSettingsVisible: (visible: boolean) => Promise<void>;
	setDebugSettingsVisible: (visible: boolean) => Promise<void>;
	setDebugMode: (enabled: boolean) => Promise<void>;
	setRememberWindowBounds: (enabled: boolean) => Promise<void>;
	setMinWindowSize: (width: number, height: number) => Promise<void>;
	setSidebarEntryHidden: (
		entry: SidebarEntryKey,
		hidden: boolean,
	) => Promise<void>;
	setShortcutBinding: (
		action: ShortcutAction,
		binding: string,
	) => Promise<void>;
	resetShortcuts: () => Promise<void>;
	updateAudioEngine: (
		patch: Partial<AudioSettings>,
		options?: UpdateAudioEngineOptions,
	) => Promise<void>;
};

export const useSettingStore = create<SettingStore>((set, get) => ({
	appearance: defaultAppearanceSettings,
	lyric: defaultLyricSettings,
	desktopLyric: defaultDesktopLyricSettings,
	audio: defaultAudioSettings,
	network: defaultNetworkSettings,
	window: defaultWindowSettings,
	sidebar: defaultSidebarSettings,
	debug: defaultDebugSettings,
	shortcuts: defaultShortcutSettings,
	hydrated: false,

	setTheme: (theme) => {
		set((state) => ({
			appearance: {
				...state.appearance,
				theme,
			},
		}));
		get().saveSettings();
		broadcastAppearance(get().appearance);
	},

	setMaterial: (material) => {
		set((state) => ({
			appearance: {
				...state.appearance,
				material,
			},
		}));
		get().saveSettings();
		broadcastAppearance(get().appearance);
	},

	setAccentColor: (color) => {
		// 在这里就规范化：非法输入（半截 hex、空串等）一律落回 null，
		// 避免脏值进 store 与 settings.json
		const accentColor = normalizeAccentColor(color);
		set((state) => ({
			appearance: {
				...state.appearance,
				accentColor,
			},
		}));
		get().saveSettings();
		broadcastAppearance(get().appearance);
	},

	updateFont: (patch) => {
		set((state) => ({
			appearance: {
				...state.appearance,
				font: {
					...state.appearance.font,
					...patch,
				},
			},
		}));
		get().saveSettings();
		broadcastAppearance(get().appearance);
	},

	loadSettings: async () => {
		const store = await getSettingsStore();
		const savedAppearance = await store.get<AppearanceSettings>("appearance");
		const savedLyric = await store.get<Partial<LyricSettings>>("lyric");
		const savedDesktopLyric =
			await store.get<Partial<DesktopLyricSettings>>("desktopLyric");
		const savedAudio = await store.get<Partial<AudioSettings>>("audio");
		const savedNetwork = await store.get<Partial<NetworkSettings>>("network");
		const savedWindow = await store.get<Partial<WindowSettings>>("window");
		const savedSidebar = await store.get<Partial<SidebarSettings>>("sidebar");
		const savedDebug = await store.get<Partial<DebugSettings>>("debug");
		const savedShortcuts =
			await store.get<Partial<ShortcutSettings>>("shortcuts");

		set({
			appearance: savedAppearance
				? {
						...defaultAppearanceSettings,
						...savedAppearance,
						accentColor: normalizeAccentColor(savedAppearance.accentColor),
						font: normalizeFontSettings(savedAppearance.font),
					}
				: defaultAppearanceSettings,
			lyric: normalizeLyricSettings(savedLyric),
			desktopLyric: normalizeDesktopLyricSettings(savedDesktopLyric),
			audio: normalizeAudioSettings(savedAudio, get().audio),
			network: normalizeNetworkSettings(savedNetwork),
			window: normalizeWindowSettings(savedWindow),
			sidebar: normalizeSidebarSettings(savedSidebar),
			debug: normalizeDebugSettings(savedDebug),
			shortcuts: normalizeShortcutSettings(savedShortcuts),
			hydrated: true,
		});
	},

	saveSettings: async () => {
		const store = await getSettingsStore();
		await store.set("appearance", get().appearance);
		await store.set("lyric", get().lyric);
		await store.set("desktopLyric", get().desktopLyric);
		await store.set("audio", get().audio);
		await store.set("network", get().network);
		await store.set("window", get().window);
		await store.set("sidebar", get().sidebar);
		await store.set("debug", get().debug);
		await store.set("shortcuts", get().shortcuts);
		await store.save();
	},

	resetAppearance: async () => {
		const store = await getSettingsStore();
		set({ appearance: defaultAppearanceSettings });
		await store.set("appearance", defaultAppearanceSettings);
		await store.save();
		broadcastAppearance(defaultAppearanceSettings);
	},

	setLyricBlurEnabled: async (enabled) => {
		const store = await getSettingsStore();
		set((state) => ({ lyric: { ...state.lyric, blurEnabled: enabled } }));
		await store.set("lyric", get().lyric);
		await store.save();
	},

	setAutoLoadLocalLyric: async (enabled) => {
		const store = await getSettingsStore();
		set((state) => ({
			lyric: { ...state.lyric, autoLoadLocalLyric: enabled },
		}));
		await store.set("lyric", get().lyric);
		await store.save();
	},

	setLyricAnimationStyle: async (style) => {
		const store = await getSettingsStore();
		set((state) => ({ lyric: { ...state.lyric, animationStyle: style } }));
		await store.set("lyric", get().lyric);
		await store.save();
	},

	setDesktopLyricEnabled: async (enabled) => {
		const store = await getSettingsStore();
		set((state) => ({ desktopLyric: { ...state.desktopLyric, enabled } }));
		await store.set("desktopLyric", get().desktopLyric);
		await store.save();
	},

	setDesktopLyricDirection: async (direction) => {
		const store = await getSettingsStore();
		set((state) => ({ desktopLyric: { ...state.desktopLyric, direction } }));
		await store.set("desktopLyric", get().desktopLyric);
		await store.save();
	},

	setDesktopLyricAnimationStyle: async (style) => {
		const store = await getSettingsStore();
		set((state) => ({
			desktopLyric: { ...state.desktopLyric, animationStyle: style },
		}));
		await store.set("desktopLyric", get().desktopLyric);
		await store.save();
	},

	setDesktopLyricFontSize: async (size) => {
		const store = await getSettingsStore();
		const fontSize = clampNumber(
			size,
			DESKTOP_LYRIC_FONT_LIMITS.min,
			DESKTOP_LYRIC_FONT_LIMITS.max,
			defaultDesktopLyricSettings.fontSize,
		);
		set((state) => ({ desktopLyric: { ...state.desktopLyric, fontSize } }));
		await store.set("desktopLyric", get().desktopLyric);
		await store.save();
	},

	setDesktopLyricColors: async (colors) => {
		const store = await getSettingsStore();
		// 局部合并 + 逐字段规范化：调用方只传要改的那一项，
		// 非法值（手改配置 / 取色器异常）在这里就被挡回默认色
		set((state) => ({
			desktopLyric: {
				...state.desktopLyric,
				colors: normalizeDesktopLyricColors({
					...state.desktopLyric.colors,
					...colors,
				}),
			},
		}));
		await store.set("desktopLyric", get().desktopLyric);
		await store.save();
	},

	resetDesktopLyricColors: async () => {
		const store = await getSettingsStore();
		set((state) => ({
			desktopLyric: {
				...state.desktopLyric,
				colors: { ...DESKTOP_LYRIC_DEFAULT_COLORS },
			},
		}));
		await store.set("desktopLyric", get().desktopLyric);
		await store.save();
	},

	setPreferQuality: async (quality: QualityKey) => {
		const store = await getSettingsStore();
		set({ audio: { ...get().audio, preferQuality: quality } });
		await store.set("audio", get().audio);
		await store.save();
	},

	setMaxCacheSize: async (size: number) => {
		const store = await getSettingsStore();
		set({ audio: { ...get().audio, maxCacheSize: size } });
		await store.set("audio", get().audio);
		await store.save();
		if (get().audio.maxCacheSize !== size) return;
		await applyAudioCacheLimit(size);
	},

	setCacheDir: async (dir) => {
		const cacheDir = dir.trim();
		if (!cacheDir || cacheDir === get().audio.cacheDir) return;
		// 先让原生侧完成校验与迁移，成功后才落设置：
		// 否则失败时会留下「设置写着新目录、实际仍写在旧目录」的不一致
		await setAudioCacheDir(cacheDir);
		const store = await getSettingsStore();
		set({ audio: { ...get().audio, cacheDir } });
		await store.set("audio", get().audio);
		await store.save();
	},

	resetCacheDir: async () => {
		await resetAudioCacheDir();
		const store = await getSettingsStore();
		set({ audio: { ...get().audio, cacheDir: "" } });
		await store.set("audio", get().audio);
		await store.save();
	},

	upsertOutputDeviceProfiles: async (devices) => {
		const store = await getSettingsStore();
		const now = Date.now();
		set((state) => {
			const outputDeviceProfiles = {
				...state.audio.outputDeviceProfiles,
			};
			for (const device of devices) {
				if (!device.id) continue;
				const previous = outputDeviceProfiles[device.id];
				outputDeviceProfiles[device.id] = {
					id: device.id,
					displayName: previous?.displayName,
					iconKey: previous?.iconKey,
					lastKnownName:
						device.name ||
						previous?.lastKnownName ||
						getFallbackDeviceName(device.id),
					firstSeenAt: previous?.firstSeenAt ?? now,
					lastSeenAt: now,
				};
			}

			return {
				audio: {
					...state.audio,
					outputDeviceProfiles,
				},
			};
		});
		await store.set("audio", get().audio);
		await store.save();
	},

	updateOutputDeviceProfile: async (id, patch) => {
		if (!id) return;

		const store = await getSettingsStore();
		const now = Date.now();
		set((state) => {
			const previous = state.audio.outputDeviceProfiles[id];
			const nextProfile: OutputDeviceProfile = {
				...previous,
				id,
				lastKnownName: previous?.lastKnownName ?? getFallbackDeviceName(id),
				firstSeenAt: previous?.firstSeenAt ?? now,
				lastSeenAt: previous?.lastSeenAt ?? now,
			};

			if ("displayName" in patch) {
				nextProfile.displayName = normalizeOptionalString(patch.displayName);
			}
			if ("iconKey" in patch) {
				nextProfile.iconKey = normalizeOptionalString(patch.iconKey);
			}
			if (!nextProfile.displayName) delete nextProfile.displayName;
			if (!nextProfile.iconKey) delete nextProfile.iconKey;

			return {
				audio: {
					...state.audio,
					outputDeviceProfiles: {
						...state.audio.outputDeviceProfiles,
						[id]: nextProfile,
					},
				},
			};
		});
		await store.set("audio", get().audio);
		await store.save();
	},

	setApiBaseUrl: async (url) => {
		const apiBaseUrl = normalizeApiBaseUrl(url);
		const store = await getSettingsStore();
		set((state) => ({
			network: {
				...state.network,
				apiBaseUrl,
			},
		}));
		await store.set("network", get().network);
		await store.save();
		applyApiBaseUrl(apiBaseUrl);
	},

	setUnblockEnabled: async (enabled) => {
		const store = await getSettingsStore();
		set((state) => ({
			network: {
				...state.network,
				unblockEnabled: enabled,
			},
		}));
		await store.set("network", get().network);
		await store.save();
	},

	setUnblockSource: async (source) => {
		const store = await getSettingsStore();
		set((state) => ({
			network: {
				...state.network,
				// "auto" 为旧版哨兵值，归一为空字符串（自动模式）
				unblockSource:
					source.trim() === UNBLOCK_SOURCE_AUTO ? "" : source.trim(),
			},
		}));
		await store.set("network", get().network);
		await store.save();
	},

	setUnblockEnableFlac: async (enabled) => {
		const store = await getSettingsStore();
		set((state) => ({
			network: { ...state.network, unblockEnableFlac: enabled },
		}));
		await store.set("network", get().network);
		await store.save();
	},

	setUnblockOrderFirst: async (orderFirst) => {
		const store = await getSettingsStore();
		set((state) => ({
			network: { ...state.network, unblockOrderFirst: orderFirst },
		}));
		await store.set("network", get().network);
		await store.save();
	},

	setUnblockQqCookie: async (cookie) => {
		const store = await getSettingsStore();
		set((state) => ({
			network: { ...state.network, unblockQqCookie: cookie.trim() },
		}));
		await store.set("network", get().network);
		await store.save();
	},

	setUnblockJooxCookie: async (cookie) => {
		const store = await getSettingsStore();
		set((state) => ({
			network: { ...state.network, unblockJooxCookie: cookie.trim() },
		}));
		await store.set("network", get().network);
		await store.save();
	},

	setUnblockSettingsVisible: async (visible) => {
		const store = await getSettingsStore();
		set((state) => ({
			network: { ...state.network, unblockSettingsVisible: visible },
		}));
		await store.set("network", get().network);
		await store.save();
	},

	setDebugSettingsVisible: async (visible) => {
		const store = await getSettingsStore();
		set((state) => ({ debug: { ...state.debug, visible } }));
		await store.set("debug", get().debug);
		await store.save();
	},

	setDebugMode: async (enabled) => {
		const store = await getSettingsStore();
		set((state) => ({ debug: { ...state.debug, enabled } }));
		await store.set("debug", get().debug);
		await store.save();
	},

	setRememberWindowBounds: async (enabled) => {
		const store = await getSettingsStore();
		set((state) => ({
			window: {
				...state.window,
				rememberWindowBounds: enabled,
			},
		}));
		await store.set("window", get().window);
		await store.save();
		setWindowBoundsTracking(enabled);
		if (enabled) {
			await captureCurrentBounds();
		}
	},

	setMinWindowSize: async (width, height) => {
		const store = await getSettingsStore();
		set((state) => ({
			window: {
				...state.window,
				minWindowWidth: clampNumber(
					width,
					MIN_WINDOW_WIDTH_LIMITS.min,
					MIN_WINDOW_WIDTH_LIMITS.max,
					state.window.minWindowWidth,
				),
				minWindowHeight: clampNumber(
					height,
					MIN_WINDOW_HEIGHT_LIMITS.min,
					MIN_WINDOW_HEIGHT_LIMITS.max,
					state.window.minWindowHeight,
				),
			},
		}));
		await store.set("window", get().window);
		await store.save();
		const { minWindowWidth, minWindowHeight } = get().window;
		await applyMinWindowSize(minWindowWidth, minWindowHeight);
	},

	setSidebarEntryHidden: async (entry, hidden) => {
		if (!isSidebarEntryKey(entry)) return;
		const store = await getSettingsStore();
		set((state) => {
			const hiddenEntries = hidden
				? Array.from(new Set([...state.sidebar.hiddenEntries, entry]))
				: state.sidebar.hiddenEntries.filter((key) => key !== entry);
			return { sidebar: { hiddenEntries } };
		});
		await store.set("sidebar", get().sidebar);
		await store.save();
	},

	setShortcutBinding: async (action, binding) => {
		const store = await getSettingsStore();
		set((state) => ({
			shortcuts: {
				...state.shortcuts,
				bindings: {
					...state.shortcuts.bindings,
					[action]: binding,
				},
			},
		}));
		await store.set("shortcuts", get().shortcuts);
		await store.save();
	},

	resetShortcuts: async () => {
		const store = await getSettingsStore();
		set({ shortcuts: defaultShortcutSettings });
		await store.set("shortcuts", defaultShortcutSettings);
		await store.save();
	},

	updateAudioEngine: async (patch, options) => {
		const store = await getSettingsStore();
		set({ audio: { ...get().audio, ...patch } });
		await store.set("audio", get().audio);
		await store.save();
		await applyAudioEngine(get().audio, options?.throwOnApplyError);
	},
}));

function normalizeLyricSettings(saved?: Partial<LyricSettings>): LyricSettings {
	return {
		...defaultLyricSettings,
		...saved,
		blurEnabled: saved?.blurEnabled ?? true,
		autoLoadLocalLyric: saved?.autoLoadLocalLyric ?? true,
		// 动效风格只认枚举值，未知取值（旧版残留/手改配置）回落默认动效
		animationStyle: saved?.animationStyle === "stroke" ? "stroke" : "default",
	};
}

function normalizeDesktopLyricSettings(
	saved?: Partial<DesktopLyricSettings>,
): DesktopLyricSettings {
	return {
		...defaultDesktopLyricSettings,
		enabled: saved?.enabled ?? false,
		// 方向/动效只认枚举值，未知取值（旧版残留/手改配置）回落默认
		direction: saved?.direction === "vertical" ? "vertical" : "horizontal",
		animationStyle: saved?.animationStyle === "stroke" ? "stroke" : "default",
		fontSize: clampNumber(
			saved?.fontSize,
			DESKTOP_LYRIC_FONT_LIMITS.min,
			DESKTOP_LYRIC_FONT_LIMITS.max,
			defaultDesktopLyricSettings.fontSize,
		),
		// 旧版本没有 colors 字段：规范化后即出厂配色，外观与升级前一致
		colors: normalizeDesktopLyricColors(saved?.colors),
	};
}

function normalizeAudioSettings(
	savedAudio?: Partial<AudioSettings>,
	fallbackAudio: AudioSettings = defaultAudioSettings,
): AudioSettings {
	const audio = savedAudio
		? { ...defaultAudioSettings, ...savedAudio }
		: fallbackAudio;
	return {
		...audio,
		// 脏值（手改配置 / 旧版残留）一律落回空串＝系统默认目录
		cacheDir: typeof audio.cacheDir === "string" ? audio.cacheDir.trim() : "",
		outputDeviceProfiles: normalizeOutputDeviceProfiles(
			audio.outputDeviceProfiles,
		),
	};
}

function normalizeNetworkSettings(
	saved?: Partial<NetworkSettings>,
): NetworkSettings {
	const savedSource =
		typeof saved?.unblockSource === "string" ? saved.unblockSource : "";
	return {
		...defaultNetworkSettings,
		...saved,
		apiBaseUrl: normalizeApiBaseUrl(saved?.apiBaseUrl ?? ""),
		unblockEnabled: saved?.unblockEnabled ?? true,
		// 旧版 "auto" 哨兵值与空串都表示自动模式
		unblockSource:
			savedSource.trim() === UNBLOCK_SOURCE_AUTO ? "" : savedSource.trim(),
		unblockEnableFlac: saved?.unblockEnableFlac ?? true,
		unblockOrderFirst: saved?.unblockOrderFirst ?? true,
		unblockQqCookie:
			typeof saved?.unblockQqCookie === "string" ? saved.unblockQqCookie : "",
		unblockJooxCookie:
			typeof saved?.unblockJooxCookie === "string"
				? saved.unblockJooxCookie
				: "",
		unblockSettingsVisible: saved?.unblockSettingsVisible ?? false,
	};
}

function normalizeDebugSettings(saved?: Partial<DebugSettings>): DebugSettings {
	return {
		...defaultDebugSettings,
		visible: saved?.visible ?? false,
		enabled: saved?.enabled ?? false,
	};
}

function normalizeWindowSettings(
	saved?: Partial<WindowSettings>,
): WindowSettings {
	return {
		...defaultWindowSettings,
		...saved,
		minWindowWidth: clampNumber(
			saved?.minWindowWidth,
			MIN_WINDOW_WIDTH_LIMITS.min,
			MIN_WINDOW_WIDTH_LIMITS.max,
			defaultWindowSettings.minWindowWidth,
		),
		minWindowHeight: clampNumber(
			saved?.minWindowHeight,
			MIN_WINDOW_HEIGHT_LIMITS.min,
			MIN_WINDOW_HEIGHT_LIMITS.max,
			defaultWindowSettings.minWindowHeight,
		),
	};
}

function normalizeSidebarSettings(
	saved?: Partial<SidebarSettings>,
): SidebarSettings {
	const hiddenEntries = Array.isArray(saved?.hiddenEntries)
		? saved.hiddenEntries.filter(isSidebarEntryKey)
		: [];
	return { hiddenEntries };
}

function isSidebarEntryKey(value: unknown): value is SidebarEntryKey {
	return SIDEBAR_ENTRY_KEYS.includes(value as SidebarEntryKey);
}

function normalizeShortcutSettings(
	saved?: Partial<ShortcutSettings>,
): ShortcutSettings {
	return {
		bindings: {
			...defaultShortcutBindings,
			...(saved?.bindings ?? {}),
		},
	};
}

function normalizeFontSettings(font?: Partial<FontSettings>): FontSettings {
	return {
		...defaultAppearanceSettings.font,
		...font,
		lyricFontWeight: clampNumber(font?.lyricFontWeight, 100, 900, 400),
		lyricFontSize: clampNumber(font?.lyricFontSize, 16, 72, 46),
	};
}

function clampNumber(
	value: number | undefined,
	min: number,
	max: number,
	fallback: number,
) {
	return Number.isFinite(value)
		? Math.min(max, Math.max(min, Math.round(value as number)))
		: fallback;
}

function normalizeOutputDeviceProfiles(
	profiles?: OutputDeviceProfiles,
): OutputDeviceProfiles {
	return {
		...defaultOutputDeviceProfiles,
		...(profiles ?? {}),
	};
}

function getFallbackDeviceName(id: string) {
	return id === DEFAULT_AUDIO_OUTPUT_DEVICE_ID
		? DEFAULT_AUDIO_OUTPUT_DEVICE_NAME
		: id;
}

function normalizeOptionalString(value: string | undefined) {
	const trimmed = value?.trim();
	return trimmed ? trimmed : undefined;
}

async function broadcastAppearance(appearance: AppearanceSettings) {
	if (!isTauri()) return;

	try {
		const { emit } = await import("@tauri-apps/api/event");
		await emit(APPEARANCE_SYNC_EVENT, appearance);
	} catch (error) {
		console.error("Failed to broadcast appearance settings", error);
	}
}

/** 把 "system" 解析成实际生效的明暗。DOM 类名与窗口主题必须共用同一判定，
 *  否则会出现"页面是深色、窗口材质是浅色"的错配。 */
function resolveTheme(theme: AppearanceSettings["theme"]): "light" | "dark" {
	if (theme !== "system") return theme;
	return window.matchMedia("(prefers-color-scheme: dark)").matches
		? "dark"
		: "light";
}

/**
 * 应用用户自定义主题色。
 *
 * 只覆写 `--primary`（写到 <html> 的行内样式，因此深浅两套内置值都会被盖住，
 * 明度必须由 buildAccentPrimary 按当前解析出的明暗重新给档）。
 * `--primary-foreground` 沿用内置两档：浅色主题恒为中等偏暗的 --primary、
 * 深色恒为偏亮的 --primary，白色 / 近黑文字两种情况下都成立。
 * `--ring`、`--accent`、`--sidebar-primary` 刻意保持中性，不跟随主题色。
 * 传 null 或非法颜色时撤销覆写，回落到 index.css 的内置值。
 */
function applyAccentColor(color: string | null, theme: AccentTheme) {
	const root = window.document.documentElement;
	const value = buildAccentPrimary(color, theme);

	if (value) {
		root.style.setProperty("--primary", value);
	} else {
		root.style.removeProperty("--primary");
	}
}

function applyTheme(theme: AppearanceSettings["theme"]) {
	const root = window.document.documentElement;
	root.classList.remove("light", "dark");
	const resolvedTheme = resolveTheme(theme);
	root.classList.add(resolvedTheme);
	// 主题色必须跟在明暗之后落：--primary 的明度按主题取档，
	// 系统主题变化（theme === "system"）时也要跟着重算。
	applyAccentColor(
		useSettingStore.getState().appearance.accentColor,
		resolvedTheme,
	);
}

/**
 * 同步窗口自身的明暗属性。
 *
 * Windows 的 Mica / Acrylic 背景底色跟随**窗口**的深色标志，而不是页面的 CSS
 * 变量——这是两套独立通道。只切 CSS 类名时，窗口材质会一直沿用系统"应用模式"，
 * 于是应用内选深色而系统为浅色时会出现"浅色材质 + 浅色文字"（反向同理），
 * 表现为标题栏与侧栏文字失去对比度。
 *
 * 传 null 让窗口跟随系统，与 theme === "system" 在 CSS 侧的语义严格一致；
 * 三个窗口各自执行一次，因为每个窗口的深色标志是独立的。
 */
async function applyWindowTheme(theme: AppearanceSettings["theme"]) {
	if (!isTauri()) return;
	try {
		await getCurrentWindow().setTheme(theme === "system" ? null : theme);
	} catch (error) {
		console.error("Failed to sync window theme", error);
	}
}

/**
 * 辅助窗口（托盘菜单）可用的材质集合：云母 / 亚克力 / 纯色。
 *
 * 这类窗口自身是 transparent，且没有主窗口那样的内容区兜底。一旦系统特效
 * 被清除（clearEffects），若窗口内没有不透明底色就会整窗透出桌面。
 * 因此"透明"不在集合内，任何不在集合中的取值（含 none 与将来新增的未知值）
 * 统一回落到"纯色"，保证托盘菜单永远不会出现透明显示异常。
 */
const AUX_WINDOW_MATERIALS = new Set<AppearanceSettings["material"]>([
	"mica",
	"acrylic",
	"solid",
]);

/** 需要收窄材质集合的窗口；主窗口与 mini 播放器保留用户原样选择 */
const AUX_WINDOW_LABELS = new Set(["tray-menu"]);

/**
 * Mica 的明暗变体名。
 *
 * `@tauri-apps/api` 的 Effect 枚举没有导出 MicaDark / MicaLight，但 Rust 侧
 * WindowEffect（与 tauri.conf.json 的 effects 同一类型）接受这两个 camelCase 取值。
 * 直传字符串能让 DWM 在同一次调用里"设深色标志 + 重建背景"，不依赖
 * "改完窗口主题后 DWM 会不会自动重绘已有的 Mica 背景"这一未文档化行为。
 * 若该 Tauri 版本不认这两个取值，调用会被拒绝，再退回通用 Mica（见下方）。
 */
const MICA_EFFECT_BY_THEME: Record<"light" | "dark", string> = {
	light: "micaLight",
	dark: "micaDark",
};

function resolveWindowMaterial(
	material: AppearanceSettings["material"],
): AppearanceSettings["material"] {
	if (!isTauri()) return material;
	if (!AUX_WINDOW_LABELS.has(getCurrentWindow().label)) return material;
	return AUX_WINDOW_MATERIALS.has(material) ? material : "solid";
}

/**
 * 材质应用串行队列。
 *
 * 用户在设置里连点材质时，多次 applyMaterial 会并发执行，可能出现
 * A.clearEffects → B.clearEffects → B.setEffects → A.setEffects 的交错，
 * 最终系统特效与 data-material 分属两次调用结果——正是本模块最需要避免的
 * "效果与样式分支不一致"。串行化后每一步都建立在上一步的终态之上。
 */
let materialQueue: Promise<void> = Promise.resolve();

function applyMaterial(
	material: AppearanceSettings["material"],
): Promise<void> {
	materialQueue = materialQueue
		.then(() => applyMaterialToWindow(material))
		// 队列自身永不 reject：否则一次失败会卡死后续所有材质切换
		.catch(() => {});
	return materialQueue;
}

async function applyMaterialToWindow(material: AppearanceSettings["material"]) {
	if (!isTauri()) return;
	// 桌面歌词窗口：浮在任意壁纸上的纯文字层，背景材质必须与主题/用户
	// 材质设置完全解耦——既不下系统特效（mica/acrylic 会整窗着色且随
	// 主题翻转变体），也不写 data-material（避免 :root[data-material]
	// 分支的变量侧染色）。窗口本身 transparent + 无特效，恒定全透明，
	// 可读性由歌词文字的暗色描边背衬层保证。
	if (getCurrentWindow().label === "desktop-lyric") return;
	// 按窗口收窄后再应用：效果与 data-material 必须用同一个解析结果，
	// 否则样式分支会与实际系统特效不一致
	let resolved = resolveWindowMaterial(material);
	const appWindow = getCurrentWindow();
	try {
		await appWindow.clearEffects();
		if (resolved === "mica") {
			// 明暗由应用主题决定：Windows 默认按系统"应用模式"渲染 Mica 底色，
			// 只有显式指定变体（或同步窗口深色标志）才会跟应用主题一致。
			const theme = resolveTheme(useSettingStore.getState().appearance.theme);
			try {
				await appWindow.setEffects({
					effects: [MICA_EFFECT_BY_THEME[theme] as Effect],
				});
			} catch (error) {
				// 该 Tauri 版本不认明暗变体时退回通用 Mica（明暗交给窗口主题）
				console.error(
					"Mica theme variant rejected, falling back to Mica",
					error,
				);
				await appWindow.setEffects({ effects: [Effect.Mica] });
			}
		} else if (resolved === "acrylic") {
			// Acrylic 与 Mica 不同：Tauri / window-vibrancy 各层都没有明暗变体
			// （JS 的 Effect 枚举同样没有），明暗完全依赖窗口深色标志——
			// applyWindowTheme 必须先于本调用执行（initSettings 与主题变化
			// 的 subscribe 均已保证），且主题切换后必须重下材质，DWM 才会按
			// 新标志重绘背景。Win11 22H2+（build ≥ 22523）走 DWM 系统亚克力，
			// tint/模糊/噪点由系统按深色标志决定，setEffects 的 color 参数在
			// 该路径上被忽略；更老的构建走 SWCA，系统基本不给 tint。
			await appWindow.setEffects({ effects: [Effect.Acrylic] });
		}
		// "solid" / "none"：仅清除系统特效，背景由 CSS data-material 分支提供
	} catch (error) {
		// 系统特效不可用（Win10 / 虚拟机 / 远程桌面 / 系统策略限制）时不能沿用
		// 半透明分支：clearEffects 已经清掉旧特效，而 mica 分支的 --background
		// 接近全透明，继续按 mica 写 data-material 会让整窗透出桌面。
		// 退化为纯色是这里唯一既保底又可读的取值。
		console.error("Failed to set window effects, falling back to solid", error);
		resolved = "solid";
	}

	document.documentElement.dataset.material = resolved;
}

async function applyInterfaceFont(fontStr: string) {
	const root = document.documentElement;
	if (fontStr) {
		root.style.setProperty("--app-font-family", fontStr);
	} else {
		root.style.removeProperty("--app-font-family");
	}
}

function applyLyricFont(fontStr: string) {
	const root = document.documentElement;
	if (fontStr) {
		root.style.setProperty("--app-lyric-font-family", fontStr);
	} else {
		root.style.removeProperty("--app-lyric-font-family");
	}
}

function applyLyricTypography(font: FontSettings) {
	const root = document.documentElement;
	root.style.setProperty(
		"--app-lyric-font-weight",
		String(font.lyricFontWeight),
	);
	root.style.setProperty("--app-lyric-font-size", `${font.lyricFontSize}px`);
}

/**
 * 启动时把用户自定义缓存目录下发给原生侧。
 *
 * 设为空串（默认）时不发命令：覆盖根目录是进程级全局量，每次启动都从
 * `None`（默认位置）开始，无需再「恢复」；只有非空时才需要切过去。
 */
async function applyCacheDir(cacheDir: string) {
	if (!isTauri() || !cacheDir.trim()) return;

	try {
		await setAudioCacheDir(cacheDir.trim());
	} catch (error) {
		console.error("Failed to apply audio cache dir", error);
	}
}

async function applyAudioCacheLimit(maxCacheSize: number) {
	if (!isTauri()) return;

	const normalizedSize = Number.isFinite(maxCacheSize)
		? Math.max(0, Math.round(maxCacheSize * 2) / 2)
		: 0;
	const maxBytes = Math.round(normalizedSize * BYTES_PER_GIB);
	await invoke("set_audio_cache_limit", { maxBytes });
}

async function applyAudioEngine(audio: AudioSettings, throwOnError = false) {
	if (!isTauri()) return;

	try {
		await Promise.all([
			corePlayer.setOutputDevice(audio.outputDeviceId),
			corePlayer.setReplayGain(
				audio.replayGainEnabled,
				audio.replayGainPreampDb,
			),
			corePlayer.setEqualizer(audio.equalizerEnabled, audio.equalizerGainsDb),
			corePlayer.setCrossfade(audio.crossfadeDuration),
		]);
	} catch (error) {
		console.error("Failed to apply native audio engine settings", error);
		if (throwOnError) throw error;
	}
}

export async function initSettings() {
	await useSettingStore.getState().loadSettings();

	const {
		appearance,
		audio,
		network,
		window: windowSettings,
	} = useSettingStore.getState();
	applyApiBaseUrl(network.apiBaseUrl);
	applyTheme(appearance.theme);
	// 先定窗口明暗再上材质：让 DWM 用正确的深色标志渲染第一帧背景材质，
	// 否则首帧会以系统默认明暗渲染，随后再跳变
	await applyWindowTheme(appearance.theme);
	await applyMaterial(appearance.material);
	applyInterfaceFont(appearance.font.interfaceFontStr);
	applyLyricFont(appearance.font.lyricFontStr);
	applyLyricTypography(appearance.font);
	applyAudioEngine(audio);
	// 缓存目录必须先于上限下发：上限生效时会按目录扫描淘汰，目录没切过去就会扫错地方
	await applyCacheDir(audio.cacheDir);
	applyAudioCacheLimit(audio.maxCacheSize).catch((error) => {
		console.error("Failed to apply audio cache limit", error);
	});

	// 主窗口：恢复最小尺寸与记忆的 bounds（在窗口显示前完成，避免闪烁）
	if (isTauri() && getCurrentWindow().label === "main") {
		await initMainWindowState({
			rememberBounds: windowSettings.rememberWindowBounds,
			minWidth: windowSettings.minWindowWidth,
			minHeight: windowSettings.minWindowHeight,
		});
	}

	const systemThemeMedia = window.matchMedia("(prefers-color-scheme: dark)");
	const handleSystemThemeChange = () => {
		const { appearance } = useSettingStore.getState();
		if (appearance.theme !== "system") return;
		applyTheme("system");
		// Mica 的明暗变体由解析后的明暗决定，系统切换明暗后必须重下材质才会跟着换
		void applyMaterial(appearance.material);
	};
	systemThemeMedia.addEventListener("change", handleSystemThemeChange);

	let unlistenAppearanceSync: (() => void) | undefined;
	if (isTauri()) {
		const { listen } = await import("@tauri-apps/api/event");
		unlistenAppearanceSync = await listen<AppearanceSettings>(
			APPEARANCE_SYNC_EVENT,
			(event) => {
				useSettingStore.setState((state) => ({
					appearance: {
						...state.appearance,
						...event.payload,
						font: {
							...state.appearance.font,
							...event.payload.font,
						},
					},
				}));
			},
		);
	}

	useSettingStore.subscribe((state, prevState) => {
		if (state.appearance.theme !== prevState.appearance.theme) {
			applyTheme(state.appearance.theme);
			// 窗口主题变了必须把材质一起重下一次：DWM 不会因为深色标志变化就重绘
			// 已存在的 Mica / Acrylic 背景，只调 setTheme 的话浅色主题会一直显示
			// 深色材质（反向同理）。重新 clearEffects + setEffects 会强制重建背景，
			// 这一步才会读到新的窗口明暗。
			void applyWindowTheme(state.appearance.theme).then(() =>
				applyMaterial(state.appearance.material),
			);
		}
		if (state.appearance.material !== prevState.appearance.material) {
			void applyMaterial(state.appearance.material);
		}
		// 主题色单独变化时不必重下材质；跨窗口同步（APPEARANCE_SYNC_EVENT）
		// 也是走这条分支落到各自窗口的
		if (state.appearance.accentColor !== prevState.appearance.accentColor) {
			applyAccentColor(
				state.appearance.accentColor,
				resolveTheme(state.appearance.theme),
			);
		}
		if (
			state.appearance.font.interfaceFontStr !==
			prevState.appearance.font.interfaceFontStr
		) {
			applyInterfaceFont(state.appearance.font.interfaceFontStr);
		}
		if (
			state.appearance.font.lyricFontStr !==
			prevState.appearance.font.lyricFontStr
		) {
			applyLyricFont(state.appearance.font.lyricFontStr);
		}
		if (
			state.appearance.font.lyricFontWeight !==
				prevState.appearance.font.lyricFontWeight ||
			state.appearance.font.lyricFontSize !==
				prevState.appearance.font.lyricFontSize
		) {
			applyLyricTypography(state.appearance.font);
		}
	});

	return () => {
		systemThemeMedia.removeEventListener("change", handleSystemThemeChange);
		unlistenAppearanceSync?.();
	};
}
