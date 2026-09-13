import {
	ArrowExit20Regular,
	ChevronRight24Filled,
	Next24Regular,
	Pause24Regular,
	Play24Regular,
	Previous24Regular,
	Speaker220Regular,
	SpeakerMute20Regular,
	TabDesktopMultiple20Regular,
} from "@fluentui/react-icons";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { exit } from "@tauri-apps/plugin-process";
import { useEffect, useState } from "react";

/** 播放模式（主窗口推送的状态里已派生好，直接展示） */
type PlayMode = "sequential" | "list" | "random" | "single";

const PLAY_MODE_LABELS: Record<PlayMode, string> = {
	sequential: "顺序播放",
	list: "列表循环",
	random: "随机播放",
	single: "单曲循环",
};

/** 托盘菜单状态（由主窗口经 Rust 中转推送的完整快照，字段均为可选增量合并） */
interface TrayState {
	title: string;
	artist: string;
	coverUrl: string;
	isPlaying: boolean;
	playMode: PlayMode;
	volume: number;
}

const INITIAL_STATE: TrayState = {
	title: "WispMusic",
	artist: "未播放",
	coverUrl: "",
	isPlaying: false,
	playMode: "list",
	volume: 0.7,
};

/**
 * 播放指令不走 JS emit 广播（webview 间事件在部分环境下不可达），
 * 而是经 Rust 的 tray_player_command 定向转发到主窗口，
 * 与 SMTC 硬件按键共用主窗口的 smtc-event 处理链路。
 */
function sendCommand(payload: Record<string, unknown>) {
	invoke("tray_player_command", { payload }).catch(console.error);
}

export default function TrayMenu() {
	const [state, setState] = useState<TrayState>(INITIAL_STATE);
	const [isPlayModeMenuOpen, setIsPlayModeMenuOpen] = useState(false);

	useEffect(() => {
		// 主窗口通过 Rust 中转推送的完整状态快照（菜单每次弹出时也会推送一次）
		const unlisten = listen<Partial<TrayState>>("tray-sync-state", (e) => {
			setState((prev) => ({ ...prev, ...e.payload }));
		});
		return () => {
			unlisten.then((f) => f());
		};
	}, []);

	// 底色与描边由 index.css 的 .tray-menu-root 统一给出：托盘是「浮层菜单」，
	// 用卡片色 86% 起底（纯色材质 100%），外缘用 22% 前景色描边。
	// 原先这里是 bg-white/0（浅色下完全透明、完全依赖系统材质）+ dark:bg-black/60，
	// 深浅两侧根本不是同一个东西，系统特效一缺失整窗就透出桌面；
	// backdrop-blur-2xl 也是无效开销——它只能模糊页面内部内容，
	// 够不到窗口后面的系统材质与桌面。
	return (
		<div
			className="tray-menu-root w-screen h-screen overflow-y-auto overscroll-contain border flex flex-col p-2 select-none [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
			style={{ fontFamily: '"Microsoft Yahei"' }}
		>
			<div className="px-2 py-1.5 flex items-center gap-3 mb-0.5 hover:bg-fill-hover rounded-sm">
				{state.coverUrl ? (
					<img
						src={state.coverUrl}
						alt="专辑封面"
						className="size-10 rounded-sm object-cover border"
					/>
				) : (
					<div className="size-10 rounded-md bg-fill-hover shadow-sm" />
				)}
				<span className="flex flex-col flex-1 overflow-hidden">
					<span className="text-xs font-bold truncate text-foreground">
						{state.title}
					</span>
					<span className="text-xs text-muted-foreground truncate">
						{state.artist}
					</span>
				</span>
			</div>

			<TrayMenuSeparator />

			<TrayMenuItem
				icon={<TabDesktopMultiple20Regular className="size-4" />}
				onClick={() => invoke("tray_show_main_window").catch(console.error)}
			>
				显示窗口
			</TrayMenuItem>

			<TrayMenuSeparator />

			<TrayMenuItem
				icon={<Previous24Regular className="size-4" />}
				onClick={() => sendCommand({ event: "previous" })}
			>
				上一首
			</TrayMenuItem>
			<TrayMenuItem
				icon={
					state.isPlaying ? (
						<Pause24Regular className="size-4" />
					) : (
						<Play24Regular className="size-4" />
					)
				}
				onClick={() => sendCommand({ event: "toggle" })}
			>
				{state.isPlaying ? "暂停" : "播放"}
			</TrayMenuItem>
			<TrayMenuItem
				icon={<Next24Regular className="size-4" />}
				onClick={() => sendCommand({ event: "next" })}
			>
				下一首
			</TrayMenuItem>

			{/* 播放模式：子菜单，当前模式打勾显示 */}
			<TrayMenuItem
				icon={
					<span className="size-4 text-center text-[11px] leading-4 font-bold">
						✓
					</span>
				}
				hasChevron
				onClick={() => setIsPlayModeMenuOpen((open) => !open)}
			>
				{PLAY_MODE_LABELS[state.playMode]}
			</TrayMenuItem>
			{isPlayModeMenuOpen && (
				<div className="flex flex-col">
					{(Object.keys(PLAY_MODE_LABELS) as PlayMode[]).map((mode) => (
						<TrayMenuItem
							key={mode}
							icon={
								<span
									className={
										"size-4 text-center text-[11px] leading-4 font-bold " +
										(state.playMode === mode ? "text-primary" : "invisible")
									}
								>
									✓
								</span>
							}
							className="pl-6!"
							onClick={() => {
								setState((prev) => ({ ...prev, playMode: mode }));
								setIsPlayModeMenuOpen(false);
								sendCommand({ event: "setPlayMode", mode });
							}}
						>
							{PLAY_MODE_LABELS[mode]}
						</TrayMenuItem>
					))}
				</div>
			)}

			<TrayMenuSeparator />

			{/* 音量：当前值只读展示 */}
			<TrayMenuItem icon={<Speaker220Regular className="size-4" />} disabled>
				音量 {Math.round(state.volume * 100)}%
			</TrayMenuItem>
			<TrayMenuItem
				icon={<Speaker220Regular className="size-4" />}
				onClick={() => sendCommand({ event: "volumeUp" })}
			>
				增大音量
			</TrayMenuItem>
			<TrayMenuItem
				icon={<Speaker220Regular className="size-4 rotate-180" />}
				onClick={() => sendCommand({ event: "volumeDown" })}
			>
				减小音量
			</TrayMenuItem>
			<TrayMenuItem
				icon={<SpeakerMute20Regular className="size-4" />}
				onClick={() => sendCommand({ event: "toggleMute" })}
			>
				静音 / 取消静音
			</TrayMenuItem>

			<TrayMenuSeparator />

			<TrayMenuItem
				icon={<ArrowExit20Regular className="size-4" />}
				onClick={() => exit(0)}
			>
				退出
			</TrayMenuItem>
		</div>
	);
}

function TrayMenuSeparator() {
	// bg-border（#E5E5E5）压在浅色云母/白色菜单面上约 1.13:1，等于不可见；
	// 改由前景色派生的 15%，深浅两侧自动对称
	return <div className="w-full h-px bg-foreground/15 my-0.5 shrink-0" />;
}

function TrayMenuItem({
	children,
	icon,
	onClick,
	hasChevron = false,
	disabled = false,
	className,
}: {
	children: React.ReactNode;
	icon?: React.ReactNode;
	onClick?: () => void;
	hasChevron?: boolean;
	disabled?: boolean;
	className?: string;
}) {
	return (
		<button
			type="button"
			data-disabled={disabled ? "true" : undefined}
			className={
				// 菜单行四态（hover / 按下 / 选中 / 禁用）统一由 .wisp-menu-item 给出，
				// 原先这里手写的 hover 10% / 按下 15% / focus-visible 12% 与
				// 右键菜单的 7% / 10% / 14% 是两套互不相干的阶梯。
				// 键盘焦点现在由全局 :focus-visible 画轮廓（见 index.css 焦点策略），
				// 不再需要单独补一层底色。
				"wisp-menu-item px-2 py-1.5 rounded-sm text-left text-sm font-normal " +
				(className ? ` ${className}` : "")
			}
			disabled={disabled}
			onClick={onClick}
		>
			<div className="flex items-center gap-2 justify-between">
				<div className="flex items-center gap-2">
					{icon}
					{children}
				</div>
				{hasChevron && (
					<ChevronRight24Filled className="size-4 text-foreground/60" />
				)}
			</div>
		</button>
	);
}
