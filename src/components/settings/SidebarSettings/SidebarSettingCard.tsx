import { Navigation20Regular } from "@fluentui/react-icons";
import SettingsExpandar, {
	SettingsExpandarDetail,
} from "@/components/settings/SettingsExpandar";
import { Switch } from "@/components/ui/switch";
import {
	SIDEBAR_ENTRY_KEYS,
	useSettingStore,
	type SidebarEntryKey,
} from "@/lib/store/settingStore/settingStore";

// 入口 key 到展示名的映射（主页与设置为固定入口，不可隐藏）
const SIDEBAR_ENTRY_LABELS: Record<SidebarEntryKey, string> = {
	explore: "探索",
	recent: "最近播放",
	download: "下载",
	local: "本地歌曲",
	cloud: "网盘",
	favorite: "喜爱歌曲",
	playlists: "歌单",
};

export function SidebarSettingCard() {
	const hiddenEntries = useSettingStore((s) => s.sidebar.hiddenEntries);
	const setSidebarEntryHidden = useSettingStore(
		(s) => s.setSidebarEntryHidden,
	);

	return (
		<SettingsExpandar
			icon={<Navigation20Regular />}
			title="功能侧栏"
			subtitle="选择侧栏中显示的功能入口（主页与设置为固定入口）"
		>
			{SIDEBAR_ENTRY_KEYS.map((key) => (
				<SettingsExpandarDetail
					key={key}
					desc={SIDEBAR_ENTRY_LABELS[key]}
				>
					<Switch
						checked={!hiddenEntries.includes(key)}
						onCheckedChange={(checked) =>
							setSidebarEntryHidden(key, !checked)
						}
					/>
				</SettingsExpandarDetail>
			))}
		</SettingsExpandar>
	);
}
