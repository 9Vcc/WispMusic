import { PaintBrush20Regular } from "@fluentui/react-icons";
import { useEffect, useState } from "react";
import SettingsExpandar, {
	SettingsExpandarDetail,
} from "@/components/settings/SettingsExpandar";
import { Input } from "@/components/ui/input";
import { WispButton } from "@/components/wisp-button";
import { useSettingStore } from "@/lib/store/settingStore/settingStore";
import { cn } from "@/lib/utils";
import {
	ACCENT_COLOR_PRESETS,
	BUILT_IN_ACCENT_HEX,
	normalizeAccentColor,
} from "@/lib/utils/accent-color";

/**
 * 主题色（强调色）设置。
 *
 * 三种入口各司其职：预设色板用于快速挑选、系统取色器用于任意取色、
 * hex 输入框用于精确输入。三者都落到 appearance.accentColor，
 * 明度按深浅主题自动取档（见 utils/accent-color.ts）。
 */
export function AccentColorSettingCard() {
	const accentColor = useSettingStore((s) => s.appearance.accentColor);
	const setAccentColor = useSettingStore((s) => s.setAccentColor);
	const [draft, setDraft] = useState(accentColor ?? "");
	const [isInvalid, setIsInvalid] = useState(false);

	// 外部变化（点预设、跨窗口同步、恢复默认）要回灌到输入框
	useEffect(() => {
		setDraft(accentColor ?? "");
		setIsInvalid(false);
	}, [accentColor]);

	const isDefault = accentColor === null;
	// 内置状态下 --primary 就是内置配色，用它的 sRGB 近似值展示即可
	const displayHex = accentColor ?? BUILT_IN_ACCENT_HEX;

	function commitDraft() {
		const trimmed = draft.trim();
		// 空输入不提交：只把显示恢复成当前生效值
		if (!trimmed) {
			setDraft(accentColor ?? "");
			setIsInvalid(false);
			return;
		}

		const normalized = normalizeAccentColor(trimmed);
		if (!normalized) {
			setIsInvalid(true);
			return;
		}

		setIsInvalid(false);
		setAccentColor(normalized);
	}

	return (
		<SettingsExpandar
			icon={<PaintBrush20Regular />}
			title="主题色"
			subtitle="自定义强调色，用于按钮、进度条与激活态"
			trailing={
				<div className="flex items-center gap-2">
					<span
						className="size-6 rounded-full border border-border"
						style={{ background: displayHex }}
					/>
					<span className="w-20 text-right text-sm text-muted-foreground">
						{isDefault ? "内置配色" : accentColor}
					</span>
				</div>
			}
		>
			<SettingsExpandarDetail desc="预设配色">
				<div className="flex max-w-[420px] flex-wrap justify-end gap-2">
					{ACCENT_COLOR_PRESETS.map((preset) => {
						const isActive = accentColor === preset.value;
						return (
							<button
								key={preset.value}
								type="button"
								title={preset.name}
								aria-label={preset.name}
								aria-pressed={isActive}
								onClick={() => setAccentColor(preset.value)}
								className={cn(
									"size-7 rounded-full border border-black/10 transition-transform duration-150 hover:scale-110",
									isActive && "ring-2 ring-ring ring-offset-2 ring-offset-card",
								)}
								style={{ background: preset.value }}
							/>
						);
					})}
				</div>
			</SettingsExpandarDetail>

			<SettingsExpandarDetail desc="自定义颜色">
				<div className="flex items-center gap-2">
					<label
						className="relative flex size-8 cursor-pointer items-center justify-center overflow-hidden rounded-md border border-input"
						title="打开系统取色器"
					>
						<input
							type="color"
							value={displayHex}
							aria-label="打开系统取色器"
							onChange={(event) => setAccentColor(event.target.value)}
							className="absolute inset-0 cursor-pointer opacity-0"
						/>
						<span
							className="pointer-events-none size-5 rounded-sm"
							style={{ background: displayHex }}
						/>
					</label>

					<Input
						className={cn("w-32 bg-card", isInvalid && "border-destructive")}
						value={draft}
						placeholder="#rrggbb"
						spellCheck={false}
						aria-invalid={isInvalid}
						aria-label="主题色十六进制值"
						onChange={(event) => setDraft(event.target.value)}
						onBlur={commitDraft}
						onKeyDown={(event) => {
							if (event.key === "Enter") event.currentTarget.blur();
						}}
					/>

					<WispButton
						variant="default"
						disabled={isDefault}
						onClick={() => setAccentColor(null)}
					>
						恢复默认
					</WispButton>
				</div>
			</SettingsExpandarDetail>
		</SettingsExpandar>
	);
}
