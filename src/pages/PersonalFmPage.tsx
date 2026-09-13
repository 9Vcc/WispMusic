import {
	sfAntennaRadiowavesLeftAndRight,
	sfForwardEndFill,
	sfHeartSlashFill,
	sfPauseFill,
	sfPlayFill,
} from "@bradleyhodges/sfsymbols";
import SFIcon from "@bradleyhodges/sfsymbols-react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { Vibrant } from "node-vibrant/browser";
import { Suspense, useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { Entrance, entranceEase } from "@/components/entrance";
import { PinnedBar, usePinned } from "@/components/pinned-bar";
import { Skeleton } from "@/components/ui/skeleton";
import { WispButton } from "@/components/wisp-button";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { useUserStore } from "@/lib/store/userStore/userStore";
import { cn, GetThumbnail } from "@/lib/utils";

/**
 * 私人漫游页：参考 EchoMusic 私人 FM 页的布局——
 * 当前黑胶唱片居中、左右两侧是即将播放的唱片堆叠，
 * 下方是播放控制卡（播放/暂停、不喜欢、下一首）与当前歌曲信息。
 */
function PersonalFmPageContent() {
	const isLoggedin = useUserStore((s) => s.isLoggedin);
	const fmPlaylist = usePlayerStore((s) => s.fmPlaylist);
	const fetchFmSongs = usePlayerStore((s) => s.fetchFmSongs);
	const playFm = usePlayerStore((s) => s.playFm);
	const isFmMode = usePlayerStore((s) => s.isFmMode);
	const isPlaying = usePlayerStore((s) => s.isPlaying);
	const togglePlay = usePlayerStore((s) => s.togglePlay);
	const nextFmSong = usePlayerStore((s) => s.nextFmSong);
	const trashFmSong = usePlayerStore((s) => s.trashFmSong);
	const currentSong = usePlayerStore((s) => s.currentSong);

	const [accentColor, setAccentColor] = useState("");
	const headerRef = useRef<HTMLDivElement>(null);
	const isPinned = usePinned(headerRef);
	const reduceMotion = useReducedMotion();

	useEffect(() => {
		if (isLoggedin && fmPlaylist.length === 0) {
			void fetchFmSongs();
		}
	}, [isLoggedin, fetchFmSongs, fmPlaylist.length]);

	// FM 模式下当前歌即队首；未进入 FM 模式时也用队首作封面预览
	const currentFmSong = isFmMode ? currentSong : fmPlaylist[0];
	const upNext = isFmMode
		? fmPlaylist.filter((s) => s.id !== currentSong?.id).slice(0, 2)
		: fmPlaylist.slice(1, 3);

	useEffect(() => {
		const url = currentFmSong?.al?.picUrl;
		if (!url) return;
		let cancelled = false;
		const v = new Vibrant(url);
		v.getPalette()
			.then((palette) => {
				if (cancelled) return;
				setAccentColor(palette.DarkVibrant?.hex || palette.Vibrant?.hex || "");
			})
			.catch(() => {});
		return () => {
			cancelled = true;
		};
	}, [currentFmSong?.al?.picUrl]);

	if (!isLoggedin) {
		return (
			<div className="flex h-full w-full flex-col items-center justify-center gap-5 text-center">
				<div className="flex size-18 items-center justify-center rounded-3xl bg-primary/10 text-primary">
					<SFIcon icon={sfAntennaRadiowavesLeftAndRight} className="size-8" />
				</div>
				<div className="flex flex-col gap-2">
					<p className="text-xl font-semibold">登录后收听私人漫游</p>
					<p className="text-sm text-foreground/50">
						实时推荐契合口味的音乐，需要登录状态
					</p>
				</div>
			</div>
		);
	}

	const isPlayingFm = isFmMode && isPlaying;

	return (
		<div className="relative flex h-full w-full flex-col pb-8">
			<div
				ref={headerRef}
				aria-hidden="true"
				className="absolute top-4 left-0 h-px w-px"
			/>

			<PinnedBar isPinned={isPinned} title="私人漫游">
				{null}
			</PinnedBar>

			{/* 唱片区：左中右三张黑胶，中间为当前播放（旋转动画） */}
			<div
				className="relative flex min-h-72 w-full items-center justify-center overflow-hidden px-8 pt-8"
				style={
					accentColor
						? ({
								background: `radial-gradient(60% 80% at 50% 40%, color-mix(in srgb, ${accentColor}, transparent 78%) 0%, transparent 70%)`,
							} as React.CSSProperties)
						: undefined
				}
			>
				{/* 左右即将播放的唱片：切歌时队列左移（新碟自右淡入、旧碟向左淡出） */}
				<div className="pointer-events-none absolute inset-0 flex items-center justify-between px-[12%]">
					{[
						{
							song: upNext[1],
							discClass:
								"size-40 -translate-x-6 opacity-35 blur-[1px] xl:size-48",
						},
						{
							song: upNext[0],
							discClass: "size-44 translate-x-8 opacity-55 xl:size-52",
						},
					].map((slot) => (
						<AnimatePresence
							key={slot.discClass}
							mode="popLayout"
							initial={false}
						>
							{slot.song && (
								<motion.div
									key={slot.song.id}
									initial={
										reduceMotion ? { opacity: 0 } : { x: 44, opacity: 0 }
									}
									animate={{ x: 0, opacity: 1 }}
									exit={reduceMotion ? { opacity: 0 } : { x: -44, opacity: 0 }}
									transition={{ duration: 0.35, ease: entranceEase }}
								>
									<VinylDisc
										coverUrl={slot.song.al?.picUrl}
										className={slot.discClass}
									/>
								</motion.div>
							)}
						</AnimatePresence>
					))}
				</div>

				<button
					type="button"
					aria-label={isPlayingFm ? "暂停" : "播放"}
					className="relative z-10 outline-none"
					onClick={() => {
						if (isFmMode) togglePlay();
						else void playFm();
					}}
				>
					<AnimatePresence mode="popLayout" initial={false}>
						{currentFmSong ? (
							<motion.div
								key={currentFmSong.id}
								initial={
									reduceMotion
										? { opacity: 0 }
										: { x: 150, opacity: 0, rotate: 22 }
								}
								animate={{ x: 0, opacity: 1, rotate: 0 }}
								exit={
									reduceMotion
										? { opacity: 0 }
										: { x: -190, opacity: 0, rotate: -24 }
								}
								transition={{ type: "spring", stiffness: 240, damping: 26 }}
								whileHover={{ scale: 1.03 }}
							>
								<VinylDisc
									coverUrl={currentFmSong.al?.picUrl}
									spinning={isPlayingFm}
									className="size-56 shadow-[0_24px_48px_rgba(0,0,0,0.35)] xl:size-64"
								/>
							</motion.div>
						) : (
							<Skeleton className="size-56 rounded-full xl:size-64" />
						)}
					</AnimatePresence>
				</button>
			</div>

			{/* 控制区 + 当前歌曲信息 */}
			<Entrance className="flex flex-col items-center gap-6 px-8">
				<div className="flex items-center gap-6">
					<WispButton
						variant="ghost"
						aria-label="不喜欢"
						className="size-11 rounded-full bg-fill-subtle hover:bg-fill-hover text-foreground/60 hover:text-foreground transition-all active:scale-95"
						icon={<SFIcon icon={sfHeartSlashFill} className="size-5" />}
						onClick={() => void trashFmSong()}
					/>
					<WispButton
						variant="glass"
						size="lg"
						aria-label={isPlayingFm ? "暂停" : "播放"}
						className="size-14 rounded-full"
						icon={
							<SFIcon
								icon={isPlayingFm ? sfPauseFill : sfPlayFill}
								className={cn("size-6", !isPlayingFm && "ml-0.5")}
							/>
						}
						onClick={() => {
							if (isFmMode) togglePlay();
							else void playFm();
						}}
					/>
					<WispButton
						variant="ghost"
						aria-label="下一首"
						className="size-11 rounded-full bg-fill-subtle hover:bg-fill-hover text-foreground/60 hover:text-foreground transition-all active:scale-95"
						icon={<SFIcon icon={sfForwardEndFill} className="size-5" />}
						onClick={() => nextFmSong()}
					/>
				</div>

				<div className="relative flex max-w-2xl flex-col items-center gap-1.5 text-center">
					<AnimatePresence mode="popLayout" initial={false}>
						{currentFmSong ? (
							<motion.div
								key={currentFmSong.id}
								className="flex flex-col items-center gap-1.5"
								initial={reduceMotion ? { opacity: 0 } : { y: 14, opacity: 0 }}
								animate={{ y: 0, opacity: 1 }}
								exit={reduceMotion ? { opacity: 0 } : { y: -10, opacity: 0 }}
								transition={{ duration: 0.3, ease: entranceEase }}
							>
								<span className="text-xl font-bold line-clamp-1">
									{currentFmSong.name}
								</span>
								<div className="flex flex-wrap items-center justify-center gap-1 text-sm text-foreground/60">
									{/* 同一歌手可能在 ar 里重复出现，先按 id 去重再渲染，
									    key 直接用 id（避免 noArrayIndexKey） */}
									{(currentFmSong.ar
										? [
												...new Map(
													currentFmSong.ar.map((a) => [a.id, a]),
												).values(),
											]
										: []
									).map((ar, idx, arr) => (
										<Link
											to={`/detail/artist?id=${ar.id}`}
											key={ar.id}
											className="hover:text-foreground/80"
											draggable={false}
										>
											{ar.name}
											{idx < arr.length - 1 && "、"}
										</Link>
									))}
									{currentFmSong.al?.name && (
										<>
											<span className="text-foreground/35">-</span>
											<Link
												to={`/detail/album?id=${currentFmSong.al.id}`}
												className="hover:text-foreground/80"
												draggable={false}
											>
												{currentFmSong.al.name}
											</Link>
										</>
									)}
								</div>
							</motion.div>
						) : (
							<div className="flex flex-col items-center gap-2">
								<Skeleton className="h-7 w-64" />
								<Skeleton className="h-4 w-40" />
							</div>
						)}
					</AnimatePresence>
				</div>

				<p className="text-xs text-foreground/40 select-none">
					实时推送契合口味的音乐，随反馈动态更新
				</p>
			</Entrance>
		</div>
	);
}

/** 黑胶唱片：封面圆形裁切 + 中心圆孔，spinning 时旋转 */
function VinylDisc({
	coverUrl,
	spinning = false,
	className,
}: {
	coverUrl?: string;
	spinning?: boolean;
	className?: string;
}) {
	return (
		<div
			className={cn(
				"relative aspect-square overflow-hidden rounded-full bg-black/80 shadow-lg",
				spinning && "animate-[spin_18s_linear_infinite]",
				className,
			)}
			style={{
				backgroundImage: coverUrl
					? `url("${GetThumbnail(coverUrl).replace(/"/g, "%22")}")`
					: undefined,
				backgroundSize: "100% 100%",
			}}
		>
			{/* 黑胶纹理：径向暗角 + 中心圆孔 */}
			<div
				className="absolute inset-0"
				style={{
					background:
						"radial-gradient(circle, transparent 26%, rgba(0,0,0,0.28) 30%, transparent 34%, transparent 46%, rgba(0,0,0,0.22) 50%, transparent 54%)",
				}}
			/>
			<div className="absolute top-1/2 left-1/2 size-[18%] -translate-x-1/2 -translate-y-1/2 rounded-full border border-white/25 bg-white/85" />
		</div>
	);
}

export default function PersonalFmPage() {
	return (
		<Suspense>
			<PersonalFmPageContent />
		</Suspense>
	);
}
