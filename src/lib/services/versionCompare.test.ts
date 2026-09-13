import { describe, expect, it } from "vitest";
import { compareVersions, isVersionNewer } from "./versionCompare";

describe("compareVersions", () => {
	it("相同版本返回 0", () => {
		expect(compareVersions("1.0.1", "1.0.1")).toBe(0);
	});

	it("缺失的数字段按 0 处理", () => {
		expect(compareVersions("1.2", "1.2.0")).toBe(0);
		expect(compareVersions("1", "1.0.0")).toBe(0);
	});

	it("忽略前导 v 与 build 元数据", () => {
		expect(compareVersions("v1.0.2", "1.0.2")).toBe(0);
		expect(compareVersions("V1.0.1+build.5", "1.0.1")).toBe(0);
	});

	it("按 major/minor/patch 逐段比较", () => {
		expect(compareVersions("2.0.0", "1.9.9")).toBeGreaterThan(0);
		expect(compareVersions("1.1.0", "1.0.99")).toBeGreaterThan(0);
		expect(compareVersions("1.0.2", "1.0.1")).toBeGreaterThan(0);
		expect(compareVersions("1.0.1", "1.0.2")).toBeLessThan(0);
		expect(compareVersions("1.10.0", "1.9.0")).toBeGreaterThan(0);
	});

	it("预发布版本低于同版本正式版", () => {
		expect(compareVersions("1.0.1-beta.1", "1.0.1")).toBeLessThan(0);
		expect(compareVersions("1.0.1", "1.0.1-rc.1")).toBeGreaterThan(0);
	});

	it("预发布版本按点分段逐段比较", () => {
		expect(compareVersions("1.0.1-beta.1", "1.0.1-beta.2")).toBeLessThan(0);
		expect(compareVersions("1.0.1-alpha", "1.0.1-alpha.1")).toBeLessThan(0);
		expect(compareVersions("1.0.1-beta", "1.0.1-rc")).toBeLessThan(0);
		// semver 规则：数字标识符恒低于字母标识符
		expect(compareVersions("1.0.1-1", "1.0.1-alpha")).toBeLessThan(0);
	});

	it("预发布比较仍遵循主版本号", () => {
		expect(compareVersions("1.0.2-beta.1", "1.0.1")).toBeGreaterThan(0);
		expect(compareVersions("0.9.0-beta.1", "1.0.0-alpha")).toBeLessThan(0);
	});

	it("无法解析的版本视为相等（由调用方兜底）", () => {
		expect(compareVersions("abc", "1.0.1")).toBe(0);
		expect(compareVersions("", "1.0.1")).toBe(0);
		expect(compareVersions("1.2.x", "1.2.0")).toBe(0);
	});
});

describe("isVersionNewer", () => {
	it("正常比较（含 v 前缀与预发布）", () => {
		expect(isVersionNewer("1.0.2", "1.0.1")).toBe(true);
		expect(isVersionNewer("v1.1.0", "1.0.9")).toBe(true);
		expect(isVersionNewer("1.0.1", "1.0.1")).toBe(false);
		expect(isVersionNewer("1.0.0", "1.0.1")).toBe(false);
		expect(isVersionNewer("1.0.1-beta.1", "1.0.1")).toBe(false);
		expect(isVersionNewer("1.0.2-beta.1", "1.0.1")).toBe(true);
	});

	it("任一版本无法解析时按有更新兜底（信任 updater 插件的判断）", () => {
		expect(isVersionNewer("abc", "1.0.1")).toBe(true);
		expect(isVersionNewer("1.0.2", "")).toBe(true);
	});
});
