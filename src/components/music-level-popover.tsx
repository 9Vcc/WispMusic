import { Children, isValidElement, type ReactNode } from "react";
import { QUALITY_BY_KEY, type QualityKey } from "@/lib/constants/song";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import type { SongQualityDetail } from "@/lib/types/song";
import { cn, formatFileSize } from "@/lib/utils";
import { QualityTriggerIcon } from "./quality-trigger-icon";
import { Popover, PopoverContent, PopoverTrigger } from "./ui/popover";

export function MusicLevelPopover({
	open,
	onOpenChange,
	side = "top",
	sideOffset = 48,
	contentClassName,
	className,
	children,
}: {
	open?: boolean;
	onOpenChange?: (value: boolean) => void;
	variant?: "light" | "dark";
	side?: "top" | "right" | "bottom" | "left";
	sideOffset?: number;
	contentClassName?: string;
	className?: string;
	children?: ReactNode;
}) {
	const currentMusicLevelKey = usePlayerStore((s) => s.currentMusicLevelKey);
	const currentSongMusicDetail = usePlayerStore(
		(s) => s.currentSongMusicDetail,
	);
	const setCurrentMusicLevelKey = usePlayerStore(
		(s) => s.setCurrentMusicLevelKey,
	);

	const isUnlock = currentMusicLevelKey === "unlock";
	const shouldShowQualityText = currentMusicLevelKey !== "db";
	const trigger = Children.toArray(children).find(isValidElement);
	const handleSelect = (qualityKey: QualityKey) => {
		onOpenChange?.(false);
		window.setTimeout(() => setCurrentMusicLevelKey(qualityKey), 0);
	};

	return (
		<Popover open={open} onOpenChange={onOpenChange}>
			<PopoverTrigger asChild={!!trigger}>
				{trigger ? (
					trigger
				) : (
					<span
						className={cn(
							"inline-flex items-center gap-1.5 rounded-md border bg-card/50 px-2 py-1 font-bold text-foreground/60 text-xs hover:bg-card/30",
							!shouldShowQualityText && "px-2.5",
							className,
						)}
					>
						<QualityTriggerIcon qualityKey={currentMusicLevelKey} />
						{shouldShowQualityText && QUALITY_BY_KEY[currentMusicLevelKey].desc}
					</span>
				)}
			</PopoverTrigger>
			<PopoverContent
				side={side}
				sideOffset={sideOffset}
				className={cn("w-64 select-none", contentClassName)}
			>
				{isUnlock || currentMusicLevelKey === "local" ? (
					<div className="text-center">
						{isUnlock ? "灰色音源歌曲不支持修改音质" : "本地音乐"}
					</div>
				) : (
					<ul className="flex flex-col gap-2">
						{currentSongMusicDetail?.map((quality: SongQualityDetail) => (
							<AudioLevelItem
								key={quality.key}
								qualityKey={quality.key as QualityKey}
								size={formatFileSize(quality.size)}
								selected={quality.key === currentMusicLevelKey}
								onClick={handleSelect}
							/>
						))}
					</ul>
				)}
			</PopoverContent>
		</Popover>
	);
}

interface AudioLevelItemProps {
	qualityKey: QualityKey;
	size?: string;
	selected?: boolean;
	onClick: (level: QualityKey) => void;
}

export function AudioLevelItem({
	qualityKey,
	size,
	selected = false,
	onClick,
}: AudioLevelItemProps) {
	const option = QUALITY_BY_KEY[qualityKey];

	return (
		<div
			data-active={selected ? "true" : undefined}
			className="wisp-menu-item relative flex cursor-pointer items-center justify-between rounded-md px-4 py-2"
			onClick={() => onClick(qualityKey)}
		>
			<span className="font-bold">{option.desc}</span>

			<div className="flex items-center gap-2">
				{size && (
					<span className={cn("text-foreground/60 text-xs")}>{size}</span>
				)}
				{selected && (
					<span className="absolute top-1/2 left-0 h-4 w-1 -translate-1/2 rounded-full bg-primary" />
				)}
			</div>
		</div>
	);
}
