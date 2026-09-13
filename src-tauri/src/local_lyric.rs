use encoding_rs::{Encoding, GB18030};
use std::path::{Path, PathBuf};

/// 歌词文本大小上限：正常 LRC 远小于此值，超过基本可判定选错了文件
const LYRIC_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// 读取本地歌词（或任意文本）文件。
///
/// 编码按「BOM → 严格 UTF-8 → GB18030 → 有损 UTF-8」依次探测：
/// 国内本地音乐目录里的 `.lrc` 相当一部分是 GBK/GB18030 编码，
/// 直接按 UTF-8 读会整篇乱码，而乱码在解析阶段只会表现为「没有时间标签」，
/// 用户拿不到可理解的提示。这里在读取阶段就把编码问题解决掉。
#[tauri::command]
pub fn read_text_file(path: String) -> Result<String, String> {
    let file_path = Path::new(&path);

    if !file_path.is_file() {
        return Err(format!("歌词文件不存在：{}", path));
    }

    let metadata =
        std::fs::metadata(file_path).map_err(|e| format!("无法读取歌词文件信息：{}", e))?;

    if metadata.len() > LYRIC_MAX_BYTES {
        return Err(format!(
            "歌词文件过大（{} MB），请确认选择的是歌词文件",
            metadata.len() / 1024 / 1024
        ));
    }

    let bytes = std::fs::read(file_path).map_err(|e| format!("读取歌词文件失败：{}", e))?;

    Ok(decode_text(&bytes))
}

fn decode_text(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    // 1) 带 BOM：直接按 BOM 指定的编码解码（UTF-8 / UTF-16LE / UTF-16BE）
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes) {
        let (text, _, _) = encoding.decode(&bytes[bom_len..]);
        return text.into_owned();
    }

    // 2) 无 BOM 的严格 UTF-8
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }

    // 3) 中文 GB18030（GBK 超集）
    let (text, _, had_errors) = GB18030.decode(bytes);
    if !had_errors {
        return text.into_owned();
    }

    // 4) 兜底：有损解码，至少保留可读部分，让后续解析给出「不是有效歌词」的提示
    String::from_utf8_lossy(bytes).into_owned()
}

/// 查找与音频文件同目录、同名的歌词文件。
///
/// 匹配顺序：
/// 1. 完全同名（忽略大小写）的 `.lrc`；
/// 2. `<音频名>.xxx.lrc` 形式（`.zh.lrc` / `.trans.lrc` 等常见命名），
///    存在多个时取文件名最短的一个，保证结果稳定。
#[tauri::command]
pub fn find_sibling_lyric(audio_path: String) -> Result<Option<String>, String> {
    let audio = Path::new(&audio_path);

    let dir = match audio.parent() {
        Some(d) => d,
        None => return Ok(None),
    };
    let stem = match audio.file_stem().and_then(|s| s.to_str()) {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(None),
    };

    if !dir.is_dir() {
        return Ok(None);
    }

    let stem_lower = stem.to_lowercase();
    let prefix = format!("{}.", stem_lower);
    let mut fallbacks: Vec<PathBuf> = Vec::new();

    let entries = std::fs::read_dir(dir).map_err(|e| format!("读取歌词所在目录失败：{}", e))?;

    for entry in entries.filter_map(|e| e.ok()) {
        let candidate = entry.path();
        if !candidate.is_file() {
            continue;
        }

        let is_lrc = candidate
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("lrc"))
            .unwrap_or(false);
        if !is_lrc {
            continue;
        }

        let candidate_stem = candidate
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        if candidate_stem == stem_lower {
            return Ok(Some(candidate.to_string_lossy().to_string()));
        }

        if candidate_stem.starts_with(&prefix) {
            fallbacks.push(candidate);
        }
    }

    // read_dir 顺序不保证稳定，这里按「文件名短 → 字典序」排序后取第一个
    fallbacks.sort_by(|a, b| {
        let a_name = a.file_name().map(|n| n.to_string_lossy().to_lowercase());
        let b_name = b.file_name().map(|n| n.to_string_lossy().to_lowercase());
        match (a_name, b_name) {
            (Some(a_name), Some(b_name)) => {
                a_name.len().cmp(&b_name.len()).then_with(|| a_name.cmp(&b_name))
            }
            _ => std::cmp::Ordering::Equal,
        }
    });

    Ok(fallbacks
        .into_iter()
        .next()
        .map(|p| p.to_string_lossy().to_string()))
}
