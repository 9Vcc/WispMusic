import { useRef } from "react";
import { toast } from "sonner";
import { AboutSettingCard } from "@/components/settings/About/AboutSettingCard";
import { DebugSettingCard } from "@/components/settings/About/DebugSettingCard";
import { UpdateSettingCard } from "@/components/settings/About/UpdateSettingCard";
import { AccentColorSettingCard } from "@/components/settings/AppearanceSettings/AccentColorSettingCard";
import { AppearanceSettingCard } from "@/components/settings/AppearanceSettings/AppearanceSettingCard";
import { FontSettingCard } from "@/components/settings/AppearanceSettings/FontSettingCard";
import { AudioCacheCard } from "@/components/settings/AudioSettings/AudioCacheCard";
import { AudioEngineCard } from "@/components/settings/AudioSettings/AudioEngineCard";
import { AudioSettingCard } from "@/components/settings/AudioSettings/AudioSettingCard";
import { DownloadSettingCard } from "@/components/settings/AudioSettings/DownloadSettignCard";
import { MusicFolderSettingCard } from "@/components/settings/AudioSettings/MusicFolderSettingCard";
import { LyricSettingCard } from "@/components/settings/LyricSettings/LyricSettingCard";
import { ApiSettingCard } from "@/components/settings/NetworkSettings/ApiSettingCard";
import { UnblockSettingCard } from "@/components/settings/NetworkSettings/UnblockSettingCard";
import { ShortcutSettingCard } from "@/components/settings/ShortcutSettings/ShortcutSettingCard";
import { SidebarSettingCard } from "@/components/settings/SidebarSettings/SidebarSettingCard";
import { WindowSettingCard } from "@/components/settings/WindowSettings/WindowSettingCard";
import { useSettingStore } from "@/lib/store/settingStore/settingStore";

/** 连续点击「关于」解锁调试模式所需的次数 */
const DEBUG_UNLOCK_CLICK_COUNT = 5;
/** 连续点击判定的时间窗口（毫秒） */
const DEBUG_UNLOCK_WINDOW_MS = 2000;

export default function SettingPage() {
	// 解灰设置为隐藏功能：在「关于」分组连点 5 次版本号解锁后才会显示
	const unblockSettingsVisible = useSettingStore(
		(s) => s.network.unblockSettingsVisible,
	);

	// 调试模式为隐藏功能：在「关于」分组标题上 2 秒内连点 5 次解锁（持久化）
	const debugSettingsVisible = useSettingStore((s) => s.debug.visible);
	const setDebugSettingsVisible = useSettingStore(
		(s) => s.setDebugSettingsVisible,
	);
	const aboutClickTimestamps = useRef<number[]>([]);

	const handleAboutTitleClick = () => {
		if (debugSettingsVisible) return;
		const now = Date.now();
		aboutClickTimestamps.current = aboutClickTimestamps.current.filter(
			(t) => now - t < DEBUG_UNLOCK_WINDOW_MS,
		);
		aboutClickTimestamps.current.push(now);
		if (aboutClickTimestamps.current.length >= DEBUG_UNLOCK_CLICK_COUNT) {
			aboutClickTimestamps.current = [];
			void setDebugSettingsVisible(true);
			toast.success("已解锁调试模式", { duration: 3000 });
		}
	};

	return (
		<div className="flex h-full w-full flex-col gap-8 px-8 py-8">
			<div className="flex h-full w-full flex-col gap-3">
				<h2 className="text-sm font-medium text-foreground/88">音频与下载</h2>

				<div className="flex flex-col gap-2">
					<AudioSettingCard />
					<AudioEngineCard />
					<MusicFolderSettingCard />
					<DownloadSettingCard />
					<AudioCacheCard />
				</div>
			</div>

			<div className="flex h-full w-full flex-col gap-3">
				<h2 className="text-sm font-medium text-foreground/88">个性化</h2>

				<div className="flex flex-col gap-2">
					<AppearanceSettingCard />
					<AccentColorSettingCard />
					<FontSettingCard />
					<LyricSettingCard />
					<SidebarSettingCard />
					<WindowSettingCard />
				</div>
			</div>

			<div className="flex h-full w-full flex-col gap-3">
				<h2 className="text-sm font-medium text-foreground/88">快捷键</h2>

				<div className="flex flex-col gap-2">
					<ShortcutSettingCard />
				</div>
			</div>

			<div className="flex h-full w-full flex-col gap-3">
				<h2 className="text-sm font-medium text-foreground/88">网络</h2>
				<div className="flex flex-col gap-2">
					<ApiSettingCard />
					{unblockSettingsVisible && <UnblockSettingCard />}
				</div>
			</div>

			<div className="flex h-full w-full flex-col gap-3">
				{/* biome-ignore lint/a11y/useKeyWithClickEvents: 隐藏彩蛋入口，刻意仅限鼠标连点解锁（同版本号彩蛋模式） */}
				<h2
					className="cursor-pointer text-sm font-medium text-foreground/88 select-none"
					onClick={handleAboutTitleClick}
				>
					关于
				</h2>
				<div className="flex flex-col gap-2">
					<AboutSettingCard />
					<UpdateSettingCard />
					{debugSettingsVisible && <DebugSettingCard />}
				</div>
			</div>
		</div>
	);
}
