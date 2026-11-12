//! Windows' Media Foundation, which FFmpeg's `_mf` encoders start as they open and shut down as
//! they close, or when they fail to open.
//!
//! If one shuts it down as another opens, each waits for a lock the other holds and both freeze
//! for good. So only one at a time may open or close. Encoding itself isn't affected.

use std::{
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
    sync::{Mutex, MutexGuard, PoisonError},
};

use ffmpeg_next::{self as ffmpeg, Dictionary, encoder};

static CHANGING: Mutex<()> = Mutex::new(());

fn is_media_foundation(name: &str) -> bool {
    name.ends_with("_mf")
}

fn wait_for_turn(media_foundation: bool) -> Option<MutexGuard<'static, ()>> {
    media_foundation.then(take_turn)
}

/// Waits until nothing else is starting or shutting down Media Foundation. Anything else that
/// starts or shuts it down, such as a camera, holds this while it does.
pub fn take_turn() -> MutexGuard<'static, ()> {
    CHANGING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An encoder that takes its turn to close when it is dropped.
pub(crate) struct OpenEncoder {
    encoder: ManuallyDrop<encoder::Video>,
    media_foundation: bool,
}

impl OpenEncoder {
    pub(crate) fn open(
        unopened: encoder::video::Video,
        name: &str,
        options: Dictionary<'_>,
    ) -> Result<Self, ffmpeg::Error> {
        let media_foundation = is_media_foundation(name);
        let _turn = wait_for_turn(media_foundation);
        Ok(Self {
            encoder: ManuallyDrop::new(unopened.open_with(options)?),
            media_foundation,
        })
    }
}

impl Deref for OpenEncoder {
    type Target = encoder::Video;

    fn deref(&self) -> &Self::Target {
        &self.encoder
    }
}

impl DerefMut for OpenEncoder {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.encoder
    }
}

impl Drop for OpenEncoder {
    fn drop(&mut self) {
        let _turn = wait_for_turn(self.media_foundation);
        // SAFETY: this is the only place it is dropped, and nothing can use it afterwards.
        unsafe { ManuallyDrop::drop(&mut self.encoder) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_windows_own_encoders_take_turns() {
        assert!(is_media_foundation("h264_mf"));
        assert!(is_media_foundation("hevc_mf"));
        assert!(!is_media_foundation("h264_nvenc"));
        assert!(!is_media_foundation("libopenh264"));
    }

    #[test]
    fn other_encoders_never_wait() {
        let _someone_else = CHANGING.lock().unwrap_or_else(PoisonError::into_inner);

        assert!(wait_for_turn(false).is_none());
    }
}
