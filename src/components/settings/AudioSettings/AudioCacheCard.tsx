import { DatabaseLightning20Regular } from "@fluentui/react-icons";
import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import SettingsExpandar, {
	SettingsExpandarDetail,
} from "@/components/settings/SettingsExpandar";
import {
	Dialog,
	DialogAction,
	DialogBody,
	DialogCancel,
	DialogContent,
	DialogFooter,
	DialogTitle,
} from "@/components/ui/dialog";
import { NumberStepper } from "@/components/ui/number-stepper";
import { WispButton } from "@/components/wisp-button";
import {
	type AudioCacheDirInfo,
	clearAllAudioCache,
	getAudioCacheDir,
} from "@/lib/player/audio-cache-dir";
import { useSettingStore } from "@/lib/store/settingStore/settingStore";
import { formatFileSize } from "@/lib/utils";

function errorMessage(error: unknown): string {
	if (typeof error === "string") return error;
	if (error instanceof Error) return error.message;
	return "未知错误";
}

export function AudioCacheCard() {
	const maxCacheSize = useSettingStore((s) => s.audio.maxCacheSize);
	const setMaxCacheSize = useSettingStore((s) => s.setMaxCacheSize);
	const setCacheDir = useSettingStore((s) => s.setCacheDir);
	const resetCacheDir = useSettingStore((s) => s.resetCacheDir);

	const [cacheInfo, setCacheInfo] = useState<AudioCacheDirInfo | null>(null);
	const [clearOpen, setClearOpen] = useState(false);
	const [clearing, setClearing] = useState(false);
	const [switchingDir, setSwitchingDir] = useState(false);

	const totalCacheBytes = cacheInfo?.totalSize ?? 0;

	const refreshCacheInfo = useCallback(async () => {
		try {
			setCacheInfo(await getAudioCacheDir());
		} catch (e) {
			console.error("Failed to get cache info", e);
		}
	}, []);

	useEffect(() => {
		refreshCacheInfo();
	}, [refreshCacheInfo]);

	const handleClearCache = async () => {
		setClearing(true);
		try {
			const freed = await clearAllAudioCache();
			toast.success(
				freed > 0
					? `缓存已清理，释放 ${formatFileSize(freed)}`
					: "没有可清理的缓存",
			);
			setClearOpen(false);
			await refreshCacheInfo();
		} catch (e) {
			// 失败时保持弹窗打开，便于用户直接重试
			toast.error(`清理缓存失败：${errorMessage(e)}`);
			console.error(e);
		} finally {
			setClearing(false);
		}
	};

	const handleChangeCacheDir = async () => {
		const selected = await open({
			directory: true,
			title: "选择缓存存储位置",
		});
		if (!selected) return;

		setSwitchingDir(true);
		try {
			await setCacheDir(selected as string);
			await refreshCacheInfo();
			toast.success("缓存存储位置已更新");
		} catch (e) {
			toast.error(`更改缓存位置失败：${errorMessage(e)}`);
			console.error(e);
		} finally {
			setSwitchingDir(false);
		}
	};

	const handleResetCacheDir = async () => {
		setSwitchingDir(true);
		try {
			await resetCacheDir();
			await refreshCacheInfo();
			toast.success("已恢复默认缓存位置");
		} catch (e) {
			toast.error(`恢复默认位置失败：${errorMessage(e)}`);
			console.error(e);
		} finally {
			setSwitchingDir(false);
		}
	};

	const handleMaxCacheChange = async (value: number) => {
		try {
			await setMaxCacheSize(value);
			await refreshCacheInfo();
		} catch (e) {
			console.error("Failed to enforce cache limit", e);
		}
	};

	return (
		<div className="flex flex-col gap-1">
			<SettingsExpandar
				title="缓存管理"
				subtitle="管理缓存位置与占用空间"
				icon={<DatabaseLightning20Regular />}
			>
				<div className="flex flex-col gap-0">
					<SettingsExpandarDetail>
						<div className="flex w-full items-center justify-between">
							<span className="w-full text-sm text-foreground/60">
								当前缓存占用：{formatFileSize(totalCacheBytes)}
							</span>

							<WispButton variant="default" onClick={() => setClearOpen(true)}>
								清除缓存
							</WispButton>
						</div>
					</SettingsExpandarDetail>

					<SettingsExpandarDetail>
						<div className="flex w-full items-center justify-between gap-3">
							<div className="flex min-w-0 flex-col">
								<span className="text-sm text-foreground/60">缓存存储位置</span>
								<span
									className="truncate text-sm text-muted-foreground"
									title={cacheInfo?.path ?? ""}
								>
									{cacheInfo
										? cacheInfo.isCustom
											? cacheInfo.path
											: `默认位置（${cacheInfo.path}）`
										: "读取中…"}
								</span>
							</div>

							<div className="flex shrink-0 items-center gap-2">
								{cacheInfo?.isCustom && (
									<WispButton
										variant="default"
										disabled={switchingDir}
										onClick={handleResetCacheDir}
									>
										恢复默认
									</WispButton>
								)}
								<WispButton
									variant="default"
									disabled={switchingDir}
									onClick={handleChangeCacheDir}
								>
									{switchingDir ? "切换中…" : "更改位置"}
								</WispButton>
							</div>
						</div>
					</SettingsExpandarDetail>

					<SettingsExpandarDetail>
						<div className="flex w-full items-center justify-between">
							<span className="mr-2 text-sm text-foreground/60">
								缓存最大占用
							</span>

							<div className="flex items-center gap-2">
								<NumberStepper
									label="缓存最大占用"
									value={maxCacheSize}
									onValueChange={(value) => void handleMaxCacheChange(value)}
									max={50}
									min={1}
									step={0.5}
								/>
								<span className="text-sm text-muted-foreground">GB</span>
							</div>
						</div>
					</SettingsExpandarDetail>
				</div>
			</SettingsExpandar>

			<Dialog open={clearOpen} onOpenChange={setClearOpen}>
				<DialogContent className="sm:max-w-md">
					<DialogTitle>清除全部缓存</DialogTitle>
					<DialogBody>
						将删除缓存目录中的全部音频缓存文件（当前占用{" "}
						{formatFileSize(totalCacheBytes)}
						），不会影响音乐库、下载和播放列表。清除后歌曲会在下次播放时重新缓存。
					</DialogBody>
					<DialogFooter>
						<DialogAction onClick={handleClearCache} disabled={clearing}>
							{clearing ? "清理中…" : "清除缓存"}
						</DialogAction>
						<DialogCancel>取消</DialogCancel>
					</DialogFooter>
				</DialogContent>
			</Dialog>
		</div>
	);
}
