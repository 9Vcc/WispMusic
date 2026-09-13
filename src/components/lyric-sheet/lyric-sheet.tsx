import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import type { MouseEvent } from "react";
import {
	Children,
	cloneElement,
	isValidElement,
	useCallback,
	useEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import { createPortal } from "react-dom";
import { useHotkeys } from "react-hotkeys-hook";
import { CLOSE_LYRIC_SHEET_EVENT } from "@/lib/events/lyric-sheet";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { selectActiveLocalLyric } from "@/lib/store/playerStore/playerStoreSlice/createLocalLyricSlice";
import { selectActiveLyricVersion } from "@/lib/store/playerStore/playerStoreSlice/createLyricVersionSlice";
import { extractBackgroundColors } from "@/lib/utils/color-extractor";
import { ParseLyric } from "@/lib/utils/lyric-parser";
import { Lyric } from "../lyric/lyric";
import {
	DEFAULT_LYRIC_SHEET_COLORS,
	LyricSheetBackground,
} from "./lyric-sheet-background";
import { LyricSheetBottomBar } from "./lyric-sheet-bottom-bar";
import { getSidePanelMotion, lyricSheetEase } from "./lyric-sheet-motion";
import { LyricSheetSonginfo } from "./lyric-sheet-songinfo";
import { LyricSheetSonglist } from "./lyric-sheet-songlist";
import { LyricSheetTitlebar } from "./lyric-sheet-titlebar";
import { LyricSideControls } from "./lyric-side-controls";

// Keep the stage entrance to a brief opacity-only fade: while an ancestor of
// the blend surfaces animates opacity/filter/transform, plus-lighter is
// suspended (isolated group) and visibly switches on when the animation
// settles. A short fade ends while the blend elements are still fading
// themselves in, so the switch lands on moving, semi-transparent content
// where it can't be seen. (The exit runs on the AnimatePresence wrapper
// around the whole sheet — slide-down + fade, no filter: a residual filter
// would form a stacking context at rest and permanently isolate the
// plus-lighter blending; the temporary isolation during the exit itself is
// invisible because everything is moving out.)
const sheetStageMotion = {
	initial: {
		opacity: 0,
	},
	animate: {
		opacity: 1,
	},
	transition: {
		duration: 0.25,
		ease: "easeOut" as const,
	},
};

// 收起过渡：整页（含 mesh 背景）向下拉出屏幕，呈“下拉收起”的抽屉感。
const sheetRootExit = {
	opacity: 0,
	y: "100%",
	transition: { duration: 0.5, ease: lyricSheetEase },
};

// 歌词页按设计基准尺寸（1440×900）随窗口整体缩放：
// 小窗口（用户可将最小尺寸调到 800×480）下歌词/封面等比缩小避免溢出，
// 大窗口下适度放大。缩放通过 --app-lyric-scale 注入，各元素以 calc() 消费，
// 不使用 transform/zoom，避免破坏 plus-lighter 混合与滚动居中计算。
const LYRIC_SHEET_BASE_W = 1440;
const LYRIC_SHEET_BASE_H = 900;
const LYRIC_SCALE_MIN = 0.55;
const LYRIC_SCALE_MAX = 1.15;

function computeLyricScale() {
	const w = window.innerWidth;
	const h = window.innerHeight;
	return Math.min(
		LYRIC_SCALE_MAX,
		Math.max(
			LYRIC_SCALE_MIN,
			Math.min(w / LYRIC_SHEET_BASE_W, h / LYRIC_SHEET_BASE_H),
		),
	);
}

export function LyricSheet({ children }: { children: React.ReactNode }) {
	const [isOpen, setIsOpen] = useState(false);
	const [lyricScale, setLyricScale] = useState(1);
	const currentSong = usePlayerStore((s) => s.currentSong);
	const currentCoverUrl = currentSong?.al?.picUrl || currentSong?.album?.picUrl;
	const [isMounted, setIsMounted] = useState(false);
	const [isPlaylistOpen, setIsPlaylistOpen] = useState(false);
	const [isLyricOpen, setIsLyricOpen] = useState(true);
	const togglePlay = usePlayerStore((s) => s.togglePlay);
	const currentSongLyrics = usePlayerStore((s) => s.currentSongLyrics);
	// 本地歌词优先：它没有翻译/罗马音分轨，激活时对应开关一并下线
	const activeLocalLyric = usePlayerStore(selectActiveLocalLyric);
	const reduceMotion = Boolean(useReducedMotion());
	const sidePanelMotion = getSidePanelMotion(reduceMotion);

	useEffect(() => {
		const updateScale = () => setLyricScale(computeLyricScale());
		updateScale();
		window.addEventListener("resize", updateScale);
		return () => window.removeEventListener("resize", updateScale);
	}, []);

	useEffect(() => {
		if (currentCoverUrl) void extractBackgroundColors(currentCoverUrl);
	}, [currentCoverUrl]);

	const [showTrans, setShowTrans] = useState(false);
	const [showRoma, setShowRoma] = useState(false);
	// 歌词页右侧悬浮工具条：默认隐藏，鼠标悬浮歌词页右侧区域时淡入
	// （hover 热区 = 歌词面板 + 右缘 48px 工具槽；120ms 宽限避免面板↔按钮间闪烁）
	const [isLyricSideHovered, setIsLyricSideHovered] = useState(false);
	const sideHoverHideTimerRef = useRef<NodeJS.Timeout | null>(null);
	const handleLyricSideEnter = useCallback(() => {
		if (sideHoverHideTimerRef.current) {
			clearTimeout(sideHoverHideTimerRef.current);
			sideHoverHideTimerRef.current = null;
		}
		setIsLyricSideHovered(true);
	}, []);
	const handleLyricSideLeave = useCallback(() => {
		if (sideHoverHideTimerRef.current) clearTimeout(sideHoverHideTimerRef.current);
		sideHoverHideTimerRef.current = setTimeout(() => {
			sideHoverHideTimerRef.current = null;
			setIsLyricSideHovered(false);
		}, 120);
	}, []);
	useEffect(
		() => () => {
			if (sideHoverHideTimerRef.current)
				clearTimeout(sideHoverHideTimerRef.current);
		},
		[],
	);
	// 手动选择的歌词版本（酷狗候选）没有翻译/罗马音分轨，激活时下线这两个开关
	const hasManualLyricVersion = usePlayerStore(selectActiveLyricVersion) !== null;
	const [lastBackgroundColors, setLastBackgroundColors] = useState<
		[number, number, number][]
	>(DEFAULT_LYRIC_SHEET_COLORS);
	const hasTransLyric = useMemo(() => {
		if (hasManualLyricVersion || activeLocalLyric) return false;
		const ytlrc = ParseLyric(currentSongLyrics?.ytlrc?.lyric);
		const tlyric = ParseLyric(currentSongLyrics?.tlyric?.lyric);
		return Boolean(ytlrc?.length || tlyric?.length);
	}, [hasManualLyricVersion, activeLocalLyric, currentSongLyrics]);
	const hasRomaLyric = useMemo(() => {
		if (hasManualLyricVersion || activeLocalLyric) return false;
		const yromaLyric = ParseLyric(currentSongLyrics?.yromalrc?.lyric);
		const romaLyric = ParseLyric(currentSongLyrics?.romalrc?.lyric);
		return Boolean(yromaLyric?.length || romaLyric?.length);
	}, [hasManualLyricVersion, activeLocalLyric, currentSongLyrics]);
	const trigger = Children.toArray(children).find(isValidElement);
	const triggerElement = isValidElement<{
		onClick?: (event: MouseEvent) => void;
	}>(trigger)
		? cloneElement(trigger, {
				onClick: (event: MouseEvent) => {
					trigger.props.onClick?.(event);
					if (!event.defaultPrevented) setIsOpen(true);
				},
			})
		: null;
	// plus-lighter adds glyph luminance onto the background, so on bright
	// backdrops the played-word alpha (0.85) clips straight to white. Scale
	// every lyric mask alpha down as the extracted palette gets brighter;
	// consumed in verbatim-word via var(--lyric-alpha-scale).
	const lyricAlphaScale = useMemo(() => {
		const luminance =
			lastBackgroundColors.reduce(
				(sum, [r, g, b]) => sum + 0.2126 * r + 0.7152 * g + 0.0722 * b,
				0,
			) / Math.max(1, lastBackgroundColors.length);
		return Math.min(1, Math.max(0.68, 1.08 - luminance * 0.75));
	}, [lastBackgroundColors]);
	const handleBackgroundColorsChange = useCallback(
		(colors: [number, number, number][]) => {
			setLastBackgroundColors(colors);
		},
		[],
	);

	useEffect(() => {
		if (!hasTransLyric && showTrans) setShowTrans(false);
		if (!hasRomaLyric && showRoma) setShowRoma(false);
	}, [hasRomaLyric, hasTransLyric, showRoma, showTrans]);

	// 每次打开歌词页都默认展开歌词面板，用户仍可在底部栏手动收起
	useEffect(() => {
		if (!isOpen) return;
		setIsMounted(true);
		setIsLyricOpen(true);
	}, [isOpen]);

	useEffect(() => {
		function handleCloseLyricSheet() {
			setIsOpen(false);
			setIsPlaylistOpen(false);
			setIsLyricOpen(false);
		}

		window.addEventListener(CLOSE_LYRIC_SHEET_EVENT, handleCloseLyricSheet);
		return () => {
			window.removeEventListener(
				CLOSE_LYRIC_SHEET_EVENT,
				handleCloseLyricSheet,
			);
		};
	}, []);

	useEffect(() => {
		if (!isOpen) return;

		const handleKeyDown = (event: KeyboardEvent) => {
			if (event.key === "Escape") setIsOpen(false);
		};

		window.addEventListener("keydown", handleKeyDown);
		return () => window.removeEventListener("keydown", handleKeyDown);
	}, [isOpen]);

	useHotkeys(
		"space",
		(e) => {
			e.preventDefault();
			togglePlay();
		},
		{ enableOnFormTags: false, enabled: isOpen },
		[isOpen, togglePlay],
	);

	return (
		<>
			{triggerElement}
			{isMounted &&
				createPortal(
					<div
						className={`fixed inset-0 z-50 h-screen w-screen overflow-hidden text-sm ${
							isOpen ? "pointer-events-auto" : "pointer-events-none"
						}`}
						onContextMenu={(e) => e.preventDefault()}
						style={
							{
								"--lyric-alpha-scale": lyricAlphaScale,
								"--app-lyric-scale": lyricScale,
							} as React.CSSProperties
						}
					>
						<h2 className="sr-only">Lyrics</h2>
					<AnimatePresence onExitComplete={() => setIsMounted(false)}>
						{isOpen && (
							<motion.div
								key="lyric-sheet-root"
								className="absolute inset-0"
								initial={false}
								animate={{ opacity: 1, scale: 1, y: "0%" }}
								exit={
									reduceMotion
										? { opacity: 0, transition: { duration: 0.2 } }
										: sheetRootExit
								}
							>
								<LyricSheetBackground
									onColorsChange={handleBackgroundColorsChange}
								/>
								<motion.div
									{...sheetStageMotion}
									className="relative flex h-full w-full flex-col"
									onMouseDown={(e) => {
										e.preventDefault();
									}}
								>
									<LyricSheetTitlebar setIsOpen={setIsOpen} />

									<div className="relative flex min-h-0 w-full flex-1 justify-between">
										<motion.div
											layout
											initial={false}
											animate={{
												x: "0%",
												width: isPlaylistOpen || isLyricOpen ? "50%" : "100%",
											}}
											transition={
												reduceMotion
													? { duration: 0 }
													: { duration: 0.48, ease: lyricSheetEase }
											}
											className="relative flex h-full shrink-0 flex-col items-center justify-center pt-16 text-white"
										>
											<LyricSheetSonginfo setIsOpen={setIsOpen} />
										</motion.div>
										<AnimatePresence>
											{isPlaylistOpen && (
												<motion.div
													{...sidePanelMotion}
													className="absolute top-24 right-24 bottom-12 w-[calc(50%-48px)]"
												>
													<LyricSheetSonglist
														className="flex h-full w-full"
														setOpen={setIsOpen}
													/>
												</motion.div>
											)}
										</AnimatePresence>
									<AnimatePresence>
										{isLyricOpen && (
											// biome-ignore lint/a11y/noStaticElementInteractions: 这是右侧工具条的 hover 热区（纯显隐检测层，pointer-events 交给歌词面板与工具条）
											<div
												className="pointer-events-none absolute top-18 right-0 bottom-0 w-[50%]"
												onMouseEnter={handleLyricSideEnter}
												onMouseLeave={handleLyricSideLeave}
											>
												<div className="pointer-events-auto absolute top-0 right-12 bottom-0 w-[calc(100%-48px)]">
													<Lyric
														className="flex h-full w-full"
														showTrans={showTrans}
														showRoma={showRoma}
													/>
												</div>
												<LyricSideControls visible={isLyricSideHovered} />
											</div>
										)}
									</AnimatePresence>
									</div>

									<LyricSheetBottomBar
										isLyricOpen={isLyricOpen}
										isPlaylistOpen={isPlaylistOpen}
										hasTransLyric={hasTransLyric}
										hasRomaLyric={hasRomaLyric}
										showTrans={showTrans}
										showRoma={showRoma}
										onLyricOpenChangeAction={setIsLyricOpen}
										onPlaylistOpenChangeAction={setIsPlaylistOpen}
										onShowTransChangeAction={setShowTrans}
										onShowRomaChangeAction={setShowRoma}
									/>
								</motion.div>
							</motion.div>
						)}
					</AnimatePresence>
				</div>,
					document.body,
				)}
	</>
	);
}
