//! Recording what is on screen or in front of a camera: listing what can be recorded, and
//! capturing it. Both go through the same recorder.

mod app_sound;
mod camera;
mod recorder;
mod screen;
mod sources;

pub use app_sound::{APP_SOUND_RATE, AppSound, window_process};
pub use camera::{Camera, CameraCapture, CameraMode, CameraState, CameraView, cameras};
pub use recorder::{PictureSource, ScreenRecording, ScreenReport, ScreenSettings, SoundFeed};
pub use screen::{Picture, ScreenCapture};
pub use sources::{Offered, Source, screens, windows};
