import { Color20Regular, Window20Regular } from "@fluentui/react-icons";
import SettingsExpandar, {
  SettingsExpandarDetail,
} from "@/components/settings/SettingsExpandar";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  type AppearanceSettings,
  useSettingStore,
} from "@/lib/store/settingStore/settingStore";

export function AppearanceSettingCard() {
  const theme = useSettingStore((s) => s.appearance.theme);
  const setTheme = useSettingStore((s) => s.setTheme);
  const material = useSettingStore((s) => s.appearance.material);
  const setMaterial = useSettingStore((s) => s.setMaterial);

  const themeStr =
    theme === "system" ? "跟随系统" : theme === "light" ? "浅色" : "深色";

  return (
    <div className="flex flex-col gap-2">
      <SettingsExpandar
        title="主题"
        subtitle="选择显示主题"
        icon={<Color20Regular />}
        trailing={
          <span className="text-muted-foreground text-sm">{themeStr}</span>
        }
      >
        <div className="flex flex-col gap-0">
          <SettingsExpandarDetail>
            <div className="w-full flex flex-col items-start">
              <RadioGroup
                defaultValue={theme}
                onValueChange={(val) => {
                  setTheme(val as AppearanceSettings["theme"]);
                }}
              >
                <div className="flex gap-2 items-center">
                  <RadioGroupItem value="light" />
                  <span>浅色</span>
                </div>
                <div className="flex gap-2 items-center">
                  <RadioGroupItem value="dark" />
                  <span>深色</span>
                </div>
                <div className="flex gap-2 items-center">
                  <RadioGroupItem value="system" />
                  <span>跟随系统</span>
                </div>
              </RadioGroup>
            </div>
          </SettingsExpandarDetail>
        </div>
      </SettingsExpandar>

      <SettingsExpandar
        title="材质"
        subtitle="选择窗口材质"
        icon={<Window20Regular />}
        trailing={
          <Select
            value={material}
            onValueChange={(value) => {
              setMaterial(value as AppearanceSettings["material"]);
            }}
          >
            <SelectTrigger className="w-32">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="acrylic">亚克力</SelectItem>
              <SelectItem value="mica">云母</SelectItem>
              <SelectItem value="solid">纯色</SelectItem>
              <SelectItem value="none">透明</SelectItem>
            </SelectContent>
          </Select>
        }
      />
    </div>
  );
}
