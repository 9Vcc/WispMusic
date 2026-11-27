//! Type-erased decoders for the engine's audio thread.

use std::io::{self, IoSliceMut, Read, Seek, SeekFrom};
use std::time::Duration;

use rodio::Source;
use rodio::source::SeekError;

use crate::audio::Trimmed;

/// A provider reader behind one type, so every decoder is compiled in this crate.
pub struct Media(Box<dyn Readable>);

trait Readable: Read + Seek + Send + Sync {}
impl<T: Read + Seek + Send + Sync> Readable for T {}

impl Media {
    pub fn new(reader: impl Read + Seek + Send + Sync + 'static) -> Self {
        Self(Box::new(reader))
    }
}

impl Read for Media {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.0.read_vectored(bufs)
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        self.0.read_to_end(buf)
    }

    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        self.0.read_to_string(buf)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.0.read_exact(buf)
    }
}

impl Seek for Media {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0.seek(pos)
    }

    fn stream_position(&mut self) -> io::Result<u64> {
        self.0.stream_position()
    }

    fn seek_relative(&mut self, offset: i64) -> io::Result<()> {
        self.0.seek_relative(offset)
    }
}

/// A decoder as the engine plays it.
pub struct Decoded(Inner);

enum Inner {
    Whole(rodio::Decoder<Media>),
    Trimmed(Trimmed<rodio::Decoder<Media>>),
    Wrapped(Box<dyn Erased>),
}

trait Erased: Source<Item = f32> + Send {}
impl<T: Source<Item = f32> + Send + 'static> Erased for T {}

impl Decoded {
    /// Every sample `decoder` yields.
    pub fn new(decoder: rodio::Decoder<Media>) -> Self {
        Self(Inner::Whole(decoder))
    }

    /// The samples of `decoder` from `skip` on, and only the `take` that follow when given.
    pub fn trimmed(decoder: rodio::Decoder<Media>, skip: Duration, take: Option<Duration>) -> Self {
        Self(Inner::Trimmed(Trimmed::new(decoder, skip, take)))
    }

    /// A provider-specific source wrapper, such as Wisp's seamless speed control.
    pub fn wrapped(source: impl Source<Item = f32> + Send + 'static) -> Self {
        Self(Inner::Wrapped(Box::new(source)))
    }

    pub fn channels(&self) -> u16 {
        match &self.0 {
            Inner::Whole(decoder) => decoder.channels().get(),
            Inner::Trimmed(trimmed) => trimmed.channels().get(),
            Inner::Wrapped(source) => source.channels().get(),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        match &self.0 {
            Inner::Whole(decoder) => decoder.sample_rate().get(),
            Inner::Trimmed(trimmed) => trimmed.sample_rate().get(),
            Inner::Wrapped(source) => source.sample_rate().get(),
        }
    }

    pub fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        match &mut self.0 {
            Inner::Whole(decoder) => decoder.try_seek(position),
            Inner::Trimmed(trimmed) => trimmed.try_seek(position),
            Inner::Wrapped(source) => source.try_seek(position),
        }
    }

    /// Pulls up to `wanted` samples onto the end of `samples`, each multiplied by `gain`.
    pub fn read(&mut self, wanted: usize, gain: f32, samples: Vec<f32>) -> Vec<f32> {
        match &mut self.0 {
            Inner::Whole(decoder) => fill(decoder, wanted, gain, samples),
            Inner::Trimmed(trimmed) => fill(trimmed, wanted, gain, samples),
            Inner::Wrapped(source) => fill(&mut **source, wanted, gain, samples),
        }
    }
}

impl Iterator for Decoded {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.0 {
            Inner::Whole(decoder) => decoder.next(),
            Inner::Trimmed(trimmed) => trimmed.next(),
            Inner::Wrapped(source) => source.next(),
        }
    }
}

impl Source for Decoded {
    fn current_span_len(&self) -> Option<usize> {
        match &self.0 {
            Inner::Whole(decoder) => decoder.current_span_len(),
            Inner::Trimmed(trimmed) => trimmed.current_span_len(),
            Inner::Wrapped(source) => source.current_span_len(),
        }
    }

    fn total_duration(&self) -> Option<Duration> {
        match &self.0 {
            Inner::Whole(decoder) => decoder.total_duration(),
            Inner::Trimmed(trimmed) => trimmed.total_duration(),
            Inner::Wrapped(source) => source.total_duration(),
        }
    }

    fn channels(&self) -> rodio::ChannelCount {
        rodio::ChannelCount::new(Decoded::channels(self)).expect("decoder channel count is nonzero")
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        rodio::SampleRate::new(Decoded::sample_rate(self)).expect("decoder sample rate is nonzero")
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        Decoded::try_seek(self, position)
    }
}

fn fill(
    source: &mut (impl Source<Item = f32> + ?Sized),
    wanted: usize,
    gain: f32,
    mut samples: Vec<f32>,
) -> Vec<f32> {
    for _ in 0..wanted {
        let Some(sample) = source.next() else {
            break;
        };
        samples.push(sample * gain);
    }
    samples
}
