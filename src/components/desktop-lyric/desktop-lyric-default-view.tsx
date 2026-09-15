import { AnimatePresence, type MotionValue, motion } from "framer-motion";
import type { CSSProperties } from "react";
import { VerbatimWord } from "@/components/lyric/verbatim-word";
import { cn } from "@/lib/utils";
import type { ILyricLine } from "@/lib/utils/lyric-parser";

/**
 * 桌面歌词「默认动效」视图：当前句整行展示，逐字填充与歌词页共用
 * VerbatimWord（横排从左到右、竖排直书从上到下），行间切换淡入上移。
 *
 * 与歌词页的关键差异：桌面歌词浮在任意壁纸上，plus-lighter 叠加不可用，
 * 可读性由独立的暗色描边背衬层保证——背衬层与填充层同文本同布局完全
 * 重叠，永远完整描出整行轮廓，填充层（含逐字遮罩）叠在其上。
 */

export type DesktopLyricOrientation = "horizontal" | "vertical";

interface DesktopDefaultViewProps {
	line?: ILyricLine;
	/** 行唯一标识（驱动进出场过渡） */
	lineKey: string;
	direction: DesktopLyricOrientation;
	fontSize: number;
	currentTimeMotion: MotionValue<number>;
}

export function DesktopDefaultView({
	line,
	lineKey,
	direction,
	fontSize,
	currentTimeMotion,
}: DesktopDefaultViewProps) {
	if (!line || line.isLeadDots) {
		return <DesktopLeadDots vertical={direction === "vertical"} />;
	}

	const isVertical = direction === "vertical";
	// 竖排的进出场沿水平轴（列从右侧推入），横排沿垂直轴，与书写方向一致
	const enter = isVertical ? { opacity: 0, x: 14 } : { opacity: 0, y: 10 };
	const exit = isVertical ? { opacity: 0, x: -12 } : { opacity: 0, y: -8 };

	return (
		<div
			className={cn(
				"flex h-full w-full items-center justify-center overflow-hidden",
				isVertical ? "px-3 py-2" : "px-8",
			)}
		>
			<AnimatePresence mode="popLayout" initial={false}>
				<motion.div
					key={lineKey}
					initial={enter}
					animate={{ opacity: 1, x: 0, y: 0 }}
					exit={exit}
					transition={{ duration: 0.24, ease: "easeOut" }}
					className={cn("max-w-full", isVertical && "h-full")}
					style={isVertical ? { writingMode: "vertical-rl" } : undefined}
				>
					<DesktopLineText
						line={line}
						direction={direction}
						fontSize={fontSize}
						currentTimeMotion={currentTimeMotion}
					/>
				</motion.div>
			</AnimatePresence>
		</div>
	);
}

/** 单行渲染：暗色描边背衬层 + 逐字填充层（两层同文本同布局完全重叠） */
function DesktopLineText({
	line,
	direction,
	fontSize,
	currentTimeMotion,
}: {
	line: ILyricLine;
	direction: DesktopLyricOrientation;
	fontSize: number;
	currentTimeMotion: MotionValue<number>;
}) {
	const isVertical = direction === "vertical";
	// 横排单行不换行（窗口高度按字号给定，换行必然溢出）；
	// 竖排靠窗口高度自然折列，textAlign center 在竖排下沿行内方向（纵向）居中
	const textStyle: CSSProperties = {
		fontSize,
		fontWeight: 700,
		lineHeight: 1.35,
		fontFamily: "var(--app-lyric-font-family, inherit)",
		fontSynthesis: "none",
		WebkitFontSmoothing: "antialiased",
		whiteSpace: isVertical ? "normal" : "nowrap",
		textAlign: isVertical ? "center" : undefined,
	};

	const hasWords = Boolean(line.words && line.words.length > 0);

	return (
		<span className="relative inline-block" style={textStyle}>
			{/* 暗色描边背衬层：永远完整描出整行轮廓（任意壁纸下的可读性兜底） */}
			<span
				aria-hidden
				className="desktop-lyric-glyph-backdrop absolute inset-0"
			>
				{line.lineText}
			</span>
			<span className="desktop-lyric-glyph-halo relative">
				{hasWords && line.words ? (
					line.words.map((word) => (
						<VerbatimWord
							key={`${word.startTime}-${word.duration}-${word.char}`}
							word={word}
							currentTimeMotion={currentTimeMotion}
							orientation={isVertical ? "vertical" : "horizontal"}
						/>
					))
				) : (
					<span style={{ color: "rgb(255 255 255 / 0.88)" }}>
						{line.lineText}
					</span>
				)}
			</span>
		</span>
	);
}

/** 过门 / 空闲的呼吸圆点（竖排时纵向排列） */
export function DesktopLeadDots({ vertical }: { vertical: boolean }) {
	return (
		<div
			className={cn(
				"flex items-center justify-center gap-3",
				vertical && "flex-col",
			)}
		>
			{[0, 1, 2].map((i) => (
				<motion.span
					key={i}
					className="rounded-full bg-white/70"
					style={{
						width: 10,
						height: 10,
						boxShadow: "0 1px 6px rgb(0 0 0 / 0.5)",
					}}
					animate={{ opacity: [0.2, 0.8, 0.2] }}
					transition={{
						duration: 1.4,
						repeat: Infinity,
						delay: i * 0.22,
						ease: "easeInOut",
					}}
				/>
			))}
		</div>
	);
}
