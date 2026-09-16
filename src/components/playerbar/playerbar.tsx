import { useSongLogic } from "@/hooks/use-song-logic";
import type { RepeatType, ShuffleType } from "@/lib/constants/player";
import { useContextMenuStore } from "@/lib/store/contextMenuStore/contextMenuStore";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { useSettingStore } from "@/lib/store/settingStore/settingStore";
import { GetThumbnail, cn } from "@/lib/utils";
import { invoke } from "@tauri-apps/api/core";
import {
	ClosedCaption20Filled,
	ClosedCaption20Regular,
	MoreHorizontal20Regular,
	MusicNote224Filled,
	PictureInPicture20Regular,
} from "@fluentui/react-icons";
import {
	sfArrowDownBackwardAndArrowUpForwardSquareFill,
	sfBackwardFill,
	sfBrandItunesNote,
	sfForwardFill,
	sfHeart,
	sfHeartFill,
	sfHeartSlashFill,
	sfInfinity,
	sfPauseFill,
	sfPlayFill,
	sfRepeat,
	sfRepeat1,
	sfShuffle,
} from "@bradleyhodges/sfsymbols";
import SFIcon from "@bradleyhodges/sfsymbols-react";
import { lazy, Suspense } from "react";
import { Link } from "react-router-dom";
import { Marquee } from "../marquee/marquee";
import { WispButton } from "../wisp-button";
import { MusicLevelPopover } from "../music-level-popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";
import { PlayerBarVolumePopover } from "./player-bar-volume-popover";
import { PlayerBarSlider } from "./playerbar-slider";
import { PlayerBarSpeedPopover } from "./player-bar-speed-popover";
import { PlayerBarAddToPlaylist } from "./player-bar-add-to-playlist";
import { PlaylistSheet } from "./playlist-sheet";

// 歌词页依赖 three/@react-three/fiber，懒加载使其不进首包，首次渲染触发区时才拉取
const LyricSheet = lazy(() =>
	import("../lyric-sheet/lyric-sheet").then((m) => ({ default: m.LyricSheet })),
);

// 播放模式图标统一取播放器区的 SF 符号族（与上一首/下一首/播放同一族），
// 不再混用 Tabler 的描边图标 —— 同一控件组里描边与实心并排会显得两套系统。
const PLAY_MODE_VIEW = {
	sequential: { icon: sfRepeat, label: "顺序播放", active: false },
	list: { icon: sfRepeat, label: "列表循环", active: true },
	random: { icon: sfShuffle, label: "随机播放", active: true },
	single: { icon: sfRepeat1, label: "单曲循环", active: true },
} as const;

type PlayModeView = keyof typeof PLAY_MODE_VIEW;

function derivePlayModeView(
	repeatMode: RepeatType,
	isShuffle: ShuffleType,
): PlayModeView {
	if (isShuffle === "on") return "random";
	if (repeatMode === "repeat") return "list";
	if (repeatMode === "single") return "single";
	return "sequential";
}

export function PlayerBar() {
	return (
		<footer
			className="relative flex h-20 w-full items-center justify-between gap-3 border-t border-border bg-card/60 px-4 backdrop-blur-xl select-none"
			onContextMenu={(e) => e.preventDefault()}
		>
			{/* 进度条贴顶边全宽，与顶部分隔线融合 */}
			<PlayerBarSlider />
			<LeftButtonRegion />
			<CenterButtonRegion />
			<RightButtonRegion />
		</footer>
	);
}

function LeftButtonRegion() {
	const { checkIsLiked, handleLike } = useSongLogic();
	const currentSong = usePlayerStore((s) => s.currentSong);
	const isLike = checkIsLiked("song", currentSong);
	const LikeIcon = isLike ? sfHeartFill : sfHeart;
	const songStr = currentSong?.name || "";

	// 触发区独立成变量：LyricSheet chunk 未就绪时作为 Suspense fallback 原样显示封面
	const coverTrigger = (
		<div className="group relative shrink-0">
			<div className="relative h-14 w-14 overflow-hidden rounded-lg border shadow-sm">
				{currentSong?.al?.picUrl ? (
					<img
						src={GetThumbnail(
							currentSong?.al?.picUrl || currentSong?.album?.picUrl || "",
						)}
						alt="Album cover"
						loading="eager"
						className="h-14 w-14 transition-[filter] duration-150 ease-[ease] group-hover:brightness-50"
						draggable={false}
					/>
				) : (
					<div className="flex size-14 items-center justify-center bg-card text-foreground/40 transition-[filter] duration-150 ease-[ease] group-hover:brightness-50">
						<MusicNote224Filled />
					</div>
				)}
			</div>
			<SFIcon
				icon={sfArrowDownBackwardAndArrowUpForwardSquareFill}
				className="absolute top-1/2 left-1/2 size-5 -translate-x-1/2 -translate-y-1/2 text-white opacity-0 transition-opacity duration-150 ease-[ease] group-hover:opacity-100"
			/>
		</div>
	);

	return (
		<div className="flex min-w-30 max-w-[320px] flex-1 items-center gap-3 overflow-hidden">
			{currentSong ? (
				<>
					<Suspense fallback={coverTrigger}>
						<LyricSheet>{coverTrigger}</LyricSheet>
					</Suspense>
					<div className="flex h-14 min-w-0 flex-1 flex-col justify-center gap-1.5">
					{/* 单行布局：歌名 + 歌手同行，超长分别截断；mt 使文字块整体下移，避免贴顶 */}
					<div className="mt-2 flex min-w-0 items-baseline gap-1.5">
						<Marquee
							text={songStr}
							containerClassName="min-w-0 shrink"
							textClassName="text-sm font-bold text-primary"
						/>
						{/* 歌手列：块级 + truncate 才能对行内链接生效（flex 容器上
						    text-overflow 不作用到 flex 子项，旧写法是硬裁切，
						    宽度一变就露出半个字）；min-w-0 保证能收缩到省略号 */}
						<div className="hidden min-w-0 shrink truncate md:block">
							{currentSong?.ar?.map((ar, idx) => (
								<Link
									to={`/detail/artist?id=${ar.id}`}
									key={`${ar.id}-${idx}`}
									className="text-[13px] text-foreground/60 hover:text-foreground/80"
									draggable={false}
								>
									{ar.name}
									{idx < currentSong!.ar!.length - 1 && "、"}
								</Link>
							))}
						</div>
					</div>
					<div className="flex items-center gap-1">
						<WispButton
							variant="ghost"
							size="icon-xs"
							aria-label="收藏"
							onClick={() => handleLike("song", currentSong)}
							icon={
								<SFIcon
									icon={LikeIcon}
									className={cn(
										"size-5 transition-transform duration-200 hover:scale-110 active:scale-90",
										isLike ? "text-red-500" : "text-foreground/50",
									)}
								/>
							}
						/>
						<PlayerBarAddToPlaylist />
					</div>
				</div>
				</>
			) : (
				<>
					<div className="flex h-14 w-14 items-center justify-center overflow-hidden rounded-lg border shadow-sm">
						<SFIcon
							icon={sfBrandItunesNote}
							className="size-5 text-muted-foreground"
						/>
					</div>
					<span className="text-sm text-foreground/40 select-none">
						未在播放
					</span>
				</>
			)}
		</div>
	);
}

/** 循环方式控件：FM 模式为 FM 单曲循环开关，普通模式为播放模式切换 */
function PlayModeButton() {
	const repeatMode = usePlayerStore((s) => s.repeatMode);
	const isShuffle = usePlayerStore((s) => s.isShuffle);
	const cyclePlayMode = usePlayerStore((s) => s.cyclePlayMode);
	const isFmMode = usePlayerStore((s) => s.isFmMode);
	const fmRepeatMode = usePlayerStore((s) => s.fmRepeatMode);
	const toggleFmRepeatMode = usePlayerStore((s) => s.toggleFmRepeatMode);

	if (isFmMode) {
		return (
			<WispButton
				variant="ghost"
				onClick={toggleFmRepeatMode}
				icon={
					fmRepeatMode ? (
						<SFIcon
							icon={sfRepeat1}
							className="size-5 text-foreground/80"
						/>
					) : (
						<SFIcon
							icon={sfInfinity}
							className="size-5 text-foreground/80"
						/>
					)
				}
			/>
		);
	}

	const playModeConfig = PLAY_MODE_VIEW[derivePlayModeView(repeatMode, isShuffle)];
	const PlayModeIcon = playModeConfig.icon;
	return (
		<Tooltip>
			<TooltipTrigger asChild>
				<WispButton
					variant="ghost"
					onClick={cyclePlayMode}
					icon={<SFIcon icon={PlayModeIcon} className="size-5" />}
					className={cn(
						playModeConfig.active ? "text-foreground" : "text-foreground/50",
					)}
				/>
			</TooltipTrigger>
			<TooltipContent side="top" sideOffset={10}>
				{playModeConfig.label}
			</TooltipContent>
		</Tooltip>
	);
}

/** 桌面歌词开关：与设置页共享 settingStore 唯一可信源，显隐由同步引擎驱动 */
function DesktopLyricButton() {
	const enabled = useSettingStore((s) => s.desktopLyric.enabled);
	const setDesktopLyricEnabled = useSettingStore(
		(s) => s.setDesktopLyricEnabled,
	);
	const CaptionIcon = enabled ? ClosedCaption20Filled : ClosedCaption20Regular;

	return (
		<Tooltip>
			<TooltipTrigger asChild>
				<WispButton
					variant="ghost"
					onClick={() => void setDesktopLyricEnabled(!enabled)}
					icon={
						<CaptionIcon
							className={cn(
								"size-5",
								enabled ? "text-primary" : "text-foreground/70",
							)}
						/>
					}
				/>
			</TooltipTrigger>
			<TooltipContent side="top" sideOffset={10}>
				桌面歌词
			</TooltipContent>
		</Tooltip>
	);
}

function CenterButtonRegion() {
	const togglePlay = usePlayerStore((s) => s.togglePlay);
	const next = usePlayerStore((s) => s.next);
	const prev = usePlayerStore((s) => s.prev);
	const isPlaying = usePlayerStore((s) => s.isPlaying);
	const isLoadingMusic = usePlayerStore((s) => s.isLoadingMusic);
	const isFmMode = usePlayerStore((s) => s.isFmMode);
	const trashFmSong = usePlayerStore((s) => s.trashFmSong);

	const busyClassName = cn(isLoadingMusic && "opacity-75");

	return (
		<div className="flex min-w-37.5 flex-[1.5] items-center justify-center gap-4">
			{isFmMode ? (
				<WispButton
					variant="ghost"
					onClick={trashFmSong}
					icon={<SFIcon icon={sfHeartSlashFill} className="size-6" />}
				/>
			) : (
				<WispButton
					variant="ghost"
					onClick={() => prev(true)}
					icon={<SFIcon icon={sfBackwardFill} className="size-6" />}
					className={busyClassName}
				/>
			)}
			<WispButton
				variant="ghost"
				onClick={() => togglePlay()}
				className="size-11 rounded-full bg-fill-subtle hover:bg-fill-hover"
				icon={
					isLoadingMusic ? (
						<span className="size-5 animate-spin rounded-full border-2 border-current border-t-transparent text-primary" />
					) : isPlaying ? (
						<SFIcon icon={sfPauseFill} className="size-5" />
					) : (
						<SFIcon icon={sfPlayFill} className="size-5" />
					)
				}
			/>
			<WispButton
				variant="ghost"
				onClick={() => next(true)}
				icon={<SFIcon icon={sfForwardFill} className="size-6" />}
				className={busyClassName}
			/>
		</div>
	);
}

function RightButtonRegion() {
	const openMenu = useContextMenuStore((s) => s.openMenu);
	const currentSong = usePlayerStore((s) => s.currentSong);
	const isFmMode = usePlayerStore((s) => s.isFmMode);

	return (
		<div className="flex min-w-30 max-w-[320px] flex-1 items-center justify-end gap-1">
			<PlayerBarSpeedPopover />
			<PlayerBarVolumePopover />
			<PlayModeButton />
			<MusicLevelPopover />
			{!isFmMode && <PlaylistSheet />}
			<DesktopLyricButton />
			<Tooltip>
				<TooltipTrigger asChild>
					<WispButton
						variant="ghost"
						onClick={() =>
							invoke("mini_enter").catch(console.error)
						}
						icon={
							<PictureInPicture20Regular className="size-5 text-foreground/70" />
						}
					/>
				</TooltipTrigger>
				<TooltipContent side="top" sideOffset={10}>
					迷你模式
				</TooltipContent>
			</Tooltip>
			<WispButton
				variant="ghost"
				onClick={(e) => {
					e.preventDefault();
					// 播放列表无歌曲时 currentSong 为 null，song 菜单的操作全部
					// 依赖歌曲数据，弹空菜单无意义（菜单组件已有判空兜底）
					if (!currentSong) return;
					openMenu(e.clientX, e.clientY, "song", currentSong);
				}}
				icon={<MoreHorizontal20Regular className="size-6" />}
			/>
		</div>
	);
}
