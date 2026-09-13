import { motion, useMotionValue, useReducedMotion } from "framer-motion";
import type React from "react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { corePlayer } from "@/lib/player/corePlayer";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import {
	selectActiveLyricVersionLines,
	selectActiveLyricVersion,
} from "@/lib/store/playerStore/playerStoreSlice/createLyricVersionSlice";
import { selectCurrentLyricOffset } from "@/lib/store/playerStore/playerStoreSlice/createLyricOffsetSlice";
import { selectActiveLocalLyric } from "@/lib/store/playerStore/playerStoreSlice/createLocalLyricSlice";
import { useProgressStore } from "@/lib/store/playerStore/progressStore";
import { useSettingStore } from "@/lib/store/settingStore/settingStore";
import { cn } from "@/lib/utils";
import {
  type ILyricLine,
  ParseLyric,
} from "@/lib/utils/lyric-parser";
import { resolveLyricLines } from "@/lib/utils/lyric-source";
import { getSidePanelMotion } from "../lyric-sheet/lyric-sheet-motion";
import { LyricLine } from "./lyric-line";

const LYRIC_CROLL_DELAY = 0.04;
const MASK_IMAGE =
  "linear-gradient(to bottom, transparent 0%, black 10%, black 90%, transparent 100%)";

interface LyricProps {
  className?: string;
  showTrans: boolean;
  showRoma: boolean;
}

export function Lyric({ className, showTrans, showRoma }: LyricProps) {
  const [currentScrollY, setCurrentScrollY] = useState(0);
  const [isScrolling, setIsScrolling] = useState(false);
  const [isLargeJump, setIsLargeJump] = useState(false);
  const [isLayoutChanging, setIsLayoutChanging] = useState(false);
  const reduceMotion = Boolean(useReducedMotion());

  const currentSong = usePlayerStore((s) => s.currentSong);
  const currentSongLyrics = usePlayerStore((s) => s.currentSongLyrics);
  // 手动选择的歌词版本（酷狗候选）优先级最高，其次本地歌词文件，再是在线歌词
  const manualVersionLines = usePlayerStore(selectActiveLyricVersionLines);
  const hasManualVersion = usePlayerStore(selectActiveLyricVersion) !== null;
  // 本地歌词（手动选择或同目录自动匹配）优先于在线歌词
  const activeLocalLyric = usePlayerStore(selectActiveLocalLyric);
  // 歌词偏移（按歌曲持久化，正值=歌词提前），只在歌词时基上生效
  const lyricOffsetMs = usePlayerStore(selectCurrentLyricOffset);
  // 全局开关：关闭后所有行都不再模糊（手动滚动时的临时解除仍照常生效）
  const lyricBlurEnabled = useSettingStore((s) => s.lyric.blurEnabled);
  const targetScrollYRef = useRef(0);

  const scrollTimeoutRef = useRef<NodeJS.Timeout | null>(null);
  const [blurDisabled, setBlurDisabled] = useState(false);
  const isUserScrolling = useRef(false);

  const containerRef = useRef<HTMLDivElement | null>(null);

  const lyricRefs = useRef<(HTMLDivElement | null)[]>([]);
  const lyricDisplayModeRef = useRef({ showTrans, showRoma });

  // 歌词来源解析统一收口在 resolveLyricLines：
  // 手动版本 > 本地歌词 > 在线逐字（yrc）> 在线逐行（lrc），
  // 并保证逐字动效不因来源不同而丢失（缺逐字数据时按行时长兜底）。
  const lyricResolution = useMemo(
    () =>
      resolveLyricLines({
        manualLines: manualVersionLines,
        localLines: activeLocalLyric?.lines,
        verbatimSource: currentSongLyrics?.yrc?.lyric,
        plainSource: currentSongLyrics?.lrc?.lyric,
      }),
    [manualVersionLines, activeLocalLyric, currentSongLyrics],
  );
  const lyric = lyricResolution.lines;
  const lyricLength = lyric.length;

  // 手动版本/本地歌词文件没有翻译/罗马音分轨，激活时不再叠加在线歌词的附加轨
  const transLyric = useMemo(() => {
    if (hasManualVersion || activeLocalLyric) return [];
    return (
      ParseLyric(currentSongLyrics?.ytlrc?.lyric) ||
      ParseLyric(currentSongLyrics?.tlyric?.lyric) ||
      []
    );
  }, [hasManualVersion, activeLocalLyric, currentSongLyrics]);

  const romaLyric = useMemo(() => {
    if (hasManualVersion || activeLocalLyric) return [];
    return (
      ParseLyric(currentSongLyrics?.yromalrc?.lyric) ||
      ParseLyric(currentSongLyrics?.romalrc?.lyric) ||
      []
    );
  }, [hasManualVersion, activeLocalLyric, currentSongLyrics]);

  const transMap = useMemo(() => {
    const map = new Map<number, ILyricLine>();
    transLyric?.forEach((t) => {
      map.set(t.lineStart, t);
    });
    return map;
  }, [transLyric]);

  const romaMap = useMemo(() => {
    const map = new Map<number, ILyricLine>();
    romaLyric?.forEach((r) => {
      map.set(r.lineStart, r);
    });
    return map;
  }, [romaLyric]);

  const getScrollBounds = useCallback(() => {
    if (!containerRef.current || lyricLength === 0) {
      return { min: 0, max: 0 };
    }

    const firstElement = lyricRefs.current[0];
    const lastElement = lyricRefs.current[lyricLength - 1];
    if (!firstElement || !lastElement) {
      return { min: 0, max: 0 };
    }

    const containerHeight = containerRef.current.clientHeight;
    const firstCentered = -(
      firstElement.offsetTop -
      containerHeight / 2 +
      firstElement.clientHeight / 2
    );
    const lastCentered = -(
      lastElement.offsetTop -
      containerHeight / 2 +
      lastElement.clientHeight / 2
    );

    return {
      min: Math.min(firstCentered, lastCentered),
      max: Math.max(firstCentered, lastCentered),
    };
  }, [lyricLength]);

  const clampScrollY = useCallback(
    (scrollY: number) => {
      const { min, max } = getScrollBounds();
      // Whole-pixel rest positions: lines sitting on fractional offsets get
      // resampled blur, which the additive (plus-lighter) compositing then
      // amplifies into visible fringing around bright glyphs.
      return Math.round(Math.min(max, Math.max(min, scrollY)));
    },
    [getScrollBounds],
  );

  const scrollToIndex = useCallback(
    (index: number) => {
      const el = lyricRefs.current[index];
      if (!el || !containerRef.current) return;
      const containerHeight = containerRef.current.clientHeight;
      const offset = el.offsetTop - containerHeight / 2 + el.clientHeight / 2;
      const nextScrollY = clampScrollY(-offset);
      targetScrollYRef.current = nextScrollY;
      setCurrentScrollY(nextScrollY);
    },
    [clampScrollY],
  );

  const findCurrentIndex = useCallback(
    (lyrics: ILyricLine[]) => {
      // 歌词偏移统一在这里进时基：行切换、滚动、逐字遮罩（currentTimeMotion）共用同一个时间，
      // 所以「对齐」是整体生效的，不会出现行准而字偏的情况。
      const currentTimeMs =
        useProgressStore.getState().currentTime * 1000 + lyricOffsetMs;
      let idx = -1;
      for (let i = lyrics.length - 1; i >= 0; i--) {
        if (lyrics[i].lineStart <= currentTimeMs && lyrics[i].lineStart >= 0) {
          idx = i;
          break;
        }
      }
      return Math.max(0, idx);
    },
    [lyricOffsetMs],
  );

  const currentTimeMotion = useMotionValue(0);
  const [currentIndex, setCurrentIndex] = useState(-1);

  useEffect(() => {
    lyricRefs.current.length = lyricLength;
  }, [lyricLength]);

  useEffect(() => {
    if (!currentSong) return;

    if (containerRef.current) {
      containerRef.current.scrollTop = 0;
    }
    targetScrollYRef.current = 0;
    if (lyric.length) {
      const idx = findCurrentIndex(lyric);
      setCurrentIndex(idx);
      requestAnimationFrame(() => scrollToIndex(idx));
    }
  }, [currentSong, scrollToIndex, lyric, findCurrentIndex]);

  useEffect(() => {
    if (!lyric.length) return;
    const idx = findCurrentIndex(lyric);
    setCurrentIndex(idx);
    requestAnimationFrame(() => scrollToIndex(idx));
  }, [lyric, scrollToIndex, findCurrentIndex]);

  // 高频订阅：直接从 corePlayer 获取时间，绕过 Zustand
  useEffect(() => {
    const unsubscribe = corePlayer.subscribeTime((currentTime) => {
      const lyricTimeMs = currentTime * 1000 + lyricOffsetMs;
      currentTimeMotion.set(lyricTimeMs);

      if (!lyric.length) return;
      let newIndex = -1;
      for (let i = lyric.length - 1; i >= 0; i--) {
        if (lyric[i].lineStart <= lyricTimeMs && lyric[i].lineStart >= 0) {
          newIndex = i;
          break;
        }
      }
      setCurrentIndex((prev) => (prev !== newIndex ? newIndex : prev));
    });
    return unsubscribe;
  }, [lyric, currentTimeMotion, lyricOffsetMs]);

  useEffect(() => {
    return () => {
      if (scrollTimeoutRef.current) clearTimeout(scrollTimeoutRef.current);
    };
  }, []);

  useEffect(() => {
    if (isLargeJump) {
      const timer = setTimeout(() => {
        setIsLargeJump(false);
      }, 500);
      return () => clearTimeout(timer);
    }
  }, [isLargeJump]);

  const scrollToCurrentIndex = useCallback(
    (skipAnimation = false) => {
      if (currentIndex < 0 || !containerRef.current) return;
      const el = lyricRefs.current[currentIndex];
      if (!el) return;

      if (skipAnimation) {
        scrollToIndex(currentIndex);
        return;
      }

      const containerHeight = containerRef.current.clientHeight;
      const offset = el.offsetTop - containerHeight / 2 + el.clientHeight / 2;
      const newTargetScrollY = clampScrollY(-offset);

      const jumpDistancePx = Math.abs(
        targetScrollYRef.current - newTargetScrollY,
      );
      if (jumpDistancePx > 150) {
        setIsLargeJump(true);
      }

      scrollToIndex(currentIndex);
    },
    [clampScrollY, currentIndex, scrollToIndex],
  );

  const recenterCurrentLineAfterLayoutChange = useCallback(() => {
    if (isUserScrolling.current || currentIndex < 0) return;

    // 必须同步补偿（layout effect 阶段 DOM 已更新、尚未绘制）：
    // 滚动补偿的 y 弹簧与各行 layout 位移弹簧参数相同且同帧启动，
    // 二者逐帧抵消，当前行才能在开关翻译/罗马音时保持居中不动。
    // 若延迟到 rAF 之后，两个弹簧错帧启动，当前行会先弹开再弹回。
    scrollToIndex(currentIndex);
  }, [currentIndex, scrollToIndex]);

  useLayoutEffect(() => {
    const previous = lyricDisplayModeRef.current;
    if (previous.showTrans === showTrans && previous.showRoma === showRoma) {
      return;
    }

    lyricDisplayModeRef.current = { showTrans, showRoma };
    setIsLayoutChanging(true);
    recenterCurrentLineAfterLayoutChange();

    const timer = window.setTimeout(() => setIsLayoutChanging(false), 50);
    return () => window.clearTimeout(timer);
  }, [showTrans, showRoma, recenterCurrentLineAfterLayoutChange]);

  useEffect(() => {
    // 手动滚动时不跳回
    if (isUserScrolling.current) return;
    scrollToCurrentIndex();
  }, [scrollToCurrentIndex]);

  useEffect(() => {
    if (!isScrolling && !isUserScrolling.current) {
      scrollToCurrentIndex();
    }
  }, [isScrolling, scrollToCurrentIndex]);

  const handleUserInteraction = () => {
    isUserScrolling.current = true;
    setBlurDisabled(true);
    setIsScrolling(true);

    if (scrollTimeoutRef.current) clearTimeout(scrollTimeoutRef.current);

    scrollTimeoutRef.current = setTimeout(() => {
      isUserScrolling.current = false;
      setBlurDisabled(false);
      setIsScrolling(false);
    }, 2000);
  };

  function handleWheel(e: React.WheelEvent<HTMLDivElement>) {
    e.preventDefault();

    if (!containerRef.current || lyricLength === 0) return;

    handleUserInteraction();
    targetScrollYRef.current = clampScrollY(
      targetScrollYRef.current - e.deltaY,
    );
    setCurrentScrollY(targetScrollYRef.current);
  }

  const visualIndex = useMemo(() => {
    if (!containerRef.current || lyricRefs.current.length === 0)
      return Math.max(0, currentIndex);
    const targetOffset = -currentScrollY;
    const containerHeight = containerRef.current.clientHeight;

    let minDiff = Infinity;
    let bestIndex = Math.max(0, currentIndex);
    for (let i = 0; i < lyricRefs.current.length; i++) {
      const el = lyricRefs.current[i];
      if (el) {
        const offset = el.offsetTop - containerHeight / 2 + el.clientHeight / 2;
        const diff = Math.abs(offset - targetOffset);
        if (diff < minDiff) {
          minDiff = diff;
          bestIndex = i;
        }
      }
    }
    return bestIndex;
  }, [currentScrollY, currentIndex]);

  return (
    <motion.div
      className={cn("h-full w-full relative", className)}
      // The panel entrance lives on this element on purpose: animating an
      // ANCESTOR (opacity/filter/transform) isolates the blending until the
      // animation settles and the glow visibly pops in, while animating the
      // blend element itself keeps plus-lighter live from the first frame.
      {...getSidePanelMotion(reduceMotion)}
      style={{
        // Additive blending against the mesh background is what gives the
        // lyrics their tinted, light-emitting Apple Music look. It must sit
        // on this outer element: the masked scroll container below forms an
        // isolated group, and a blend mode inside it would never reach the
        // background.
        mixBlendMode: "plus-lighter",
      }}
    >
      <div
        className={cn(
          "h-full w-full flex justify-center overflow-hidden relative",
          className,
        )}
        ref={containerRef}
        onWheel={handleWheel}
        onTouchStart={handleUserInteraction}
        style={{
          WebkitMaskImage: MASK_IMAGE,
          maskImage: MASK_IMAGE,
        }}
      >
        <motion.div
          className="w-full flex flex-col items-start"
          style={{
            color: "white",
            fontFamily: "var(--app-lyric-font-family, inherit)",
            gap: "calc(0.5rem * var(--app-lyric-scale, 1))",
          }}
        >
          {lyric.map((lyricLine, idx) => {
            const inWindow =
              Math.abs(idx - visualIndex) <= 10 ||
              Math.abs(idx - currentIndex) <= 5;
            const distance = Math.abs(idx - currentIndex);
            const scrollDelay = distance * LYRIC_CROLL_DELAY;

            const isActive = idx === currentIndex;
            const shouldBlur = lyricBlurEnabled && !blurDisabled && !isActive;

            const dynamicOpacity = isActive ? 1 : 0.4;
            const dynamicBlur = shouldBlur ? Math.min(6, distance * 1.8) : 0;

            return (
              <LyricLine
                key={`${lyricLine.lineStart}-${lyricLine.lineText}`}
                ref={(el) => {
                  lyricRefs.current[idx] = el;
                }}
                lyricLine={lyricLine}
                transLine={transMap.get(lyricLine.lineStart)}
                romaLine={romaMap.get(lyricLine.lineStart)}
                showTrans={showTrans}
                showRoma={showRoma}
                currentTimeMotion={currentTimeMotion}
                scrollDelay={scrollDelay}
                isActive={isActive}
                opacity={dynamicOpacity}
                blur={dynamicBlur}
                targetScrollY={currentScrollY}
                isScrolling={isScrolling}
                isLargeJump={isLargeJump}
                isLayoutChanging={isLayoutChanging}
                inWindow={inWindow}
                offsetMs={lyricOffsetMs}
              />
            );
          })}
          <div className="w-full h-[50vh] shrink-0 pointer-events-none" />
        </motion.div>
      </div>
    </motion.div>
  );
}
