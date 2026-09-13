import type * as React from "react";
import { Entrance } from "@/components/entrance";
import { cn } from "@/lib/utils";

/**
 * 详情页（歌单 / 专辑 / 艺人正文）的统一 header。
 *
 * 迁移前歌单页封面是 `h-44 w-44` + `drop-shadow-2xl`，专辑页是 `size-64` +
 * `drop-shadow-xl`，信息栏一个靠 gap 撑、一个硬写 `h-64`，连标题字重都是
 * `font-bold` 与 `font-bold` 之外的两套。这里把封面尺寸、圆角、投影、
 * 标题排印、间距与入场时序收到一处。
 *
 * 封面固定 `size-44`（UI_GUIDELINES §8.2 的 w-44 h-44）：
 * 同一应用里"歌单封面"和"专辑封面"是同一类物件，尺寸必须一致。
 * 艺人页的沉浸式大图属于另一类表达，不用本组件。
 */
export function WispDetailHeader({
	cover,
	title,
	badges,
	meta,
	description,
	actions,
	className,
	ref,
}: {
	/** 封面内容（img 或占位）；外层容器尺寸/圆角/投影由本组件负责 */
	cover?: React.ReactNode;
	title: React.ReactNode;
	/** 标题右侧的状态标记，例如私密歌单的锁图标 */
	badges?: React.ReactNode;
	/** 标题下方一行：作者 / 发布日期等 */
	meta?: React.ReactNode;
	/** 简介区块（可折叠、可点击展开） */
	description?: React.ReactNode;
	/** 底部主操作区（播放 / 收藏 / 编辑…） */
	actions?: React.ReactNode;
	className?: string;
	ref?: React.Ref<HTMLDivElement>;
}) {
	return (
		<div
			ref={ref}
			className={cn("mb-8 flex items-center gap-8 px-8", className)}
		>
			{cover && (
				<Entrance className="flex-none">
					<div className="relative size-44 overflow-hidden rounded-md bg-fill-subtle shadow-wisp-float">
						{cover}
					</div>
				</Entrance>
			)}

			{/* min-w-0 让长标题能真正截断；min-h-44 与封面等高，
          信息少时不会比封面矮出一截，信息多时也不会把封面撑变形 */}
			<Entrance
				delay={0.06}
				className="flex min-h-44 min-w-0 flex-1 flex-col justify-between gap-4"
			>
				<div className="flex min-w-0 flex-col gap-2">
					<h1 className="flex items-center gap-2 text-2xl font-semibold select-text">
						{title}
						{badges}
					</h1>
					{meta}
				</div>

				{description}

				{actions && <div className="flex flex-wrap gap-4">{actions}</div>}
			</Entrance>
		</div>
	);
}

/** header 里的元信息行（作者、发布时间等）统一排印 */
export function WispDetailHeaderMeta({
	className,
	...props
}: React.ComponentProps<"div">) {
	return <div className={cn("flex flex-col gap-4", className)} {...props} />;
}

/**
 * 封面图片：尺寸由 WispDetailHeader 的容器决定，这里只负责 object-cover。
 * coverAlt 必填以保证可访问性（详情页封面不是装饰图）。
 */
export function WispDetailCoverImage({
	src,
	alt,
	className,
}: {
	src?: string | null;
	alt: string;
	className?: string;
}) {
	return (
		<img
			src={src ?? ""}
			alt={alt}
			draggable={false}
			className={cn("h-full w-full object-cover", className)}
		/>
	);
}
