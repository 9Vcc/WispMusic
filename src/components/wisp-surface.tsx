import type * as React from "react";
import { cn } from "@/lib/utils";

/**
 * 表面组件的样式本体在 src/index.css 的 `@layer components`（.wisp-surface 等），
 * 这里只负责把它包成可组合的 React 组件。
 *
 * 为什么不把样式写在组件里：这套表面要被非 React 上下文复用
 * （Portal 出来的菜单、第三方弹层、纯 HTML 片段），CSS class 才是唯一事实源；
 * 组件只是一个方便挂载的壳子。
 */

/** 通用面板 / 卡片表面：内容区里"浮起来的一块"，不承担浮层层级。 */
export function WispSurface({
	className,
	...props
}: React.ComponentProps<"div">) {
	return (
		<div
			data-slot="wisp-surface"
			className={cn("wisp-surface", className)}
			{...props}
		/>
	);
}

/**
 * 浮层菜单表面：菜单 / 下拉 / 弹出层 / 右键菜单的统一实现
 * （卡片色 80% 起底 + 12px 背景模糊 + --border 描边 + 大阴影）。
 *
 * 迁移前这些样式在 6 个文件里各写了一遍 `bg-card/80 backdrop-blur-md border`，
 * 任何一处漏了 border 就会在浅色云母上"没有面板"。
 */
export function WispMenuSurface({
	className,
	...props
}: React.ComponentProps<"div">) {
	return (
		<div
			data-slot="wisp-menu-surface"
			className={cn("wisp-menu-surface", className)}
			{...props}
		/>
	);
}

/** 二级菜单 / 强可读性场景：玻璃叠玻璃会糊，这类直接吃满卡片色。 */
export function WispMenuSolidSurface({
	className,
	...props
}: React.ComponentProps<"div">) {
	return (
		<div
			data-slot="wisp-menu-solid-surface"
			className={cn("wisp-menu-surface-solid", className)}
			{...props}
		/>
	);
}

/**
 * 菜单项：hover / 按下 / 选中 / 禁用四态统一。
 * data-active 走 .wisp-menu-item[data-active="true"] 的选中态。
 */
export function WispMenuItem({
	className,
	active = false,
	...props
}: React.ComponentProps<"div"> & { active?: boolean }) {
	return (
		<div
			data-slot="wisp-menu-item"
			data-active={active ? "true" : undefined}
			className={cn("wisp-menu-item", className)}
			{...props}
		/>
	);
}
