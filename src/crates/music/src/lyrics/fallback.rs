//! 兜底逐字时间轴。
//!
//! 逐字歌词（网易云的 yrc）只覆盖一部分歌曲；酷狗的 KRC 逐字自 2025 年起
//! 加密后无法解开，接口只能拿到逐行的 LRC；本地歌词文件也常见只有逐行时间。
//! 这类歌词过去只能整行淡入淡出，与逐字歌词的「逐字扫过」观感不一致。
//!
//! 这里按行时长把逐行歌词摊成逐字时间轴（首字落在行首、末字收在行尾），
//! 让歌词页无论歌词来自哪里都用同一套动效渲染——与原版
//! `applyFallbackWordTiming` 的做法一致。
//!
//! 跨度取值（直接决定高亮跟不跟得上人声）：
//! - 有行尾标记时用标记，它本身就是演唱结束点；
//! - 没有标记时**不能直接拉满到下一行**：那会把中间的伴奏也算成演唱时长，
//!   实测有行被拉到 31s、46s，扫过整段慢放，听感就是「跟不上人声」。
//!   这里用同曲参照跨度 ×1.5 封顶，超出部分算作伴奏，扫完即停——
//!   宁可「唱完就停」也不要「整段慢放」。

use std::time::Duration;

use crate::{LyricsLine, LyricsWord};

/// 没有下一行、也没有行尾标记时的兜底行时长。
const FALLBACK_LINE_DURATION: Duration = Duration::from_millis(3000);

/// 为缺少逐字时间的行生成兜底时间轴。有逐字时间的行原样保留。
pub fn apply(lines: &mut [LyricsLine]) {
    let span_cap = reference_span(lines).map(|span| span.mul_f64(1.5));
    for index in 0..lines.len() {
        if lines[index].words.as_ref().is_some_and(|words| !words.is_empty()) {
            continue;
        }
        let tokens = tokens_of(&lines[index].text);
        if tokens.is_empty() {
            continue;
        }
        let start = lines[index].start;
        let end = line_end(lines, index, span_cap);
        let span = end.saturating_sub(start);
        let weights: Vec<usize> = tokens
            .iter()
            .map(|token| token.chars().count().max(1))
            .collect();
        let weight_sum: usize = weights.iter().sum();
        if weight_sum == 0 {
            continue;
        }

        let mut words = Vec::with_capacity(tokens.len());
        let mut cursor = start;
        for (index, token) in tokens.iter().enumerate() {
            let remaining = end.saturating_sub(cursor);
            let share = if index + 1 == tokens.len() {
                remaining
            } else {
                Duration::from_millis(
                    (span.as_millis() as u64 * weights[index] as u64 / weight_sum as u64) as u64,
                )
            };
            let duration = share.min(remaining).max(Duration::from_millis(1));
            words.push(LyricsWord {
                start: cursor,
                end: cursor + duration,
                text: token.clone(),
            });
            cursor += duration;
        }
        if let Some(last) = words.last() {
            lines[index].end = Some(last.end);
        }
        lines[index].words = Some(words);
    }
}

/// 这一行唱到哪里：有行尾标记就用标记，否则用下一行的行首（受参照跨度封顶）。
fn line_end(lines: &[LyricsLine], index: usize, span_cap: Option<Duration>) -> Duration {
    let start = lines[index].start;
    if let Some(end) = lines[index].end
        && end > start
    {
        return end;
    }
    let next = lines.get(index + 1).map(|line| line.start);
    match next {
        Some(next) if next > start => {
            let gap = next - start;
            match span_cap {
                Some(cap) => start + gap.min(cap),
                None => next,
            }
        }
        _ => match span_cap {
            Some(cap) => start + FALLBACK_LINE_DURATION.min(cap),
            None => start + FALLBACK_LINE_DURATION,
        },
    }
}

/// 兜底跨度的参照值：优先用「本行到下一行行首的间隔」中位数——
/// 间隔几乎总是可取且样本多、抗离群，连续演唱的歌词里它与「一句的时长」
/// 同量级，足以把大段间奏这类离群值挑出来。间隔取不到时才退回
/// 「已知行尾的真跨度」中位数。
fn reference_span(lines: &[LyricsLine]) -> Option<Duration> {
    let mut gaps = Vec::new();
    for pair in lines.windows(2) {
        let gap = pair[1].start.saturating_sub(pair[0].start);
        if !gap.is_zero() {
            gaps.push(gap);
        }
    }
    if let Some(median) = median(&gaps) {
        return Some(median);
    }
    let spans: Vec<Duration> = lines
        .iter()
        .filter_map(|line| line.end.map(|end| end.saturating_sub(line.start)))
        .filter(|span| !span.is_zero())
        .collect();
    median(&spans)
}

/// 中位数；样本不足两笔时返回 None（单笔不可信）。
fn median(values: &[Duration]) -> Option<Duration> {
    if values.len() < 2 {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort();
    let middle = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        (sorted[middle - 1] + sorted[middle]) / 2
    })
}

/// 把一整行歌词切成逐字动效的词元。
///
/// 切分只影响动效粒度，拼接结果必须与原文逐字相同；空白并入前一个词元，
/// 避免扫过时在空格处停顿。
fn tokens_of(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut latin = String::new();
    for character in text.chars() {
        if is_wide(character) {
            if !latin.is_empty() {
                tokens.push(std::mem::take(&mut latin));
            }
            tokens.push(character.to_string());
        } else if character.is_whitespace() {
            // 空白并入「当前」词元：优先尚未提交的拉丁缓冲，否则并入上一个
            // 已提交词元，都没有则另起一个。顺序必须严格保持，词元拼接才能
            // 与原文逐字相同。
            if !latin.is_empty() {
                latin.push(character);
            } else if let Some(last) = tokens.last_mut() {
                last.push(character);
            } else {
                latin.push(character);
            }
        } else {
            latin.push(character);
        }
    }
    if !latin.is_empty() {
        tokens.push(latin);
    }
    tokens
}

/// 是否是宽字符（中日韩文字与全角标点）。
fn is_wide(character: char) -> bool {
    // 0x3000-0x303F 的中日韩符号已含在 0x2E80-0x9FFF 内。
    matches!(character as u32, 0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(start: u64, end: Option<u64>, text: &str) -> LyricsLine {
        LyricsLine {
            start: Duration::from_millis(start),
            end: end.map(Duration::from_millis),
            text: text.to_owned(),
            romanized: None,
            words: None,
            secondary: Vec::new(),
            voice: crate::Voice::Lead,
        }
    }

    #[test]
    fn a_line_without_words_gets_a_timeline() {
        let mut lines = vec![
            line(0, None, "你好"),
            line(2000, None, "世界"),
            line(4000, None, "再见"),
        ];
        apply(&mut lines);
        let words = lines[0].words.as_ref().expect("words");
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, "你");
        assert_eq!(words[1].text, "好");
        assert_eq!(words[0].start, Duration::ZERO);
        assert!(words[1].end > words[0].end);
        assert!(lines[0].end.is_some());
    }

    #[test]
    fn a_line_with_words_is_left_alone() {
        let mut line = line(0, None, "你好");
        line.words = Some(vec![LyricsWord {
            start: Duration::ZERO,
            end: Duration::from_millis(100),
            text: "你".to_owned(),
        }]);
        let mut lines = vec![line];
        apply(&mut lines);
        assert_eq!(lines[0].words.as_ref().map(|words| words.len()), Some(1));
    }

    #[test]
    fn a_long_gap_is_capped_by_the_reference_span() {
        // 前三行是密唱（1s 一句），后面隔了 30s 才开始唱——间奏不能算成
        // 演唱时长，高亮必须扫完就停。
        let mut lines = vec![
            line(0, None, "a"),
            line(1000, None, "b"),
            line(2000, None, "c"),
            line(31000, None, "d"),
        ];
        apply(&mut lines);
        for index in 2..lines.len() {
            let words = lines[index].words.as_ref().expect("words");
            let span = words.last().expect("a word").end - words[0].start;
            assert!(
                span <= Duration::from_millis(1500),
                "line {index} should be capped near the reference span, got {span:?}"
            );
        }
    }

    #[test]
    fn latin_words_stay_whole() {
        let mut lines = vec![line(0, Some(1000), "hello world")];
        apply(&mut lines);
        let words = lines[0].words.as_ref().expect("words");
        // 拉丁词整体成一个词元（空格并入未提交的拉丁缓冲，与原版一致）。
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "hello world");
    }
}
