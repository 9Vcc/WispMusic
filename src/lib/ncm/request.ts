// @neteasecloudmusicapienhanced/api@4.30.2 util/request.js 的浏览器移植版
// - 网络发送改为 Tauri Rust 命令 ncm_fetch（绕过 CORS 与 Set-Cookie 过滤）
// - 匿名 token / 设备 ID 改为 localStorage 管理
// - 移除代理（tunnel/pac-proxy-agent）与 Node HTTP Agent 逻辑
import CryptoJS from "crypto-js";
import { invoke } from "@tauri-apps/api/core";
import { eapi, eapiResDecrypt, linuxapi, weapi } from "./crypto";
import {
	base64ToBytes,
	bytesToHex,
	type NcmFetchResponse,
} from "./fetch";
import { getAnonymousToken, getDeviceId } from "./state";

const APP_CONF = {
	apiDomain: "https://interface.music.163.com",
	domain: "https://music.163.com",
	encrypt: true,
	encryptResponse: false,
};

const DOMAIN = APP_CONF.domain;
const API_DOMAIN = APP_CONF.apiDomain;
const ENCRYPT_RESPONSE = APP_CONF.encryptResponse;
const SPECIAL_STATUS_CODES = new Set([201, 302, 400, 502, 800, 801, 802, 803]);

const osMap = {
	pc: {
		os: "pc",
		appver: "3.1.17.204416",
		osver: "Microsoft-Windows-10-Professional-build-19045-64bit",
		channel: "netease",
	},
	linux: {
		os: "linux",
		appver: "1.2.1.0428",
		osver: "Deepin 20.9",
		channel: "netease",
	},
	android: {
		os: "android",
		appver: "8.20.20.231215173437",
		osver: "14",
		channel: "xiaomi",
	},
	iphone: {
		os: "iPhone OS",
		appver: "9.0.90",
		osver: "16.2",
		channel: "distribution",
	},
} as const;

const userAgentMap = {
	weapi: {
		pc: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36 Edg/124.0.0.0",
	},
	linuxapi: {
		linux:
			"Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/60.0.3112.90 Safari/537.36",
	},
	api: {
		pc: "Mozilla/5.0 (Windows NT 10.0; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Safari/537.36 Chrome/91.0.4472.164 NeteaseMusicDesktop/3.0.18.203152",
		android:
			"NeteaseMusic/9.1.65.240927161425(9001065);Dalvik/2.1.0 (Linux; U; Android 14; 23013RK75C Build/UKQ1.230804.001)",
		iphone:
			"NeteaseMusic 9.0.90/5038 (iPhone; iOS 16.2; zh_CN)",
	},
};

// WNMCID 只计算一次
const WNMCID = (() => {
	const characters = "abcdefghijklmnopqrstuvwxyz";
	let randomString = "";
	for (let i = 0; i < 6; i++) {
		randomString += characters.charAt(
			Math.floor(Math.random() * characters.length),
		);
	}
	return `${randomString}.${Date.now().toString()}.01.0`;
})();

export interface RequestAnswer {
	status: number;
	body: any;
	cookie: string[];
}

export interface RequestOptions {
	crypto?: string;
	cookie?: string | Record<string, string>;
	ua?: string;
	proxy?: string;
	realIP?: string;
	ip?: string;
	randomCNIP?: boolean;
	e_r?: boolean;
	domain?: string;
	checkToken?: boolean;
	headers?: Record<string, string>;
}

type CookieObject = Record<string, any>;

function cookieToJson(cookie: string): CookieObject {
	if (!cookie) return {};
	const obj: CookieObject = {};
	for (const item of cookie.split(";")) {
		const arr = item.split("=");
		if (arr.length === 2) {
			obj[arr[0].trim()] = arr[1].trim();
		}
	}
	return obj;
}

function cookieObjToString(cookie: CookieObject): string {
	return Object.keys(cookie)
		.map(
			(key) =>
				`${encodeURIComponent(key)}=${encodeURIComponent(cookie[key] ?? "")}`,
		)
		.join("; ");
}

function toBoolean(val: unknown): boolean {
	if (typeof val === "boolean") return val;
	if (val === "") return false;
	return val === "true" || val === "1";
}

function chooseUserAgent(crypto: string, uaType = "pc"): string {
	return (
		(userAgentMap as Record<string, Record<string, string>>)[crypto]?.[
			uaType
		] || ""
	);
}

function processCookieObject(cookie: CookieObject, uri: string): CookieObject {
	const _ntes_nuid = CryptoJS.lib.WordArray.random(32).toString();
	const os = osMap[(cookie.os as keyof typeof osMap) || "pc"] || osMap.pc;

	const processedCookie: CookieObject = {
		...cookie,
		__remember_me: "true",
		ntes_kaola_ad: "1",
		_ntes_nuid: cookie._ntes_nuid || _ntes_nuid,
		_ntes_nnid: cookie._ntes_nnid || `${_ntes_nuid},${Date.now().toString()}`,
		WNMCID: cookie.WNMCID || WNMCID,
		WEVNSM: cookie.WEVNSM || "1.0.0",
		osver: cookie.osver || os.osver,
		deviceId: cookie.deviceId || getDeviceId(),
		os: cookie.os || os.os,
		channel: cookie.channel || os.channel,
		appver: cookie.appver || os.appver,
	};

	if (uri.indexOf("login") === -1) {
		processedCookie["NMTID"] = CryptoJS.lib.WordArray.random(16).toString();
	}

	if (!processedCookie.MUSIC_U) {
		processedCookie.MUSIC_A =
			processedCookie.MUSIC_A || getAnonymousToken();
	}

	return processedCookie;
}

function createHeaderCookie(header: Record<string, string>): string {
	return Object.keys(header)
		.map(
			(key) =>
				`${encodeURIComponent(key)}=${encodeURIComponent(header[key])}`,
		)
		.join("; ");
}

function generateRequestId(): string {
	return `${Date.now()}_${Math.floor(Math.random() * 1000)
		.toString()
		.padStart(4, "0")}`;
}

export function createRequest(
	uri: string,
	data: Record<string, any>,
	options: RequestOptions,
): Promise<RequestAnswer> {
	return new Promise((resolve, reject) => {
		const headers: Record<string, string> = options.headers
			? { ...options.headers }
			: {};
		const ip = options.realIP || options.ip || "";
		if (ip) {
			headers["X-Real-IP"] = ip;
			headers["X-Forwarded-For"] = ip;
		}

		let cookie = options.cookie || {};
		if (typeof cookie === "string") {
			cookie = cookieToJson(cookie);
		}

		if (typeof cookie === "object") {
			cookie = processCookieObject(cookie, uri);
			headers["Cookie"] = cookieObjToString(cookie);
		}
		let url = "";
		let encryptData: Record<string, any> = {};
		let crypto = options.crypto || "";
		const csrfToken = cookie["__csrf"] || "";

		if (crypto === "") {
			crypto = APP_CONF.encrypt ? "eapi" : "api";
		}

		const answer: RequestAnswer = { status: 500, body: {}, cookie: [] };

		switch (crypto) {
			case "weapi": {
				headers["Referer"] = options.domain || DOMAIN;
				headers["User-Agent"] = options.ua || chooseUserAgent("weapi");
				data.csrf_token = csrfToken;
				encryptData = weapi(data);
				url = (options.domain || DOMAIN) + "/weapi/" + uri.substr(5);
				break;
			}

			case "linuxapi": {
				headers["User-Agent"] =
					options.ua || chooseUserAgent("linuxapi", "linux");
				encryptData = linuxapi({
					method: "POST",
					url: (options.domain || DOMAIN) + uri,
					params: data,
				});
				url = (options.domain || DOMAIN) + "/api/linux/forward";
				break;
			}

			case "eapi":
			case "api": {
				const header: Record<string, string> = {
					osver: cookie.osver ?? "",
					deviceId: cookie.deviceId ?? "",
					os: cookie.os ?? "",
					appver: cookie.appver ?? "",
					versioncode: cookie.versioncode || "140",
					mobilename: cookie.mobilename || "",
					buildver: cookie.buildver || Date.now().toString().substr(0, 10),
					resolution: cookie.resolution || "1920x1080",
					__csrf: csrfToken,
					channel: cookie.channel ?? "",
					requestId: generateRequestId(),
				};

				if (cookie.MUSIC_U) header["MUSIC_U"] = cookie.MUSIC_U;
				if (cookie.MUSIC_A) header["MUSIC_A"] = cookie.MUSIC_A;

				headers["Cookie"] = createHeaderCookie(header);
				headers["User-Agent"] =
					options.ua || chooseUserAgent("api", "iphone");

				if (crypto === "eapi") {
					data.header = header;
					data.e_r = toBoolean(
						options.e_r !== undefined
							? options.e_r
							: data.e_r !== undefined
								? data.e_r
								: ENCRYPT_RESPONSE,
					);
					encryptData = eapi(uri, data);
					url = (options.domain || API_DOMAIN) + "/eapi/" + uri.substr(5);
				} else if (crypto === "api") {
					url = (options.domain || API_DOMAIN) + uri;
					encryptData = data;
				}
				break;
			}

			default:
				console.error("Unknown Crypto:", crypto);
				break;
		}

		headers["Content-Type"] = "application/x-www-form-urlencoded";
		const requestBody = new URLSearchParams(
			encryptData as Record<string, string>,
		).toString();

		invoke<NcmFetchResponse>("ncm_fetch", {
			method: "POST",
			url,
			headers,
			body: requestBody,
			bodyIsBase64: false,
			followRedirects: true,
		})
			.then((res) => {
				answer.cookie = res.setCookies.map((x) =>
					x.replace(/\s*Domain=[^(;|$)]+;*/, ""),
				);

				try {
					if (crypto === "eapi" && data.e_r) {
						const hex = res.bodyIsBase64
							? bytesToHex(base64ToBytes(res.body))
							: bytesToHex(new TextEncoder().encode(res.body));
						answer.body = eapiResDecrypt(
							hex.toUpperCase(),
						) as Record<string, any>;
					} else {
						answer.body = JSON.parse(res.body);
					}

					if (answer.body.code) {
						answer.body.code = Number(answer.body.code);
					}

					answer.status = Number(answer.body.code || res.status);

					if (SPECIAL_STATUS_CODES.has(answer.body.code)) {
						answer.status = 200;
					}
				} catch {
					answer.body = res.body;
					answer.status = res.status;
				}

				answer.status =
					answer.status > 100 && answer.status < 600 ? answer.status : 400;

				if (answer.status === 200) {
					resolve(answer);
				} else {
					reject(answer);
				}
			})
			.catch((err) => {
				answer.status = 502;
				answer.body = { code: 502, msg: String(err) };
				reject(answer);
			});
	});
}
