import {
	ArrowSync20Regular,
	Copy20Regular,
	FolderOpen24Regular,
} from "@fluentui/react-icons";
import { invoke } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogAction,
	DialogBody,
	DialogCancel,
	DialogContent,
	DialogFooter,
	DialogTitle,
} from "@/components/ui/dialog";
import { formatBytes } from "@/lib/debug/memory-stats";
import { toast } from "sonner";

/**
 * 一次读多少尾部行。日志单文件上限 5MB，全部读出来既慢又没人看；
 * 定位「刚才发生了什么」只需要末尾这些，真要翻历史就去目录里打开文件。
 */
const TAIL_LINES = 800;

interface LogFileInfo {
	name: string;
	sizeBytes: number;
}

/**
 * 应用内日志查看器。
 *
 * 存在的理由：release 构建没有控制台（见 main.rs 的 windows_subsystem），
 * 用户报「播放卡住」时如果只能靠"打开某个文件夹再找文件"，多半找不到也懒得找。
 * 直接在应用里看末尾几十行，PANIC / ERROR 一眼能对上时间。
 */
export function LogViewerDialog({
	open,
	onClose,
}: {
	open: boolean;
	onClose: () => void;
}) {
	const [files, setFiles] = useState<LogFileInfo[]>([]);
	const [selected, setSelected] = useState<string | null>(null);
	const [content, setContent] = useState("");
	const [directory, setDirectory] = useState<string | null>(null);
	const [keyword, setKeyword] = useState("");
	const [error, setError] = useState<string | null>(null);
	const [nonce, setNonce] = useState(0);
	const bodyRef = useRef<HTMLPreElement>(null);

	const loadFile = useCallback(async (name: string | null) => {
		if (!name) {
			setContent("");
			return;
		}
		setError(null);
		try {
			setContent(
				await invoke<string>("log_file_tail", { name, maxLines: TAIL_LINES }),
			);
		} catch (err) {
			setContent("");
			setError(err instanceof Error ? err.message : String(err));
		}
	}, []);

	/** 重新列目录；nonce 让「选中文件没变但想看最新内容」也能触发重读 */
	const refresh = useCallback(async () => {
		setError(null);
		try {
			const [list, dir] = await Promise.all([
				invoke<LogFileInfo[]>("log_files"),
				invoke<string | null>("log_directory"),
			]);
			setFiles(list);
			setDirectory(dir);
			// 保持用户当前看的那份；它不在了（被轮转走/清理）就回到最大的一份
			setSelected((current) =>
				current && list.some((file) => file.name === current)
					? current
					: (list[0]?.name ?? null),
			);
		} catch (err) {
			setError(err instanceof Error ? err.message : String(err));
		}
		setNonce((value) => value + 1);
	}, []);

	useEffect(() => {
		if (open) void refresh();
	}, [open, refresh]);

	useEffect(() => {
		void loadFile(selected);
	}, [selected, nonce, loadFile]);

	// 看日志的人要的是最新几行，读完就滚到底
	useEffect(() => {
		const el = bodyRef.current;
		if (el) el.scrollTop = el.scrollHeight;
	}, [content]);

	const visibleLines = keyword
		? content.split("\n").filter((line) => line.toLowerCase().includes(keyword.toLowerCase()))
		: content.split("\n");

	const copyAll = async () => {
		try {
			await navigator.clipboard.writeText(
				`日志目录：${directory ?? "未知"}\n\n${content}`,
			);
			toast.success("日志末尾已复制，可直接发给开发者");
		} catch {
			toast.error("复制失败，可先在上方「日志目录」里手动打开文件");
		}
	};

	return (
		<Dialog
			open={open}
			onOpenChange={(value) => {
				if (!value) onClose();
			}}
		>
			<DialogContent className="sm:max-w-4xl">
				<DialogTitle>查看日志</DialogTitle>
				<DialogBody>
					<div className="flex flex-col gap-2 p-4 pt-2 text-sm">
						<div className="flex flex-wrap items-center gap-2">
							{files.map((file) => (
								<Button
									key={file.name}
									variant={selected === file.name ? "winui" : "ghost"}
									size="sm"
									onClick={() => setSelected(file.name)}
								>
									{file.name}
									<span className="ml-1 text-foreground/50">
										{formatBytes(file.sizeBytes)}
									</span>
								</Button>
							))}
							{files.length === 0 && (
								<span className="text-foreground/55">还没有日志文件</span>
							)}
							<input
								className="ml-auto w-48 rounded-md border border-input bg-transparent px-2 py-1 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring"
								placeholder="过滤关键字，如 PANIC"
								value={keyword}
								onChange={(event) => setKeyword(event.target.value)}
							/>
						</div>

						{error && <div className="text-destructive">{error}</div>}

						<pre
							ref={bodyRef}
							className="max-h-[52vh] overflow-y-auto rounded-md bg-black/25 p-3 font-mono text-[11px] leading-4 whitespace-pre-wrap text-foreground/85 dark:bg-white/5"
						>
							{visibleLines.join("\n") || "（没有匹配的行）"}
						</pre>

						<div className="text-xs text-foreground/55">
							只显示末尾 {TAIL_LINES} 行；完整日志在
							<span className="mx-1 select-all font-mono">{directory ?? "日志目录"}</span>
						</div>
					</div>
				</DialogBody>
				<DialogFooter>
					<DialogAction asChild>
						<Button variant="winui" onClick={() => void refresh()}>
							<ArrowSync20Regular className="size-4" />
							刷新
						</Button>
					</DialogAction>
					<DialogAction asChild>
						<Button variant="outline" disabled={!content} onClick={() => void copyAll()}>
							<Copy20Regular className="size-4" />
							复制
						</Button>
					</DialogAction>
					<DialogAction asChild>
						<Button
							variant="outline"
							disabled={!directory}
							onClick={async () => {
								if (!directory) return;
								try {
									await openPath(directory);
								} catch {
									toast.error("日志目录打不开，可手动复制下方路径");
								}
							}}
						>
							<FolderOpen24Regular className="size-4" />
							打开目录
						</Button>
					</DialogAction>
					<DialogCancel asChild>
						<Button variant="ghost">关闭</Button>
					</DialogCancel>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	);
}
