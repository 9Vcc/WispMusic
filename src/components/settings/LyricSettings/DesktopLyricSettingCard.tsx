import { TabDesktopMultiple20Regular } from "@fluentui/react-icons";
import SettingsExpandar, {
	SettingsExpandarDetail,
} from "@/components/settings/SettingsExpandar";
import { ColorField } from "@/components/ui/color-field";
import { NumberStepper } from "@/components/ui/number-stepper";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { WispButton } from "@/components/wisp-button";
import {
	DESKTOP_LYRIC_FONT_LIMITS,
	useSettingStore,
} from "@/lib/store/settingStore/settingStore";
import { cn } from "@/lib/utils";
import {
	DESKTOP_LYRIC_BACKGROUND_OPACITY_LIMITS,
	DESKTOP_LYRIC_COLOR_SCHEMES,
	isDefaultDesktopLyricColors,
	isSameDesktopLyricColors,
	normalizeDesktopLyricColors,
} from "@/lib/utils/desktop-lyric-colors";

/**
 * 桌面歌词悬浮控件：开关 / 横竖方向 / 动效方案 / 字号 / 配色。
 *
 * 配色（文字色、高亮色、描边阴影色、背景不透明度）存 settingStore，是
 * 唯一可信源：改动即落库并推送给桌面歌词窗口，无需「应用」按钮；
 * 「恢复默认」回到白字 + 深色描边 + 全透明底的内置外观。
 */
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
	const setDesktopLyricColors = useSettingStore((s) => s.setDesktopLyricColors);
	const resetDesktopLyricColors = useSettingStore(
		(s) => s.resetDesktopLyricColors,
	);

	const colors = normalizeDesktopLyricColors(desktopLyric.colors);
	const isDefaultColors = isDefaultDesktopLyricColors(colors);

	return (
		<SettingsExpandar
			icon={<TabDesktopMultiple20Regular />}
			title="桌面歌词"
			subtitle="置顶悬浮在桌面上的歌词控件，支持横向 / 竖向显示、两种动效与自定义配色"
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

			<SettingsExpandarDetail desc="配色方案（一键套用，之后仍可单独微调）">
				<div className="flex max-w-[420px] flex-wrap justify-end gap-2">
					{DESKTOP_LYRIC_COLOR_SCHEMES.map((scheme) => {
						const isActive = isSameDesktopLyricColors(scheme.colors, colors);
						return (
							<button
								key={scheme.name}
								type="button"
								title={scheme.name}
								aria-label={`配色方案：${scheme.name}`}
								aria-pressed={isActive}
								onClick={() => setDesktopLyricColors(scheme.colors)}
								className={cn(
									"flex items-center gap-2 rounded-full border border-border bg-card/60 py-1 pr-3 pl-1.5 text-xs text-foreground/80 transition-colors hover:bg-fill-hover",
									isActive && "ring-2 ring-ring ring-offset-1 ring-offset-card",
								)}
							>
								{/* 双色点 = 高亮色 + 普通文字色，叠在描边色的深底上，一次看全整套方案 */}
								<span
									className="flex size-5 items-center justify-center rounded-full"
									style={{ background: scheme.colors.strokeColor }}
								>
									<span className="flex">
										<span
											className="size-2.5 rounded-full"
											style={{ background: scheme.colors.highlightColor }}
										/>
										<span
											className="-ml-1 size-2.5 rounded-full opacity-80"
											style={{ background: scheme.colors.textColor }}
										/>
									</span>
								</span>
								{scheme.name}
							</button>
						);
					})}
				</div>
			</SettingsExpandarDetail>

			<SettingsExpandarDetail desc="歌词文字颜色（下一句预览与未高亮的行）">
				<ColorField
					label="桌面歌词文字颜色"
					value={colors.textColor}
					onChange={(value) => setDesktopLyricColors({ textColor: value })}
				/>
			</SettingsExpandarDetail>

			<SettingsExpandarDetail desc="高亮 / 当前行颜色（当前演唱句与已唱逐字填充）">
				<ColorField
					label="桌面歌词高亮颜色"
					value={colors.highlightColor}
					onChange={(value) => setDesktopLyricColors({ highlightColor: value })}
				/>
			</SettingsExpandarDetail>

			<SettingsExpandarDetail desc="描边 / 阴影颜色（浮在壁纸上的可读性背衬）">
				<ColorField
					label="桌面歌词描边颜色"
					value={colors.strokeColor}
					onChange={(value) => setDesktopLyricColors({ strokeColor: value })}
				/>
			</SettingsExpandarDetail>

			<SettingsExpandarDetail desc="背景不透明度（0 = 完全透明，仅悬浮时显示深色背板）">
				<div className="flex items-center gap-3">
					<Slider
						aria-label="桌面歌词背景不透明度"
						value={[colors.backgroundOpacity]}
						min={DESKTOP_LYRIC_BACKGROUND_OPACITY_LIMITS.min}
						max={DESKTOP_LYRIC_BACKGROUND_OPACITY_LIMITS.max}
						step={5}
						className="w-40"
						onValueChange={([value]) =>
							setDesktopLyricColors({ backgroundOpacity: value })
						}
					/>
					<span className="w-10 text-right text-sm tabular-nums text-muted-foreground">
						{colors.backgroundOpacity}%
					</span>
				</div>
			</SettingsExpandarDetail>

			<SettingsExpandarDetail desc="恢复出厂配色（白字 + 深色描边 + 全透明底）">
				<WispButton
					variant="default"
					disabled={isDefaultColors}
					onClick={() => resetDesktopLyricColors()}
				>
					恢复默认
				</WispButton>
			</SettingsExpandarDetail>
		</SettingsExpandar>
	);
}
