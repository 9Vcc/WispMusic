export const CLOSE_LYRIC_SHEET_EVENT = "wisp-music:close-lyric-sheet";

export function requestCloseLyricSheet() {
	if (typeof window === "undefined") return;

	window.dispatchEvent(new Event(CLOSE_LYRIC_SHEET_EVENT));
}
