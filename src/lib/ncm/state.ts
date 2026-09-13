// 浏览器端的匿名 token 与设备 ID 持久化（替代 Node 版的 anonymous_token 文件与 global.deviceId）
const ANONYMOUS_TOKEN_KEY = "ncm_anonymous_token";
const DEVICE_ID_KEY = "ncm_device_id";

let memoryAnonymousToken = "";
let memoryDeviceId = "";

export function getAnonymousToken(): string {
	try {
		return localStorage.getItem(ANONYMOUS_TOKEN_KEY) || memoryAnonymousToken;
	} catch {
		return memoryAnonymousToken;
	}
}

export function setAnonymousToken(token: string): void {
	memoryAnonymousToken = token;
	try {
		localStorage.setItem(ANONYMOUS_TOKEN_KEY, token);
	} catch {
		// localStorage 不可用时退化为内存态
	}
}

function generateDeviceId(): string {
	const hexChars = "0123456789ABCDEF";
	const chars: string[] = [];
	for (let i = 0; i < 52; i++) {
		chars.push(hexChars[Math.floor(Math.random() * hexChars.length)]);
	}
	return chars.join("");
}

export function getDeviceId(): string {
	const globalDeviceId = (globalThis as { deviceId?: string }).deviceId;
	if (globalDeviceId) return globalDeviceId;
	if (memoryDeviceId) return memoryDeviceId;

	let stored: string | null = null;
	try {
		stored = localStorage.getItem(DEVICE_ID_KEY);
	} catch {
		stored = null;
	}
	if (stored) {
		memoryDeviceId = stored;
		return stored;
	}

	memoryDeviceId = generateDeviceId();
	try {
		localStorage.setItem(DEVICE_ID_KEY, memoryDeviceId);
	} catch {
		// 忽略持久化失败
	}
	return memoryDeviceId;
}

export function persistDeviceId(): void {
	const globalDeviceId = (globalThis as { deviceId?: string }).deviceId;
	if (!globalDeviceId) return;
	memoryDeviceId = globalDeviceId;
	try {
		localStorage.setItem(DEVICE_ID_KEY, globalDeviceId);
	} catch {
		// 忽略持久化失败
	}
}
