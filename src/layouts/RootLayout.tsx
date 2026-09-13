import { Suspense } from "react";
import { Outlet } from "react-router-dom";
import { AppSidebar } from "@/components/app-sidebar";
import { GlobalContextMenu } from "@/components/context-menu/global-context-menu";
import { GlassHighlightController } from "@/components/glass-highlight-controller";
import { LocalMusicMatchDialog } from "@/components/local-music/local-music-match-dialog";
import { PageTransition } from "@/components/page-transition";
import { PlayerBar } from "@/components/playerbar/playerbar";
import { AuthConfig } from "@/components/providers/auth-provider";
import { ScrollIndicator } from "@/components/scroll-indicator";
import { Titlebar } from "@/components/titlebar/titlebar";
import { SidebarProvider } from "@/components/ui/sidebar";
import { Toaster } from "@/components/ui/sonner";
import { UpdateDialog } from "@/components/update/update-dialog";
import { TitlebarProvider } from "@/contexts/titlebar-context";
import { useLocalLyricAutoLoad } from "@/hooks/use-local-lyric-auto-load";
import { useStartupUpdateCheck } from "@/hooks/use-startup-update-check";
import { useContextMenuStore } from "@/lib/store/contextMenuStore/contextMenuStore";

export default function RootLayout() {
	// 切歌时自动匹配音频同目录的同名歌词文件（仅主窗口需要）
	useLocalLyricAutoLoad();

	// 启动时静默检查更新（仅主窗口需要），发现新版本时弹窗提示，
	// 已是最新或检查失败则完全静默
	const { updateInfo, dialogOpen, dismissUpdate } = useStartupUpdateCheck();

	// bg-background 是窗口底色的**唯一**绘制点，不要移除：
	// index.html 的关键内联样式里有未分层的 body { background: transparent }，
	// 它优先于 index.css 中 @layer base 的 body 规则（Tailwind v4 下未分层样式
	// 优先于所有分层样式），所以 body 那条是死代码。拿掉这里就等于窗口没了底。
	// wisp-window-base 是材质/明暗切换时压暗层 background-color 过渡的挂点。
	return (
		<div className="wisp-window-base font-sans antialiased h-screen flex flex-col overflow-hidden select-none bg-background">
			<GlassHighlightController />
			<AuthConfig />
			<TitlebarProvider>
				<SidebarProvider className="flex w-full h-full flex-col overflow-hidden min-h-0!">
					<div className="w-full shrink-0 z-50">
						<Suspense>
							<Titlebar />
						</Suspense>
					</div>

					<div className="flex-1 w-full overflow-hidden flex relative">
						<Suspense fallback={null}>
							<AppSidebar />
						</Suspense>

						<div
							id="main-wrapper"
							className="relative flex flex-col flex-1 overflow-hidden bg-main-background border border-border rounded-tl-lg border-b-0 "
						>
							<main
								id="main-scroll-container"
								className="flex-1 w-full h-full overflow-y-auto no-scrollbar"
								onDragStart={(e) => e.preventDefault()}
								onContextMenu={(e) => {
									e.preventDefault();
									if (useContextMenuStore.getState().isOpen) return;
									const selection = window.getSelection();
									const text = selection?.toString().trim();
									if (text) {
										useContextMenuStore
											.getState()
											.openMenu(e.clientX, e.clientY, "text-selection", {
												selectedText: text,
											});
									}
								}}
							>
								<PageTransition>
									<Outlet />
								</PageTransition>

								<Toaster
									containerAriaLabel="Notifications"
									className="absolute"
									style={{ position: "absolute" }}
								/>
							</main>
							<ScrollIndicator />
						</div>
					</div>
				</SidebarProvider>
			</TitlebarProvider>
			<GlobalContextMenu />
			<LocalMusicMatchDialog />
			<UpdateDialog
				info={updateInfo}
				open={dialogOpen}
				onDismiss={dismissUpdate}
			/>
			<div className="w-full z-40">
				<PlayerBar />
			</div>
		</div>
	);
}
