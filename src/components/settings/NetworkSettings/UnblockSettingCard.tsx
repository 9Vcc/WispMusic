import { MusicNote220Regular } from "@fluentui/react-icons";
import SettingsExpandar, {
	SettingsExpandarDetail,
} from "@/components/settings/SettingsExpandar";
import { Input } from "@/components/ui/input";
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { useSettingStore } from "@/lib/store/settingStore/settingStore";

// 与 VutronMusic 的解灰设置一一对应：
// 启用音源替换 / 启用 FLAC / 音源搜索模式 / 候选音源（自由文本）/ JOOX Cookie / QQ Cookie
export function UnblockSettingCard() {
	const unblockEnabled = useSettingStore((s) => s.network.unblockEnabled);
	const unblockSource = useSettingStore((s) => s.network.unblockSource);
	const unblockEnableFlac = useSettingStore(
		(s) => s.network.unblockEnableFlac,
	);
	const unblockOrderFirst = useSettingStore(
		(s) => s.network.unblockOrderFirst,
	);
	const unblockQqCookie = useSettingStore((s) => s.network.unblockQqCookie);
	const unblockJooxCookie = useSettingStore(
		(s) => s.network.unblockJooxCookie,
	);
	const setUnblockEnabled = useSettingStore((s) => s.setUnblockEnabled);
	const setUnblockSource = useSettingStore((s) => s.setUnblockSource);
	const setUnblockEnableFlac = useSettingStore((s) => s.setUnblockEnableFlac);
	const setUnblockOrderFirst = useSettingStore((s) => s.setUnblockOrderFirst);
	const setUnblockQqCookie = useSettingStore((s) => s.setUnblockQqCookie);
	const setUnblockJooxCookie = useSettingStore((s) => s.setUnblockJooxCookie);

	return (
		<SettingsExpandar
			icon={<MusicNote220Regular />}
			title="歌曲解灰"
			subtitle="灰色歌曲（版权受限/下架）通过第三方音源匹配播放；本地引擎为跨平台搜索匹配（可解无损），需 Node 运行时"
		>
			<SettingsExpandarDetail desc="启用音源替换（关闭后灰色歌曲将无法播放）">
				<Switch
					checked={unblockEnabled}
					onCheckedChange={(checked) => setUnblockEnabled(checked)}
				/>
			</SettingsExpandarDetail>
			<SettingsExpandarDetail desc="启用 FLAC（启用后需要清除歌曲缓存才能生效）">
				<Switch
					checked={unblockEnableFlac}
					disabled={!unblockEnabled}
					onCheckedChange={(checked) => setUnblockEnableFlac(checked)}
				/>
			</SettingsExpandarDetail>
			<SettingsExpandarDetail desc="音源搜索模式">
				<Select
					value={unblockOrderFirst ? "order" : "speed"}
					onValueChange={(value) => setUnblockOrderFirst(value === "order")}
					disabled={!unblockEnabled}
				>
					<SelectTrigger className="w-36 bg-card">
						<SelectValue />
					</SelectTrigger>
					<SelectContent>
						<SelectItem value="order">顺序优先</SelectItem>
						<SelectItem value="speed">速度优先</SelectItem>
					</SelectContent>
				</Select>
			</SettingsExpandarDetail>
			<SettingsExpandarDetail desc="候选音源（音源代号，多个音源请用 , 逗号分隔；留空自动。本地引擎可用：bodian, kuwo, kugou, migu, qq, bilibili）">
				<Input
					className="w-72"
					placeholder="例 bodian, bilibili, kuwo"
					value={unblockSource}
					disabled={!unblockEnabled}
					onChange={(e) => setUnblockSource(e.target.value)}
				/>
			</SettingsExpandarDetail>
			<SettingsExpandarDetail desc="QQ 引擎的 Cookie（留空则不进行相关设置）">
				<Input
					className="w-72"
					placeholder="uin=..; qm_keyst=..;"
					value={unblockQqCookie}
					disabled={!unblockEnabled}
					onChange={(e) => setUnblockQqCookie(e.target.value)}
				/>
			</SettingsExpandarDetail>
			<SettingsExpandarDetail desc="JOOX 引擎的 Cookie（留空则不进行相关设置）">
				<Input
					className="w-72"
					placeholder="wmid=..; session_key=.."
					value={unblockJooxCookie}
					disabled={!unblockEnabled}
					onChange={(e) => setUnblockJooxCookie(e.target.value)}
				/>
			</SettingsExpandarDetail>
		</SettingsExpandar>
	);
}
