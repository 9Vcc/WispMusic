import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import React from "react";
import ReactDOM from "react-dom/client";
import { SWRConfig } from "swr";
import App from "./App";
import "./index.css";
import { initGlobalShortcuts } from "./lib/shortcuts/global-shortcut-manager";
import { initLocalMusic } from "./lib/store/localMusicStore/localMusicStore";
import { initSettings } from "./lib/store/settingStore/settingStore";
import { createSwrLruCache } from "./lib/utils/swr-cache";

const settingsReady = initSettings();
settingsReady.then(() => initGlobalShortcuts());

if (isTauri() && getCurrentWindow().label === "main") {
	// 本地音乐库可能有数 MB JSON 要过 IPC，推迟到首帧空闲再水合，
	// 且仅主窗口执行，避免 tray-menu 窗口重复加载一遍
	const idle = (cb: () => void) =>
		"requestIdleCallback" in window
			? window.requestIdleCallback(() => cb())
			: setTimeout(cb, 200);
	idle(() => initLocalMusic());

	// 主窗口以 visible: false 启动，待设置（主题/材质）随首帧应用后再显示；
	// 双 rAF 确保首帧已提交，超时兜底保证前端异常时窗口也能出现
	let shown = false;
	const show = () => {
		if (shown) return;
		shown = true;
		const win = getCurrentWindow();
		win.show().then(() => win.setFocus());
	};
	const fallbackTimer = setTimeout(show, 3000);
	settingsReady.finally(() => {
		requestAnimationFrame(() =>
			requestAnimationFrame(() => {
				clearTimeout(fallbackTimer);
				show();
			}),
		);
	});
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
	<React.StrictMode>
		{/* SWR 默认缓存是无上限的 Map，而 playlist-tracks / user-cloud / 探索分页这些
		    key 会装下整张歌单或整个云盘的数据；这里换成带上限的 LRU（见 utils/swr-cache.ts） */}
		<SWRConfig value={{ provider: () => createSwrLruCache() }}>
			<App />
		</SWRConfig>
	</React.StrictMode>,
);
