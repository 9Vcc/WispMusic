import { TextQuote20Regular } from "@fluentui/react-icons";
import SettingsExpandar, {
	SettingsExpandarDetail,
} from "@/components/settings/SettingsExpandar";
import { Switch } from "@/components/ui/switch";
import { useSettingStore } from "@/lib/store/settingStore/settingStore";

export function LyricSettingCard() {
	const lyricBlurEnabled = useSettingStore((s) => s.lyric.blurEnabled);
	const setLyricBlurEnabled = useSettingStore((s) => s.setLyricBlurEnabled);
	const autoLoadLocalLyric = useSettingStore((s) => s.lyric.autoLoadLocalLyric);
	const setAutoLoadLocalLyric = useSettingStore((s) => s.setAutoLoadLocalLyric);

	return (
		<SettingsExpandar
			icon={<TextQuote20Regular />}
			title="歌词"
			subtitle="歌词页默认展开歌词，非当前行是否模糊显示，本地歌词是否自动匹配"
		>
			<SettingsExpandarDetail desc="非当前行模糊显示">
				<Switch
					checked={lyricBlurEnabled}
					onCheckedChange={(checked) => setLyricBlurEnabled(checked)}
				/>
			</SettingsExpandarDetail>
			<SettingsExpandarDetail desc="自动匹配同目录同名歌词">
				<Switch
					checked={autoLoadLocalLyric}
					onCheckedChange={(checked) => setAutoLoadLocalLyric(checked)}
				/>
			</SettingsExpandarDetail>
		</SettingsExpandar>
	);
}
