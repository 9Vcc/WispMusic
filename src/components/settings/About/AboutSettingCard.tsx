import SettingsExpandar, {
  SettingsExpandarDetail,
} from "@/components/settings/SettingsExpandar";
// 唯一保留的 Tabler 业务引用：GitHub 品牌标志在 Fluent 里没有对应图标，
// 换任何近似图标都会丢失品牌语义，所以这里按"品牌标志"例外处理。
// 其余业务图标一律使用 @fluentui/react-icons（见 UI_GUIDELINES §12）。
import { IconBrandGithub } from "@tabler/icons-react";
import { openUrl } from "@tauri-apps/plugin-opener";
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
      </div>
    </SettingsExpandar>
  );
}
