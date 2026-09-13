// @neteasecloudmusicapienhanced/api@4.30.2 util/crypto.js 的浏览器移植版
// RSA 部分改用原生 BigInt 实现（等价于 node-forge 的无填充 raw RSA）
import CryptoJS from "crypto-js";

const iv = "0102030405060708";
const presetKey = "0CoJUm6Qyw8W8jud";
const linuxapiKey = "rFgB&h#%2?^eDg:Q";
const base62 =
	"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const publicKeyPem = `-----BEGIN PUBLIC KEY-----
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDgtQn2JZ34ZC28NWYpAUd98iZ37BUrX/aKzmFbt7clFSs6sXqHauqKWqdtLkF2KexO40H1YTX8z2lSgBBOAxLsvaklV8k4cBFK9snQXE9/DDaFt6Rr7iVZMldczhC0JNgTz+SHXT6CBHuX3e9SdB1Ua44oncaTWz7OBGLbCiK45wIDAQAB
-----END PUBLIC KEY-----`;
const eapiKey = "e82ckenh8dichen8";

type AesMode = "cbc" | "ecb";

function aesEncrypt(
	text: string,
	mode: AesMode,
	key: string,
	ivValue: string,
	format = "base64",
): string {
	const encrypted = CryptoJS.AES.encrypt(
		CryptoJS.enc.Utf8.parse(text),
		CryptoJS.enc.Utf8.parse(key),
		{
			iv: CryptoJS.enc.Utf8.parse(ivValue),
			mode: CryptoJS.mode[mode.toUpperCase() as "CBC" | "ECB"],
			padding: CryptoJS.pad.Pkcs7,
		},
	);
	if (format === "base64") {
		return encrypted.toString();
	}

	return encrypted.ciphertext.toString().toUpperCase();
}

function aesDecrypt(
	ciphertext: string,
	key: string,
	ivValue: string,
	format = "base64",
): string {
	let bytes: CryptoJS.lib.WordArray;
	if (format === "base64") {
		bytes = CryptoJS.AES.decrypt(ciphertext, CryptoJS.enc.Utf8.parse(key), {
			iv: CryptoJS.enc.Utf8.parse(ivValue),
			mode: CryptoJS.mode.ECB,
			padding: CryptoJS.pad.Pkcs7,
		});
	} else {
		bytes = CryptoJS.AES.decrypt(
			{ ciphertext: CryptoJS.enc.Hex.parse(ciphertext) } as CryptoJS.lib.CipherParams,
			CryptoJS.enc.Utf8.parse(key),
			{
				iv: CryptoJS.enc.Utf8.parse(ivValue),
				mode: CryptoJS.mode.ECB,
				padding: CryptoJS.pad.Pkcs7,
			},
		);
	}
	return bytes.toString(CryptoJS.enc.Utf8);
}

interface RsaPublicKey {
	n: bigint;
	e: bigint;
}

function readTLV(der: Uint8Array, offset: number): { content: Uint8Array; next: number } {
	let pos = offset;
	pos++; // tag
	const first = der[pos++];
	let length = first;
	if (first & 0x80) {
		const numBytes = first & 0x7f;
		length = 0;
		for (let i = 0; i < numBytes; i++) {
			length = (length << 8) | der[pos++];
		}
	}
	return { content: der.subarray(pos, pos + length), next: pos + length };
}

function bytesToBigInt(bytes: Uint8Array): bigint {
	let result = 0n;
	for (const byte of bytes) {
		result = (result << 8n) | BigInt(byte);
	}
	return result;
}

let cachedRsaKey: RsaPublicKey | null = null;

function getRsaPublicKey(): RsaPublicKey {
	if (cachedRsaKey) return cachedRsaKey;
	const b64 = publicKeyPem
		.replace(/-----[^-]+-----/g, "")
		.replace(/\s+/g, "");
	const der = Uint8Array.from(atob(b64), (char) => char.charCodeAt(0));
	// SubjectPublicKeyInfo ::= SEQUENCE(AlgorithmIdentifier, BIT STRING(RSAPublicKey))
	const outer = readTLV(der, 0);
	const algorithm = readTLV(outer.content, 0);
	const bitString = readTLV(outer.content, algorithm.next);
	const rsaPublicKey = readTLV(bitString.content, 1); // 跳过 1 字节 unused bits
	const modulus = readTLV(rsaPublicKey.content, 0);
	const exponent = readTLV(rsaPublicKey.content, modulus.next);
	cachedRsaKey = {
		n: bytesToBigInt(modulus.content),
		e: bytesToBigInt(exponent.content),
	};
	return cachedRsaKey;
}

function rsaEncryptNoPadding(text: string): string {
	const { n, e } = getRsaPublicKey();
	const bytes = new TextEncoder().encode(text);
	const m = bytesToBigInt(bytes);
	if (m >= n) {
		throw new Error("message too long for RSA modulus");
	}

	let result = 1n;
	let base = m % n;
	let exp = e;
	while (exp > 0n) {
		if (exp & 1n) result = (result * base) % n;
		base = (base * base) % n;
		exp >>= 1n;
	}

	// 定长 128 字节 hex（1024-bit RSA），前导零不影响服务端解析
	let hex = result.toString(16);
	if (hex.length % 2) hex = `0${hex}`;
	return hex.padStart(256, "0");
}

export function weapi(object: Record<string, unknown>) {
	const text = JSON.stringify(object);
	let secretKey = "";
	for (let i = 0; i < 16; i++) {
		secretKey += base62.charAt(Math.round(Math.random() * 61));
	}
	return {
		params: aesEncrypt(
			aesEncrypt(text, "cbc", presetKey, iv),
			"cbc",
			secretKey,
			iv,
		),
		encSecKey: rsaEncryptNoPadding(secretKey.split("").reverse().join("")),
	};
}

export function linuxapi(object: Record<string, unknown>) {
	const text = JSON.stringify(object);
	return {
		eparams: aesEncrypt(text, "ecb", linuxapiKey, "", "hex"),
	};
}

export function eapi(url: string, object: Record<string, unknown>) {
	const text = typeof object === "object" ? JSON.stringify(object) : object;
	const message = `nobody${url}use${text}md5forencrypt`;
	const digest = CryptoJS.MD5(message).toString();
	const data = `${url}-36cd479b6b5-${text}-36cd479b6b5-${digest}`;
	return {
		params: aesEncrypt(data, "ecb", eapiKey, "", "hex"),
	};
}

export function eapiResDecrypt(encryptedParams: string): unknown {
	try {
		const decryptedData = aesDecrypt(encryptedParams, eapiKey, "", "hex");
		return JSON.parse(decryptedData);
	} catch (error) {
		console.log("eapiResDecrypt error:", error);
		return null;
	}
}
