import { Bug20Regular } from "@fluentui/react-icons";
import type { Update as TauriUpdate } from "@tauri-apps/plugin-updater";
import { useState } from "react";
import SettingsExpandar, {
	SettingsExpandarDetail,
} from "@/components/settings/SettingsExpandar";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import {
	UpdateDialog,
	type UpdatePhase,
} from "@/components/update/update-dialog";
import type { AppUpdateInfo } from "@/lib/services/appUpdate";
import { useSettingStore } from "@/lib/store/settingStore/settingStore";
import { MemoryReportDialog } from "./MemoryReportDialog";

/** 连续点击「关于」解锁调试模式所需的次数（与版本号彩蛋同规则） */
export const DEBUG_UNLOCK_CLICK_COUNT = 5;
/** 连续点击判定的时间窗口（毫秒） */
export const DEBUG_UNLOCK_WINDOW_MS = 2000;

const PREVIEW_NOTES =
	"1. 修复了若干已知问题\n2. 优化播放体验与稳定性\n3. 调整了部分界面细节\n\n——这是一段用于预览弹窗排版样式的示例更新说明，支持多行文本。";

// 预览模式不会触碰真实 Update 对象（下载走模拟进度），这里仅需满足类型
function createPreviewInfo(): AppUpdateInfo {
	return {
		update: null as unknown as TauriUpdate,
		version: "9.9.9",
		notes: PREVIEW_NOTES,
		currentVersion: "",
	};
}

const PREVIEW_ITEMS: { phase: UpdatePhase; desc: string }[] = [
	{ phase: "idle", desc: "预览更新弹窗 · 初始状态（发现新版本）" },
	{ phase: "downloading", desc: "预览更新弹窗 · 下载中（模拟进度）" },
	{ phase: "restarting", desc: "预览更新弹窗 · 即将重启" },
];

/**
 * 调试模式（隐藏功能）：默认不可见，需在设置页「关于」分组标题上
 * 2 秒内连点 5 次解锁（解锁状态持久化）。开启调试模式后可手动触发
 * 更新弹窗预览，验证初始 / 下载中 / 即将重启三种状态下的显示效果，
 * 以及对 WebView 内存做一次归因采样。
 */
export function DebugSettingCard() {
	const debugMode = useSettingStore((s) => s.debug.enabled);
	const setDebugMode = useSettingStore((s) => s.setDebugMode);
	const [previewOpen, setPreviewOpen] = useState(false);
	const [previewPhase, setPreviewPhase] = useState<UpdatePhase>("idle");
	const [memoryOpen, setMemoryOpen] = useState(false);

	function openPreview(phase: UpdatePhase) {
		setPreviewPhase(phase);
		setPreviewOpen(true);
	}

	return (
		<>
			<SettingsExpandar
				icon={<Bug20Regular />}
				title="调试模式"
				subtitle="开发者选项：手动触发更新弹窗预览，验证各状态下的显示效果"
			>
				<SettingsExpandarDetail desc="启用调试模式">
					<Switch
						checked={debugMode}
						onCheckedChange={(checked) => void setDebugMode(checked)}
					/>
				</SettingsExpandarDetail>
				{debugMode && (
					<>
						{PREVIEW_ITEMS.map((item) => (
							<SettingsExpandarDetail key={item.phase} desc={item.desc}>
								<Button
									variant="winui"
									size="sm"
									onClick={() => openPreview(item.phase)}
								>
									预览
								</Button>
							</SettingsExpandarDetail>
						))}
						<SettingsExpandarDetail desc="内存归因：进程树工作集 + JS 侧逐项占比">
							<Button
								variant="winui"
								size="sm"
								onClick={() => setMemoryOpen(true)}
							>
								采样
							</Button>
						</SettingsExpandarDetail>
					</>
				)}
			</SettingsExpandar>
			<UpdateDialog
				info={createPreviewInfo()}
				open={previewOpen}
				onDismiss={() => setPreviewOpen(false)}
				preview
				initialPhase={previewPhase}
			/>
			<MemoryReportDialog
				open={memoryOpen}
				onClose={() => setMemoryOpen(false)}
			/>
		</>
	);
}
