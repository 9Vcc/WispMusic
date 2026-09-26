import { cn } from "@/lib/utils";
import { computeListWindow } from "@/lib/utils/list-window";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
	currentMonitor,
	getCurrentWindow,
} from "@tauri-apps/api/window";
import {
	Dismiss20Regular,
	List20Regular,
	MusicNote224Filled,
	PictureInPictureExit20Regular,
} from "@fluentui/react-icons";
import { useEffect, useMemo, useRef, useState } from "react";
import { Marquee } from "../components/marquee/marquee";

/** 折叠条高度 / 展开后高度（逻辑像素）。
    折叠条 72px：比 EchoMusic 的 64px 略高，给贴底进度条更从容的留白 */
const MINI_WIDTH = 360;
const MINI_COLLAPSED_H = 72;
const MINI_EXPANDED_H = 360;

/** 播放列表行高（显式定高：窗口化的两侧占位高度必须与真实行高完全一致，
    否则滚动条长度会与未窗口化时不一致）。32px ≈ 原 py-1.5 + 13px 文本 */
const PLAYLIST_ITEM_H = 32;
/** 视口外额外渲染的行数，高速滚动时不露白 */
const PLAYLIST_OVERSCAN = 12;

/** 卡片高度过渡时长：展开时 CSS 端 duration-[240ms] 与此保持一致
    （收起方向不做高度动画编排——窗口先行原子缩回，卡片过渡在裁剪下不可见） */
const CARD_TRANSITION_MS = 240;
/** 收起看门狗：收起点击后强制按物理尺寸校对一次的时刻。
    缩窗是单次 IPC，可能因渲染暂停/队列卡死/热重载状态重置而失效——
    看门狗按物理尺寸兜底归位（物理尺寸是唯一可信源，ref 只是记账） */
const COLLAPSE_WATCHDOG_MS = 700;

const raf = () =>
	new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
const delay = (ms: number) =>
	new Promise<void>((resolve) => window.setTimeout(resolve, ms));
/** 等两帧：确保 React 提交的 DOM 变更已经上屏。窗口隐藏时 rAF 不触发，
    用 60ms 超时兜底，避免窗口操作队列在隐藏态被永久卡死 */
const waitForPaint = () => Promise.race([raf().then(raf), delay(60)]);
/** 等一帧：确保刚落地的窗口尺寸已被合成器呈现 */
const waitForFrame = () => Promise.race([raf(), delay(40)]);

/** 播放模式（主窗口推送的状态里已派生好，直接展示） */
type PlayMode = "sequential" | "list" | "random" | "single";

const PLAY_MODE_LABELS: Record<PlayMode, string> = {
	sequential: "顺序播放",
	list: "列表循环",
	random: "随机播放",
	single: "单曲循环",
};

/** 播放列表条目（主窗口压缩后的最小展示字段） */
interface MiniPlaylistItem {
	id: number;
	name: string;
	artist: string;
	dt: number;
}

/** 主窗口经 Rust 中转推送的快照（字段均为可选增量合并） */
interface MiniState {
	title: string;
	artist: string;
	coverUrl: string;
	isPlaying: boolean;
	playMode: PlayMode;
	volume: number;
	/** 当前播放进度（秒） */
	currentTime: number;
	/** 总时长（秒） */
	duration: number;
	/** 播放列表（仅列表变化/窗口显示时推送） */
	playlist?: MiniPlaylistItem[];
	/** 当前曲目在列表中的索引 */
	currentIndex?: number;
}

/** 收起 = 窗口先行：点击即入队缩窗，落地前窗口显示完整卡片（无空腔），
    落地后即终态；看门狗按物理尺寸兜底整链路失效 */

const INITIAL_STATE: MiniState = {
	title: "WispMusic",
	artist: "未播放",
	coverUrl: "",
	isPlaying: false,
	playMode: "list",
	volume: 0.7,
	currentTime: 0,
	duration: 0,
};

/**
 * 播放指令与托盘菜单同链路：经 Rust 的 tray_player_command
 * 定向转发到主窗口的 smtc-event 处理器（webview 间 JS 事件不可靠）。
 */
function sendCommand(payload: Record<string, unknown>) {
	invoke("tray_player_command", { payload }).catch(console.error);
}

function formatTime(seconds: number) {
	if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
	const m = Math.floor(seconds / 60);
	const s = Math.floor(seconds % 60);
	return `${m}:${s.toString().padStart(2, "0")}`;
}

export default function MiniPlayer() {
	const [state, setState] = useState<MiniState>(INITIAL_STATE);
	// expanded：目标态（驱动窗口尺寸与逻辑）；panelOpen：卡片视觉态（CSS 过渡）
	const [expanded, setExpanded] = useState(false);
	const [panelOpen, setPanelOpen] = useState(false);
	// 拖动进度条时的本地值（未提交）；null 表示未在拖动
	const [seekingValue, setSeekingValue] = useState<number | null>(null);

	// 窗口句柄只取一次：getCurrentWindow() 每次调用返回新实例，
	// 若放进 effect 依赖会导致每次状态更新都重复执行尺寸/位置操作（窗口跳动）
	const winRef = useRef(getCurrentWindow());
	const cardRef = useRef<HTMLDivElement | null>(null);

	// —— 展开/收起编排 ——
	// 核心不变量：控制条在屏幕上的位置永不瞬移（向下展开顶边不动、
	// 向上展开底边不动），窗口尺寸变化一次性原子完成（Win32 SetWindowPos，
	// 逐帧动画窗口会闪烁），卡片生长/收缩走 CSS 高度过渡（240ms 弹性曲线）。
	//
	// 时序（展开）：改锚定 →（仅方向翻转时）等上屏 → 原子扩窗（多出区域是
	//   透明 webview，不可见）→ 等 1 帧 → setPanelOpen 驱动卡片生长 + 内容淡入。
	//   扩窗必须先于卡片动画完成：旧实现里 CSS 过渡与扩窗 IPC 赛跑，窗口尚未
	//   扩出时卡片快速增长的部分（曲线前段最陡）会被窗口边缘裁剪，IPC 一慢
	//   就出现"卡片顶死、窗口突然弹出"的顿挫；现在竞态被显式等待消除。
	// 时序（收起）：**窗口先行**——点击即入队缩窗（单次原子 SetWindowPos，
	//   失败自动重试），不等任何动画事件。缩窗落地前窗口显示的仍是完整
	//   卡片（连贯画面），落地后窗口裁掉卡片下部、只留控制条——卡片自身的
	//   高度过渡在裁剪下不可见，只负责终帧与窗口边缘对齐。"窗口 > 卡片"
	//   的透明空腔在收起方向结构性不存在，不依赖任何时序运气。此前两版
	//  （动画后串行缩窗 / 引导时刻并行缩窗）都残留在"卡片已收完、窗口还没
	//   缩回"的空腔上——IPC 与系统材质重绘的抖动无法用时序补偿（用户实测
	//   零点几秒残留）。收起中反悔由 shrinkWindowOp 的意图校验自行放弃，
	//   窗口不动、卡片从当前高度直接重新展开。
	//
	// 中断安全：desiredExpandedRef 记录"用户最新意图"，每个排队操作在执行前
	//   与每个 await 之后校验意图，被更新操作取代的旧操作直接跳过——修复
	//   快速连点时"缩窗、扩窗交错执行"的闪烁。windowExpandedRef 跟踪窗口
	//   实际尺寸，挂载与看门狗按物理尺寸校对记账。
	// 无障碍：prefers-reduced-motion 下 CSS 过渡关闭（motion-reduce 变体），
	//   JS 侧跳过等待直接切换终态。
	// 收起后不复位 expandUp（保持贴底反转布局），控制条零位移；方向在下次
	//   展开时重新判定，切换的细微位移由展开动画掩盖。
	const boundsOpRef = useRef(Promise.resolve());
	// 收起看门狗定时器（按物理尺寸强校对，防任何单点失效留下的错位态）
	const collapseWatchRef = useRef<number | null>(null);
	// 窗口当前是否处于展开尺寸（收起快速路径的判定依据）
	const windowExpandedRef = useRef(false);
	// 用户最新意图（desired state），过期窗口操作的跳变熔断器
	const desiredExpandedRef = useRef(false);
	// 向上展开标记：影响卡片锚定（底部）与列方向（反转）；state 供渲染，ref 供异步操作读取
	const [expandUp, setExpandUp] = useState(false);
	const expandUpRef = useRef(false);

	/** 排队执行窗口操作：所有 SetWindowPos 串行化，防快速点击交错 */
	const enqueueBoundsOp = (op: () => Promise<void>) => {
		boundsOpRef.current = boundsOpRef.current.then(op).catch(console.error);
	};

	/** 读取窗口当前物理几何（位置/尺寸/缩放并行取回，一次到位） */
	const readWindowGeometry = async () => {
		const win = winRef.current;
		const [pos, size, scale] = await Promise.all([
			win.outerPosition(),
			win.innerSize(),
			win.scaleFactor(),
		]);
		return { pos, size, scale, bottom: pos.y + size.height };
	};

	// 展开：先定方向并原子扩窗，扩窗呈现后再触发卡片高度过渡。
	// 锚定边全部按"当前实际底边"计算（物理坐标），即使窗口处于意外尺寸
	// （上一次操作被跳过等）也能自纠偏落到正确位置；缩放取自窗口自身，
	// 与 Rust 端 mini_set_bounds 使用的 scale_factor 严格一致（比
	// currentMonitor().scaleFactor 在混合 DPI 下更可靠）
	const expandWindowOp = async () => {
		if (!desiredExpandedRef.current) return; // 排队期间意图已反转
		const geo = await readWindowGeometry();
		let targetYPhys = geo.pos.y;
		let up = false;
		const monitor = await currentMonitor();
		if (monitor) {
			const expandedHPhys = MINI_EXPANDED_H * geo.scale;
			const screenBottom = monitor.position.y + monitor.size.height;
			if (geo.pos.y + expandedHPhys > screenBottom) {
				// 向下空间不足 → 向上展开（底边不动，顶边上移）
				up = true;
				targetYPhys = Math.max(
					geo.bottom - expandedHPhys,
					monitor.position.y,
				);
			}
		}

		// 方向翻转时先提交锚定并等上屏，控制条从原位起算；方向不变则零等待
		if (up !== expandUpRef.current) {
			expandUpRef.current = up;
			setExpandUp(up);
			await waitForPaint();
			if (!desiredExpandedRef.current) return;
		}

		// 原子扩窗：新增区域是透明 webview，视觉上不可见
		await invoke("mini_set_bounds", {
			x: geo.pos.x / geo.scale,
			y: targetYPhys / geo.scale,
			width: MINI_WIDTH,
			height: MINI_EXPANDED_H,
		});
		windowExpandedRef.current = true;
		if (!desiredExpandedRef.current) return;

		// 等扩窗后的窗口呈现 1 帧，卡片生长的首帧不会被窗口边缘裁剪
		await waitForFrame();
		if (!desiredExpandedRef.current) return;
		setPanelOpen(true);
	};

	// 收起完成后的缩窗：窗口缩回 72px（按锚定边计算，方向沿用展开时记录）。
	// 缩窗是收起链路的终点，失败重试一次——静默失败会留下收不回的空腔
	const shrinkWindowOp = async () => {
		if (desiredExpandedRef.current) return; // 用户已重新展开，放弃缩窗
		for (let attempt = 0; attempt < 2; attempt++) {
			try {
				const geo = await readWindowGeometry();
				const targetYPhys = expandUpRef.current
					? geo.bottom - MINI_COLLAPSED_H * geo.scale
					: geo.pos.y;
				await invoke("mini_set_bounds", {
					x: geo.pos.x / geo.scale,
					y: targetYPhys / geo.scale,
					width: MINI_WIDTH,
					height: MINI_COLLAPSED_H,
				});
				windowExpandedRef.current = false;
				return;
			} catch (e) {
				if (attempt > 0) throw e;
				await delay(100);
				if (desiredExpandedRef.current) return;
			}
		}
	};

	// 收起看门狗：不信任任何单一路径，700ms 后按窗口物理尺寸强制校对。
	// 只在"用户意图仍是收起 且 物理窗口仍展开"时补缩窗；否则仅同步标记。
	// 物理尺寸是唯一可信源——ref 只反映 webview 侧的记账，可能因热重载失真
	const armCollapseWatchdog = () => {
		if (collapseWatchRef.current !== null) {
			window.clearTimeout(collapseWatchRef.current);
		}
		collapseWatchRef.current = window.setTimeout(() => {
			collapseWatchRef.current = null;
			if (desiredExpandedRef.current) return;
			void (async () => {
				try {
					const [size, scale] = await Promise.all([
						winRef.current.innerSize(),
						winRef.current.scaleFactor(),
					]);
					if (size.height > (MINI_COLLAPSED_H + 1) * scale) {
						windowExpandedRef.current = true;
						enqueueBoundsOp(shrinkWindowOp);
					} else {
						windowExpandedRef.current = false;
					}
				} catch (e) {
					console.error(e);
				}
			})();
		}, COLLAPSE_WATCHDOG_MS);
	};

	const cancelCollapse = () => {
		if (collapseWatchRef.current !== null) {
			window.clearTimeout(collapseWatchRef.current);
			collapseWatchRef.current = null;
		}
	};

	// 收起：窗口先行。点击即入队缩窗（单次原子 IPC，失败自动重试一次），
	// 不等任何动画事件。落地前窗口显示完整卡片（连贯画面），落地后即终态。
	// 卡片自身的 240ms 高度过渡保留——它在裁剪下不可见，只负责终帧圆角
	// 与窗口边缘对齐；面板内容 150ms 快速淡出、进度条淡入，仅落地前的
	// 两三帧可见，不构成观感负担
	const requestCollapse = () => {
		setPanelOpen(false);
		armCollapseWatchdog();
		enqueueBoundsOp(shrinkWindowOp);
	};

	const toggleExpanded = () => {
		const next = !expanded;
		desiredExpandedRef.current = next;
		if (next) {
			// 收起动画进行中反悔：取消待决缩窗，卡片从当前高度直接重新展开
			// （已入队但未执行的缩窗由 shrinkWindowOp 的意图校验自行放弃）
			cancelCollapse();
			enqueueBoundsOp(expandWindowOp);
		} else {
			requestCollapse();
		}
		setExpanded(next);
	};

	// 卸载时清掉看门狗，避免悬空回调
	useEffect(() => {
		return () => {
			if (collapseWatchRef.current !== null) {
				window.clearTimeout(collapseWatchRef.current);
				collapseWatchRef.current = null;
			}
		};
	}, []);

	// 挂载自愈：webview 侧的 React 状态可能在 OS 窗口保持展开尺寸时被整体
	// 重置（Vite HMR 热重载、前端崩溃恢复），出现"卡片已折叠、窗口仍是
	// 360px"的错位空腔，且没有任何待决缩窗会去纠正它。挂载时按物理尺寸
	// 校对一次，错位即归位（仅物理尺寸是可信源，ref 记账可能失真）
	// biome-ignore lint/correctness/useExhaustiveDependencies: mount-only self-heal, ops close over stable refs
	useEffect(() => {
		let cancelled = false;
		void (async () => {
			try {
				const [size, scale] = await Promise.all([
					winRef.current.innerSize(),
					winRef.current.scaleFactor(),
				]);
				if (cancelled) return;
				if (size.height > (MINI_COLLAPSED_H + 1) * scale) {
					windowExpandedRef.current = true;
					if (!desiredExpandedRef.current) {
						enqueueBoundsOp(shrinkWindowOp);
					}
				}
			} catch (e) {
				console.error(e);
			}
		})();
		return () => {
			cancelled = true;
		};
	}, []);

	useEffect(() => {
		let cancelled = false;
		const unlisten = listen<Partial<MiniState>>("mini-sync-state", (e) => {
			setState((prev) => ({ ...prev, ...e.payload }));
		});
		// mini 窗口是首次进入 mini 模式时才创建的：Rust 在 show 那一刻推送的
		// 快照会早于本监听器注册而丢失。等监听器真正注册完成后再主动要一次
		// （主窗口侧复用既有的 mini-entered → 全量推送链路）
		void unlisten.then(() => {
			if (cancelled) return;
			invoke("aux_window_ready", { label: "mini-player" }).catch(
				console.error,
			);
		});
		return () => {
			cancelled = true;
			unlisten.then((f) => f());
		};
	}, []);

	// 本地时间插值：推送间隔 500ms，两次推送之间按播放态线性递推，
	// 让进度条平滑
	const [localTime, setLocalTime] = useState(0);
	const syncRef = useRef({ time: 0, at: 0 });
	useEffect(() => {
		syncRef.current = { time: state.currentTime, at: Date.now() };
	}, [state.currentTime]);
	useEffect(() => {
		const timer = setInterval(() => {
			// 窗口隐藏（关闭按钮只隐藏、不销毁）时跳过重渲染：下一次可见后的
			// 首个 tick 会按锚点重新算出正确时间，不会残留旧值
			if (document.hidden) return;
			const { time, at } = syncRef.current;
			if (!state.isPlaying) {
				setLocalTime(time);
				return;
			}
			setLocalTime(time + (Date.now() - at) / 1000);
		}, 200);
		return () => clearInterval(timer);
	}, [state.isPlaying]);

	// 播放列表窗口化的滚动状态：只渲染视口附近的行，因此滚动位置必须自己跟踪
	const listRef = useRef<HTMLDivElement | null>(null);
	const [listScrollTop, setListScrollTop] = useState(0);
	const [listViewportH, setListViewportH] = useState(0);
	const listScrollRafRef = useRef<number | null>(null);

	useEffect(() => {
		const container = listRef.current;
		if (!container) return;
		setListViewportH(container.clientHeight);
		const observer = new ResizeObserver(() => {
			setListViewportH(container.clientHeight);
		});
		observer.observe(container);
		return () => {
			observer.disconnect();
		};
	}, []);

	useEffect(() => {
		return () => {
			if (listScrollRafRef.current !== null) {
				cancelAnimationFrame(listScrollRafRef.current);
				listScrollRafRef.current = null;
			}
		};
	}, []);

	// 滚动事件按帧合并：高速滚动时不逐像素触发一次 setState
	const handleListScroll = (e: React.UIEvent<HTMLDivElement>) => {
		const next = e.currentTarget.scrollTop;
		if (listScrollRafRef.current !== null) return;
		listScrollRafRef.current = requestAnimationFrame(() => {
			listScrollRafRef.current = null;
			setListScrollTop(next);
		});
	};

	// 音量：实时控制——拖动即生效（本地乐观更新 + 立即下发指令）
	const handleVolumeChange = (value: number) => {
		setState((prev) => ({ ...prev, volume: value }));
		sendCommand({ event: "setVolume", volume: value });
	};

	// 进度条拖动结束才提交 seek（拖动中仅本地更新填充，避免高频指令）
	const commitSeek = () => {
		if (seekingValue !== null) {
			sendCommand({ event: "set_position", position: seekingValue });
			setSeekingValue(null);
		}
	};

	// 拖动窗口：整个控制条按住即可拖动（按钮/滑杆区域除外）。
	// data-tauri-drag-region 要求 mousedown 落点元素本身带属性，
	// 会被封面 img、Marquee 内层元素等子节点遮挡失效，故统一手动 startDragging
	const handleDragPointerDown = (e: React.PointerEvent) => {
		if (e.button !== 0) return;
		if ((e.target as HTMLElement).closest("button, input")) return;
		winRef.current.startDragging().catch(console.error);
	};

	const playlist = state.playlist ?? [];
	const playlistLength = playlist.length;
	const currentIndex = state.currentIndex ?? -1;
	const displayVolume = state.volume;
	const isShuffle = state.playMode === "random";
	// 进度条填充百分比（拖动中用本地值）
	const progressTime = seekingValue ?? localTime;
	const progressFill =
		state.duration > 0
			? Math.min(100, (progressTime / state.duration) * 100)
			: 0;

	const playlistLabel = useMemo(() => {
		if (currentIndex < 0 || !playlist[currentIndex]) return undefined;
		return `${currentIndex + 1} / ${playlist.length}`;
	}, [playlist, currentIndex]);

	const listWindow = useMemo(
		() =>
			computeListWindow({
				total: playlist.length,
				itemHeight: PLAYLIST_ITEM_H,
				scrollTop: listScrollTop,
				viewportHeight: listViewportH,
				overscan: PLAYLIST_OVERSCAN,
			}),
		[playlist.length, listScrollTop, listViewportH],
	);

	// 当前曲目行滚动居中（仅面板可见时；刚展开等高度过渡完再滚）。
	// 窗口化后当前行不一定在 DOM 里，无法用 scrollIntoView，改为直接算目标
	// scrollTop —— 位置只由行高与索引决定，反而更可控
	useEffect(() => {
		if (!panelOpen) return;
		const timer = window.setTimeout(() => {
			const container = listRef.current;
			const viewportH = container?.clientHeight ?? 0;
			if (!container || viewportH <= 0 || currentIndex < 0) return;
			const maxTop = Math.max(playlistLength * PLAYLIST_ITEM_H - viewportH, 0);
			const center =
				currentIndex * PLAYLIST_ITEM_H - (viewportH - PLAYLIST_ITEM_H) / 2;
			container.scrollTo({
				top: Math.min(Math.max(center, 0), maxTop),
				behavior: "smooth",
			});
		}, CARD_TRANSITION_MS + 20);
		return () => window.clearTimeout(timer);
		// playlistLength 而非 playlist 引用：状态每次推送都会换引用，
		// 依赖引用会导致列表反复被拉回当前曲目、用户无法滚动
	}, [panelOpen, currentIndex, playlistLength]);

	return (
		// 锚定容器：填满窗口的透明层。向下展开时卡片顶部锚定（默认），
		// 向上展开时底部锚定（justify-end）——窗口底边不动，卡片从底部向上长，
		// 控制条在屏幕上保持静止（EchoMusic 向上展开的关键）
		<div
			className={cn(
				"flex h-screen w-screen flex-col",
				expandUp ? "justify-end" : "justify-start",
			)}
			style={{ fontFamily: '"Microsoft Yahei"' }}
		>
			<div
				// mini-player-root：底色由 index.css 的窗口材质压暗层统一给出
				//（--scrim-float，与托盘菜单同一取值）。展开：窗口先一次性扩到
				// 目标尺寸（透明区不可见），卡片再用 CSS 过渡长高——窗口逐帧
				// 动画会闪烁，尺寸必须一次设置。收起：窗口先行原子缩回，卡片
				// 高度过渡在窗口裁剪下不可见，只负责终帧与窗口边缘对齐。
				// expandUp 时列反转：控制条贴卡片底部、面板从其上方长出
				ref={cardRef}
				className={cn(
					"mini-player-root flex w-screen flex-col overflow-hidden rounded-lg border border-border select-none transition-[height,background-color] duration-[240ms] ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none",
					expandUp && "flex-col-reverse",
				)}
				style={{
					height: panelOpen ? MINI_EXPANDED_H : MINI_COLLAPSED_H,
				}}
			>
				{/* 顶部控制条（折叠态即全部内容；EchoMusic 式布局：
				    封面 | 信息↔hover播放控制 | 随机/播放列表/回主窗口/关闭 | 贴底进度条）。
				    高度恒定 70px（窗口 72 - 2px 边框），不随折叠/展开变化。
				    封面与文字/按钮统一上移 3px：视觉中心偏上，给贴底进度条留白，
				    所有元素同一基线（错位会显得不协调） */}
				<div
					className="group relative flex h-[70px] shrink-0 items-center gap-3 pl-3 pr-4"
					onPointerDown={handleDragPointerDown}
				>
					{/* 封面：56px，占窗口高度约 78%（EchoMusic 式饱满比例） */}
					<div className="size-12 shrink-0 -translate-y-[4.5px] overflow-hidden rounded-md border shadow-sm">
						{state.coverUrl ? (
							<img
								src={state.coverUrl}
								alt="Album cover"
								draggable={false}
								className="size-full object-cover"
							/>
						) : (
							<div className="size-full bg-black/5 dark:bg-white/10" />
						)}
					</div>

					{/* 中间：默认显示标题/艺人，hover 控制条时淡出、切换为播放控制；
					    整体上移 3px 给贴底进度条留白 */}
					<div className="relative min-w-0 flex-1 -translate-y-[3px] self-stretch">
						<div className="flex h-full flex-col justify-center transition-opacity duration-200 group-hover:opacity-0">
							<Marquee
								text={state.title}
								containerClassName="min-w-0"
								textClassName="text-[13px] font-bold leading-[18px] text-foreground"
							/>
							<span className="truncate text-[11px] leading-4 text-foreground/60">
								{state.artist}
							</span>
						</div>
						{/* hover 播放控制：上一首/播放/下一首（无底色，hover 才跟主题色） */}
						<div className="pointer-events-none absolute inset-0 flex items-center justify-center gap-2.5 opacity-0 transition-opacity duration-200 group-hover:pointer-events-auto group-hover:opacity-100">
							<button
								type="button"
								aria-label="上一首"
								title="上一首"
								className="flex size-6 items-center justify-center rounded-full text-foreground/60 transition-all hover:-translate-y-px hover:text-primary"
								onClick={() => sendCommand({ event: "previous" })}
							>
								<PrevIcon className="size-4" />
							</button>
							<button
								type="button"
								aria-label={state.isPlaying ? "暂停" : "播放"}
								title={state.isPlaying ? "暂停" : "播放"}
								// EchoMusic 式圆形遮罩：中性底色 + 细边框（不跟主题色），
								// 播放/hover 时图标才转主题色
								className={cn(
									"flex size-[30px] items-center justify-center rounded-full border border-border bg-fill-subtle transition-all hover:scale-105",
									state.isPlaying
										? "text-primary"
										: "text-foreground/70 hover:text-primary",
								)}
								onClick={() => sendCommand({ event: "toggle" })}
							>
								{state.isPlaying ? (
									<svg
										viewBox="0 0 24 24"
										className="size-[18px]"
										fill="currentColor"
									>
										<rect x="6" y="5" width="4" height="14" rx="1" />
										<rect x="14" y="5" width="4" height="14" rx="1" />
									</svg>
								) : (
									<svg
										viewBox="0 0 24 24"
										className="ml-0.5 size-[18px]"
										fill="currentColor"
									>
										<path d="M8 5.14v13.72c0 .8.87 1.3 1.56.88l10.54-6.86a1.05 1.05 0 0 0 0-1.76L9.56 4.26A1.04 1.04 0 0 0 8 5.14Z" />
									</svg>
								)}
							</button>
							<button
								type="button"
								aria-label="下一首"
								title="下一首"
								className="flex size-6 items-center justify-center rounded-full text-foreground/60 transition-all hover:-translate-y-px hover:text-primary"
								onClick={() => sendCommand({ event: "next" })}
							>
								<NextIcon className="size-4" />
							</button>
						</div>
					</div>

					{/* 右侧功能组：随机 / 播放列表 / 回主窗口 / 关闭
					    （音量在展开态底栏；上移 3px 留白；28px 点击区） */}
					<div className="flex shrink-0 -translate-y-[3px] items-center gap-1 text-foreground/70">
						<button
							type="button"
							aria-label="随机播放"
							title="随机播放"
							className={cn(
								"flex size-7 items-center justify-center rounded-full transition-colors hover:bg-fill-hover hover:text-foreground",
								isShuffle && "text-primary hover:text-primary",
							)}
							onClick={() => sendCommand({ event: "shuffle" })}
						>
							<ShuffleIcon className="size-[18px]" />
						</button>
						<button
							type="button"
							aria-label={expanded ? "收起播放列表" : "展开播放列表"}
							title={expanded ? "收起播放列表" : "展开播放列表"}
							className={cn(
								"flex size-7 items-center justify-center rounded-full transition-colors hover:bg-fill-hover hover:text-foreground",
								expanded && "text-primary hover:text-primary",
							)}
							onClick={toggleExpanded}
						>
							<List20Regular className="size-[18px]" />
						</button>
						<button
							type="button"
							aria-label="回到主窗口"
							title="回到主窗口"
							className="flex size-7 items-center justify-center rounded-full transition-colors hover:bg-fill-hover hover:text-foreground"
							onClick={() =>
								invoke("mini_restore_main").catch(console.error)
							}
						>
							<PictureInPictureExit20Regular className="size-[18px]" />
						</button>
						<button
							type="button"
							aria-label="关闭（托盘可再次呼出）"
							title="关闭（托盘可再次呼出）"
							className="flex size-7 items-center justify-center rounded-full transition-colors hover:bg-fill-hover hover:text-foreground"
							onClick={() => invoke("mini_close").catch(console.error)}
						>
							<Dismiss20Regular className="size-[18px]" />
						</button>
					</div>

					{/* 贴底进度条（仅折叠态可见）。常挂载 + 透明度交叉过渡：
					    旧实现按 panelOpen 直接挂载/卸载，收起开始瞬间无过渡地
					    "啪"地出现；现在展开淡出、收起淡入，与卡片高度过渡衔接。
					    展开态用 inert 移出焦点链与无障碍树，不残留幽灵焦点 */}
					<div
						inert={panelOpen}
						className={cn(
							"group/progress absolute bottom-[10px] left-[71px] right-4 h-2 cursor-pointer transition-opacity duration-150 ease-out motion-reduce:transition-none",
							panelOpen
								? "pointer-events-none opacity-0"
								: "opacity-100",
						)}
					>
						<div className="absolute inset-x-0 top-1/2 h-[3px] -translate-y-1/2 rounded-full bg-fill-strong" />
						<div
							className="absolute left-0 top-1/2 h-[3px] -translate-y-1/2 rounded-full bg-primary"
							style={{ width: `${progressFill}%` }}
						/>
						<div
							className="absolute top-1/2 size-2 -translate-y-1/2 rounded-full bg-primary opacity-0 shadow transition-opacity group-hover/progress:opacity-100"
							style={{ left: `calc(${progressFill}% - 4px)` }}
						/>
						<input
							type="range"
							min={0}
							max={Math.max(state.duration, 1)}
							step={1}
							value={progressTime}
							className="absolute inset-0 h-full w-full cursor-pointer opacity-0"
							onChange={(e) => setSeekingValue(Number(e.target.value))}
							onPointerUp={commitSeek}
						/>
					</div>
				</div>

				{/* 展开面板：始终挂载（高度过渡期间不能卸载内容），
				    卡片高度动画对它做裁切展开，内容再叠一层淡入+上移。
				    展开时内容延迟 40ms 跟进（40+200=240ms 与卡片同步收尾，
				    "容器先行、内容跟随"的层次感，避免首帧被生长边缘裁切）；
				    收起时内容 150ms 先走、无延迟（卡片收尾不残留空壳），
				    缓动统一为卡片同款弹性曲线 */}
				<div
					className={cn(
						"flex min-h-0 flex-1 flex-col transition-[opacity,transform] ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none",
						panelOpen
							? "translate-y-0 opacity-100 delay-[40ms] duration-200"
							: "pointer-events-none -translate-y-2 opacity-0 duration-150",
					)}
				>
					{/* 列表头 */}
					<div className="flex shrink-0 items-center justify-between px-4 pt-1 pb-0.5">
						<span className="text-[11px] font-medium text-foreground/50">
							播放列表
						</span>
						{playlistLabel && (
							<span className="text-[11px] tabular-nums text-foreground/55">
								{playlistLabel}
							</span>
						)}
					</div>

					{/* 播放列表滚动区。窗口化渲染：只挂视口附近的行，两侧用等高
					    占位撑起总高（播放列表上千首时不再生成上千个 DOM 子树） */}
					<div
						ref={listRef}
						onScroll={handleListScroll}
						className="min-h-0 flex-1 overflow-y-auto no-scrollbar px-2"
					>
						{playlistLength > 0 ? (
							<>
								{listWindow.topPad > 0 && (
									<div
										style={{ height: listWindow.topPad }}
										aria-hidden="true"
									/>
								)}
								{playlist
									.slice(listWindow.start, listWindow.end)
									.map((item, offset) => {
										const i = listWindow.start + offset;
										return (
											<button
												type="button"
												key={`${item.id}-${i}`}
												onClick={() =>
													sendCommand({ event: "playIndex", index: i })
												}
												className={cn(
													"flex h-8 w-full items-center gap-2 rounded-md px-2 text-left transition-colors",
													i === currentIndex
														? "bg-fill-strong"
														: "hover:bg-fill-hover",
												)}
											>
												{item.id === playlist[currentIndex]?.id ? (
													<svg
														viewBox="0 0 24 24"
														className="size-3.5 shrink-0 text-primary"
														fill="currentColor"
													>
														<path d="M8 5.14v13.72c0 .8.87 1.3 1.56.88l10.54-6.86a1.05 1.05 0 0 0 0-1.76L9.56 4.26A1.04 1.04 0 0 0 8 5.14Z" />
													</svg>
												) : (
													<MusicNote224Filled className="size-3.5 shrink-0 text-foreground/45" />
												)}
												<span
													className={cn(
														"min-w-0 shrink truncate text-[13px]",
														i === currentIndex
															? "font-bold text-primary"
															: "text-foreground/80",
													)}
												>
													{item.name}
												</span>
												<span className="min-w-0 flex-1 truncate text-[11px] text-foreground/55">
													{item.artist}
												</span>
												<span className="shrink-0 text-[11px] tabular-nums text-foreground/55">
													{formatTime(item.dt / 1000)}
												</span>
											</button>
										);
									})}
								{listWindow.bottomPad > 0 && (
									<div
										style={{ height: listWindow.bottomPad }}
										aria-hidden="true"
									/>
								)}
							</>
						) : (
							<div className="flex h-full items-center justify-center text-sm text-foreground/55">
								播放列表为空
							</div>
						)}
					</div>

					{/* 进度条（填充色显示进度；拖动结束才提交 seek，避免高频指令） */}
					<div className="flex shrink-0 items-center gap-2 px-4 pb-1 pt-2">
						<span className="w-9 shrink-0 text-right text-[11px] tabular-nums text-foreground/50">
							{formatTime(progressTime)}
						</span>
						<input
							type="range"
							min={0}
							max={Math.max(state.duration, 1)}
							step={1}
							value={progressTime}
							style={{ "--fill": `${progressFill}%` } as React.CSSProperties}
							className="mini-range h-1.5 flex-1"
							onChange={(e) => setSeekingValue(Number(e.target.value))}
							onPointerUp={commitSeek}
						/>
						<span className="w-9 shrink-0 text-[11px] tabular-nums text-foreground/50">
							{formatTime(state.duration)}
						</span>
					</div>

					{/* 随机切换 + 播放模式 + 音量（实时拖动） */}
					<div className="flex shrink-0 items-center justify-between gap-3 border-t border-border px-4 py-1.5">
						<div className="flex items-center gap-2">
							<MiniButton
								label="随机播放"
								className={cn(
									"size-7",
									isShuffle && "text-primary hover:text-primary",
								)}
								onClick={() => sendCommand({ event: "shuffle" })}
							>
								<ShuffleIcon className="size-4" />
							</MiniButton>
							<span
								className={cn(
									"text-[11px]",
									isShuffle
										? "font-medium text-primary"
										: "text-foreground/50",
								)}
							>
								{PLAY_MODE_LABELS[state.playMode]}
							</span>
						</div>
						<div className="flex items-center gap-2">
							<VolumeIcon muted={displayVolume === 0} />
							<input
								type="range"
								min={0}
								max={1}
								step={0.01}
								value={displayVolume}
								style={
									{
										"--fill": `${displayVolume * 100}%`,
									} as React.CSSProperties
								}
								className="mini-range h-1.5 w-28"
								onChange={(e) => handleVolumeChange(Number(e.target.value))}
							/>
							<span className="w-8 text-[11px] tabular-nums text-foreground/50">
								{Math.round(displayVolume * 100)}%
							</span>
						</div>
					</div>
				</div>
			</div>
		</div>
	);
}

/** 折叠条控制按钮：与 PlayerBar ghost 按钮同一视觉语言 */
function MiniButton({
	children,
	label,
	onClick,
	className,
}: {
	children: React.ReactNode;
	label: string;
	onClick: () => void;
	className?: string;
}) {
	return (
		<button
			type="button"
			aria-label={label}
			title={label}
			onClick={onClick}
			className={cn(
				"flex size-7 items-center justify-center rounded-full text-foreground/80 transition-colors hover:bg-fill-hover hover:text-foreground active:scale-90",
				className,
			)}
		>
			{children}
		</button>
	);
}

function PrevIcon({ className = "size-4" }: { className?: string }) {
	return (
		<svg viewBox="0 0 24 24" className={className} fill="currentColor">
			<path d="M7 5a1 1 0 0 1 1 1v5.22l8.24-5.94A1.05 1.05 0 0 1 18 6.14v11.72c0 .83-.92 1.33-1.6.87L8 12.78V18a1 1 0 1 1-2 0V6a1 1 0 0 1 1-1Z" />
		</svg>
	);
}

function NextIcon({ className = "size-4" }: { className?: string }) {
	return (
		<svg viewBox="0 0 24 24" className={className} fill="currentColor">
			<path d="M17 5a1 1 0 0 1 1 1v12a1 1 0 1 1-2 0v-5.22l-8.24 5.94A1.05 1.05 0 0 1 6 18.14V6.42c0-.83.92-1.33 1.6-.87L16 11.28V6a1 1 0 0 1 1-1Z" />
		</svg>
	);
}

function ShuffleIcon({ className = "size-4" }: { className?: string }) {
	return (
		<svg
			viewBox="0 0 24 24"
			className={className}
			fill="none"
			stroke="currentColor"
			strokeWidth="2"
			strokeLinecap="round"
			strokeLinejoin="round"
		>
			<path d="m18 14 4 4-4 4" />
			<path d="m18 2 4 4-4 4" />
			<path d="M2 18h1.973a4 4 0 0 0 3.3-1.7l5.454-8.6a4 4 0 0 1 3.3-1.7H22" />
			<path d="M2 6h1.972a4 4 0 0 1 3.6 2.2" />
			<path d="M22 18h-6.041a4 4 0 0 1-3.3-1.8l-.359-.45" />
		</svg>
	);
}

function VolumeIcon({ muted }: { muted: boolean }) {
	return muted ? (
		<svg viewBox="0 0 24 24" className="size-4 shrink-0 text-foreground/60" fill="currentColor">
			<path d="M12.53 3.47a.75.75 0 0 0-1.28.53v4.19L5.53 9.47 3.4 7.34a.75.75 0 1 0-1.06 1.06l2.13 2.13a.75.75 0 0 0 .97.08l13.13-9.42a.75.75 0 0 1 .86 1.23L12.53 3.47Z" />
			<path d="M13.25 9.33V6.06l6.34-4.55a2.25 2.25 0 0 1 3.47 2.53L13.25 9.33ZM20.6 20.69 3.51 8.43 2.1 9.84a1.5 1.5 0 0 0-.09 2.1l4.6 5.31-2.36 1.69a.75.75 0 0 0 .9 1.2l2.66-1.9a1.5 1.5 0 0 0 1.88.13l2.06 2.37a1.5 1.5 0 0 0 2.1.09l6.87-6.87-.62-.62Z" />
		</svg>
	) : (
		<svg viewBox="0 0 24 24" className="size-4 shrink-0 text-foreground/60" fill="currentColor">
			<path d="M13.4 3.36A.9.9 0 0 1 14.8 4v16a.9.9 0 0 1-1.4.75L8 17H5a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2h3l5.4-3.64ZM17.3 8.3a.75.75 0 0 1 1.06 0 6 6 0 0 1 0 8.49.75.75 0 1 1-1.06-1.06 4.5 4.5 0 0 0 0-6.37.75.75 0 0 1 0-1.06Z" />
		</svg>
	);
}
