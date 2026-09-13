import type { Window } from "@tauri-apps/api/window";
import { useReducedMotion } from "framer-motion";
import { useEffect, useState } from "react";
import { runFullscreenTransition } from "@/lib/window/fullscreenTransition";

let wasMaximizedBeforeFullscreen = false;

function wait(ms: number): Promise<void> {
	return new Promise((resolve) => {
		window.setTimeout(resolve, ms);
	});
}

/** 等 n 帧：窗口尺寸由系统异步应用，需等合成器拿到新尺寸再揭开遮罩 */
function nextFrames(count: number): Promise<void> {
	return new Promise((resolve) => {
		const step = (remaining: number) => {
			if (remaining <= 0) {
				resolve();
				return;
			}
			requestAnimationFrame(() => step(remaining - 1));
		};
		step(count);
	});
}

export function useAppWindow() {
	const reduceMotion = Boolean(useReducedMotion());
	const [appWindow, setAppWindow] = useState<Window | null>(null);
	const [isMaximized, setIsMaximized] = useState(false);
	const [isFullscreen, setIsFullscreen] = useState(false);

	useEffect(() => {
		let unlistenResize: (() => void) | null = null;

		import("@tauri-apps/api/window").then(async (module) => {
			const window = module.getCurrentWindow();
			setAppWindow(window);

			const _isMaximized = await window.isMaximized();
			setIsMaximized(_isMaximized);

			const _isFullscreen = await window.isFullscreen();
			setIsFullscreen(_isFullscreen);

			const unlisten = await window.onResized(async () => {
				const curMaximized = await window.isMaximized();
				setIsMaximized(curMaximized);
				const curFullscreen = await window.isFullscreen();
				setIsFullscreen(curFullscreen);
			});

			unlistenResize = unlisten;
		});

		return () => {
			if (unlistenResize) unlistenResize();
		};
	}, []);

	const toogleMaximize = async () => {
		if (appWindow) {
			await appWindow.toggleMaximize();
		}
	};

	// 全屏切换统一走过渡编排（src/lib/window/fullscreenTransition.ts）：
	// 先用与当前界面同色的遮罩盖住画面，在被遮住的瞬间执行系统级窗口切换，
	// 再让遮罩淡出露出新布局 —— 把无法补间的硬切换藏起来。
	const toggleFullscreen = async () => {
		if (!appWindow) return;

		const entering = !(await appWindow.isFullscreen());

		await runFullscreenTransition({
			reduceMotion,
			swap: async () => {
				if (entering) {
					const isMax = await appWindow.isMaximized();
					if (isMax) {
						wasMaximizedBeforeFullscreen = true;
						await appWindow.unmaximize();
						// Give the OS a tiny fraction of time to process the unmaximize
						// before applying fullscreen, to reliably bypass the Windows 11 taskbar bug.
						await wait(100);
					} else {
						wasMaximizedBeforeFullscreen = false;
					}
					await appWindow.setFullscreen(true);
				} else {
					await appWindow.setFullscreen(false);
					if (wasMaximizedBeforeFullscreen) {
						await wait(100);
						await appWindow.maximize();
						wasMaximizedBeforeFullscreen = false;
					}
				}

				// 尺寸变化是异步生效的，等两帧再让遮罩揭开，
				// 否则可能先露出旧尺寸的画面再跳到新尺寸
				await nextFrames(2);
			},
		});
	};

	const minimize = () => appWindow?.minimize();
	const maximize = () => appWindow?.maximize();
	const close = () => appWindow?.close();
	const startDragging = () => appWindow?.startDragging();

	return {
		appWindow,
		isMaximized,
		isFullscreen,
		toogleMaximize,
		toggleFullscreen,
		minimize,
		maximize,
		close,
		startDragging,
	};
}
