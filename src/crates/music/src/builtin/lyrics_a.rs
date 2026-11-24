use std::time::Duration;

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use tokio::task::JoinSet;

use crate::lyrics::sheet;
use crate::{
    Lyrics, LyricsHit, LyricsLine, LyricsProvider, LyricsQuery, LyricsWord, Voice, escape,
};

const SOURCE: &str = "Kugou";
const SEARCH: &str = "https://mobiles.kugou.com/api/v3/search/song";
const CANDIDATES: &str = "https://lyrics.kugou.com/search";
const DOWNLOAD: &str = "https://lyrics.kugou.com/download";
const SONGS: usize = 1;
const PAGE: usize = 8;
const SHEETS: usize = 20;

const AGENT: &str = concat!(
    "wisp/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/9Vcc/WispMusic)"
);

pub struct SourceA {
    http: reqwest::Client,
}

impl SourceA {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder().dns_resolver(crate::dns::dns())
                .dns_resolver(crate::dns::dns())
                .build()
                .expect("reqwest client"),
        }
    }

    async fn songs(&self, wanted: &str) -> Result<Vec<Song>> {
        let keyword = escape::component(wanted);
        let response = self
            .http
            .get(format!(
                "{SEARCH}?format=json&keyword={keyword}&page=1&pagesize={PAGE}&showtype=1"
            ))
            .header("User-Agent", AGENT)
            .send()
            .await
            .context("cannot reach kugou")?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("kugou answered with status {status}");
        }
        let answer: SearchAnswer = response
            .json()
            .await
            .context("cannot read the kugou search response")?;
        Ok(answer.data.map(|data| data.info).unwrap_or_default())
    }

    async fn candidates(&self, song: &Song) -> Result<Vec<Candidate>> {
        let response = self
            .http
            .get(CANDIDATES)
            .query(&[
                ("ver", "1"),
                ("man", "yes"),
                ("client", "mobi"),
                ("hash", song.hash.as_str()),
                ("duration", &(song.duration * 1000).to_string()),
            ])
            .header("User-Agent", AGENT)
            .send()
            .await
            .context("cannot reach the kugou lyric index")?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("the kugou lyric index answered with status {status}");
        }
        let answer: CandidateAnswer = response
            .json()
            .await
            .context("cannot read the kugou candidate response")?;
        Ok(answer.candidates)
    }

    /// [wisp] 歌词正文。逐字候选（krctype==1）先试 KRC：密文用旧算法
    /// （krc1 + XOR 0x4a + deflate）能解就用真逐字，解不开（酷狗 2025 年
    /// 后换了加密）自动回落逐行 LRC，再由兜底时间轴摊出逐字动效。
    async fn sheet(&self, candidate: &Candidate) -> Result<Sheet> {
        if candidate.krctype == 1
            && let Some(worded) = self.worded(candidate).await?
        {
            return Ok(Sheet::Worded(worded));
        }
        Ok(Sheet::Lines(self.lines(candidate).await?))
    }

    /// 取逐字 KRC。接口只给密文，解不出来就返回 None（不当作错误）。
    async fn worded(&self, candidate: &Candidate) -> Result<Option<String>> {
        let response = self
            .http
            .get(DOWNLOAD)
            .query(&[
                ("ver", "1"),
                ("client", "pc"),
                ("id", candidate.id.to_string().as_str()),
                ("accesskey", candidate.accesskey.as_str()),
                ("fmt", "krc"),
                ("charset", "utf8"),
                ("decode", "true"),
            ])
            .header("User-Agent", AGENT)
            .send()
            .await
            .context("cannot reach the kugou lyric download")?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("the kugou lyric download answered with status {status}");
        }
        let answer: Download = response
            .json()
            .await
            .context("cannot read the kugou download response")?;
        let Some(content) = answer.content.filter(|content| !content.is_empty()) else {
            return Ok(None);
        };
        Ok(decode_krc(&content))
    }

    /// 取逐行 LRC。
    async fn lines(&self, candidate: &Candidate) -> Result<String> {
        let response = self
            .http
            .get(DOWNLOAD)
            .query(&[
                ("ver", "1"),
                ("client", "pc"),
                ("id", candidate.id.to_string().as_str()),
                ("accesskey", candidate.accesskey.as_str()),
                ("fmt", "lrc"),
                ("charset", "utf8"),
            ])
            .header("User-Agent", AGENT)
            .send()
            .await
            .context("cannot reach the kugou lyric download")?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("the kugou lyric download answered with status {status}");
        }
        let answer: Download = response
            .json()
            .await
            .context("cannot read the kugou download response")?;
        let content = answer
            .content
            .filter(|content| !content.is_empty())
            .context("kugou handed over an empty sheet")?;
        decode(&content)
    }
}

/// 歌词正文：逐行或逐字。
enum Sheet {
    Lines(String),
    Worded(String),
}

impl Default for SourceA {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct SearchAnswer {
    data: Option<SearchData>,
}

#[derive(Deserialize)]
struct SearchData {
    #[serde(default)]
    info: Vec<Song>,
}

#[derive(Clone, Deserialize)]
struct Song {
    hash: String,
    #[serde(default)]
    duration: u64,
    #[serde(default, rename = "songname")]
    name: String,
    #[serde(default, rename = "singername")]
    singer: String,
    #[serde(default, rename = "album_name")]
    album: String,
}

#[derive(Deserialize)]
struct CandidateAnswer {
    #[serde(default)]
    candidates: Vec<Candidate>,
}

#[derive(Clone, Deserialize)]
struct Candidate {
    id: String,
    accesskey: String,
    #[serde(default)]
    krctype: u8,
    #[serde(default)]
    score: Option<i64>,
    /// 官方推荐歌词 / ugc / 第三方歌词，与原版对话框的徽章一致。
    #[serde(default)]
    product_from: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    nickname: Option<String>,
}

#[derive(Deserialize)]
struct Download {
    content: Option<String>,
}

#[async_trait]
impl LyricsProvider for SourceA {
    fn name(&self) -> &'static str {
        SOURCE
    }

    async fn search(&self, query: &LyricsQuery) -> Result<Vec<LyricsHit>> {
        let wanted = format!("{} {}", query.artist, query.title);
        let songs = shortlist(self.songs(&wanted).await?, query.duration);

        let mut tasks = JoinSet::new();
        for song in songs {
            let kugou = Self {
                http: self.http.clone(),
            };
            let title = query.title.clone();
            tasks.spawn(async move {
                let candidates = kugou
                    .candidates(&song)
                    .await
                    .inspect_err(|error| {
                        log::warn!("lyrics: kugou did not list {}: {error:#}", song.hash)
                    })
                    .ok()?;
                let mut hits = Vec::new();
                for candidate in worded_first(candidates) {
                    let Ok(sheet) = kugou.sheet(&candidate).await.inspect_err(|error| {
                        log::warn!(
                            "lyrics: kugou did not hand over {}: {error:#}",
                            candidate.id
                        )
                    }) else {
                        continue;
                    };
                    hits.extend(hit(&song, &candidate, &sheet, &title));
                }
                Some(hits)
            });
        }

        let mut hits = Vec::new();
        while let Some(found) = tasks.join_next().await {
            hits.extend(found.ok().flatten().unwrap_or_default());
        }
        Ok(hits)
    }
}

fn shortlist(songs: Vec<Song>, duration: Duration) -> Vec<Song> {
    let mut songs = songs;
    songs.sort_by_key(|song| {
        Duration::from_secs(song.duration)
            .as_secs()
            .abs_diff(duration.as_secs())
    });
    songs.truncate(SONGS);
    songs
}

fn worded_first(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut candidates = candidates;
    candidates.sort_by_key(|candidate| u8::from(candidate.krctype != 2));
    candidates.truncate(SHEETS);
    candidates
}

/// KRC 密文的 XOR 密钥（16 字节循环使用）。
///
/// 酷狗客户端一直用这把密钥，公开算法能解；先前流传的「单字节 0x4a」
/// 是旧版本的做法，现在解不开。
const EN_KEY: [u8; 16] = [
    64, 71, 97, 119, 94, 50, 116, 71, 81, 54, 49, 45, 206, 210, 110, 105,
];

/// 解 KRC 密文：base64 → "krc1" 头 → 16 字节密钥循环 XOR → zlib 解压。
///
/// 解不开（旧版客户端的单字节密钥、或酷狗换了加密）就返回 None，调用方
/// 回落逐行 LRC。
fn decode_krc(content: &str) -> Option<String> {
    use flate2::read::{DeflateDecoder, ZlibDecoder};
    use std::io::Read;

    let raw = STANDARD.decode(content).ok()?;
    let body = raw.strip_prefix(b"krc1")?;

    // 现役密钥与旧版单字节密钥都试一遍。
    let mut candidates = Vec::with_capacity(2);
    let current: Vec<u8> = body
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ EN_KEY[index % EN_KEY.len()])
        .collect();
    candidates.push(current);
    candidates.push(body.iter().map(|byte| byte ^ 0x4a).collect());

    for plain in candidates {
        let mut text = String::new();
        if ZlibDecoder::new(&plain[..])
            .read_to_string(&mut text)
            .is_ok()
            && looks_like_krc(&text)
        {
            return Some(text);
        }
        let mut text = String::new();
        if DeflateDecoder::new(&plain[..])
            .read_to_string(&mut text)
            .is_ok()
            && looks_like_krc(&text)
        {
            return Some(text);
        }
    }
    None
}

/// 解出来的内容像不像 KRC（时间标签 + 逐字标记）。
fn looks_like_krc(text: &str) -> bool {
    text.starts_with('[') && (text.contains('<') || text.contains('>'))
}

/// LRC 下载的 content 是 base64 的纯文本，不需要 4 字节头/异或/解压。
fn decode(content: &str) -> Result<String> {
    let bytes = STANDARD
        .decode(content)
        .context("cannot decode the kugou sheet")?;
    String::from_utf8(bytes).context("the kugou sheet is not text")
}

fn hit(song: &Song, candidate: &Candidate, sheet: &Sheet, title: &str) -> Option<LyricsHit> {
    let mut lines = match sheet {
        Sheet::Lines(text) => crate::lyrics::lrc::parse(text),
        Sheet::Worded(text) => parse(text),
    };
    let artists: Vec<String> = song
        .singer
        .split(['、', ',', '&'])
        .map(str::trim)
        .filter(|artist| !artist.is_empty())
        .map(str::to_owned)
        .collect();
    if !sheet::headed(&mut lines, title, &artists) {
        return None;
    }
    let quiet = sheet::instrumental(&lines);
    crate::lyrics::lrc::normalize(&mut lines);
    if lines.is_empty() && !quiet {
        return None;
    }

    Some(LyricsHit {
        source: SOURCE,
        badge: candidate.product_from.clone(),
        score: candidate
            .score
            .map(|score| score.clamp(0, 100) as u32),
        trust: 0,
        lyrics: match quiet {
            true => Lyrics::plain(""),
            false => Lyrics::Synced {
                lines: lines.into(),
            },
        },
        instrumental: quiet,
        title: song.name.clone(),
        artist: song.singer.clone(),
        album: (!song.album.is_empty()).then(|| song.album.clone()),
        duration: (song.duration > 0).then(|| Duration::from_secs(song.duration)),
        writers: Vec::new(),
    })
}

fn parse(krc: &str) -> Vec<LyricsLine> {
    krc.lines().filter_map(read).collect()
}

fn read(line: &str) -> Option<LyricsLine> {
    let (header, rest) = line.strip_prefix('[')?.split_once(']')?;
    let (start, span) = pair_of(header)?;

    let mut words = Vec::new();
    let mut text = String::new();
    let mut rest = rest;
    while let Some(open) = rest.find('<') {
        let tail = &rest[open + 1..];
        let Some(shut) = tail.find('>') else { break };
        if !text.is_empty() || open > 0 {
            text.push_str(&rest[..open]);
        }
        let stamp = stamp_of(&tail[..shut]);
        rest = &tail[shut + 1..];
        let spoken = rest.find('<').map(|next| &rest[..next]).unwrap_or(rest);
        if let Some((at, length)) = stamp {
            words.push(LyricsWord {
                start: start + at,
                end: start + at + length,
                text: spoken.to_owned(),
                real_timed: true,
            });
        }
        text.push_str(spoken);
        rest = &rest[spoken.len()..];
    }
    text.push_str(rest);

    let text = text.trim().to_owned();
    if text.is_empty() {
        return None;
    }
    Some(LyricsLine {
        start,
        end: Some(start + span),
        words: (!words.is_empty()).then_some(words),
        text,
        romanized: None,
        secondary: Vec::new(),
        voice: Voice::Lead,
    })
}

fn pair_of(header: &str) -> Option<(Duration, Duration)> {
    let (start, span) = header.split_once(',')?;
    Some((
        Duration::from_millis(start.trim().parse().ok()?),
        Duration::from_millis(span.trim().parse().ok()?),
    ))
}

fn stamp_of(stamp: &str) -> Option<(Duration, Duration)> {
    let mut parts = stamp.split(',');
    let at = parts.next()?.trim().parse().ok()?;
    let length = parts.next()?.trim().parse().ok()?;
    Some((Duration::from_millis(at), Duration::from_millis(length)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实 KRC 密文（客户端密钥加密）：解出来应当是逐字时间轴。
    const ENCRYPTED: &str = "a3JjMTjb6lmXhg5xIv3iCOWx18tNM+37Ej8WKl1CPeDCpmKFwFVXV0i2cxMyVV0pLfu28RhnykRtcyaAVDY3i9VN";

    #[test]
    fn an_encrypted_sheet_decodes_to_its_words() {
        let sheet = decode_krc(ENCRYPTED).expect("the sheet should decode");
        assert!(sheet.starts_with("[ti:晴天]"), "got {sheet:?}");
        let lines = parse(&sheet);
        let line = lines.first().expect("a line");
        assert_eq!(line.start, Duration::from_millis(0));
        assert_eq!(line.end, Some(Duration::from_millis(2250)));
        let words = line.words.as_ref().expect("word timings");
        assert_eq!(words.first().expect("a word").text, "晴");
        assert_eq!(words[1].text, "天");
        assert_eq!(words[1].start, Duration::from_millis(160));
    }

    #[test]
    fn a_sheet_that_is_not_encoded_gives_nothing() {
        assert!(decode_krc("bm90IGEgc2hlZXQ=").is_none());
    }

    /// 测试里没有二进制的 rustls 安装步骤，这里补一次（幂等）。
    fn install_crypto() {
        rustls::crypto::ring::default_provider().install_default().ok();
    }

    /// 真网络探针：搜索 → 候选 → LRC 下载，确认渠道与内容可用。
    #[test]
    #[ignore]
    fn kugou_live_sheet() {
        install_crypto();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let kugou = SourceA::new();
            let songs = kugou.songs("周杰伦 晴天").await.expect("songs");
            println!("LIVE: songs {}", songs.len());
            let song = shortlist(songs, Duration::from_secs(269))
                .into_iter()
                .next()
                .expect("a song");
            println!("LIVE: picked {} - {}", song.name, song.singer);
            let candidates = kugou.candidates(&song).await.expect("candidates");
            println!("LIVE: candidates {}", candidates.len());
            for candidate in candidates.iter().take(3) {
                match kugou.sheet(candidate).await {
                    Ok(sheet) => {
                        let body = match &sheet {
                            Sheet::Lines(text) | Sheet::Worded(text) => text.as_str(),
                        };
                        println!(
                            "LIVE: krc={} worded={} head={:?}",
                            candidate.krctype,
                            matches!(sheet, Sheet::Worded(_)),
                            body.lines().take(2).collect::<Vec<_>>()
                        );
                    }
                    Err(error) => println!("LIVE: sheet failed: {error:#}"),
                }
            }
        });
    }
}
