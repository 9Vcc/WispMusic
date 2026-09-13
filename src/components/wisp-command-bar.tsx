import {
	ChevronDown24Regular,
	Play24Filled,
	Search24Filled,
} from "@fluentui/react-icons";
import { type ReactNode, useCallback } from "react";
import { Input } from "@/components/ui/input";
import { WispButton } from "@/components/wisp-button";
import { Popover, PopoverItem } from "@/components/wisp-popover";
import { SONG_SORT_OPTIONS } from "@/lib/constants/song";
import { cn } from "@/lib/utils";

/**
 * 详情页 / 列表页 sticky 命令栏的内容骨架。
 *
 * 分工：`PinnedBar` 决定"什么时候浮现、要不要模糊层、标题怎么收缩"，
 * `WispCommandBar` 决定"里面怎么排"。两者分离后，各页只需要给左右两槽内容，
 * 不必再各自记得写 `pl-8` / `pr-8`（此前 Download / Cloud / Local / Recent /
 * Playlist / Artist 六处都在手写这两个内边距，漏一个就与页面内容错位）。
 */
export function WispCommandBar({
	leading,
	trailing,
	className,
}: {
	/** 左槽：主操作（播放、收藏、编辑、Tabs…） */
	leading?: ReactNode;
	/** 右槽：次级操作（排序、搜索、打开目录…） */
	trailing?: ReactNode;
	className?: string;
}) {
	return (
		<div
			className={cn(
				"flex w-full items-center justify-between gap-4",
				className,
			)}
		>
			<div className="flex min-w-0 items-center gap-4 pl-8">{leading}</div>
			<div className="flex shrink-0 items-center gap-4 pr-8">{trailing}</div>
		</div>
	);
}

/**
 * 命令栏主播放按钮：六个页面里出现过同一段
 * `variant="glass" + w-26 + Play24Filled + 播放` 的复制粘贴。
 */
export function WispCommandBarPlay({
	onClick,
	disabled,
	label = "播放",
	className,
}: {
	onClick?: () => void;
	disabled?: boolean;
	label?: string;
	className?: string;
}) {
	return (
		<WispButton
			variant="glass"
			size="lg"
			disabled={disabled}
			onClick={onClick}
			className={cn("text-primary!", className)}
			content={
				<div className="flex w-26 items-center justify-center gap-2">
					<Play24Filled className="size-4" />
					<span>{label}</span>
				</div>
			}
		/>
	);
}

/**
 * 命令栏里的展开式搜索框。
 *
 * 云盘 / 本地音乐 / 歌单 / 歌手四处的实现逐字符相同（含 focus/blur 的展开收起
 * 规则），唯一差别是歌手页图标色用了 80% 而其余用 60%。统一取 60%。
 */
export function WispSearchField({
	value,
	onChange,
	open,
	onOpenChange,
	placeholder = "搜索...",
	ariaLabel = "搜索",
	className,
}: {
	value: string;
	onChange: (value: string) => void;
	open: boolean;
	onOpenChange: (open: boolean) => void;
	placeholder?: string;
	ariaLabel?: string;
	className?: string;
}) {
	const handleBlur = useCallback(() => {
		// 有内容时不收起，否则用户没法接着编辑
		if (!value) onOpenChange(false);
	}, [value, onOpenChange]);

	return (
		<div className={cn("relative flex items-center", className)}>
			<Search24Filled className="pointer-events-none absolute top-1/2 left-2.5 z-10 size-4 -translate-y-1/2 text-foreground/60" />
			<Input
				aria-label={ariaLabel}
				showIndicator={false}
				placeholder={open ? placeholder : ""}
				className={cn(
					"h-9 rounded-full border-transparent bg-transparent! text-foreground",
					"focus:border-transparent focus:ring-0!",
					"transition-all duration-300 ease-in-out",
					open ? "w-48 pl-8" : "w-9",
				)}
				containerClassName="rounded-full wisp-glass-surface"
				value={value}
				onChange={(e) => onChange(e.target.value)}
				onFocus={() => onOpenChange(true)}
				onBlur={handleBlur}
			/>
		</div>
	);
}

/**
 * 命令栏里的排序菜单。本地音乐页与歌单页原本是同一段代码、
 * 只有"排序方式"后面那个值的字重不同（semibold / bold），统一为 semibold。
 */
export function WispSortMenu({
	value,
	onChange,
	options = SONG_SORT_OPTIONS,
	label = "排序方式",
}: {
	value: string;
	onChange: (value: string) => void;
	options?: Record<string, string>;
	label?: string;
}) {
	return (
		<Popover
			trigger={
				<button
					type="button"
					className="flex items-center gap-2 rounded-sm px-4 py-2 text-sm hover:bg-fill-hover"
				>
					<span className="font-light text-sm">{label}：</span>
					<span className="font-semibold text-primary text-sm">
						{options[value]}
					</span>
					<ChevronDown24Regular className="size-4 text-foreground/60" />
				</button>
			}
		>
			{Object.entries(options).map(([optionValue, optionLabel]) => (
				<PopoverItem
					key={optionValue}
					isActive={value === optionValue}
					onClick={() => onChange(optionValue)}
				>
					{optionLabel}
				</PopoverItem>
			))}
		</Popover>
	);
}
