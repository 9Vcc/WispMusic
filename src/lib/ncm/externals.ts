// 注入给 vendored 补丁模块的外部实现（替代 axios 直连与 unblockmusic-utils 包）
import { fetchViaRust } from "./fetch";
import { matchID } from "./unblock";

export interface EmbeddedExternals {
	fetchImpl: typeof fetchViaRust;
	unblockImpl: { matchID: typeof matchID };
}

export const embeddedExternals: EmbeddedExternals = {
	fetchImpl: fetchViaRust,
	unblockImpl: { matchID },
};
