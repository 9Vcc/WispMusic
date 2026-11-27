//! Real-time audio decoding, processing and output. This crate owns every per-sample loop and is
//! built at full optimisation even in debug builds.
#![allow(dead_code)]

pub mod aac;
pub mod audio;
pub mod decode;
pub mod dns;
pub mod equalizer;
pub mod mp4;
pub mod output;
pub mod ranged;
pub mod sink;
pub mod spectrum;
pub mod speed;
pub mod stream;
pub mod trim;

pub use audio::Meter;
pub use output::{Server as SoundServer, missing_bridge};
pub use spectrum::{Analyzer, Level, Listening, Spectrum, Watch};
