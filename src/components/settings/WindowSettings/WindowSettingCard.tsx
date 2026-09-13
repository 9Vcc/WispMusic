import { SlideSize20Regular } from "@fluentui/react-icons";
import SettingsExpandar, {
	SettingsExpandarDetail,
} from "@/components/settings/SettingsExpandar";
import { NumberStepper } from "@/components/ui/number-stepper";
import { Switch } from "@/components/ui/switch";
import {
	MIN_WINDOW_HEIGHT_LIMITS,
	MIN_WINDOW_WIDTH_LIMITS,
	useSettingStore,
} from "@/lib/store/settingStore/settingStore";

export function WindowSettingCard() {
	const rememberWindowBounds = useSettingStore(
		(s) => s.window.rememberWindowBounds,
	);
	const minWindowWidth = useSettingStore((s) => s.window.minWindowWidth);
	const minWindowHeight = useSettingStore((s) => s.window.minWindowHeight);
	const setRememberWindowBounds = useSettingStore(
		(s) => s.setRememberWindowBounds,
	);
	const setMinWindowSize = useSettingStore((s) => s.setMinWindowSize);

	return (
		<SettingsExpandar
			icon={<SlideSize20Regular />}
			title="窗口"
			subtitle="调整最小窗口尺寸，记住窗口大小和位置"
		>
			<SettingsExpandarDetail desc="记住窗口大小和位置">
				<Switch
					checked={rememberWindowBounds}
					onCheckedChange={(checked) => setRememberWindowBounds(checked)}
				/>
			</SettingsExpandarDetail>
			<SettingsExpandarDetail desc={`最小宽度（${minWindowWidth} 像素）`}>
				<NumberStepper
					label="最小宽度"
					value={minWindowWidth}
					min={MIN_WINDOW_WIDTH_LIMITS.min}
					max={MIN_WINDOW_WIDTH_LIMITS.max}
					step={10}
					onValueChange={(width) => setMinWindowSize(width, minWindowHeight)}
				/>
			</SettingsExpandarDetail>
			<SettingsExpandarDetail desc={`最小高度（${minWindowHeight} 像素）`}>
				<NumberStepper
					label="最小高度"
					value={minWindowHeight}
					min={MIN_WINDOW_HEIGHT_LIMITS.min}
					max={MIN_WINDOW_HEIGHT_LIMITS.max}
					step={10}
					onValueChange={(height) => setMinWindowSize(minWindowWidth, height)}
				/>
			</SettingsExpandarDetail>
		</SettingsExpandar>
	);
}
