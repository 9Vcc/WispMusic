"use client";

import { useEffect, useState } from "react";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { tryNormalizeLyricColor } from "@/lib/utils/desktop-lyric-colors";

/**
 * 颜色输入行：系统取色器 + hex 输入框（与主题色设置同一交互语言）。
 *
 * - 取色器拖动过程中即时回调，所见即所得（桌面歌词窗口实时跟随）；
 * - hex 输入在失焦 / 回车时提交，非法值只标红、不回写，避免半截输入
 *   把窗口刷成透明；
 * - 外部值变化（跨窗口同步 / 恢复默认 / 预设）会回灌输入框。
 */
export function ColorField({
	value,
	onChange,
	label,
	className,
}: {
	value: string;
	onChange: (value: string) => void;
	/** 无障碍名称，如「桌面歌词文字颜色」 */
	label: string;
	className?: string;
}) {
	const [draft, setDraft] = useState(value);
	const [isInvalid, setIsInvalid] = useState(false);

	useEffect(() => {
		setDraft(value);
		setIsInvalid(false);
	}, [value]);

	function commitDraft() {
		const trimmed = draft.trim();
		// 空输入不提交：只把显示恢复成当前生效值
		if (!trimmed) {
			setDraft(value);
			setIsInvalid(false);
			return;
		}

		const normalized = tryNormalizeLyricColor(trimmed);
		if (!normalized) {
			setIsInvalid(true);
			return;
		}

		setIsInvalid(false);
		onChange(normalized);
	}

	return (
		<div className={cn("flex items-center gap-2", className)}>
			<label
				className="relative flex size-8 cursor-pointer items-center justify-center overflow-hidden rounded-md border border-input"
				title="打开系统取色器"
			>
				<input
					type="color"
					value={value}
					aria-label={`${label}（系统取色器）`}
					onChange={(event) => {
						const normalized = tryNormalizeLyricColor(event.target.value);
						if (normalized) onChange(normalized);
					}}
					className="absolute inset-0 cursor-pointer opacity-0"
				/>
				<span
					className="pointer-events-none size-5 rounded-sm"
					style={{ background: value }}
				/>
			</label>

			<Input
				className={cn("w-28 bg-card", isInvalid && "border-destructive")}
				value={draft}
				placeholder="#rrggbb"
				spellCheck={false}
				aria-invalid={isInvalid}
				aria-label={`${label}十六进制值`}
				onChange={(event) => setDraft(event.target.value)}
				onBlur={commitDraft}
				onKeyDown={(event) => {
					if (event.key === "Enter") event.currentTarget.blur();
				}}
			/>
		</div>
	);
}
