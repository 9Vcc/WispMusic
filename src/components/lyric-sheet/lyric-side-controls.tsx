import {
	ArrowClockwise24Regular,
	ArrowCounterclockwise24Regular,
	ArrowReset24Regular,
	DrawText24Regular,
	List24Regular,
} from "@fluentui/react-icons";
import { motion, useReducedMotion } from "framer-motion";
import { useMemo, useState } from "react";
import {
	DEFAULT_LYRIC_OFFSET_MS,
	LYRIC_OFFSET_NUDGE_MS,
} from "@/lib/constants/lyric";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { selectActiveLocalLyric } from "@/lib/store/playerStore/playerStoreSlice/createLocalLyricSlice";
import { selectCurrentLyricOffset } from "@/lib/store/playerStore/playerStoreSlice/createLyricOffsetSlice";
import {
	selectActiveLyricVersion,
	selectActiveLyricVersionLines,
} from "@/lib/store/playerStore/playerStoreSlice/createLyricVersionSlice";
import { useSettingStore } from "@/lib/store/settingStore/settingStore";
import { cn } from "@/lib/utils";
import { resolveLyricLines } from "@/lib/utils/lyric-source";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";
import { lyricSheetEase } from "./lyric-sheet-motion";
import { LyricVersionDialog } from "./lyric-version-dialog";

/**
 * 歌词页右侧悬浮工具条（交互参考 EchoMusic 歌词页的 lyric-page-tools）：
 * 默认隐藏，鼠标悬浮在歌词页右侧区域时平滑淡入，移出后自动淡出。
 *
 * 与 EchoMusic 一致的操作逻辑——单击即静默生效，没有二级面板、不打 toast；
 * 状态反馈由就地指示承担：偏移非 0 时重置按钮现身 + 按钮角落绿点：
 * - 选择歌词：打开歌词版本对话框（对应 EchoMusic 的 LyricSourceDialog），
 *   手动版本生效后选择按钮亮绿点；
 * - 歌词后退 / 歌词前进：单击按 0.5s 步进调整偏移；
 * - 重置偏移：仅当前偏移非 0 时可见（visibility 占位，避免按钮列跳动）。
 *   后退 / 前进 / 重置三个按钮只在有歌词时渲染（与正文同一条解析链路判断）。
 *
 * 布局要点：
 * - 挂载点是歌词面板右侧的 48px 边距槽（面板自身 `right-12`），按钮列正好
 *   塞进槽内，**不遮盖歌词正文**；右侧垂直居中（EchoMusic 同位置）；
 * - 每颗按钮独立成玻璃圆钮（EchoMusic 同款独立圆钮列，8px 间距），表面复用
 *   `wisp-glass-surface` 玻璃基元（模糊 / 高光描边 / 按压反馈与底部栏同源），
 *   白 10% 底对齐 EchoMusic 的 rgba(255,255,255,0.1)，悬停提亮到 20%；
 * - 显隐动画只作用于本组件（歌词文本的兄弟节点），绝不套在歌词祖先链上
 *   ——plus-lighter 的祖先链禁 transform/opacity 隔离（见 lyric.tsx 顶部注释）；
 * - `visible` 由父级的热区 hover 状态驱动；对话框打开时强制保持可见，
 *   避免指针移向 portal 弹层时按钮先淡出。
 */

/** 工具条单颗玻璃圆钮：独立按钮（非共享胶囊），底色浓度对齐 EchoMusic 的 10% 白 */
const TOOL_BUTTON_SURFACE =
	"wisp-glass-surface rounded-full bg-white/10 hover:bg-white/20";

export function LyricSideControls({ visible }: { visible: boolean }) {
	const [versionDialogOpen, setVersionDialogOpen] = useState(false);
	const reduceMotion = Boolean(useReducedMotion());

	const hasManualVersion = usePlayerStore(selectActiveLyricVersion) !== null;
	const manualVersionLines = usePlayerStore(selectActiveLyricVersionLines);
	const activeLocalLyric = usePlayerStore(selectActiveLocalLyric);
	const currentSongLyrics = usePlayerStore((s) => s.currentSongLyrics);
	const lyricOffsetMs = usePlayerStore(selectCurrentLyricOffset);
	const adjustLyricOffset = usePlayerStore((s) => s.adjustLyricOffset);
	const resetLyricOffset = usePlayerStore((s) => s.resetLyricOffset);

	// 有无歌词与正文同一条解析链路（手动版本 > 本地 > 在线逐字 > 在线逐行），
	// 对齐 EchoMusic 的 hasLyrics = 歌词行数 > 0
	const hasLyrics = useMemo(
		() =>
			resolveLyricLines({
				manualLines: manualVersionLines,
				localLines: activeLocalLyric?.lines,
				verbatimSource: currentSongLyrics?.yrc?.lyric,
				plainSource: currentSongLyrics?.lrc?.lyric,
			}).lines.length > 0,
		[manualVersionLines, activeLocalLyric, currentSongLyrics],
	);

	const isOffsetAdjusted = lyricOffsetMs !== DEFAULT_LYRIC_OFFSET_MS;
	// 笔画歌词动效开关：默认动效 ↔ 笔画双列动效，持久化到设置
	const isStrokeMode = useSettingStore(
		(s) => s.lyric.animationStyle === "stroke",
	);
	const setLyricAnimationStyle = useSettingStore(
		(s) => s.setLyricAnimationStyle,
	);
	const keepVisible = visible || versionDialogOpen;
	const nudgeLabel = `${LYRIC_OFFSET_NUDGE_MS / 1000}s`;

	return (
		<motion.div
			initial={false}
			animate={{
				opacity: keepVisible ? 1 : 0,
				x: keepVisible ? 0 : 10,
				y: "-50%",
			}}
			transition={
				reduceMotion
					? { duration: 0 }
					: { duration: 0.32, ease: lyricSheetEase }
			}
			className="absolute top-1/2 right-1.5 z-20"
			style={{ pointerEvents: keepVisible ? "auto" : "none" }}
		>
			<div className="flex flex-col gap-2">
				<Tooltip>
					<TooltipTrigger asChild>
						<button
							type="button"
							aria-label="笔画歌词动效"
							aria-pressed={isStrokeMode}
							className={cn(
								"relative flex size-10 items-center justify-center rounded-full text-white transition-colors duration-300 ease-in-out",
								TOOL_BUTTON_SURFACE,
							)}
							onClick={() =>
								setLyricAnimationStyle(isStrokeMode ? "default" : "stroke")
							}
						>
							<DrawText24Regular className="size-5 drop-shadow-md" />
							{isStrokeMode && (
								<span className="absolute right-1.5 bottom-1.5 size-1.5 rounded-full bg-emerald-400" />
							)}
						</button>
					</TooltipTrigger>
					<TooltipContent side="left" sideOffset={10}>
						{isStrokeMode ? "笔画歌词动效（已开启）" : "笔画歌词动效"}
					</TooltipContent>
				</Tooltip>

				<Tooltip>
					<TooltipTrigger asChild>
						<button
							type="button"
							aria-label="选择歌词"
							className={cn(
								"relative flex size-10 items-center justify-center rounded-full text-white transition-colors duration-300 ease-in-out",
								TOOL_BUTTON_SURFACE,
							)}
							onClick={() => setVersionDialogOpen(true)}
						>
							<List24Regular className="size-5 drop-shadow-md" />
							{hasManualVersion && (
								<span className="absolute right-1.5 bottom-1.5 size-1.5 rounded-full bg-emerald-400" />
							)}
						</button>
					</TooltipTrigger>
					<TooltipContent side="left" sideOffset={10}>
						选择歌词
					</TooltipContent>
				</Tooltip>

				{hasLyrics && (
					<>
						<Tooltip>
							<TooltipTrigger asChild>
								<button
									type="button"
									aria-label={`歌词后退 ${nudgeLabel}`}
									className={cn(
										"flex size-10 items-center justify-center rounded-full text-white transition-colors duration-300 ease-in-out",
										TOOL_BUTTON_SURFACE,
									)}
									onClick={() => adjustLyricOffset(-LYRIC_OFFSET_NUDGE_MS)}
								>
									<ArrowCounterclockwise24Regular className="size-5 drop-shadow-md" />
								</button>
							</TooltipTrigger>
							<TooltipContent side="left" sideOffset={10}>
								歌词后退 {nudgeLabel}
							</TooltipContent>
						</Tooltip>

						<Tooltip>
							<TooltipTrigger asChild>
								<button
									type="button"
									aria-label={`歌词前进 ${nudgeLabel}`}
									className={cn(
										"flex size-10 items-center justify-center rounded-full text-white transition-colors duration-300 ease-in-out",
										TOOL_BUTTON_SURFACE,
									)}
									onClick={() => adjustLyricOffset(LYRIC_OFFSET_NUDGE_MS)}
								>
									<ArrowClockwise24Regular className="size-5 drop-shadow-md" />
								</button>
							</TooltipTrigger>
							<TooltipContent side="left" sideOffset={10}>
								歌词前进 {nudgeLabel}
							</TooltipContent>
						</Tooltip>

						{/* 重置仅在偏移非 0 时可见（visibility 占位，EchoMusic 同款） */}
						<Tooltip>
							<TooltipTrigger asChild>
								<button
									type="button"
									aria-label="重置偏移"
									style={{
										visibility: isOffsetAdjusted ? "visible" : "hidden",
									}}
									className={cn(
										"flex size-10 items-center justify-center rounded-full text-white transition-colors duration-300 ease-in-out",
										TOOL_BUTTON_SURFACE,
									)}
									onClick={() => resetLyricOffset()}
								>
									<ArrowReset24Regular className="size-5 drop-shadow-md" />
								</button>
							</TooltipTrigger>
							<TooltipContent side="left" sideOffset={10}>
								重置偏移
							</TooltipContent>
						</Tooltip>
					</>
				)}
			</div>

			<LyricVersionDialog
				open={versionDialogOpen}
				onOpenChange={setVersionDialogOpen}
			/>
		</motion.div>
	);
}
