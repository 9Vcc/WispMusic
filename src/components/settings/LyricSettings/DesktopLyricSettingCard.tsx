import { TabDesktopMultiple20Regular } from "@fluentui/react-icons";
import SettingsExpandar, {
	SettingsExpandarDetail,
} from "@/components/settings/SettingsExpandar";
import { NumberStepper } from "@/components/ui/number-stepper";
import { Switch } from "@/components/ui/switch";
import {
	DESKTOP_LYRIC_FONT_LIMITS,
	useSettingStore,
} from "@/lib/store/settingStore/settingStore";

/** 桌面歌词悬浮控件：开关 / 横竖方向 / 动效方案 / 字号（设置即唯一可信源） */
export function DesktopLyricSettingCard() {
	const desktopLyric = useSettingStore((s) => s.desktopLyric);
	const setDesktopLyricEnabled = useSettingStore(
		(s) => s.setDesktopLyricEnabled,
	);
	const setDesktopLyricDirection = useSettingStore(
		(s) => s.setDesktopLyricDirection,
	);
	const setDesktopLyricAnimationStyle = useSettingStore(
		(s) => s.setDesktopLyricAnimationStyle,
	);
	const setDesktopLyricFontSize = useSettingStore(
		(s) => s.setDesktopLyricFontSize,
	);

	return (
		<SettingsExpandar
			icon={<TabDesktopMultiple20Regular />}
			title="桌面歌词"
			subtitle="置顶悬浮在桌面上的歌词控件，支持横向 / 竖向显示与默认 / 笔画两种动效"
		>
			<SettingsExpandarDetail desc="启用桌面歌词（关闭主窗口后仍会悬浮显示）">
				<Switch
					checked={desktopLyric.enabled}
					onCheckedChange={(checked) => setDesktopLyricEnabled(checked)}
				/>
			</SettingsExpandarDetail>
			<SettingsExpandarDetail desc="竖向显示（传统直书，横向 / 竖向可随时在控件上切换）">
				<Switch
					checked={desktopLyric.direction === "vertical"}
					onCheckedChange={(checked) =>
						setDesktopLyricDirection(checked ? "vertical" : "horizontal")
					}
				/>
			</SettingsExpandarDetail>
			<SettingsExpandarDetail desc="笔画歌词动效（逐字按笔画飞入拼合，句尾四散散开）">
				<Switch
					checked={desktopLyric.animationStyle === "stroke"}
					onCheckedChange={(checked) =>
						setDesktopLyricAnimationStyle(checked ? "stroke" : "default")
					}
				/>
			</SettingsExpandarDetail>
			<SettingsExpandarDetail desc="歌词字号">
				<NumberStepper
					label="桌面歌词字号"
					value={desktopLyric.fontSize}
					min={DESKTOP_LYRIC_FONT_LIMITS.min}
					max={DESKTOP_LYRIC_FONT_LIMITS.max}
					step={2}
					onValueChange={(value) => setDesktopLyricFontSize(value)}
				/>
			</SettingsExpandarDetail>
		</SettingsExpandar>
	);
}
