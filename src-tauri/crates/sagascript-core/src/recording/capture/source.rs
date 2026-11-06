//! Where audio comes from (design §4.2).

use super::block::NativeFormat;
use super::sink::BlockSink;
use crate::recording::RecordingError;

/// A capture source: the microphone ([`MicSource`](super::MicSource)), later
/// system audio, or the test double [`FakeSource`](super::FakeSource).
///
/// A source delivers blocks to the [`BlockSink`] it is started with, from a
/// thread of its own (an audio callback), and reports events on the sink's
/// [`SignalHandle`](super::SignalHandle). It never writes a file, takes a
/// session lock, or waits for the writer.
pub trait CaptureSource: Send {
    /// The format of the blocks: the rate as delivered and 1 or 2 channels.
    /// Known before [`start`](Self::start), so the session can size the pool
    /// and describe the track before it opens a device.
    fn format(&self) -> NativeFormat;

    /// The device's name, a local detail of the manifest (`None` if unknown).
    fn device_name(&self) -> Option<String>;

    /// Begin delivering to `sink`. Returns once audio is flowing or the source
    /// has failed to start.
    fn start(&mut self, sink: BlockSink) -> Result<(), RecordingError>;

    /// Stop delivering and drop the sink. After it returns no callback runs.
    /// Calling it twice, or without a start, does nothing.
    fn stop(&mut self);
}
