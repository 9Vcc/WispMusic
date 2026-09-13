import {
	sfAntennaRadiowavesLeftAndRight,
	sfCalendar,
	sfForwardEndFill,
	sfHeartSlashFill,
	sfPauseFill,
	sfPlayFill,
} from "@bradleyhodges/sfsymbols";
import SFIcon from "@bradleyhodges/sfsymbols-react";
import { Vibrant } from "node-vibrant/browser";
import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import useSWR from "swr";
import { getDailyRecommend } from "@/lib/services/recommend";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { Button } from "../ui/button";
import { WispButton } from "../wisp-button";

export function RecommendAndFMSection() {
	return (
		<div className="w-full h-54 flex gap-8">
			<RecommendCard />
			<FmCard />
		</div>
	);
}

// 封面取色缓存：只缓存卡片底色用的 DarkVibrant
const colorCache: Record<string, string> = {};

function RecommendCard() {
	// 与每日推荐页共用同一个 SWR key，互相命中缓存
	const { data } = useSWR("dailyRecommend", getDailyRecommend, {
		revalidateOnFocus: false,
	});
	const songs = data ?? [];
	const coverUrl = songs[0]?.al?.picUrl || "";
	const [vibrant, setVibrant] = useState("");
	const navigate = useNavigate();
	const playQueue = usePlayerStore((s) => s.playQueue);

	useEffect(() => {
		if (!coverUrl) return;

		if (colorCache[coverUrl]) {
			setVibrant(colorCache[coverUrl]);
			return;
		}

		let cancelled = false;
		const v = new Vibrant(coverUrl, {
			quality: 1,
			colorCount: 64,
		});

		v.getPalette()
			.then((palette) => {
				const vibrant = palette.DarkVibrant?.hex || "";

				colorCache[coverUrl] = vibrant;
				if (cancelled) return;
				setVibrant(vibrant);
			})
			.catch((err) => {
				console.error("提取颜色失败", err);
			});

		return () => {
			cancelled = true;
		};
	}, [coverUrl]);

	const date = new Date();
	const day = date.getDate();
	const month = date.getMonth() + 1;

	return (
		<div
			className="group w-full h-full bg-(--dynamic-color) rounded-lg overflow-hidden relative text-white drop-shadow-[0_10px_8px_rgba(0,0,0,0.1)]"
			style={
				{
					"--dynamic-color": vibrant || "gray",
					"--mouse-x": "50%",
					"--mouse-y": "50%",
				} as React.CSSProperties
			}
			onPointerMove={(event) => {
				const rect = event.currentTarget.getBoundingClientRect();
				event.currentTarget.style.setProperty(
					"--mouse-x",
					`${event.clientX - rect.left}px`,
				);
				event.currentTarget.style.setProperty(
					"--mouse-y",
					`${event.clientY - rect.top}px`,
				);
			}}
			onClick={() => {
				navigate("/recommend/daily");
			}}
		>
			<div className="absolute inset-0 z-0 flex items-center justify-center">
				{coverUrl && (
					<img
						src={coverUrl}
						className="w-full h-full object-cover brightness-60"
						alt="Recommend cover"
					/>
				)}
			</div>

			<div
				className="absolute inset-0 z-5 pointer-events-none opacity-0 transition-opacity duration-500 group-hover:opacity-100"
				style={{
					background:
						"radial-gradient(250px circle at var(--mouse-x) var(--mouse-y), rgba(255, 255, 255, 0.3), transparent)",
				}}
			/>

			<div className="relative z-10 w-full h-full p-6 flex flex-col justify-between">
				<span className="text-md font-medium drop-shadow-lg flex items-center gap-2">
					<SFIcon icon={sfCalendar} className="size-4 text-white/80" />
					每日推荐
				</span>

				<div className="flex flex-col gap-2">
					<span className="text-white text-lg drop-shadow-md font-medium line-clamp-1">
						{month} 月 {day} 日，从《{songs[0]?.name}》听起
					</span>
					{/* 播放 CTA：磨砂玻璃质感（半透明主题色 + backdrop-blur 盖在封面上），
					    悬浮时填充加重=显色，与原交互的时机一致 */}
					<Button
						className="w-24 bg-primary/20 font-light text-white backdrop-blur-md drop-shadow-lg transition-all duration-300 hover:border-b-primary-foreground/30 hover:bg-primary/50 hover:shadow-xl"
						onClick={(e) => {
							// 与下方 FmCard 的操作行一致：点播放不该触发卡片的整卡跳转
							e.stopPropagation();
							playQueue(songs);
						}}
					>
						立即播放
					</Button>
				</div>
			</div>
		</div>
	);
}

const fmColorCache: Record<string, string> = {};

function FmCard() {
	const fmPlaylist = usePlayerStore((s) => s.fmPlaylist);
	const fetchFmSongs = usePlayerStore((s) => s.fetchFmSongs);
	const playFm = usePlayerStore((s) => s.playFm);
	const isFmMode = usePlayerStore((s) => s.isFmMode);
	const isPlaying = usePlayerStore((s) => s.isPlaying);
	const togglePlay = usePlayerStore((s) => s.togglePlay);
	const nextFmSong = usePlayerStore((s) => s.nextFmSong);
	const [bgColor, setBgColor] = useState("#2f2f2f");
	const trashFmSong = usePlayerStore((s) => s.trashFmSong);
	const navigate = useNavigate();
	const currentFmSong = fmPlaylist[0];

	useEffect(() => {
		if (fmPlaylist.length === 0) {
			fetchFmSongs();
		}
	}, [fetchFmSongs, fmPlaylist.length]);

	useEffect(() => {
		const url = currentFmSong?.al?.picUrl;
		if (url) {
			if (fmColorCache[url]) {
				setBgColor(fmColorCache[url]);
				return;
			}

			const v = new Vibrant(url);
			v.getPalette().then((palette) => {
				const color = palette.DarkVibrant?.hex || palette.DarkMuted?.hex;
				if (color) {
					fmColorCache[url] = color;
					setBgColor(color);
				}
			});
		}
	}, [currentFmSong]);

	const isPlayingFm = isFmMode && isPlaying;

	return (
		<div
			className="group isolate w-full h-full rounded-lg drop-shadow-[0_10px_8px_rgba(0,0,0,0.1)] relative text-white overflow-hidden shadow-inner cursor-pointer"
			style={{
				backgroundColor: bgColor,
			}}
			onClick={() => navigate("/fm")}
		>
			{currentFmSong?.al?.picUrl && (
				<img
					src={currentFmSong.al.picUrl}
					className="absolute right-0 top-1/2 z-0 h-[165%] w-[58%] -translate-y-1/2 object-cover object-center saturate-110 transition-transform duration-700 ease-out group-hover:scale-[1.035]"
					alt=""
					aria-hidden="true"
				/>
			)}

			<div
				className="absolute inset-0 z-[1] pointer-events-none transition-colors duration-1000"
				style={{
					background: `linear-gradient(90deg, color-mix(in srgb, ${bgColor}, black 38%) 0%, color-mix(in srgb, ${bgColor}, black 30%) 38%, color-mix(in srgb, ${bgColor} 62%, transparent) 62%, rgba(0, 0, 0, 0.1) 100%)`,
				}}
			/>
			<div className="absolute inset-0 z-[1] bg-black/5 pointer-events-none" />

			<div className="relative z-10 h-full w-[58%] min-w-0 p-6 flex flex-col justify-between">
				<span className="text-md font-medium drop-shadow-lg flex items-center gap-2">
					<SFIcon
						icon={sfAntennaRadiowavesLeftAndRight}
						className="size-4 text-white/80"
					/>
					私人漫游
				</span>

				<div className="flex flex-col gap-3">
					<div className="flex min-w-0 flex-col gap-0.5">
						<span className="text-xl font-bold line-clamp-1 drop-shadow-md">
							{currentFmSong?.name}
						</span>
						<span className="text-sm text-white/65 line-clamp-1 font-light tracking-wide">
							{currentFmSong?.ar?.map((ar) => ar.name).join(" / ")}
						</span>
					</div>

					<div
					className="flex gap-4 items-center"
					onClick={(e) => e.stopPropagation()}
				>
						<WispButton
							variant="ghost"
							aria-label="不喜欢"
							className="size-9 rounded-full bg-white/10 hover:bg-white/20! hover:text-white text-white/75 backdrop-blur-sm transition-colors"
							icon={<SFIcon icon={sfHeartSlashFill} className="size-5" />}
							onClick={() => {
								if (!currentFmSong) return;
								trashFmSong();
							}}
						/>
						<WispButton
							variant="ghost"
							aria-label={isPlayingFm ? "暂停" : "播放"}
							className="size-10 rounded-full bg-white/20 hover:bg-white/30! hover:text-white text-white shadow-sm backdrop-blur-sm transition-colors"
							icon={
								<SFIcon
									icon={isPlayingFm ? sfPauseFill : sfPlayFill}
									className="size-5"
								/>
							}
							onClick={() => {
								if (isFmMode) {
									togglePlay();
								} else {
									playFm();
								}
							}}
						/>
						<WispButton
							variant="ghost"
							aria-label="下一首"
							className="size-9 rounded-full bg-white/10 hover:bg-white/20! hover:text-white text-white/75 backdrop-blur-sm transition-colors"
							icon={<SFIcon icon={sfForwardEndFill} className="size-5" />}
							onClick={() => {
								nextFmSong();
							}}
						/>
					</div>
				</div>
			</div>
		</div>
	);
}
