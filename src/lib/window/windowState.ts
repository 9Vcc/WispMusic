// 主窗口状态：最小尺寸应用 + 大小/位置记忆（bounds 以物理像素存储，跨 DPI 稳定）
import { isTauri } from "@tauri-apps/api/core";
import {
	LogicalSize,
	PhysicalPosition,
	PhysicalSize,
	availableMonitors,
	getCurrentWindow,
} from "@tauri-apps/api/window";
import { getSettingsStore } from "@/lib/store/settingStore/settings.persistence";

const BOUNDS_KEY = "windowBounds";
const SAVE_DEBOUNCE_MS = 500;

export interface WindowBounds {
	x: number;
	y: number;
	width: number;
	height: number;
	maximized: boolean;
}

let trackingEnabled = false;
let saveTimer: ReturnType<typeof setTimeout> | null = null;

async function loadBounds(): Promise<WindowBounds | null> {
	const store = await getSettingsStore();
	const saved = await store.get<WindowBounds>(BOUNDS_KEY);
	if (
		!saved ||
		!Number.isFinite(saved.x) ||
		!Number.isFinite(saved.y) ||
		!Number.isFinite(saved.width) ||
		!Number.isFinite(saved.height) ||
		saved.width <= 0 ||
		saved.height <= 0
	) {
		return null;
	}
	return saved;
}

async function isBoundsVisible(bounds: WindowBounds): Promise<boolean> {
	try {
		const monitors = await availableMonitors();
		return monitors.some((monitor) => {
			const { x: mx, y: my } = monitor.position;
			const mw = monitor.size.width;
			const mh = monitor.size.height;
			return (
				bounds.x < mx + mw &&
				bounds.x + bounds.width > mx &&
				bounds.y < my + mh &&
				bounds.y + bounds.height > my
			);
		});
	} catch {
		return false;
	}
}

async function saveBounds(): Promise<void> {
	saveTimer = null;
	if (!trackingEnabled) return;
	try {
		const win = getCurrentWindow();
		const maximized = await win.isMaximized();
		const store = await getSettingsStore();
		if (maximized) {
			// 最大化期间只更新标志，保留最后一次常规 bounds 作为还原尺寸
			const previous = await loadBounds();
			if (previous) {
				await store.set(BOUNDS_KEY, { ...previous, maximized: true });
				await store.save();
			}
			return;
		}
		const size = await win.innerSize();
		const position = await win.outerPosition();
		await store.set(BOUNDS_KEY, {
			x: position.x,
			y: position.y,
			width: size.width,
			height: size.height,
			maximized: false,
		});
		await store.save();
	} catch (error) {
		console.error("Failed to save window bounds", error);
	}
}

function scheduleSave() {
	if (!trackingEnabled) return;
	if (saveTimer) clearTimeout(saveTimer);
	saveTimer = setTimeout(saveBounds, SAVE_DEBOUNCE_MS);
}

// 应用最小窗口尺寸；若当前窗口小于新最小值则同步放大（部分平台不会自动调整）
export async function applyMinWindowSize(
	width: number,
	height: number,
): Promise<void> {
	if (!isTauri()) return;
	const win = getCurrentWindow();
	if (win.label !== "main") return;

	try {
		await win.setMinSize(new LogicalSize(width, height));
		const size = await win.innerSize();
		const scaleFactor = await win.scaleFactor();
		const logicalWidth = size.width / scaleFactor;
		const logicalHeight = size.height / scaleFactor;
		if (logicalWidth < width || logicalHeight < height) {
			await win.setSize(
				new LogicalSize(
					Math.max(logicalWidth, width),
					Math.max(logicalHeight, height),
				),
			);
		}
	} catch (error) {
		console.error("Failed to apply min window size", error);
	}
}

// 启动时恢复 bounds（需在窗口显示前调用以避免闪烁），并开始跟踪变化
export async function initMainWindowState(options: {
	rememberBounds: boolean;
	minWidth: number;
	minHeight: number;
}): Promise<void> {
	if (!isTauri()) return;
	const win = getCurrentWindow();
	if (win.label !== "main") return;

	await applyMinWindowSize(options.minWidth, options.minHeight);

	if (options.rememberBounds) {
		const bounds = await loadBounds();
		if (bounds) {
			try {
				const scaleFactor = await win.scaleFactor();
				// 恢复尺寸不低于当前配置的最小尺寸（逻辑像素 → 物理像素）
				const minWidth = Math.round(options.minWidth * scaleFactor);
				const minHeight = Math.round(options.minHeight * scaleFactor);
				const width = Math.max(bounds.width, minWidth);
				const height = Math.max(bounds.height, minHeight);
				const visible = await isBoundsVisible({
					...bounds,
					width,
					height,
				});
				await win.setSize(new PhysicalSize(width, height));
				if (visible) {
					await win.setPosition(new PhysicalPosition(bounds.x, bounds.y));
				}
				if (bounds.maximized) {
					await win.maximize();
				}
			} catch (error) {
				console.error("Failed to restore window bounds", error);
			}
		}
	}

	// 监听器始终挂载，由 trackingEnabled 控制是否真正保存，
	// 保证运行时开关记忆功能后仍能跟踪变化
	trackingEnabled = options.rememberBounds;
	try {
		await win.onResized(() => scheduleSave());
		await win.onMoved(() => scheduleSave());
	} catch (error) {
		console.error("Failed to track window bounds", error);
	}
}

export function setWindowBoundsTracking(enabled: boolean): void {
	trackingEnabled = enabled;
	if (!enabled && saveTimer) {
		clearTimeout(saveTimer);
		saveTimer = null;
	}
}

// 开启记忆时立即采集当前 bounds，避免下次启动恢复到过期状态
export async function captureCurrentBounds(): Promise<void> {
	if (!trackingEnabled) return;
	if (saveTimer) {
		clearTimeout(saveTimer);
		saveTimer = null;
	}
	await saveBounds();
}
