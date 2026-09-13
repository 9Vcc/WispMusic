// 渲染进程 → Rust ncm_fetch 命令的封装：
// 绕过 WebView 的 CORS 与 Set-Cookie 响应头过滤，供内置 API 与解灰音源共用
import { invoke } from "@tauri-apps/api/core";

export interface NcmFetchResponse {
	status: number;
	headers: Record<string, string>;
	setCookies: string[];
	body: string;
	bodyIsBase64: boolean;
}

export interface FetchOptions {
	method?: string;
	headers?: Record<string, string>;
	data?: string | ArrayBuffer | Uint8Array;
	followRedirects?: boolean;
}

export function arrayBufferToBase64(buffer: ArrayBuffer | Uint8Array): string {
	const bytes =
		buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer);
	let binary = "";
	const chunkSize = 0x8000;
	for (let i = 0; i < bytes.length; i += chunkSize) {
		binary += String.fromCharCode(...bytes.subarray(i, i + chunkSize));
	}
	return btoa(binary);
}

export function base64ToBytes(base64: string): Uint8Array {
	const binary = atob(base64);
	const bytes = new Uint8Array(binary.length);
	for (let i = 0; i < binary.length; i++) {
		bytes[i] = binary.charCodeAt(i);
	}
	return bytes;
}

export function bytesToHex(bytes: Uint8Array): string {
	let hex = "";
	for (const byte of bytes) {
		hex += byte.toString(16).padStart(2, "0");
	}
	return hex;
}

export async function fetchViaRust(
	url: string,
	options: FetchOptions = {},
): Promise<NcmFetchResponse> {
	const method = (options.method || "get").toUpperCase();
	let body: string | null = null;
	let bodyIsBase64 = false;
	if (options.data !== undefined && options.data !== null) {
		if (typeof options.data === "string") {
			body = options.data;
		} else {
			body = arrayBufferToBase64(options.data);
			bodyIsBase64 = true;
		}
	}

	return invoke<NcmFetchResponse>("ncm_fetch", {
		method,
		url,
		headers: options.headers || {},
		body,
		bodyIsBase64,
		followRedirects: options.followRedirects ?? true,
	});
}
