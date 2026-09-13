import {
	CheckmarkCircle24Filled,
	CheckmarkStarburst20Regular,
	History20Regular,
	Info20Regular,
} from "@fluentui/react-icons";
import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import SettingsExpandar from "@/components/settings/SettingsExpandar";
import { Spinner } from "@/components/ui/spinner";
import { ChangelogDialog } from "@/components/update/changelog-dialog";
import { WispButton } from "@/components/wisp-button";
import {
	downloadAndInstallUpdate,
	UPDATE_CHECK_ENABLED,
} from "@/lib/services/appUpdate";
import { useSettingStore } from "@/lib/store/settingStore/settingStore";
import { cn } from "@/lib/utils";

/** 连续点击版本号解锁解灰设置所需的次数 */
const UNLOCK_CLICK_COUNT = 5;
/** 连续点击判定的时间窗口（毫秒） */
const UNLOCK_CLICK_WINDOW_MS = 2000;

export function UpdateSettingCard() {
	const setUnblockSettingsVisible = useSettingStore(
		(s) => s.setUnblockSettingsVisible,
	);
	const unblockUnlocked = useSettingStore(
		(s) => s.network.unblockSettingsVisible,
	);
	const [versionText, setVersionText] = useState("");
	const [checking, setChecking] = useState(false);
	const [isNewest, setIsNewest] = useState<boolean | null>(null);
	const [isUpdating, setIsUpdating] = useState(false);
	const [updateObj, setUpdateObj] = useState<Update | null>(null);
	const [progress, setProgress] = useState<{
		downloaded: number;
		contentLength: number;
	} | null>(null);
	const [changelogOpen, setChangelogOpen] = useState(false);

	// 连续点击计数：时间窗口内累计，超时或达到目标次数后重置
	const clickTimestamps = useRef<number[]>([]);

	const handleVersionClick = () => {
		if (unblockUnlocked) return;
		const now = Date.now();
		clickTimestamps.current = clickTimestamps.current.filter(
			(t) => now - t < UNLOCK_CLICK_WINDOW_MS,
		);
		clickTimestamps.current.push(now);
		if (clickTimestamps.current.length >= UNLOCK_CLICK_COUNT) {
			clickTimestamps.current = [];
			void setUnblockSettingsVisible(true);
			toast.success("已解锁歌曲解灰设置", { duration: 3000 });
		}
	};

	async function checkForUpdates() {
		const update = await check();
		if (update) {
			console.log(`found update ${update.version}`);
			setUpdateObj(update);
			setIsNewest(false);
			toast.success(`发现新版本 v${update.version}！`, { duration: 3000 });
		} else {
			setUpdateObj(null);
			setIsNewest(true);
		}
	}

	async function handleUpdate() {
		if (!updateObj) return;

		setIsUpdating(true);

		try {
			await downloadAndInstallUpdate(updateObj, setProgress);
			toast.success("更新下载完成，即将重启并挂载更新...", { duration: 3000 });
			setTimeout(() => {
				void relaunch();
			}, 3000);
		} catch (e) {
			console.log(`failed to install update: ${e}`);
			toast.error("更新失败，请稍后重试...");
		} finally {
			setIsUpdating(false);
			setProgress(null);
		}
	}

	async function handleCheck() {
		if (!UPDATE_CHECK_ENABLED) return;
		setChecking(true);
		setIsNewest(null);
		try {
			await checkForUpdates();
		} catch (e) {
			console.log(`failed to check update: ${e}`);
			toast.error("检查更新失败，请重试...");
		} finally {
			setChecking(false);
		}
	}

	useEffect(() => {
		async function loadVersion() {
			const v = await getVersion();
			setVersionText(v);
		}

		loadVersion();
	}, []);

	return (
		<div className="flex flex-col gap-0">
			<SettingsExpandar
				className={cn(isNewest !== null && "rounded-b-none")}
				title={versionText}
				onTitleClick={unblockUnlocked ? undefined : handleVersionClick}
				icon={<CheckmarkStarburst20Regular />}
				trailing={
					UPDATE_CHECK_ENABLED ? (
						<div className="flex justify-end gap-2">
							<WispButton
								variant="default"
								onClick={() => setChangelogOpen(true)}
								disabled={checking || isUpdating}
							>
								<div className="flex items-center gap-2">
									<History20Regular />
									<span>更新日志</span>
								</div>
							</WispButton>
							<WispButton
								variant="default"
								className={cn(checking && "bg-muted")}
								onClick={handleCheck}
								disabled={checking || isUpdating}
							>
								<div className="flex gap-2 transition-[width] duration-300 ease-in-out">
									<Spinner
										className={cn(
											"transition-all duration-300 -mx-2.5",
											checking
												? "opacity-100 scale-100 mx-0"
												: "opacity-0 scale-75",
										)}
									/>
									<span>{checking ? "检查中..." : "检查更新"}</span>
								</div>
							</WispButton>
						</div>
					) : undefined
				}
			></SettingsExpandar>
			{isNewest === true && (
				<div className="bg-green-400/25 rounded-b-md border-t-0 border border-border p-4 flex items-center gap-2 text-sm">
					<CheckmarkCircle24Filled className="text-green-600" />
					已更新到最新版本
				</div>
			)}
			{isNewest === false && updateObj && (
				<div className="wisp-surface rounded-t-none bg-card/60 border-t-0 p-4 flex flex-col gap-4 text-sm">
					<div className="flex justify-between items-center gap-2">
						<div className="flex items-center gap-4">
							<Info20Regular className="text-foreground" />
							检测到新版本 v{updateObj.version}，是否立即更新？
						</div>

						<div className="flex items-center gap-4">
							<WispButton
								variant="default"
								onClick={handleUpdate}
								disabled={isUpdating}
							>
								{isUpdating ? (
									<div className="flex items-center gap-2">
										<Spinner />
										<span>
											{progress && progress.contentLength > 0
												? `正在下载 (${((progress.downloaded / progress.contentLength) * 100).toFixed(0)}%)`
												: "正在更新..."}
										</span>
									</div>
								) : (
									"立即更新"
								)}
							</WispButton>
						</div>
					</div>
					{isUpdating && progress && progress.contentLength > 0 && (
						<div className="w-full h-1 bg-muted rounded-full overflow-hidden">
							<div
								className="h-full bg-primary transition-all duration-300 ease-out"
								style={{
									width: `${(progress.downloaded / progress.contentLength) * 100}%`,
								}}
							/>
						</div>
					)}
				</div>
			)}
			<ChangelogDialog open={changelogOpen} onOpenChange={setChangelogOpen} />
		</div>
	);
}
