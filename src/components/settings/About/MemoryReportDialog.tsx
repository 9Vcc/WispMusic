import { ArrowSync20Regular } from "@fluentui/react-icons";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import {
	Dialog,
	DialogAction,
	DialogBody,
	DialogCancel,
	DialogContent,
	DialogFooter,
	DialogTitle,
} from "@/components/ui/dialog";
import {
	collectMemoryReport,
	formatBytes,
	type MemoryReport,
	type MemorySample,
} from "@/lib/debug/memory-stats";

/**
 * WebView 内存归因面板（仅调试模式可见）。
 *
 * 上半部分是**进程树的真实工作集**（Rust 侧读的，含 msedgewebview2.exe 的
 * 渲染/GPU 进程），下半部分是 JS 侧逐项估算。前者是总量、后者是占比排序，
 * 合起来才能判断下一刀切哪里。
 */
export function MemoryReportDialog({
	open,
	onClose,
}: {
	open: boolean;
	onClose: () => void;
}) {
	const [report, setReport] = useState<MemoryReport | null>(null);
	const [isSampling, setIsSampling] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [copied, setCopied] = useState(false);

	const sample = useCallback(async () => {
		setIsSampling(true);
		setError(null);
		setCopied(false);
		try {
			// 采样是同步遍历 + 序列化，先让出一帧把「采样中」画出来再干活
			await new Promise((resolve) => requestAnimationFrame(resolve));
			setReport(await collectMemoryReport());
		} catch (err) {
			setError(err instanceof Error ? err.message : String(err));
		} finally {
			setIsSampling(false);
		}
	}, []);

	useEffect(() => {
		if (open) void sample();
	}, [open, sample]);

	const copyReport = useCallback(async () => {
		if (!report) return;
		const lines: string[] = ["# WispMusic WebView 内存归因"];
		if (report.processes) {
			lines.push(
				"",
				`## 进程树（工作集合计 ${formatBytes(report.processes.totalWorkingSet)}）`,
			);
			for (const process of report.processes.processes) {
				lines.push(
					`- ${process.name} (pid ${process.pid}) 工作集 ${formatBytes(process.workingSet)} / 私有 ${formatBytes(process.privateBytes)}`,
				);
			}
		}
		if (report.heap) {
			lines.push("", "## V8 堆", `- 已用 ${formatBytes(report.heap.usedJSHeapSize)}`);
		}
		lines.push("", "## JS 侧归因");
		const groups: [string, MemorySample[]][] = [
			["store", report.stores],
			["SWR 缓存", report.swr.entries],
			["缓存", report.caches],
		];
		for (const [title, samples] of groups) {
			lines.push(`### ${title}`);
			for (const item of samples) {
				lines.push(
					`- ${item.name} ${formatBytes(item.bytes)}${item.detail ? `（${item.detail}）` : ""}`,
				);
			}
		}
		lines.push(
			"### 图形与 DOM",
			`- 图像解码 ${formatBytes(report.dom.imageBytes)}（${report.dom.images} 张）`,
			`- WebGL 画布 ${report.webgl.count} 块，帧缓冲 ${formatBytes(report.webgl.framebufferBytesMin)}~${formatBytes(report.webgl.framebufferBytesMax)}`,
			`- DOM 节点 ${report.dom.nodes}`,
		);

		try {
			await navigator.clipboard.writeText(lines.join("\n"));
			setCopied(true);
		} catch (err) {
			setError(err instanceof Error ? err.message : String(err));
		}
	}, [report]);

	return (
		<Dialog
			open={open}
			onOpenChange={(value) => {
				if (!value) onClose();
			}}
		>
			<DialogContent className="sm:max-w-3xl">
				<DialogTitle>WebView 内存归因</DialogTitle>
				<DialogBody>
					<div className="flex max-h-[70vh] flex-col gap-4 overflow-y-auto p-4 pt-2 text-sm">
						{isSampling && (
							<div className="text-foreground/60">采样中…</div>
						)}
						{error && (
							<div className="text-destructive">{error}</div>
						)}

						{report && (
							<>
								<SummaryCards report={report} />

								<Section
									title="进程树（工作集）"
									note="Rust 侧读取：本进程 + 全部子进程，含 WebView2 的渲染 / GPU 进程"
								>
									{report.processes ? (
										report.processes.processes.map((process) => (
											<Row
												key={process.pid}
												name={process.name}
												detail={`pid ${process.pid} · 私有 ${formatBytes(process.privateBytes)}`}
												bytes={process.workingSet}
												max={report.processes?.processes[0]?.workingSet ?? 1}
											/>
										))
									) : (
										<div className="text-foreground/55">
											{report.processError ?? "不可用"}
										</div>
									)}
								</Section>

								<Section
									title="JS 侧归因"
									note="各 store 为序列化后的 UTF-8 体积，是稳定的相对指标；真实堆约为它的 1.5~2 倍"
								>
									<SampleGroup samples={report.stores} />
								</Section>

								<Section
									title={`SWR 缓存（${report.swr.keys} 个键）`}
									note="上限 64，超限淘汰最久未访问的键"
								>
									<SampleGroup samples={report.swr.entries} />
								</Section>

								<Section title="缓存与图形">
									<SampleGroup samples={report.caches} />
									<Row
										name="图像解码位图"
										detail={`${report.dom.images} 张 · 按 宽×高×4 估算`}
										bytes={report.dom.imageBytes}
										max={report.attributedBytes}
									/>
									<Row
										name={`WebGL 绘制缓冲（${report.webgl.count} 块）`}
										detail={`${report.webgl.framebufferBytesMin !== report.webgl.framebufferBytesMax ? "4×MSAA 上限 · " : ""}${report.webgl.sizes
											.map((size) => `${size.width}×${size.height}`)
											.join(" · ")}`}
										bytes={report.webgl.framebufferBytesMax}
										max={report.attributedBytes}
									/>
									<Row
										name="DOM 节点"
										detail={`${report.dom.nodes} 个 · ${report.dom.canvases} 个 canvas`}
										bytes={0}
										max={1}
									/>
								</Section>
							</>
						)}
					</div>
				</DialogBody>
				<DialogFooter>
					<DialogCancel onClick={onClose}>关闭</DialogCancel>
					<DialogAction
						onClick={() => void sample()}
						className="gap-2"
						disabled={isSampling}
					>
						<ArrowSync20Regular className="size-4" />
						{isSampling ? "采样中" : "重新采样"}
					</DialogAction>
					<DialogAction
						onClick={() => void copyReport()}
						className="bg-card text-foreground hover:bg-card/60"
						disabled={!report}
					>
						{copied ? "已复制" : "复制报告"}
					</DialogAction>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	);
}

function SummaryCards({ report }: { report: MemoryReport }) {
	const cards = [
		{
			label: "进程树工作集",
			value: report.processes
				? formatBytes(report.processes.totalWorkingSet)
				: "—",
			detail: report.processes
				? `${report.processes.processes.length} 个进程`
				: "不可用",
		},
		{
			label: "V8 已用堆",
			value: report.heap ? formatBytes(report.heap.usedJSHeapSize) : "—",
			detail: report.heap ? "performance.memory" : "不可用",
		},
		{
			label: "JS 侧归因合计",
			value: formatBytes(report.attributedBytes),
			detail: "store + 缓存 + 位图 + 帧缓冲",
		},
	];

	return (
		<div className="grid grid-cols-3 gap-3">
			{cards.map((card) => (
				<div
					key={card.label}
					className="rounded-md bg-card/60 px-3 py-2"
				>
					<div className="text-xs text-foreground/55">
						{card.label}
					</div>
					<div className="text-lg font-medium tabular-nums">
						{card.value}
					</div>
					<div className="text-[11px] text-foreground/45">
						{card.detail}
					</div>
				</div>
			))}
		</div>
	);
}

function Section({
	title,
	note,
	children,
}: {
	title: string;
	note?: string;
	children: ReactNode;
}) {
	return (
		<div className="flex flex-col gap-1.5">
			<div className="flex items-baseline justify-between gap-3">
				<span className="font-medium">{title}</span>
				{note && (
					<span className="text-[11px] text-foreground/45">
						{note}
					</span>
				)}
			</div>
			<div className="flex flex-col gap-1">{children}</div>
		</div>
	);
}

function SampleGroup({ samples }: { samples: MemorySample[] }) {
	const max = Math.max(...samples.map((item) => item.bytes), 1);
	return (
		<>
			{samples.map((item) => (
				<Row
					key={item.name}
					name={item.name}
					detail={item.detail}
					bytes={item.bytes}
					max={max}
				/>
			))}
		</>
	);
}

function Row({
	name,
	detail,
	bytes,
	max,
}: {
	name: string;
	detail?: string;
	bytes: number;
	max: number;
}) {
	const ratio = max > 0 ? Math.min(1, bytes / max) : 0;
	return (
		<div className="flex items-center gap-2">
			<div className="w-52 shrink-0 truncate" title={name}>
				{name}
			</div>
			<div className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-fill-strong">
				<div
					className="h-full rounded-full bg-primary"
					style={{ width: `${ratio * 100}%` }}
				/>
			</div>
			<div className="w-20 shrink-0 text-right tabular-nums">
				{bytes > 0 ? formatBytes(bytes) : "—"}
			</div>
			<div
				className="w-56 shrink-0 truncate text-[11px] text-foreground/50"
				title={detail}
			>
				{detail ?? ""}
			</div>
		</div>
	);
}
