import type * as React from "react";
import { cn } from "@/lib/utils";

/**
 * 列表行的统一样式来源。样式本体在 index.css 的 `.wisp-list-row`
 * （行高 / hover / 斑马纹 / 行内操作按钮的弱化与增强）。
 *
 * 迁移前的状况：歌曲列表写 `px-4 py-3 hover:bg-foreground/8 rounded-md`，
 * 下载与网盘的活动任务行写 `p-3 hover:bg-foreground/8 bg-foreground/5`，
 * 三处的行高、圆角、hover 与斑马纹规则互不相干，同一屏里出现两种行。
 */

/** 行高：歌曲 / 下载 / 网盘 / 本地音乐统一 64px。
 *
 * 用固定高度而不是给内容加 py —— 歌曲列表在有封面时是 40px 封面 + 24px 内边距，
 * 无封面（专辑/歌手页）时曾经只有约 44px，同一种"歌"在不同页里高度不一样。 */
export const WISP_LIST_ROW_HEIGHT = "h-16";

/** 歌曲类列表的列模板：歌名 / 歌手 / 专辑 / 时长 / 更多。
 *  尾部两列宽固定，长歌名只能自己 line-clamp，不能把时长和按钮挤走。 */
export function wispSongListGrid(showAlbum: boolean) {
	return showAlbum
		? "grid-cols-[1fr_1fr_1fr_52px_26px]"
		: "grid-cols-[1fr_1fr_52px_26px]";
}

/** 行内操作按钮（更多 / 暂停 / 取消）的统一外观：
 *  默认弱化到 55%，行 hover 或键盘聚焦进这一行时才回到 100%，
 *  避免一屏几十个按钮抢走内容层级。 */
export function wispListRowActionClass(className?: string) {
	return cn(
		"wisp-list-row-action text-muted-foreground hover:text-foreground",
		className,
	);
}

/**
 * 统一列表行容器：斑马纹、hover、圆角、行高、行内按钮四态都在这里收口。
 *
 * @param index 行序号，用于斑马纹（[data-zebra] 由偶数行开启）
 * @param zebra 显式关闭/开启斑马纹；不传则按 index 自动判定
 */
export function WispListRow({
	index,
	zebra,
	className,
	children,
	...props
}: React.ComponentProps<"div"> & { index: number; zebra?: boolean }) {
	const useZebra = zebra ?? index % 2 === 0;

	return (
		<div
			data-slot="wisp-list-row"
			data-zebra={useZebra ? "true" : undefined}
			className={cn(
				"wisp-list-row group flex items-center gap-4 px-4",
				WISP_LIST_ROW_HEIGHT,
				className,
			)}
			{...props}
		>
			{children}
		</div>
	);
}
