import { ClothesHanger20Regular } from "@fluentui/react-icons";
import { useState } from "react";
import {
	type AppearanceSettings,
	useSettingStore,
} from "@/lib/store/settingStore/settingStore";
import { Popover, PopoverContent, PopoverTrigger } from "../ui/popover";
import { WispButton } from "../wisp-button";

const THEME_OPTIONS: {
	value: AppearanceSettings["theme"];
	label: string;
}[] = [
	{ value: "light", label: "浅色" },
	{ value: "dark", label: "深色" },
	{ value: "system", label: "跟随系统" },
];

const MATERIAL_OPTIONS: {
	value: AppearanceSettings["material"];
	label: string;
}[] = [
	{ value: "acrylic", label: "亚克力" },
	{ value: "mica", label: "云母" },
	{ value: "solid", label: "纯色" },
	{ value: "none", label: "透明" },
];

/** 与侧栏歌单弹窗同一套 Radix Popover：portal 到 body、同款
    .wisp-menu-surface 毛玻璃表面与开合动画，质感完全一致 */
export function ThemeSwitcher() {
	const theme = useSettingStore((s) => s.appearance.theme);
	const setTheme = useSettingStore((s) => s.setTheme);
	const material = useSettingStore((s) => s.appearance.material);
	const setMaterial = useSettingStore((s) => s.setMaterial);
	const [open, setOpen] = useState(false);

	const renderItem = (
		label: string,
		active: boolean,
		onClick: () => void,
		key: string,
	) => (
		<button
			type="button"
			key={key}
			onClick={onClick}
			className="wisp-menu-item relative flex w-full cursor-pointer items-center rounded-sm p-2 text-left text-sm select-none"
		>
			{label}
			{active && (
				<div className="absolute top-1/2 left-0 h-1/2 w-1 -translate-y-1/2 rounded-full bg-primary" />
			)}
		</button>
	);

	return (
		// 拦截 mousedown，避免在标题栏上点开浮窗时触发窗口拖拽
		<div onMouseDown={(e) => e.stopPropagation()} className="mr-4">
			<Popover open={open} onOpenChange={setOpen}>
				<PopoverTrigger asChild>
					<WispButton
						variant="ghost"
						className="size-8 shrink-0 rounded-sm hover:bg-fill-hover"
						icon={<ClothesHanger20Regular />}
						aria-label="主题与材质"
					/>
				</PopoverTrigger>
				<PopoverContent side="bottom" align="end" className="w-auto min-w-36">
					<div className="px-2 pt-1 pb-0.5 text-xs text-muted-foreground">
						主题
					</div>
					{THEME_OPTIONS.map((option) =>
						renderItem(
							option.label,
							theme === option.value,
							() => {
								setTheme(option.value);
								setOpen(false);
							},
							option.value,
						),
					)}

					<div className="my-1 h-px bg-border" />

					<div className="px-2 pt-1 pb-0.5 text-xs text-muted-foreground">
						材质
					</div>
					{MATERIAL_OPTIONS.map((option) =>
						renderItem(
							option.label,
							material === option.value,
							() => {
								setMaterial(option.value);
								setOpen(false);
							},
							option.value,
						),
					)}
				</PopoverContent>
			</Popover>
		</div>
	);
}
