import { useEffect, useState } from "react";
import {
	type AppUpdateInfo,
	checkForAppUpdate,
	dismissUpdateForThisSession,
	isUpdateDismissedForThisSession,
} from "@/lib/services/appUpdate";

/** 启动后延迟检查：避开首帧渲染与启动期网络高峰；异步执行，不阻塞界面加载 */
const STARTUP_CHECK_DELAY_MS = 3000;

// 模块级标记，且 effect 不注册 cleanup：StrictMode 下 effect 会走
// 挂载 → 卸载 → 再挂载 双循环，cleanup 清掉定时器的话检查永远跑不到；
// 这次检查属于"应用会话"而非组件实例，组件意外卸载也不需要中断
let startupCheckScheduled = false;

/**
 * 应用启动时静默检查更新（挂在 RootLayout，仅主窗口执行）。
 *
 * - 发现新版本 → 返回更新信息并打开弹窗；
 * - 已是最新 / 检查失败 / 本次启动已忽略 → 保持 null，全程静默不打扰；
 * - 「忽略本次更新」后弹窗关闭，本次运行内不再提示，重启后恢复检查。
 */
export function useStartupUpdateCheck() {
	const [updateInfo, setUpdateInfo] = useState<AppUpdateInfo | null>(null);
	const [dialogOpen, setDialogOpen] = useState(false);

	useEffect(() => {
		if (startupCheckScheduled) return;
		startupCheckScheduled = true;

		window.setTimeout(() => {
			void checkForAppUpdate().then((info) => {
				if (info && !isUpdateDismissedForThisSession()) {
					setUpdateInfo(info);
					setDialogOpen(true);
				}
			});
		}, STARTUP_CHECK_DELAY_MS);
	}, []);

	// 忽略时保留 updateInfo 不清空，让弹窗播放完整的退出动画
	const dismissUpdate = () => {
		dismissUpdateForThisSession();
		setDialogOpen(false);
	};

	return { updateInfo, dialogOpen, dismissUpdate };
}
