import {
	ArrowSync24Regular,
	Checkmark24Regular,
	Warning24Regular,
} from "@fluentui/react-icons";
import { motion } from "framer-motion";
import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { fetchKugouLyricText } from "@/lib/services/kugouLyric";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { selectActiveLyricVersion } from "@/lib/store/playerStore/playerStoreSlice/createLyricVersionSlice";
import { useProgressStore } from "@/lib/store/playerStore/progressStore";
import type { LyricVersionSelection } from "@/lib/types/player";
import { cn } from "@/lib/utils";
import { type ILyricLine, ParseLyric } from "@/lib/utils/lyric-parser";
import {
	Dialog,
	DialogAction,
	DialogBody,
	DialogCancel,
	DialogContent,
	DialogFooter,
	DialogTitle,
} from "../ui/dialog";
import { Spinner } from "../ui/spinner";

/**
 * 歌词版本选择（布局与交互对齐 EchoMusic 的 LyricSourceDialog 横置浮窗）：
 * 左列候选列表、右列预览面板。同一首歌在酷狗歌词库常有多个版本
 * （官方推荐 / 用户上传 / 第三方），默认歌词缺失、时间轴糟糕或不完整时
 * 可以从这里换一版，按歌曲记忆。
 *
 * 注意：本应用统一下载 LRC（逐行）渲染——酷狗的逐字 KRC 密文当前无法用
 * 公开算法解开（实测），因此这里不展示「逐字」标签，逐字扫过由兜底时间轴生成。
 */

/** 实时预览窗口：当前行上方保留 3 行，共展示 11 行（EchoMusic 同款） */
const PREVIEW_WINDOW_BEFORE = 3;
const PREVIEW_WINDOW_SIZE = 11;

function sourceLabel(version: LyricVersionSelection) {
	if (version.productFrom === "官方推荐歌词") return "官方推荐";
	if (version.productFrom === "ugc") return "用户上传";
	return version.productFrom || "未知来源";
}

function versionTitle(version: LyricVersionSelection) {
	const name = [version.song, version.singer].filter(Boolean).join(" · ");
	return name || `版本 #${version.id}`;
}

/**
 * 星级评分（EchoMusic 同款）：接口返回的 score（0-100）映射为 0-5 星，
 * 0.5 粒度支持半星——实心用主题色，半星用 50% 宽度遮罩叠出。
 */
function StarRating({ score }: { score?: number }) {
	if (typeof score !== "number" || !Number.isFinite(score) || score <= 0)
		return null;
	const rating = Math.min(5, Math.max(0, Math.round((score / 20) * 2) / 2));
	return (
		<span
			className="inline-flex shrink-0 items-center gap-px text-[11px] leading-none text-foreground/30"
			title={`推荐 ${rating} 星`}
		>
			{Array.from({ length: 5 }, (_, i) => {
				const position = i + 1;
				if (position <= Math.floor(rating))
					return (
						<span key={position} className="text-primary">
							★
						</span>
					);
				if (position === Math.ceil(rating) && rating % 1 !== 0)
					return (
						<span key={position} className="relative inline-block">
							<span>★</span>
							<span className="absolute inset-y-0 left-0 w-1/2 overflow-hidden text-primary">
								★
							</span>
						</span>
					);
				return <span key={position}>★</span>;
			})}
		</span>
	);
}

export function LyricVersionDialog({
	open,
	onOpenChange,
}: {
	open: boolean;
	onOpenChange: (value: boolean) => void;
}) {
	const currentSong = usePlayerStore((s) => s.currentSong);
	const candidates = usePlayerStore((s) => s.lyricVersionCandidates);
	const status = usePlayerStore((s) => s.lyricVersionStatus);
	const activeVersion = usePlayerStore(selectActiveLyricVersion);
	const loadCandidates = usePlayerStore((s) => s.loadLyricVersionCandidates);
	const applyLyricVersion = usePlayerStore((s) => s.applyLyricVersion);
	const clearLyricVersion = usePlayerStore((s) => s.clearLyricVersion);

	const [selectedId, setSelectedId] = useState("");
	const [previewLines, setPreviewLines] = useState<ILyricLine[]>([]);
	const [previewLoading, setPreviewLoading] = useState(false);
	const [applying, setApplying] = useState(false);

	// 实时预览：按当前播放进度高亮并滚动窗口（EchoMusic 的 live preview）
	const currentTimeMs = useProgressStore((s) =>
		Math.round(s.currentTime * 1000),
	);

	const durationMs = currentSong
		? (currentSong.localOriginalMetadata?.durationSecs ?? 0) > 0
			? Math.round(
					(currentSong.localOriginalMetadata?.durationSecs ?? 0) * 1000,
				)
			: currentSong.dt
		: 0;

	useEffect(() => {
		if (!open) return;
		setSelectedId("");
		setPreviewLines([]);
		void loadCandidates(false);
	}, [open, loadCandidates]);

	// 预选（EchoMusic 的 pickInitialCandidate 同款逻辑）：候选就绪后优先选中
	// 当前生效版本，否则选第一个——打开/刷新即见预览，不必先手动点一行。
	// 只在当前选中项失效（打开初、刷新后列表变化）时接管，不干扰手动选择。
	useEffect(() => {
		if (!open || candidates.length === 0) return;
		const stillValid = candidates.some((version) => version.id === selectedId);
		if (stillValid) return;
		setSelectedId(
			activeVersion && candidates.some((v) => v.id === activeVersion.id)
				? activeVersion.id
				: candidates[0].id,
		);
	}, [open, candidates, selectedId, activeVersion]);

	const selectedVersion =
		candidates.find((version) => version.id === selectedId) ?? null;

	// 选中候选后拉取内容做预览（与应用共用同一条下载路径），保留全部行
	// 供实时窗口取段，而不是只截前 N 行
	useEffect(() => {
		if (!selectedVersion) {
			setPreviewLines([]);
			return;
		}
		let cancelled = false;
		setPreviewLoading(true);
		fetchKugouLyricText(selectedVersion)
			.then((text) => {
				if (cancelled) return;
				const lines = ParseLyric(text) ?? [];
				setPreviewLines(lines.filter((line) => !line.isLeadDots));
			})
			.catch((error) => {
				if (cancelled) return;
				setPreviewLines([]);
				toast.error("歌词预览加载失败", {
					description: error instanceof Error ? error.message : undefined,
				});
			})
			.finally(() => {
				if (!cancelled) setPreviewLoading(false);
			});
		return () => {
			cancelled = true;
		};
	}, [selectedVersion]);

	// 当前播放时间落在哪一行（行时间 <= 当前时间的最后一行）
	const activePreviewIndex = useMemo(() => {
		let index = -1;
		for (let i = 0; i < previewLines.length; i += 1) {
			if (previewLines[i].lineStart <= currentTimeMs) index = i;
			else break;
		}
		return index;
	}, [previewLines, currentTimeMs]);

	// 以当前行为中心的固定窗口：窗口平移代替滚动，不依赖滚动条位置
	const livePreviewLines = useMemo(() => {
		const start = Math.max(0, activePreviewIndex - PREVIEW_WINDOW_BEFORE);
		return previewLines
			.slice(start, start + PREVIEW_WINDOW_SIZE)
			.map((line, offset) => ({ line, index: start + offset }));
	}, [previewLines, activePreviewIndex]);

	const durationDiffLabel = (version: LyricVersionSelection) => {
		if (!version.durationMs || !durationMs) return "";
		const diff = Math.round((version.durationMs - durationMs) / 1000);
		if (diff === 0) return "";
		return `${diff > 0 ? "+" : ""}${diff}s`;
	};

	const hasLargeDurationDiff = (version: LyricVersionSelection) =>
		Boolean(
			version.durationMs &&
				durationMs &&
				Math.abs(version.durationMs - durationMs) > 5000,
		);

	const handleApply = async () => {
		if (!selectedVersion) return;
		setApplying(true);
		const ok = await applyLyricVersion(selectedVersion);
		setApplying(false);
		if (ok) onOpenChange(false);
	};

	const handleRestore = async () => {
		setApplying(true);
		await clearLyricVersion();
		setApplying(false);
		onOpenChange(false);
	};

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent
				className="sm:max-w-[760px] bg-card"
				tabIndex={-1}
				// 打开时不让 Radix 自动聚焦首个可聚焦元素（第一行按钮）：键盘模态下它会
				// 吃到 focus-visible 的灰色描边 + 焦点环，看起来像一条多余灰边。
				// 焦点改为落在容器上（tabIndex=-1），Tab 首下即进按钮，ESC/遮罩照常。
				onOpenAutoFocus={(event) => {
					event.preventDefault();
					(event.currentTarget as HTMLElement | null)?.focus();
				}}
			>
				<DialogTitle>选择歌词</DialogTitle>
				<DialogBody>
					<div className="flex h-[min(420px,calc(100vh-340px))] min-h-[300px] flex-col gap-3">
						{/* 概要行：曲名 - 歌手 + 刷新（EchoMusic 的 source-summary） */}
						<div className="flex shrink-0 items-center justify-between gap-2">
							<p className="truncate text-xs text-foreground/60">
								{currentSong?.name ?? ""}
								{currentSong?.ar?.length
									? ` · ${currentSong.ar.map((artist) => artist.name).join("/")}`
									: ""}
							</p>
							{/* 刷新候选（EchoMusic 同款）：强制绕过缓存重新搜索 */}
							<button
								type="button"
								aria-label="刷新歌词候选"
								disabled={status === "loading"}
								onClick={() => void loadCandidates(true)}
								className="flex size-7 shrink-0 items-center justify-center rounded-md text-foreground/60 transition-colors hover:bg-foreground/[0.06] hover:text-foreground disabled:pointer-events-none disabled:opacity-40"
							>
								<ArrowSync24Regular
									className={cn(
										"size-4",
										status === "loading" && "animate-spin",
									)}
								/>
							</button>
						</div>

						{/* 横置双栏：左候选列表 / 右预览面板（EchoMusic 的 source-layout） */}
						<div className="grid min-h-0 flex-1 grid-cols-1 gap-3 md:grid-cols-[minmax(220px,0.86fr)_minmax(280px,1.14fr)]">
							{/* 左栏：候选列表 */}
							<div className="flex min-h-0 flex-col gap-2 overflow-y-auto rounded-lg border border-foreground/[0.06] bg-foreground/[0.02] p-2">
								{candidates.map((version) => {
									const isActive = activeVersion?.id === version.id;
									const isSelected = selectedId === version.id;
									const diffLabel = durationDiffLabel(version);
									return (
										<button
											key={version.id}
											type="button"
											onClick={() => setSelectedId(version.id)}
											className={cn(
												"relative flex shrink-0 flex-col gap-1.5 rounded-lg border px-3 py-2.5 pr-9 text-left transition-colors",
												isSelected
													? "border-primary bg-primary/10"
													: "border-foreground/[0.06] hover:bg-foreground/[0.05]",
											)}
										>
											<span className="truncate text-sm font-semibold">
												{versionTitle(version)}
											</span>
											<div className="flex items-center justify-between gap-2">
												<span
													className={cn(
														"inline-flex h-5 shrink-0 items-center rounded-full px-2 text-[10px] leading-none",
														version.productFrom === "官方推荐歌词"
															? "bg-primary/10 text-primary"
															: "bg-foreground/[0.06] text-foreground/60",
													)}
												>
													{sourceLabel(version)}
												</span>
												<StarRating score={version.score} />
											</div>
											{(version.language || diffLabel || version.nickname) && (
												<div className="flex flex-wrap items-center gap-1.5 text-[10px] text-foreground/55">
													{version.language && <span>{version.language}</span>}
													{diffLabel && (
														<span
															className={cn(
																"rounded-full px-1.5 py-0.5",
																hasLargeDurationDiff(version)
																	? "bg-amber-400/15 text-amber-600 dark:text-amber-400"
																	: "bg-foreground/[0.06]",
															)}
														>
															{diffLabel}
														</span>
													)}
													{version.nickname && (
														<span className="truncate">
															by {version.nickname}
														</span>
													)}
												</div>
											)}
											{/* 使用中：右上角主题色勾选徽标（EchoMusic 的 current-check） */}
											{isActive && (
												<span className="absolute top-2 right-2 flex size-4 items-center justify-center rounded-full bg-primary text-primary-foreground">
													<Checkmark24Regular className="size-2.5" />
												</span>
											)}
										</button>
									);
								})}
								{candidates.length === 0 && status === "loading" && (
									<div className="flex flex-1 items-center justify-center gap-2 text-sm text-foreground/60">
										<Spinner className="size-4" />
										正在搜索歌词版本…
									</div>
								)}
								{candidates.length === 0 && status === "error" && (
									<div className="flex flex-1 items-center justify-center px-2 text-center text-sm text-foreground/60">
										歌词版本搜索失败，请稍后重试
									</div>
								)}
								{candidates.length === 0 &&
									status !== "loading" &&
									status !== "error" && (
										<div className="flex flex-1 items-center justify-center px-2 text-center text-sm text-foreground/60">
											没有找到候选版本
										</div>
									)}
							</div>

							{/* 右栏：预览面板（EchoMusic 的 preview-pane） */}
							<div className="flex min-h-0 flex-col overflow-hidden rounded-lg border border-foreground/[0.06] bg-foreground/[0.02]">
								<motion.div
									key={selectedVersion?.id ?? "empty"}
									initial={{ opacity: 0 }}
									animate={{ opacity: 1 }}
									transition={{ duration: 0.16 }}
									className="flex min-h-0 flex-1 flex-col"
								>
									{selectedVersion ? (
										<>
											{/* 预览头：标题 + 来源/上传者 | 行数徽标 */}
											<div className="flex shrink-0 items-start justify-between gap-2 border-b border-foreground/[0.06] px-3 py-2.5">
												<div className="min-w-0">
													<div className="truncate text-sm font-semibold">
														{selectedVersion.song ||
															currentSong?.name ||
															"未知歌曲"}
													</div>
													<div className="truncate text-[11px] text-foreground/55">
														{sourceLabel(selectedVersion)}
														{selectedVersion.nickname
															? ` · ${selectedVersion.nickname}`
															: ""}
													</div>
												</div>
												<span className="inline-flex h-5 shrink-0 items-center rounded-full bg-foreground/[0.06] px-2 text-[10px] leading-none text-foreground/60">
													{previewLoading && previewLines.length === 0 ? (
														<Spinner className="size-3" />
													) : (
														`${previewLines.length} 行`
													)}
												</span>
											</div>
											{/* 时长差异警告（EchoMusic 同款） */}
											{hasLargeDurationDiff(selectedVersion) && (
												<div className="flex shrink-0 items-center gap-1.5 bg-amber-400/10 px-3 py-1.5 text-[11px] text-amber-600 dark:text-amber-400">
													<Warning24Regular className="size-3.5 shrink-0" />
													这份歌词时长差异较大，可能不同步
												</div>
											)}
											{/* 实时预览窗口：跟随播放进度平移，当前行加粗 */}
											<div className="flex min-h-0 flex-1 flex-col items-center gap-2 overflow-y-auto px-3 py-3 text-center">
												{previewLoading && previewLines.length === 0 ? (
													<span className="m-auto text-xs text-foreground/50">
														正在加载预览…
													</span>
												) : livePreviewLines.length === 0 ? (
													<span className="m-auto text-xs text-foreground/50">
														该版本没有可预览的内容
													</span>
												) : (
													livePreviewLines.map(({ line, index }) => (
														// shrink-0：overflow-y-auto 的 flex 列容器里子项默认可压缩，
														// 空间不足时行会被压扁裁字而非出滚动条（EchoMusic 的预览行完整显示）
														<span
															key={`${line.lineStart}-${index}`}
															className={cn(
																"shrink-0 text-[13px] leading-relaxed transition-colors",
																index === activePreviewIndex
																	? "font-semibold text-foreground"
																	: "text-foreground/55",
															)}
														>
															{line.lineText}
														</span>
													))
												)}
											</div>
										</>
									) : (
										<span className="m-auto text-xs text-foreground/50">
											{candidates.length > 0
												? "选择左侧版本查看预览"
												: "暂无可选版本"}
										</span>
									)}
								</motion.div>
							</div>
						</div>
					</div>
				</DialogBody>
				<DialogFooter>
					{activeVersion && (
						<DialogCancel
							className="mr-auto h-8 w-auto flex-none px-3 text-xs"
							disabled={applying}
							onClick={() => void handleRestore()}
						>
							恢复默认歌词
						</DialogCancel>
					)}
					<DialogAction
						className="h-8 w-auto flex-none px-3 text-xs"
						disabled={!selectedVersion || applying}
						onClick={() => void handleApply()}
					>
						{applying ? "应用中…" : "应用此版本"}
					</DialogAction>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	);
}
