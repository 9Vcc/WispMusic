import { relaunch } from "@tauri-apps/plugin-process";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import {
	Dialog,
	DialogAction,
	DialogBody,
	DialogCancel,
	DialogContent,
	DialogFooter,
	DialogTitle,
} from "@/components/ui/dialog";
import { Spinner } from "@/components/ui/spinner";
import {
	type AppUpdateInfo,
	type DownloadProgress,
	downloadAndInstallUpdate,
} from "@/lib/services/appUpdate";
import { MarkdownView } from "./markdown-view";

export type UpdatePhase = "idle" | "downloading" | "restarting";

interface UpdateDialogProps {
	/** 待展示的更新信息；为 null 时不渲染 */
	info: AppUpdateInfo | null;
	/** 受控开关，由 useStartupUpdateCheck / 调试模式预览持有 */
	open: boolean;
	/** 关闭弹窗（ESC / 遮罩 / 「忽略本次更新」），本次启动内不再提示 */
	onDismiss: () => void;
	/**
	 * 预览模式（设置页调试模式用）：不触发真实下载与重启，
	 * 以模拟进度走完 idle → downloading → restarting 全流程后自动复位关闭。
	 */
	preview?: boolean;
	/** 预览模式的初始状态，打开时直接呈现对应 UI（仅 preview 下生效） */
	initialPhase?: UpdatePhase;
}

const PREVIEW_DOWNLOAD_STEP_MS = 80;
const PREVIEW_RESTART_HOLD_MS = 2500;

/**
 * 启动静默检查发现新版本后的更新提示弹窗。
 *
 * - 视觉走 ui/dialog 既有玻璃风格（同匹配网易云歌曲弹窗），进度条样式与
 *   设置页 UpdateSettingCard 一致；
 * - 下载期间禁止 ESC / 点击遮罩关闭，避免误触打断安装流程；
 * - 下载安装完成后 3 秒自动 relaunch（与设置页行为一致）。
 */
export function UpdateDialog({
	info,
	open,
	onDismiss,
	preview = false,
	initialPhase = "idle",
}: UpdateDialogProps) {
	const [phase, setPhase] = useState<UpdatePhase>("idle");
	const [progress, setProgress] = useState<DownloadProgress | null>(null);
	const relaunchTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

	useEffect(() => {
		return () => {
			if (relaunchTimerRef.current !== null) {
				clearTimeout(relaunchTimerRef.current);
			}
		};
	}, []);

	// 预览模式打开时复位到指定初始状态
	useEffect(() => {
		if (!preview || !open) return;
		setPhase(initialPhase);
		if (initialPhase !== "downloading") setProgress(null);
	}, [preview, open, initialPhase]);

	// 预览模式：模拟下载进度（约 4s 走完），完成后进入 restarting
	useEffect(() => {
		if (!preview || phase !== "downloading") return;
		let percent = 0;
		setProgress({ downloaded: 0, contentLength: 100 });
		const interval = setInterval(() => {
			percent = Math.min(100, percent + 2);
			setProgress({ downloaded: percent, contentLength: 100 });
			if (percent >= 100) {
				clearInterval(interval);
				setPhase("restarting");
			}
		}, PREVIEW_DOWNLOAD_STEP_MS);
		return () => clearInterval(interval);
	}, [preview, phase]);

	// 预览模式：restarting 停留片刻后复位并关闭，便于反复预览（不 relaunch）
	useEffect(() => {
		if (!preview || phase !== "restarting") return;
		const timer = setTimeout(() => {
			setPhase("idle");
			setProgress(null);
			onDismiss();
			toast.info("预览结束，弹窗已复位");
		}, PREVIEW_RESTART_HOLD_MS);
		return () => clearTimeout(timer);
	}, [preview, phase, onDismiss]);

	if (!info) return null;

	const isBusy = phase !== "idle";
	const percent =
		progress && progress.contentLength > 0
			? Math.min(
					100,
					Math.round((progress.downloaded / progress.contentLength) * 100),
				)
			: null;

	function downloadLabel() {
		if (phase === "restarting") return "即将重启...";
		if (percent !== null) return `正在下载 (${percent}%)`;
		return "正在下载...";
	}

	function handleOpenChange(nextOpen: boolean) {
		if (!nextOpen && !isBusy) onDismiss();
	}

	function blockDismissWhileBusy(event: { preventDefault: () => void }) {
		if (isBusy) event.preventDefault();
	}

	async function handleInstall() {
		if (!info || phase !== "idle") return;
		if (preview) {
			setPhase("downloading");
			return;
		}
		setPhase("downloading");
		setProgress(null);
		try {
			await downloadAndInstallUpdate(info.update, setProgress);
			setPhase("restarting");
			toast.success("更新下载完成，即将重启并挂载更新...", { duration: 3000 });
			relaunchTimerRef.current = setTimeout(() => {
				relaunch().catch((error) => {
					console.error("[UpdateDialog] 自动重启失败:", error);
					setPhase("idle");
					toast.error("自动重启失败，更新将在下次启动时生效");
				});
			}, 3000);
		} catch (error) {
			console.error("[UpdateDialog] 更新下载/安装失败:", error);
			toast.error("更新失败，请稍后重试...");
			setPhase("idle");
			setProgress(null);
		}
	}

	return (
		<Dialog open={open} onOpenChange={handleOpenChange}>
			<DialogContent
				className="max-w-md"
				tabIndex={-1}
				// 打开时不自动聚焦「忽略本次更新」（Radix 默认聚焦弹窗内第一个
				// 可聚焦元素）：键盘模态下该按钮弹出即带 focus-visible 光圈，
				// 随后点击 / Tab 等焦点变化又熄灭，视觉上呈样式闪动。焦点改为
				// 落在 Content 容器上（tabIndex=-1，无任何 focus 视觉表现），
				// Tab 首下即进按钮组，ESC / 遮罩关闭行为不受影响。
				onOpenAutoFocus={(event) => {
					event.preventDefault();
					(event.currentTarget as HTMLElement | null)?.focus();
				}}
				onEscapeKeyDown={blockDismissWhileBusy}
				onPointerDownOutside={blockDismissWhileBusy}
				onInteractOutside={blockDismissWhileBusy}
			>
				<DialogTitle>发现新版本 v{info.version}</DialogTitle>
				<DialogBody>
					<div className="flex flex-col gap-3">
						{/* notes 是 Markdown（发布脚本从 CHANGELOG 提取的版本段落）：
						    走结构化渲染展示标题/列表/加粗/代码/链接；容器沿用原有的
						    高度上限与滚动（长内容内部滚动），文字基线由 MarkdownView 继承 */}
						<div className="no-scrollbar max-h-56 overflow-y-auto rounded-md bg-fill-subtle p-3 text-muted-foreground text-sm">
							{info.notes ? (
								<MarkdownView source={info.notes} />
							) : (
								"本次更新没有提供更新说明。"
							)}
						</div>
						{phase === "restarting" && (
							<div className="text-muted-foreground text-xs">
								更新完成，应用即将自动重启...
							</div>
						)}
					</div>
				</DialogBody>
				{isBusy && percent !== null && (
					<div className="mx-4 h-1 overflow-hidden rounded-full bg-muted">
						<div
							className="h-full bg-primary transition-all duration-300 ease-out"
							style={{ width: `${percent}%` }}
						/>
					</div>
				)}
				<DialogFooter>
					<DialogCancel disabled={isBusy}>忽略本次更新</DialogCancel>
					<DialogAction onClick={handleInstall} disabled={isBusy}>
						{isBusy ? (
							<div className="flex items-center gap-2">
								<Spinner />
								<span>{downloadLabel()}</span>
							</div>
						) : (
							"立即更新"
						)}
					</DialogAction>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	);
}
