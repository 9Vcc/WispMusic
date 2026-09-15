import { describe, expect, it } from "vitest";
import { shouldEmphasizeWord } from "./verbatim-word";

describe("shouldEmphasizeWord（强调动画速度阈值）", () => {
	it("CJK 急促连唱（< 500ms/字）：不触发强调", () => {
		expect(shouldEmphasizeWord("明", 150)).toBe(false);
		expect(shouldEmphasizeWord("明", 400)).toBe(false);
	});

	it("CJK 中速一字一拍（500~760ms）：触发强调（旧 760ms 阈值漏掉的区间）", () => {
		expect(shouldEmphasizeWord("月", 500)).toBe(true);
		expect(shouldEmphasizeWord("月", 660)).toBe(true);
	});

	it("CJK 慢歌拖长音（≥ 760ms）：触发强调", () => {
		expect(shouldEmphasizeWord("几", 800)).toBe(true);
		expect(shouldEmphasizeWord("时", 1500)).toBe(true);
	});

	it("阈值边界：恰好 500 触发，499 不触发", () => {
		expect(shouldEmphasizeWord("有", 500)).toBe(true);
		expect(shouldEmphasizeWord("有", 499)).toBe(false);
	});

	it("拉丁词元：2~7 字符按时长判定，单字符与超长不触发", () => {
		expect(shouldEmphasizeWord("yeah", 800)).toBe(true);
		expect(shouldEmphasizeWord("yeah", 400)).toBe(false);
		expect(shouldEmphasizeWord("C", 800)).toBe(false);
		expect(shouldEmphasizeWord("looooooong", 800)).toBe(false);
	});

	it("带尾随半角标点的拉丁词元（非纯 CJK）：按拉丁规则判定", () => {
		expect(shouldEmphasizeWord("love,", 800)).toBe(true);
		expect(shouldEmphasizeWord("love,", 400)).toBe(false);
	});

	it("前后空白 trim 后判定", () => {
		expect(shouldEmphasizeWord(" 明 ", 600)).toBe(true);
		expect(shouldEmphasizeWord(" 明 ", 400)).toBe(false);
	});
});
