use serde::{Deserialize, Serialize};

use crate::{PluginError, ProfileDto};

/// The sign-in flows passed as the `flow` argument of `wisp_sign_in_step`.
pub const FLOW_QR: u32 = 0;
pub const FLOW_PHONE: u32 = 1;
pub const FLOW_SECRET: u32 = 2;
pub const FLOW_PASSWORD: u32 = 3;
pub const FLOW_QQ: u32 = 4;
pub const FLOW_WECHAT: u32 = 5;

/// One step of the explicit sign-in state machine. The plugin never waits on a future across
/// the ABI: it returns a step, the host shows it, then calls the plugin again with the input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum SignInStep {
    /// The host shows this prompt and waits for the user.
    Prompt(Prompt),
    /// A platform challenge the host must show and send back (KuGou's `ssa-code`, captcha).
    Verify(Verify),
    /// The host waits this long and calls again with empty input.
    Wait {
        ms: u64,
    },
    /// Sign-in succeeded. `session` is opaque JSON the host stores under the plugin's slug.
    Done {
        profile: ProfileDto,
        session: String,
    },
    Failed {
        error: PluginError,
    },
}

/// What the host shows while waiting for the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Prompt {
    /// `url` is the text the QR code encodes. A `data:` URI asks the host to show the image
    /// itself, for platforms that mint the QR before the plugin sees it.
    Qr {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
    Phone {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notice: Option<String>,
    },
    Password {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notice: Option<String>,
    },
    Secret,
}

/// A secondary verification step. KuGou answers some requests with `ssa-code` and expects a
/// behaviour fingerprint (`sid`/`edt`) after the user proves they are human.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyKind {
    Ssa,
    Captcha,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verify {
    pub kind: VerifyKind,
    /// Opaque JSON describing the challenge (image URL, remaining attempts, ...).
    #[serde(default)]
    pub context: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_qr_prompt_round_trips() {
        let step = SignInStep::Prompt(Prompt::Qr {
            url: "https://music.163.com/login?codekey=abc".to_owned(),
            hint: None,
        });
        let text = serde_json::to_string(&step).expect("serialize");
        let again: SignInStep = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(again, step);
    }

    #[test]
    fn a_password_prompt_round_trips() {
        let step = SignInStep::Prompt(Prompt::Password {
            notice: Some("check the password".to_owned()),
        });
        let text = serde_json::to_string(&step).expect("serialize");
        let again: SignInStep = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(again, step);
    }

    #[test]
    fn a_verify_step_round_trips() {
        let step = SignInStep::Verify(Verify {
            kind: VerifyKind::Ssa,
            context: r#"{"ssa":"1001"}"#.to_owned(),
        });
        let text = serde_json::to_string(&step).expect("serialize");
        let again: SignInStep = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(again, step);
    }
}
