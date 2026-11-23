use serde::{Deserialize, Serialize};

use crate::manifest::EncodingDto;

/// A request header the host must send when it fetches a stream or cover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// What a plugin hands the host instead of bytes. The host owns fetching, caching, decoding and
/// progress; the plugin only describes where the bytes live.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamPlan {
    pub url: String,
    #[serde(default)]
    pub headers: Vec<Header>,
    pub encoding: EncodingDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_length: Option<u64>,
    /// The cache partition key, usually the quality tier.
    #[serde(default)]
    pub variant: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_plan_round_trips() {
        let plan = StreamPlan {
            url: "https://p1.music.126.net/example.mp3".to_owned(),
            headers: vec![Header {
                name: "User-Agent".to_owned(),
                value: "wisp".to_owned(),
            }],
            encoding: EncodingDto::Mp3 { kbps: 320 },
            expires_at_ms: None,
            content_length: Some(1_024),
            variant: "exhigh".to_owned(),
        };
        let text = serde_json::to_string(&plan).expect("serialize");
        let again: StreamPlan = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(again, plan);
    }
}
