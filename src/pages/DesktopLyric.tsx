import {
	sfBackwardFill,
	sfForwardFill,
	sfPauseFill,
	sfPlayFill,
} from "@bradleyhodges/sfsymbols";
import SFIcon from "@bradleyhodges/sfsymbols-react";
import { Dismiss20Regular } from "@fluentui/react-icons";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useMotionValue } from "framer-motion";
import { useCallback, useEffect, useRef, useState } from "react";
import { DesktopDefaultView } from "@/components/desktop-lyric/desktop-lyric-default-view";
import { DesktopStrokeView } from "@/components/desktop-lyric/desktop-lyric-stroke-view";
import type {
	DesktopLyricDirection,
	LyricAnimationStyle,
} from "@/lib/store/settingStore/settingStore";
import {
	computeDesktopLyricGeometry,
	DESKTOP_LYRIC_HEADER_RESERVE,
} from "@/lib/utils/desktop-lyric-geometry";
import type { ILyricLine } from "@/lib/utils/lyric-parser";

/**
 * 桌面歌词悬浮控件窗口（/desktop-lyric，独立置顶透明 webview）。
 *
 * 状态全部来自主窗口经 Rust 定向转发的 desktop-lyric-sync-state 推送
 * （full=歌词全文快照+offsetMs / progress=进度锚点 / settings=方向·动效·
 * 字号），自身不持有权威状态；工具条操作经 desktop_lyric_command 回传
 * 主窗口落库/驱动播放后随推送回流。
 *
 * 播放进度本地插值：主窗口按 300ms 推锚点（currentTime + 墙钟 + 播放态），
 * 本窗口 rAF 逐帧推进并叠加歌词偏移，驱动行切换与逐字/笔画动效。
 * 窗口几何（显隐/横竖切换缩放/落位钳制）由主窗口统一驱动，本页面只做
 * 内容自适应布局。
 *
 * 布局对齐 EchoMusic：窗口恒定全透明（背景材质与主题/设置解耦，
 * 仅歌词文字浮在桌面）；横向为三区头部（歌名 / 上一首·播放·下一首 /
 * 显示工具）+ 双行歌词舞台，头部 hover 淡入；竖向头部收纳为居中
 * 悬浮胶囊，歌词占满全窗。
 */

interface FullState {
	songKey: string;
	title: string;
	artist: string;
	lines: ILyricLine[];
	offsetMs: number;
}

interface SettingsState {
	direction: DesktopLyricDirection;
	animationStyle: LyricAnimationStyle;
	fontSize: number;
}

type SyncPayload =
	| ({ type: "full" } & FullState)
	| ({ type: "settings" } & SettingsState)
	| {
			type: "progress";
			currentTime: number;
			isPlaying: boolean;
			at: number;
	  };

/** 与 settingStore 的 defaultDesktopLyricSettings 保持一致的首帧兜底值 */
const INITIAL_FULL: FullState = {
	songKey: "none",
	title: "",
	artist: "",
	lines: [],
	offsetMs: 0,
};
const INITIAL_SETTINGS: SettingsState = {
	direction: "horizontal",
	animationStyle: "default",
	fontSize: 40,
};

/** 工具条玻璃按钮：与歌词页玻璃控件同一语言（rounded-full + 白色图标） */
function ToolButton({
	label,
	onClick,
	children,
}: {
	label: string;
	onClick: () => void;
	children: React.ReactNode;
}) {
	return (
		<button
			type="button"
			aria-label={label}
			title={label}
			onClick={onClick}
			className="flex size-7 items-center justify-center rounded-full text-white/85 transition-all hover:bg-white/15 hover:text-white active:scale-90"
		>
			{children}
		</button>
	);
}

/** 方向切换图标：显示即将切换到的方向（横线组=切横向，竖线组=切竖向） */
function DirectionIcon({ targetVertical }: { targetVertical: boolean }) {
	return targetVertical ? (
		<svg
			viewBox="0 0 24 24"
			className="size-4"
			fill="none"
			stroke="currentColor"
			strokeWidth="2"
			strokeLinecap="round"
		>
			<title>切换为竖向显示</title>
			<path d="M6 4v16M12 4v16M18 4v16" />
		</svg>
	) : (
		<svg
			viewBox="0 0 24 24"
			className="size-4"
			fill="none"
			stroke="currentColor"
			strokeWidth="2"
			strokeLinecap="round"
		>
			<title>切换为横向显示</title>
			<path d="M4 6h16M4 12h16M4 18h16" />
		</svg>
	);
}

/** 动效切换图标：显示即将切换到的方案（笔触=切笔画，半填充圆=切默认） */
function TargetStyleIcon({ targetStroke }: { targetStroke: boolean }) {
	if (targetStroke) {
		return (
			<svg
				viewBox="0 0 24 24"
				className="size-4"
				fill="none"
				stroke="currentColor"
				strokeWidth="2"
				strokeLinecap="round"
				strokeLinejoin="round"
			>
				<title>切换为笔画动效</title>
				<path d="M4 17.5c3.2-.9 5.3-3.6 6.4-6.6 1-2.6 2.4-4.6 4.6-5.6 2-.9 4 .1 4 2 0 3-3.6 4.9-6.7 6-3 1.1-5.6 2.2-8.3 4.2z" />
			</svg>
		);
	}
	return (
		<svg viewBox="0 0 24 24" className="size-4">
			<title>切换为默认动效</title>
			<circle
				cx="12"
				cy="12"
				r="8"
				fill="none"
				stroke="currentColor"
				strokeWidth="2"
			/>
			<path d="M12 4a8 8 0 0 1 0 16z" fill="currentColor" />
		</svg>
	);
}

export default function DesktopLyric() {
	const [full, setFull] = useState<FullState>(INITIAL_FULL);
	const [settings, setSettings] = useState<SettingsState>(INITIAL_SETTINGS);
	// 播放态只驱动传输按钮图标，随 progress 锚点回流（值不变时 setState 无副作用）
	const [isPlaying, setIsPlaying] = useState(false);

	// 进度锚点（主窗口推送）→ 本地 rAF 插值，写入 currentTimeMotion
	const anchorRef = useRef({ time: 0, at: 0, playing: false });
	const currentTimeMotion = useMotionValue(0);

	// 窗口句柄只取一次（与 mini 播放器同因：避免 effect 重跑窗口跳动）
	const winRef = useRef(getCurrentWindow());

	// 用户可拖拽窗口边缘自由缩放（resizable: true）；歌词舞台按"设计尺寸 →
	// 实际尺寸"等比缩放（横向扣除头部保留高度），字号视觉随窗口大小变化，
	// 方向/字号设置变更时主窗口会把窗口重置回标准几何，缩放比随之回到 1
	const stageRef = useRef<HTMLDivElement>(null);
	const [contentScale, setContentScale] = useState(1);
	useEffect(() => {
		const el = stageRef.current;
		if (!el) return;
		const design = computeDesktopLyricGeometry(
			settings.direction,
			settings.fontSize,
		);
		const stageDesignHeight =
			design.height -
			(settings.direction === "horizontal" ? DESKTOP_LYRIC_HEADER_RESERVE : 0);
		const update = () => {
			const next = Math.min(
				el.clientWidth / design.width,
				el.clientHeight / stageDesignHeight,
			);
			setContentScale(Number.isFinite(next) && next > 0 ? next : 1);
		};
		update();
		const ro = new ResizeObserver(update);
		ro.observe(el);
		return () => ro.disconnect();
	}, [settings.direction, settings.fontSize]);

	useEffect(() => {
		const unlisten = listen<SyncPayload>("desktop-lyric-sync-state", (e) => {
			const p = e.payload;
			if (p.type === "full") {
				setFull({
					songKey: p.songKey,
					title: p.title,
					artist: p.artist,
					lines: p.lines,
					offsetMs: p.offsetMs,
				});
			} else if (p.type === "settings") {
				setSettings({
					direction: p.direction,
					animationStyle: p.animationStyle,
					fontSize: p.fontSize,
				});
			} else {
				anchorRef.current = {
					time: p.currentTime,
					at: p.at,
					playing: p.isPlaying,
				};
				setIsPlaying(p.isPlaying);
			}
		});
		// 挂载完成（含 HMR 重挂载）后主动要一次全量快照
		invoke("desktop_lyric_command", {
			payload: { event: "ready" },
		}).catch(console.error);
		return () => {
			unlisten.then((f) => f());
		};
	}, []);

	// rAF 插值：播放中按锚点 + 本机流逝时间推进；暂停时停在锚点。
	// 歌词时间 = 播放进度 × 1000 + 偏移（偏移只进时钟的约定与歌词页一致）
	useEffect(() => {
		let raf = 0;
		const tick = () => {
			const { time, at, playing } = anchorRef.current;
			const elapsed = playing ? (Date.now() - at) / 1000 : 0;
			currentTimeMotion.set((time + elapsed) * 1000 + full.offsetMs);
			raf = requestAnimationFrame(tick);
		};
		raf = requestAnimationFrame(tick);
		return () => cancelAnimationFrame(raf);
	}, [currentTimeMotion, full.offsetMs]);

	// 行索引跟踪（与歌词页同算法：最后一条 lineStart ≤ 当前歌词时间的行）
	const [currentIndex, setCurrentIndex] = useState(-1);
	const lines = full.lines;
	useEffect(() => {
		return currentTimeMotion.on("change", (ms) => {
			let idx = -1;
			for (let i = lines.length - 1; i >= 0; i--) {
				const l = lines[i];
				if (l.lineStart <= ms && l.lineStart >= 0) {
					idx = i;
					break;
				}
			}
			setCurrentIndex((prev) => (prev === idx ? prev : idx));
		});
	}, [lines, currentTimeMotion]);

	const getNowMs = useCallback(
		() => currentTimeMotion.get(),
		[currentTimeMotion],
	);

	const currentLine = currentIndex >= 0 ? lines[currentIndex] : undefined;
	// 下一句预览跳过过门圆点行（圆点行自己晋级时由呼吸圆点兜底接管）
	let nextLine = currentIndex >= 0 ? lines[currentIndex + 1] : undefined;
	if (nextLine?.isLeadDots) {
		nextLine = lines[currentIndex + 2];
	}

	// 工具条指令：一律回传主窗口（settingStore / playerStore 是唯一可信源）
	const send = useCallback((payload: Record<string, unknown>) => {
		invoke("desktop_lyric_command", { payload }).catch(console.error);
	}, []);

	// 拖动窗口：歌词区域按住即可拖动（工具条按钮除外），与 mini 播放器同法
	const handleDragPointerDown = useCallback((e: React.PointerEvent) => {
		if (e.button !== 0) return;
		if ((e.target as HTMLElement).closest("button, input")) return;
		winRef.current.startDragging().catch(console.error);
	}, []);

	const isVertical = settings.direction === "vertical";

	// 显示工具组：方向 / 动效 / 字号 / 关闭（横竖两种头部共用）
	const displayTools = (
		<>
			<ToolButton
				label={isVertical ? "切换为横向显示" : "切换为竖向显示"}
				onClick={() =>
					send({
						event: "setDirection",
						direction: isVertical ? "horizontal" : "vertical",
					})
				}
			>
				<DirectionIcon targetVertical={!isVertical} />
			</ToolButton>
			<ToolButton
				label={
					settings.animationStyle === "default"
						? "切换为笔画动效"
						: "切换为默认动效"
				}
				onClick={() =>
					send({
						event: "setAnimationStyle",
						style: settings.animationStyle === "default" ? "stroke" : "default",
					})
				}
			>
				<TargetStyleIcon targetStroke={settings.animationStyle === "default"} />
			</ToolButton>
			<ToolButton
				label="减小字号"
				onClick={() => send({ event: "setFontDelta", delta: -2 })}
			>
				<span className="text-[11px] font-semibold leading-none">A</span>
			</ToolButton>
			<ToolButton
				label="增大字号"
				onClick={() => send({ event: "setFontDelta", delta: 2 })}
			>
				<span className="text-[15px] font-semibold leading-none">A</span>
			</ToolButton>
			<ToolButton label="关闭桌面歌词" onClick={() => send({ event: "close" })}>
				<Dismiss20Regular className="size-4" />
			</ToolButton>
		</>
	);

	return (
		// 根层恒定全透明：桌面歌词没有任何窗口背板（非 hover 与 hover 均
		// 如此，不随主题/材质设置变化），只有歌词文字与其描边背衬层浮在
		// 桌面上；可读性由 glyph-backdrop 保证。拖拽绑定在根层而非歌词
		// 舞台：舞台会被等比缩放，绑根层保证任何缩放比下整个窗口都可
		// 按住拖动
		<div
			className="group relative flex h-full w-full select-none flex-col overflow-hidden"
			onPointerDown={handleDragPointerDown}
		>
			{isVertical ? (
				// 竖向：头部收纳为居中悬浮胶囊（hover 淡入），歌词占满全窗
				<div className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center opacity-0 transition-opacity duration-200 group-hover:opacity-100">
					<div
						className="pointer-events-none flex items-center gap-0.5 rounded-full border border-white/15 bg-black/45 p-1 shadow-[0_8px_24px_rgba(0,0,0,0.35)] group-hover:pointer-events-auto"
						style={{ fontFamily: '"Microsoft Yahei"' }}
					>
						{displayTools}
					</div>
				</div>
			) : (
				// 横向：EchoMusic 式三区头部（歌名 / 传输控制 / 显示工具），
				// 常驻占位（歌词舞台高度随之扣减），hover 才淡入。pointer-
				// events 只在 hover 时下放到各胶囊——若整层启用会拦掉拖动
				<header className="relative z-10 h-12 shrink-0 px-3 pt-2">
					<div
						className="pointer-events-none flex h-full items-center justify-between gap-2 opacity-0 transition-opacity duration-200 group-hover:opacity-100"
						style={{ fontFamily: '"Microsoft Yahei"' }}
					>
						<div className="pointer-events-none min-w-0 flex-1 group-hover:pointer-events-auto">
							<div className="flex items-center rounded-full border border-white/10 bg-black/45 px-3 py-1 shadow-[0_8px_24px_rgba(0,0,0,0.35)]">
								<span className="truncate text-[13px] font-medium text-white/75">
									{full.title || "WispMusic"}
									{full.artist ? ` - ${full.artist}` : ""}
								</span>
							</div>
						</div>
						<div className="pointer-events-none flex shrink-0 items-center gap-0.5 rounded-full border border-white/15 bg-black/45 p-1 shadow-[0_8px_24px_rgba(0,0,0,0.35)] group-hover:pointer-events-auto">
							<ToolButton
								label="上一首"
								onClick={() => send({ event: "previousTrack" })}
							>
								<SFIcon icon={sfBackwardFill} className="size-4" />
							</ToolButton>
							<ToolButton
								label={isPlaying ? "暂停" : "播放"}
								onClick={() => send({ event: "togglePlayback" })}
							>
								<SFIcon
									icon={isPlaying ? sfPauseFill : sfPlayFill}
									className="size-4"
								/>
							</ToolButton>
							<ToolButton
								label="下一首"
								onClick={() => send({ event: "nextTrack" })}
							>
								<SFIcon icon={sfForwardFill} className="size-4" />
							</ToolButton>
						</div>
						<div className="pointer-events-none flex shrink-0 items-center gap-0.5 rounded-full border border-white/15 bg-black/45 p-1 shadow-[0_8px_24px_rgba(0,0,0,0.35)] group-hover:pointer-events-auto">
							{displayTools}
						</div>
					</div>
				</header>
			)}

			{/* 歌词舞台：字体走全局歌词字体变量（含用户自定义字体），
			    内容随舞台实际尺寸等比缩放 */}
			<div
				ref={stageRef}
				className="relative flex min-h-0 flex-1 items-center justify-center"
				style={{
					fontFamily: "var(--app-lyric-font-family, inherit)",
					transform: `scale(${contentScale})`,
					transformOrigin: "center",
				}}
			>
				{settings.animationStyle === "stroke" ? (
					<DesktopStrokeView
						lines={lines}
						currentIndex={currentIndex}
						direction={settings.direction}
						getNowMs={getNowMs}
					/>
				) : (
					<DesktopDefaultView
						line={currentLine}
						nextLine={nextLine}
						currentIndex={currentIndex}
						direction={settings.direction}
						fontSize={settings.fontSize}
						currentTimeMotion={currentTimeMotion}
					/>
				)}
			</div>
		</div>
	);
}
