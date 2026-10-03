import SettingsExpandar, {
  SettingsExpandarDetail,
} from "@/components/settings/SettingsExpandar";
import { Button } from "@/components/ui/button";
// 唯一保留的 Tabler 业务引用：GitHub 品牌标志在 Fluent 里没有对应图标，
// 换任何近似图标都会丢失品牌语义，所以这里按"品牌标志"例外处理。
// 其余业务图标一律使用 @fluentui/react-icons（见 UI_GUIDELINES §12）。
import { IconBrandGithub } from "@tabler/icons-react";
import { invoke } from "@tauri-apps/api/core";
import { openPath, openUrl } from "@tauri-apps/plugin-opener";
import { FolderOpen24Regular } from "@fluentui/react-icons";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { LogViewerDialog } from "./LogViewerDialog";

/**
 * 日志目录一行：显示实际路径，点图标在资源管理器里打开。
 *
 * 目录必须问 Rust 侧要，不能前端自己拼 `%APPDATA%`：安装版优先写 exe 同级的
 * `logs`（例如装在 D:\APP\WispMusic 就是 D:\APP\WispMusic\logs），装到
 * Program Files 那种不可写的位置才会回落到系统日志目录——按固定路径显示，
 * 两种安装方式里必有一个是错的。
 */
function LogDirectoryDetail() {
  const [directory, setDirectory] = useState<string | null>(null);
  const [viewerOpen, setViewerOpen] = useState(false);

  useEffect(() => {
    invoke<string | null>("log_directory")
      .then((dir) => setDirectory(dir))
      .catch(() => setDirectory(null));
  }, []);

  if (!directory) return null;

  return (
    <>
      <SettingsExpandarDetail
        desc="日志目录"
        trailing={
          <div className="flex min-w-0 items-center gap-2">
            <span
              className="max-w-[260px] truncate text-xs text-foreground/50"
              title={directory}
            >
              {directory}
            </span>
            <Button
              variant="winui"
              size="sm"
              onClick={() => setViewerOpen(true)}
            >
              查看
            </Button>
            <div
              role="button"
              tabIndex={0}
              className="hover:bg-fill-hover p-2 rounded-sm transition-all duration-300 cursor-pointer"
              title="在资源管理器中打开"
              onClick={async () => {
                try {
                  await openPath(directory);
                } catch {
                  toast.error("日志目录暂时打不开，可手动复制上方路径");
                }
              }}
            >
              <FolderOpen24Regular className="size-4 text-foreground" />
            </div>
          </div>
        }
      />
      <LogViewerDialog open={viewerOpen} onClose={() => setViewerOpen(false)} />
    </>
  );
}

export function AboutSettingCard() {
  return (
    <SettingsExpandar
      title="WispMusic"
      subtitle="更好用的网易云音乐客户端"
      icon={
        <img src={"/icons/logo.png"} className="object-cover" alt="WispMusic" />
      }
    >
      <div className="flex flex-col gap-0">
        <SettingsExpandarDetail
          desc="开发基于"
          trailing={
            <div className="hover:bg-fill-hover p-2 rounded-sm transition-all duration-300">
              <IconBrandGithub
                className="size-4 text-foreground "
                onClick={async () =>
                  await openUrl("https://github.com/1sen3/YeeMusicTauri")
                }
              />
            </div>
          }
        />
        <LogDirectoryDetail />
      </div>
    </SettingsExpandar>
  );
}
