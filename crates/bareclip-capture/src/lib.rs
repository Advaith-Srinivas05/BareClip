//! Recording what is on screen: listing what can be recorded, and capturing it.

mod recorder;
mod screen;
mod sources;

pub use recorder::{PictureSource, ScreenRecording, ScreenReport, ScreenSettings, SoundFeed};
pub use screen::{Picture, ScreenCapture};
pub use sources::{Offered, Source, screens, windows};
