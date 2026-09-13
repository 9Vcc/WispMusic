import { type MotionValue, motion } from "framer-motion";
import { type CSSProperties, forwardRef } from "react";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import type { ILyricLine } from "@/lib/utils/lyric-parser";
import { getScrollYTransition, LAYOUT_TRANSITION } from "./lyric-animation";
import { LyricLeadDots } from "./lyric-lead-dots";
import { VerbatimWord } from "./verbatim-word";

const LINE_PRESS_TRANSITION = {
  type: "spring" as const,
  stiffness: 420,
  damping: 30,
  mass: 0.7,
};
// With the lyric column composited via mix-blend-mode: plus-lighter, glyph
// luminance is added onto the background, so no static text-shadow is needed
// for depth — emphasized words animate their own glow in verbatim-word.
const lyricTextBlendStyle: CSSProperties = {
  WebkitFontSmoothing: "antialiased",
};
const lyricMainTextStyle: CSSProperties = {
  ...lyricTextBlendStyle,
  fontSize:
    "calc(var(--app-lyric-font-size, 2.25rem) * var(--app-lyric-scale, 1))",
  fontWeight: "var(--app-lyric-font-weight, 800)",
  // 与 verbatim-word 的遮罩共用同一亮度系数，亮背景下整体压低白色，
  // 避免 plus-lighter 叠加后过曝
  color: "rgb(255 255 255 / calc(80% * var(--lyric-alpha-scale, 1)))",
};
// 行内边距与翻译/罗马音副行随 --app-lyric-scale 等比缩放（歌词页窗口自适应）；
// 所有渲染分支必须共用同一套尺寸，滚动居中计算依赖各分支布局一致
const linePaddingStyle: CSSProperties = {
  padding: "calc(1rem * var(--app-lyric-scale, 1))",
};
const subStyleBase: CSSProperties = {
  color: "rgba(255, 255, 255, 0.42)",
  ...lyricTextBlendStyle,
  fontSize: "calc(1.5rem * var(--app-lyric-scale, 1))",
  marginTop: "calc(1rem * var(--app-lyric-scale, 1))",
};

export const LyricLine = forwardRef<
  HTMLDivElement,
  {
    lyricLine: ILyricLine;
    transLine?: ILyricLine;
    romaLine?: ILyricLine;
    showTrans: boolean;
    showRoma: boolean;
    currentTimeMotion: MotionValue<number>;
    scrollDelay: number;
    isActive: boolean;
    opacity: number;
    blur: number;
    targetScrollY: number;
    isScrolling: boolean;
    isLargeJump: boolean;
    isLayoutChanging?: boolean;
    inWindow: boolean;
    /** 歌词偏移（ms，正值=歌词提前），透传给需要自行订阅时间的过门 dots */
    offsetMs?: number;
  }
>(
  (
    {
      lyricLine,
      transLine,
      romaLine,
      showTrans,
      showRoma,
      currentTimeMotion,
      scrollDelay,
      isActive,
      opacity,
      blur,
      targetScrollY,
      isScrolling,
      isLargeJump,
      isLayoutChanging,
      inWindow,
      offsetMs = 0,
    },
    ref,
  ) => {
    const duration = usePlayerStore((s) => s.duration);
    const seek = usePlayerStore((s) => s.seek);
    function handleClick() {
      seek((lyricLine.lineStart / (duration * 1000)) * 100);
    }
    const hasWords = lyricLine.words && lyricLine.words.length > 0;
    const yTransition = getScrollYTransition({
      isLayoutChanging: !!isLayoutChanging,
      isScrolling,
      isLargeJump,
      scrollDelay,
    });
    if (lyricLine.isLeadDots) {
      return (
        <LyricLeadDots
          isActive={isActive}
          targetScrollY={targetScrollY}
          yTransition={yTransition}
          ref={ref}
          lyricLine={lyricLine}
          offsetMs={offsetMs}
        />
      );
    }
    const staticMainText = lyricLine.lineText;
    const subStyle: CSSProperties = {
      ...subStyleBase,
      filter: `blur(${blur}px)`,
    };
    if (!inWindow) {
      return (
        <div
          ref={ref}
          className="rounded-xl inline-flex flex-col pointer-events-none"
          style={{
            ...linePaddingStyle,
            transform: `translateY(${targetScrollY}px)`,
            opacity: 0,
          }}
        >
          <span
            className="text-4xl text-white font-extrabold"
            style={lyricMainTextStyle}
          >
            {staticMainText}
          </span>
          {showTrans && transLine && (
            <span className="font-medium" style={subStyleBase}>
              {transLine.lineText}
            </span>
          )}
          {showRoma && romaLine && (
            <span className="font-medium" style={subStyleBase}>
              {romaLine.lineText}
            </span>
          )}
        </div>
      );
    }
    if (!isActive) {
      return (
        <motion.div
          layout="position"
          ref={ref}
          initial={false}
          animate={{ y: targetScrollY }}
          transition={{ y: yTransition, layout: LAYOUT_TRANSITION }}
          className="select-none"
        >
          <motion.div
            className="hover:bg-white/5 rounded-xl inline-flex flex-col transition-colors duration-300 transform-gpu will-change-transform"
            onClick={handleClick}
            whileTap={{ scale: 0.982 }}
            transition={LINE_PRESS_TRANSITION}
            style={{ transformOrigin: "left center", ...linePaddingStyle }}
          >
            <motion.span
              initial={false}
              className="text-4xl text-white/80 font-extrabold"
              style={lyricMainTextStyle}
              animate={{ filter: `blur(${blur}px)`, opacity }}
              transition={{ type: "spring", stiffness: 100, damping: 20 }}
            >
              {staticMainText}
            </motion.span>
            {showTrans && transLine && (
              <motion.span
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                transition={{ type: "spring", stiffness: 100, damping: 20 }}
                className="font-medium"
                style={subStyle}
              >
                {transLine.lineText}
              </motion.span>
            )}
            {showRoma && romaLine && (
              <motion.span
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                transition={{ type: "spring", stiffness: 100, damping: 20 }}
                className="font-medium"
                style={subStyle}
              >
                {romaLine.lineText}
              </motion.span>
            )}
          </motion.div>
        </motion.div>
      );
    }
    return (
      <motion.div
        layout="position"
        ref={ref}
        initial={false}
        animate={{ y: targetScrollY }}
        transition={{ y: yTransition, layout: LAYOUT_TRANSITION }}
      >
        <motion.div
          className="hover:bg-white/5 rounded-xl inline-flex flex-col transition-colors duration-300 select-none transform-gpu"
          onClick={handleClick}
          whileTap={{ scale: 0.982 }}
          transition={LINE_PRESS_TRANSITION}
          style={{ transformOrigin: "left center", ...linePaddingStyle }}
        >
          <motion.span
            initial={false}
            className="text-4xl text-white/80 font-extrabold will-change-transform"
            style={lyricMainTextStyle}
            animate={{
              filter: `blur(${blur}px)`,
              opacity,
              transformOrigin: "left center",
              y: !hasWords ? -4 : 0,
            }}
            transition={{ type: "spring", stiffness: 100, damping: 20 }}
          >
            {hasWords
              ? lyricLine.words?.map((word) => (
                  <VerbatimWord
                    key={`${word.startTime}-${word.duration}-${word.char}`}
                    word={word}
                    currentTimeMotion={currentTimeMotion}
                  />
                ))
              : lyricLine.lineText}
          </motion.span>
          {showTrans && transLine && (
            <motion.span
              initial={{ opacity: 0 }}
              animate={{ opacity }}
              transition={{ type: "spring", stiffness: 100, damping: 20 }}
              className="font-medium will-change-transform"
              style={subStyle}
            >
              {transLine.lineText}
            </motion.span>
          )}
          {showRoma && romaLine && (
            <motion.span
              initial={{ opacity: 0 }}
              animate={{ opacity }}
              transition={{ type: "spring", stiffness: 100, damping: 20 }}
              className="font-medium will-change-transform"
              style={subStyle}
            >
              {romaLine.lineText}
            </motion.span>
          )}
        </motion.div>
      </motion.div>
    );
  },
);
LyricLine.displayName = "LyricLine";
