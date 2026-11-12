//! Recording from a camera, through Windows' Media Foundation, which every webcam's driver works
//! with. The camera is opened and read on a thread of its own, which keeps the newest picture for
//! the recording and for the small live picture, so opening a slow camera never holds up the
//! interface.
//!
//! Windows turns whatever the camera sends, compressed or not, into blue, green, red and a spare
//! byte, so every camera reaches the recording the same way the screen does.

use std::{
    iter,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use bareclip_media::media_foundation_turn;
use windows::{
    Win32::{
        Foundation::{E_ACCESSDENIED, E_FAIL},
        Media::MediaFoundation::{
            IMF2DBuffer, IMFActivate, IMFAttributes, IMFMediaSource, IMFMediaType, IMFSample,
            IMFSourceReader, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
            MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
            MF_E_HW_MFT_FAILED_START_STREAMING, MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED,
            MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE,
            MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ALL_STREAMS,
            MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
            MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED, MF_SOURCE_READERF_ENDOFSTREAM,
            MF_SOURCE_READERF_ERROR, MF_VERSION, MFCreateAttributes, MFCreateDeviceSource,
            MFCreateMediaType, MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources,
            MFMediaType_Video, MFSTARTUP_LITE, MFShutdown, MFStartup, MFVideoFormat_MJPG,
            MFVideoFormat_NV12, MFVideoFormat_RGB32, MFVideoFormat_YUY2,
        },
        System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize},
    },
    core::{GUID, Interface, PCWSTR, PWSTR},
};

use crate::{Picture, PictureSource};

/// How long dropping a camera waits for it to let go, so opening it again straight away finds it
/// free. A camera that hangs is left to finish on its own after this.
const LET_GO_WAIT: Duration = Duration::from_secs(2);
/// Frame rates at least this fast count as smooth when picking where a camera starts.
const SMOOTH_FPS: f64 = 24.5;
/// The frame rate a camera starts on when it offers it, being what most webcams are made for.
const USUAL_FPS: f64 = 30.0;
/// The largest picture a camera starts on, as larger ones take far more work to record. Larger
/// sizes can still be chosen.
const LARGEST_START: u32 = 1920 * 1080;

/// A camera Windows knows about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Camera {
    /// What the camera calls itself, with a number added if two share a name.
    pub name: String,
    /// Windows' own name for the device, which opens it.
    pub link: String,
}

/// One size and frame rate a camera can send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CameraMode {
    pub width: u32,
    pub height: u32,
    /// Frames per second as a fraction, as the camera gives it.
    pub rate: (u32, u32),
}

impl CameraMode {
    pub fn frames_per_second(self) -> f64 {
        if self.rate.1 == 0 {
            0.0
        } else {
            f64::from(self.rate.0) / f64::from(self.rate.1)
        }
    }

    /// The steady frame rate a recording of it is written at: the camera's own, to the nearest
    /// whole frame.
    pub fn whole_frame_rate(self) -> u32 {
        (self.frames_per_second().round() as u32).max(1)
    }

    /// Frames per second to two places, which is how modes are told apart. Cameras write the
    /// same rate in different ways, such as 30/1 and 10000000/333333.
    fn hundredths(self) -> u64 {
        (self.frames_per_second() * 100.0).round() as u64
    }

    /// Whether two modes are the same size and frame rate, however the rate is written.
    pub fn same_as(self, other: Self) -> bool {
        (self.width, self.height, self.hundredths())
            == (other.width, other.height, other.hundredths())
    }

    /// Such as `1280 x 720, 30 fps` or `640 x 480, 7.5 fps`.
    pub fn label(self) -> String {
        let fps = self.hundredths() as f64 / 100.0;
        format!("{} x {}, {fps} fps", self.width, self.height)
    }

    fn area(self) -> u32 {
        self.width * self.height
    }
}

/// What a camera is doing.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum CameraState {
    /// Windows is still opening it, which can take a second or two.
    #[default]
    Starting,
    /// Pictures are coming, in `mode`, out of every mode it offers.
    Running {
        modes: Vec<CameraMode>,
        mode: CameraMode,
    },
    /// It can't be used, or stopped, for the reason given.
    Failed(String),
}

#[derive(Default)]
struct Shared {
    /// The newest picture and how many have come so far.
    latest: Mutex<(Option<Picture>, u64)>,
    state: Mutex<CameraState>,
}

/// A look at a camera from anywhere: its newest picture and what it is doing. The camera's
/// thread puts pictures in, and the recording and the live picture take them out.
#[derive(Clone, Default)]
pub struct CameraView(Arc<Shared>);

impl CameraView {
    pub fn new() -> Self {
        Self::default()
    }

    /// The newest picture and how many have arrived so far, if any has.
    pub fn latest(&self) -> Option<(Picture, u64)> {
        let latest = self.0.latest.lock().unwrap_or_else(PoisonError::into_inner);
        latest.0.clone().map(|picture| (picture, latest.1))
    }

    pub fn state(&self) -> CameraState {
        self.0
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Takes a new picture as the newest.
    pub fn show(&self, picture: Picture) {
        let mut latest = self.0.latest.lock().unwrap_or_else(PoisonError::into_inner);
        latest.0 = Some(picture);
        latest.1 += 1;
    }

    pub fn set_state(&self, state: CameraState) {
        *self.0.state.lock().unwrap_or_else(PoisonError::into_inner) = state;
    }
}

impl PictureSource for CameraView {
    fn latest(&self) -> Option<(Picture, u64)> {
        CameraView::latest(self)
    }

    /// The mode's size once running, or the newest picture's before that.
    fn size(&self) -> (u32, u32) {
        match self.state() {
            CameraState::Running { mode, .. } => (mode.width, mode.height),
            _ => self
                .latest()
                .map_or((2, 2), |(picture, _)| (picture.width, picture.height)),
        }
    }
}

/// A camera being read. Dropping it closes the camera.
pub struct CameraCapture {
    view: CameraView,
    stop: Arc<AtomicBool>,
    /// Ends when the camera's thread does.
    finished: mpsc::Receiver<()>,
}

impl CameraCapture {
    /// Starts opening the camera in `wanted`, or a usual mode if it's `None` or not offered.
    /// This returns at once, and the view says when pictures come or why they can't.
    pub fn start(camera: &Camera, wanted: Option<CameraMode>) -> Self {
        let view = CameraView::new();
        let stop = Arc::new(AtomicBool::new(false));
        let (done, finished) = mpsc::channel::<()>();
        let link: Vec<u16> = camera.link.encode_utf16().chain(iter::once(0)).collect();
        let (shown, stopping) = (view.clone(), stop.clone());
        let spawned = thread::Builder::new()
            .name("camera".to_owned())
            .spawn(move || {
                run(&link, wanted, &shown, &stopping);
                drop(done);
            });
        if let Err(error) = spawned {
            view.set_state(CameraState::Failed(format!(
                "The camera could not be started. {error}"
            )));
        }
        Self {
            view,
            stop,
            finished,
        }
    }

    /// A look at the camera that stays useful after the capture is handed to a recording.
    pub fn view(&self) -> CameraView {
        self.view.clone()
    }
}

impl PictureSource for CameraCapture {
    fn latest(&self) -> Option<(Picture, u64)> {
        self.view.latest()
    }

    fn size(&self) -> (u32, u32) {
        PictureSource::size(&self.view)
    }
}

impl Drop for CameraCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Ends with an error as soon as the thread has finished, or after the wait.
        let _ = self.finished.recv_timeout(LET_GO_WAIT);
    }
}

/// The cameras plugged in now, in Windows' order. Never opens one, so no camera's light comes on.
pub fn cameras() -> Vec<Camera> {
    // On a thread of its own, so Windows' setup for this thread can't clash with the window's.
    thread::spawn(|| with_media_foundation(|| unsafe { list_cameras() }).unwrap_or_default())
        .join()
        .unwrap_or_default()
}

/// Gives each camera that shares its name with an earlier one a number, so they can be told apart
/// and remembered.
fn number_repeats(found: Vec<(String, String)>) -> Vec<Camera> {
    let mut cameras: Vec<Camera> = Vec::with_capacity(found.len());
    for (name, link) in found {
        let mut shown = name.clone();
        let mut count = 1;
        while cameras.iter().any(|camera| camera.name == shown) {
            count += 1;
            shown = format!("{name} ({count})");
        }
        cameras.push(Camera { name: shown, link });
    }
    cameras
}

/// Runs `work` with COM and Media Foundation set up on this thread, or gives `None` if Media
/// Foundation won't start.
fn with_media_foundation<T>(work: impl FnOnce() -> T) -> Option<T> {
    // SAFETY: COM is set up for this thread and taken down again only if this set it up. Media
    // Foundation keeps a count, and starting and stopping take turns with everything else.
    unsafe {
        let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let started = {
            let _turn = media_foundation_turn();
            MFStartup(MF_VERSION, MFSTARTUP_LITE).is_ok()
        };
        let result = started.then(work);
        if started {
            let _turn = media_foundation_turn();
            let _ = MFShutdown();
        }
        if com {
            CoUninitialize();
        }
        result
    }
}

/// # Safety
/// COM and Media Foundation must be started on this thread.
unsafe fn list_cameras() -> Vec<Camera> {
    let mut found = Vec::new();
    // SAFETY: Windows hands back an array of its own, each of which is taken over so it is let go
    // of when dropped, and then the array is freed as it asks.
    unsafe {
        let Ok(attributes) = new_attributes() else {
            return Vec::new();
        };
        if attributes
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .is_err()
        {
            return Vec::new();
        }
        let mut array: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0_u32;
        if MFEnumDeviceSources(&attributes, &mut array, &mut count).is_err() || array.is_null() {
            return Vec::new();
        }
        for index in 0..count as usize {
            if let Some(device) = std::ptr::read(array.add(index)) {
                let name = text(&device, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME);
                let link = text(
                    &device,
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                );
                if let Some(link) = link {
                    found.push((name.unwrap_or_else(|| "Camera".to_owned()), link));
                }
            }
        }
        CoTaskMemFree(Some(array.cast_const().cast()));
    }
    number_repeats(found)
}

/// # Safety
/// COM must be started on this thread.
unsafe fn new_attributes() -> windows::core::Result<IMFAttributes> {
    let mut attributes = None;
    // SAFETY: Windows fills in the attributes it was asked to make.
    unsafe { MFCreateAttributes(&mut attributes, 2)? };
    attributes.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))
}

/// # Safety
/// COM must be started on this thread.
unsafe fn text(attributes: &IMFAttributes, key: &GUID) -> Option<String> {
    let mut value = PWSTR::null();
    let mut length = 0_u32;
    // SAFETY: Windows hands back a string of its own, which is read and then freed as it asks.
    unsafe {
        attributes
            .GetAllocatedString(key, &mut value, &mut length)
            .ok()?;
        let read = value.to_string().ok();
        CoTaskMemFree(Some(value.0.cast_const().cast()));
        read
    }
}

/// The camera's thread, from opening to letting go.
fn run(link: &[u16], wanted: Option<CameraMode>, view: &CameraView, stop: &AtomicBool) {
    // SAFETY: Media Foundation is started for all of it, and everything made is dropped inside.
    let read = with_media_foundation(|| unsafe { read_camera(link, wanted, view, stop) });
    let problem = match read {
        None => Some("Windows' camera support could not be started.".to_owned()),
        Some(Err(problem)) => Some(problem),
        Some(Ok(())) => None,
    };
    if let Some(problem) = problem {
        view.set_state(CameraState::Failed(problem));
    }
}

/// What went wrong with a camera, in words.
fn camera_problem(error: &windows::core::Error) -> String {
    let code = error.code();
    if code == E_ACCESSDENIED {
        "Windows isn't letting apps use the camera. Turn on \"Let desktop apps access your camera\" in Settings, Privacy & security, Camera.".to_owned()
    } else if code == MF_E_HW_MFT_FAILED_START_STREAMING
        || code == MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED
    {
        "Another app is using the camera. Close it and choose the camera again.".to_owned()
    } else if code == MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED {
        "The camera stopped. It may have been unplugged.".to_owned()
    } else {
        format!("The camera could not be used. {}", error.message())
    }
}

/// # Safety
/// COM and Media Foundation must be started on this thread.
unsafe fn read_camera(
    link: &[u16],
    wanted: Option<CameraMode>,
    view: &CameraView,
    stop: &AtomicBool,
) -> Result<(), String> {
    // SAFETY: `link` ends in a zero, and the source is shut down whatever happens after it opens.
    unsafe {
        let source = open_source(link).map_err(|error| camera_problem(&error))?;
        let result = reader_options()
            .and_then(|options| MFCreateSourceReaderFromMediaSource(&source, &options))
            .map_err(|error| camera_problem(&error))
            .and_then(|reader| read_from(&reader, wanted, view, stop));
        let _ = source.Shutdown();
        result
    }
}

/// # Safety
/// COM and Media Foundation must be started on this thread, and `link` must end in a zero.
unsafe fn open_source(link: &[u16]) -> windows::core::Result<IMFMediaSource> {
    // SAFETY: as the function says.
    unsafe {
        let attributes = new_attributes()?;
        attributes.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )?;
        attributes.SetString(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
            PCWSTR(link.as_ptr()),
        )?;
        MFCreateDeviceSource(&attributes)
    }
}

/// How well a camera's own format suits recording, lowest first. Uncompressed pictures are
/// sharper and cost nothing to unpack, so they win over the same mode compressed.
fn format_rank(subtype: GUID) -> u8 {
    if subtype == MFVideoFormat_NV12 || subtype == MFVideoFormat_RGB32 {
        0
    } else if subtype == MFVideoFormat_YUY2 {
        1
    } else if subtype == MFVideoFormat_MJPG {
        2
    } else {
        3
    }
}

/// One of the camera's own formats, with where it is in the camera's list.
struct Native {
    media_type: IMFMediaType,
    mode: CameraMode,
    rank: u8,
}

/// Every mode once, biggest first and then fastest.
fn offered_modes(all: impl IntoIterator<Item = CameraMode>) -> Vec<CameraMode> {
    let mut modes: Vec<CameraMode> = Vec::new();
    for mode in all {
        if mode.width > 0 && mode.height > 0 && !modes.iter().any(|kept| kept.same_as(mode)) {
            modes.push(mode);
        }
    }
    modes.sort_by(|a, b| {
        b.area()
            .cmp(&a.area())
            .then(b.width.cmp(&a.width))
            .then(b.hundredths().cmp(&a.hundredths()))
    });
    modes
}

/// The mode to use: `wanted` if the camera offers it, otherwise the largest picture up to 1080p
/// that is smooth, at the rate nearest 30.
fn chosen_mode(modes: &[CameraMode], wanted: Option<CameraMode>) -> Option<CameraMode> {
    if let Some(wanted) = wanted
        && let Some(found) = modes.iter().find(|mode| mode.same_as(wanted))
    {
        return Some(*found);
    }
    let smooth: Vec<CameraMode> = modes
        .iter()
        .copied()
        .filter(|mode| mode.frames_per_second() >= SMOOTH_FPS)
        .collect();
    let pool = if smooth.is_empty() { modes } else { &smooth };
    let capped: Vec<CameraMode> = pool
        .iter()
        .copied()
        .filter(|mode| mode.area() <= LARGEST_START)
        .collect();
    let pool = if capped.is_empty() { pool } else { &capped };
    let largest = pool.iter().map(|mode| mode.area()).max()?;
    pool.iter()
        .copied()
        .filter(|mode| mode.area() == largest)
        .min_by(|a, b| {
            let off = |mode: CameraMode| (mode.frames_per_second() - USUAL_FPS).abs();
            off(*a)
                .total_cmp(&off(*b))
                .then(b.hundredths().cmp(&a.hundredths()))
        })
}

/// # Safety
/// COM and Media Foundation must be started on this thread.
unsafe fn mode_of(media_type: &IMFMediaType) -> Option<CameraMode> {
    // SAFETY: only reads the type's own values.
    unsafe {
        let size = media_type.GetUINT64(&MF_MT_FRAME_SIZE).ok()?;
        let rate = media_type
            .GetUINT64(&MF_MT_FRAME_RATE)
            .unwrap_or(30 << 32 | 1);
        Some(CameraMode {
            width: (size >> 32) as u32,
            height: size as u32,
            rate: ((rate >> 32) as u32, rate as u32),
        })
    }
}

/// Where each row of a picture starts.
#[derive(Clone, Copy, Debug)]
struct Layout {
    width: u32,
    height: u32,
    /// Bytes from one row to the next. Negative when the bottom row comes first in memory.
    stride: i32,
}

impl Layout {
    /// # Safety
    /// COM and Media Foundation must be started on this thread.
    unsafe fn of(media_type: &IMFMediaType) -> Option<Self> {
        // SAFETY: only reads the type's own values.
        unsafe {
            let mode = mode_of(media_type)?;
            let stride = media_type
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map_or(mode.width as i32 * 4, |stride| stride as i32);
            Some(Self {
                width: mode.width,
                height: mode.height,
                stride,
            })
        }
    }
}

/// How the camera is read: Windows may unpack and convert any format into blue, green and red.
///
/// # Safety
/// COM and Media Foundation must be started on this thread.
unsafe fn reader_options() -> windows::core::Result<IMFAttributes> {
    // SAFETY: as the function says.
    unsafe {
        let options = new_attributes()?;
        options.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
        Ok(options)
    }
}

/// Picks the mode, asks for blue, green and red, and then keeps the newest picture in `view`
/// until told to stop. It ends with a problem if the pictures stop coming.
///
/// # Safety
/// COM and Media Foundation must be started on this thread.
unsafe fn read_from(
    reader: &IMFSourceReader,
    wanted: Option<CameraMode>,
    view: &CameraView,
    stop: &AtomicBool,
) -> Result<(), String> {
    let failed = |error: windows::core::Error| camera_problem(&error);
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    // SAFETY: everything here is made and used on this thread, and pictures are copied out of
    // their buffers while the buffers are locked.
    unsafe {
        let natives: Vec<Native> = (0..)
            .map_while(|index| reader.GetNativeMediaType(stream, index).ok())
            .filter_map(|media_type| {
                let mode = mode_of(&media_type)?;
                let rank = format_rank(media_type.GetGUID(&MF_MT_SUBTYPE).ok()?);
                Some(Native {
                    media_type,
                    mode,
                    rank,
                })
            })
            .collect();
        let modes = offered_modes(natives.iter().map(|native| native.mode));
        let mode = chosen_mode(&modes, wanted)
            .ok_or_else(|| "This camera doesn't offer a picture BareClip can read.".to_owned())?;
        let native = natives
            .iter()
            .filter(|native| native.mode.same_as(mode))
            .min_by_key(|native| native.rank)
            .ok_or_else(|| "This camera doesn't offer a picture BareClip can read.".to_owned())?;
        reader
            .SetCurrentMediaType(stream, None, &native.media_type)
            .map_err(failed)?;
        let wanted_out = MFCreateMediaType().map_err(failed)?;
        wanted_out
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(failed)?;
        wanted_out
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
            .map_err(failed)?;
        reader
            .SetCurrentMediaType(stream, None, &wanted_out)
            .map_err(|error| {
                format!(
                    "Windows can't turn this camera's picture into one BareClip can read. {}",
                    error.message()
                )
            })?;
        let mut layout = reader
            .GetCurrentMediaType(stream)
            .ok()
            .and_then(|current| Layout::of(&current))
            .ok_or_else(|| "The camera's picture size could not be read.".to_owned())?;
        let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
        reader.SetStreamSelection(stream, true).map_err(failed)?;
        view.set_state(CameraState::Running { modes, mode });
        let ended = MF_SOURCE_READERF_ERROR.0 as u32 | MF_SOURCE_READERF_ENDOFSTREAM.0 as u32;
        while !stop.load(Ordering::Acquire) {
            let mut flags = 0_u32;
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))
                .map_err(failed)?;
            if flags & ended != 0 {
                return Err("The camera stopped. It may have been unplugged.".to_owned());
            }
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0
                && let Some(changed) = reader
                    .GetCurrentMediaType(stream)
                    .ok()
                    .and_then(|current| Layout::of(&current))
            {
                layout = changed;
            }
            if let Some(sample) = sample
                && let Some(picture) = picture_from(&sample, layout)
            {
                view.show(picture);
            }
        }
        Ok(())
    }
}

/// # Safety
/// COM and Media Foundation must be started on this thread.
unsafe fn picture_from(sample: &IMFSample, layout: Layout) -> Option<Picture> {
    let Layout {
        width,
        height,
        stride,
    } = layout;
    if width == 0 || height == 0 {
        return None;
    }
    // SAFETY: each buffer is read only while it is locked, within the length Windows gives, and
    // unlocked afterwards.
    unsafe {
        let buffer = sample.GetBufferByIndex(0).ok()?;
        let row_bytes = width as usize * 4;
        let bgra = if let Ok(rows) = buffer.cast::<IMF2DBuffer>() {
            let mut first = std::ptr::null_mut();
            let mut pitch = 0_i32;
            rows.Lock2D(&mut first, &mut pitch).ok()?;
            let reach = pitch.unsigned_abs() as usize * (height as usize - 1) + row_bytes;
            let copied = if first.is_null() || (pitch.unsigned_abs() as usize) < row_bytes {
                None
            } else if pitch >= 0 {
                copy_rows(
                    std::slice::from_raw_parts(first, reach),
                    0,
                    pitch as isize,
                    width,
                    height,
                )
            } else {
                // The top row is the last in memory, and each next row comes before it.
                let start = first.offset(pitch as isize * (height as isize - 1));
                copy_rows(
                    std::slice::from_raw_parts(start, reach),
                    reach - row_bytes,
                    pitch as isize,
                    width,
                    height,
                )
            };
            let _ = rows.Unlock2D();
            copied
        } else {
            let mut data = std::ptr::null_mut();
            let mut length = 0_u32;
            buffer.Lock(&mut data, None, Some(&mut length)).ok()?;
            let copied = (!data.is_null()).then(|| {
                let bytes = std::slice::from_raw_parts(data, length as usize);
                // A buffer with a negative stride starts with the bottom row.
                let first = if stride < 0 {
                    stride.unsigned_abs() as usize * (height as usize - 1)
                } else {
                    0
                };
                copy_rows(bytes, first, stride as isize, width, height)
            });
            let _ = buffer.Unlock();
            copied.flatten()
        }?;
        Some(Picture {
            width,
            height,
            bgra,
            shown_at: sample.GetSampleTime().unwrap_or(0),
        })
    }
}

/// Copies `height` rows of `width` pixels out of `bytes`, starting at `first` with each next row
/// `pitch` bytes on (back, if it's negative), so the top row always comes first. The spare fourth
/// byte is made opaque. `None` if a row would run outside `bytes`.
fn copy_rows(bytes: &[u8], first: usize, pitch: isize, width: u32, height: u32) -> Option<Vec<u8>> {
    let row_bytes = width as usize * 4;
    let mut out = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height as isize {
        let start = usize::try_from(first as isize + row * pitch).ok()?;
        out.extend_from_slice(bytes.get(start..start + row_bytes)?);
    }
    for pixel in out.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(width: u32, height: u32, rate: (u32, u32)) -> CameraMode {
        CameraMode {
            width,
            height,
            rate,
        }
    }

    #[test]
    fn a_mode_reads_like_the_camera_box_says() {
        assert_eq!(mode(1280, 720, (30, 1)).label(), "1280 x 720, 30 fps");
        assert_eq!(mode(640, 480, (15, 2)).label(), "640 x 480, 7.5 fps");
        assert_eq!(
            mode(1920, 1080, (30000, 1001)).label(),
            "1920 x 1080, 29.97 fps"
        );
        assert_eq!(mode(1920, 1080, (30000, 1001)).whole_frame_rate(), 30);
        assert_eq!(mode(320, 240, (5, 1)).whole_frame_rate(), 5);
        assert_eq!(mode(320, 240, (0, 0)).whole_frame_rate(), 1);
    }

    #[test]
    fn the_same_rate_written_two_ways_is_one_mode() {
        let modes = offered_modes([
            mode(640, 480, (30, 1)),
            mode(640, 480, (10_000_000, 333_333)),
            mode(1280, 720, (30, 1)),
            mode(640, 480, (15, 1)),
            mode(0, 0, (30, 1)),
        ]);
        assert_eq!(
            modes,
            [
                mode(1280, 720, (30, 1)),
                mode(640, 480, (30, 1)),
                mode(640, 480, (15, 1)),
            ],
            "biggest first, then fastest, and nothing without a size"
        );
    }

    #[test]
    fn a_camera_starts_on_its_largest_smooth_picture_up_to_1080p_at_about_30() {
        let modes = offered_modes([
            mode(3840, 2160, (30, 1)),
            mode(1920, 1080, (60, 1)),
            mode(1920, 1080, (30, 1)),
            mode(1920, 1080, (5, 1)),
            mode(1280, 720, (30, 1)),
        ]);
        assert_eq!(chosen_mode(&modes, None), Some(mode(1920, 1080, (30, 1))));

        let slow_big = offered_modes([mode(1920, 1080, (5, 1)), mode(640, 480, (30, 1))]);
        assert_eq!(
            chosen_mode(&slow_big, None),
            Some(mode(640, 480, (30, 1))),
            "smooth beats big"
        );

        let all_slow = offered_modes([mode(1920, 1080, (5, 1)), mode(1920, 1080, (10, 1))]);
        assert_eq!(
            chosen_mode(&all_slow, None),
            Some(mode(1920, 1080, (10, 1)))
        );

        let only_4k = offered_modes([mode(3840, 2160, (30, 1))]);
        assert_eq!(chosen_mode(&only_4k, None), Some(mode(3840, 2160, (30, 1))));

        assert_eq!(chosen_mode(&[], None), None);
    }

    #[test]
    fn a_remembered_mode_is_used_if_the_camera_still_offers_it() {
        let modes = offered_modes([
            mode(1920, 1080, (30, 1)),
            mode(640, 480, (10_000_000, 333_333)),
        ]);
        assert_eq!(
            chosen_mode(&modes, Some(mode(640, 480, (30, 1)))),
            Some(mode(640, 480, (10_000_000, 333_333))),
            "written another way, it's still the same mode"
        );
        assert_eq!(
            chosen_mode(&modes, Some(mode(800, 600, (30, 1)))),
            Some(mode(1920, 1080, (30, 1))),
            "gone, so the usual start"
        );
    }

    #[test]
    fn uncompressed_formats_are_preferred() {
        assert!(format_rank(MFVideoFormat_NV12) < format_rank(MFVideoFormat_YUY2));
        assert!(format_rank(MFVideoFormat_YUY2) < format_rank(MFVideoFormat_MJPG));
        assert!(format_rank(MFVideoFormat_MJPG) < format_rank(GUID::zeroed()));
    }

    #[test]
    fn two_cameras_with_one_name_are_numbered() {
        let cameras = number_repeats(vec![
            ("USB camera".to_owned(), "a".to_owned()),
            ("Webcam".to_owned(), "b".to_owned()),
            ("USB camera".to_owned(), "c".to_owned()),
        ]);
        let names: Vec<_> = cameras.iter().map(|camera| camera.name.as_str()).collect();
        assert_eq!(names, ["USB camera", "Webcam", "USB camera (2)"]);
        assert_eq!(cameras[2].link, "c");
    }

    /// Two pixels across and two down, with four bytes of padding after each row.
    fn padded_rows() -> Vec<u8> {
        vec![
            1, 2, 3, 0, 4, 5, 6, 0, 99, 99, 99, 99, // top row
            7, 8, 9, 0, 10, 11, 12, 0, 99, 99, 99, 99, // bottom row
        ]
    }

    #[test]
    fn rows_are_copied_without_their_padding_and_made_opaque() {
        let copied = copy_rows(&padded_rows(), 0, 12, 2, 2).expect("they fit");
        assert_eq!(
            copied,
            [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
    }

    #[test]
    fn a_picture_stored_bottom_up_comes_out_top_down() {
        // Memory holds the bottom row first, so the top row starts 12 bytes in.
        let copied = copy_rows(&padded_rows(), 12, -12, 2, 2).expect("they fit");
        assert_eq!(
            copied,
            [7, 8, 9, 255, 10, 11, 12, 255, 1, 2, 3, 255, 4, 5, 6, 255]
        );
    }

    #[test]
    fn rows_that_run_outside_the_buffer_give_nothing() {
        assert_eq!(copy_rows(&padded_rows(), 0, 12, 2, 3), None);
        assert_eq!(copy_rows(&padded_rows(), 0, -12, 2, 2), None);
        assert_eq!(copy_rows(&padded_rows(), 0, 12, 4, 2), None);
    }

    #[test]
    fn a_views_size_is_its_modes_once_running() {
        let view = CameraView::new();
        assert_eq!(view.state(), CameraState::Starting);
        assert_eq!(PictureSource::size(&view), (2, 2));
        view.show(Picture {
            width: 4,
            height: 2,
            bgra: vec![0; 32],
            shown_at: 0,
        });
        assert_eq!(PictureSource::size(&view), (4, 2));
        assert_eq!(view.latest().map(|(_, count)| count), Some(1));
        view.set_state(CameraState::Running {
            modes: vec![mode(640, 480, (30, 1))],
            mode: mode(640, 480, (30, 1)),
        });
        assert_eq!(PictureSource::size(&view), (640, 480));
    }

    #[test]
    fn listing_cameras_never_fails() {
        // Machines without a camera, such as the test servers, list none.
        for camera in cameras() {
            assert!(!camera.name.is_empty());
            assert!(!camera.link.is_empty());
        }
    }

    #[test]
    fn a_camera_that_isnt_there_says_so() {
        let capture = CameraCapture::start(
            &Camera {
                name: "Gone".to_owned(),
                link: r"\\?\usb#vid_0000&pid_0000#nothing#{e5323777-f976-4f5b-9b55-b94699c46e44}\global"
                    .to_owned(),
            },
            None,
        );
        let view = capture.view();
        let started = std::time::Instant::now();
        while view.state() == CameraState::Starting && started.elapsed() < Duration::from_secs(10) {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            matches!(view.state(), CameraState::Failed(_)),
            "{:?}",
            view.state()
        );
        assert!(capture.latest().is_none());
    }

    /// A short video, written by BareClip's own encoder, from one picture given as blue, green,
    /// red and alpha.
    fn video_of(name: &str, (width, height): (u32, u32), bgra: &[u8]) -> std::path::PathBuf {
        use bareclip_media::{BgraConverter, Quality, VideoFormat, VideoSettings, VideoWriter};
        let path =
            std::env::temp_dir().join(format!("bareclip-camera-{name}-{}.mp4", std::process::id()));
        let settings = VideoSettings {
            width,
            height,
            frame_rate: (30, 1),
            bit_rate: 2_000_000,
            sound_tracks: Vec::new(),
            quality: Quality::Balanced,
            format: VideoFormat::H264,
        };
        let mut writer = VideoWriter::create(&path, &settings).expect("the video can be made");
        let picture = BgraConverter::new((width, height), (width, height))
            .and_then(|mut converter| converter.convert(bgra, width as usize * 4))
            .expect("the picture converts");
        for _ in 0..15 {
            writer
                .write_picture(&picture.view())
                .expect("a picture is written");
        }
        writer.finish().expect("the video is finished");
        path
    }

    /// Reads a file through the same route as a camera, apart from opening the device, until it
    /// ends. Gives what the view holds, every state it was seen in, and how it ended.
    fn read_like_a_camera(
        path: &std::path::Path,
    ) -> (CameraView, Vec<CameraState>, Result<(), String>) {
        let view = CameraView::new();
        let stop = AtomicBool::new(false);
        let url: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(iter::once(0))
            .collect();
        let (watcher, finished) = (view.clone(), Arc::new(AtomicBool::new(false)));
        let done = finished.clone();
        let watching = thread::spawn(move || {
            let mut states = Vec::new();
            loop {
                let state = watcher.state();
                if states.last() != Some(&state) {
                    states.push(state);
                }
                if done.load(Ordering::Acquire) {
                    return states;
                }
                thread::sleep(Duration::from_millis(1));
            }
        });
        // SAFETY: Media Foundation is started around it, and the URL ends in a zero.
        let ended = with_media_foundation(|| unsafe {
            reader_options()
                .and_then(|options| {
                    windows::Win32::Media::MediaFoundation::MFCreateSourceReaderFromURL(
                        PCWSTR(url.as_ptr()),
                        &options,
                    )
                })
                .map_err(|error| camera_problem(&error))
                .and_then(|reader| read_from(&reader, None, &view, &stop))
        })
        .expect("Media Foundation starts");
        finished.store(true, Ordering::Release);
        let states = watching.join().expect("the watcher finishes");
        (view, states, ended)
    }

    #[test]
    fn the_cameras_route_reads_every_picture_in_its_colours() {
        // Channels that can't be mistaken for each other.
        let bgra = [40_u8, 120, 200, 255].repeat(320 * 240);
        let path = video_of("colour", (320, 240), &bgra);
        let (view, states, ended) = read_like_a_camera(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(
            ended,
            Err("The camera stopped. It may have been unplugged.".to_owned()),
            "a file ends the way an unplugged camera does"
        );
        assert!(
            states.iter().any(|state| matches!(
                state,
                CameraState::Running { mode, .. } if (mode.width, mode.height) == (320, 240)
            )),
            "{states:?}"
        );
        let (picture, count) = view.latest().expect("pictures came");
        assert!(count >= 10, "{count} of 15 pictures");
        assert_eq!((picture.width, picture.height), (320, 240));
        assert_eq!(picture.bgra.len(), 320 * 240 * 4);
        let middle = &picture.bgra[(120 * 320 + 160) * 4..][..4];
        for (got, wanted) in middle.iter().zip([40_u8, 120, 200, 255]) {
            assert!(got.abs_diff(wanted) <= 6, "{middle:?}");
        }
    }

    #[test]
    fn the_cameras_route_keeps_the_top_of_the_picture_at_the_top() {
        // White on top and black below, so an upside-down picture can't pass.
        let (width, height) = (160_u32, 120_u32);
        let mut bgra = Vec::new();
        for row in 0..height {
            let shade = if row < height / 2 { 255 } else { 0 };
            bgra.extend([shade, shade, shade, 255].repeat(width as usize));
        }
        let path = video_of("upright", (width, height), &bgra);
        let (view, _, _) = read_like_a_camera(&path);
        let _ = std::fs::remove_file(&path);
        let (picture, _) = view.latest().expect("pictures came");
        let at = |row: u32| picture.bgra[((row * width + width / 2) * 4) as usize];
        assert!(at(10) > 200, "the top is white: {}", at(10));
        assert!(
            at(height - 10) < 50,
            "the bottom is black: {}",
            at(height - 10)
        );
    }

    /// Records the first camera plugged in for three seconds, with no sound. Its light comes on.
    #[test]
    #[ignore = "needs a camera plugged in, and turns it on"]
    fn a_real_camera_records_into_a_file() {
        let camera = cameras()
            .into_iter()
            .next()
            .expect("a camera is plugged in");
        println!("Recording {}", camera.name);
        let capture = CameraCapture::start(&camera, None);
        let view = capture.view();
        let started = std::time::Instant::now();
        while view.latest().is_none() && started.elapsed() < Duration::from_secs(10) {
            if let CameraState::Failed(problem) = view.state() {
                panic!("{problem}");
            }
            thread::sleep(Duration::from_millis(20));
        }
        let CameraState::Running { modes, mode } = view.state() else {
            panic!("the camera didn't start: {:?}", view.state());
        };
        println!(
            "Offers {:?}",
            modes.iter().map(|mode| mode.label()).collect::<Vec<_>>()
        );
        println!("Using {}", mode.label());
        let (first, _) = view.latest().expect("a picture came");
        assert_eq!((first.width, first.height), (mode.width, mode.height));
        assert!(
            first
                .bgra
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[..3] != [0, 0, 0]),
            "the picture isn't all black"
        );
        let path = std::env::temp_dir().join(format!("bareclip-camera-{}.mp4", std::process::id()));
        let settings = crate::ScreenSettings {
            frame_rate: mode.whole_frame_rate(),
            ..crate::ScreenSettings::default()
        };
        let recording =
            crate::ScreenRecording::start(&path, Box::new(capture), Vec::new(), settings)
                .expect("the recording starts");
        let counted_from = view.latest().map_or(0, |(_, count)| count);
        thread::sleep(Duration::from_secs(3));
        let counted = view.latest().map_or(0, |(_, count)| count) - counted_from;
        let report = recording.stop().expect("the recording finishes");
        println!(
            "{counted} pictures came in 3 s, recorded {:.2} s with {}",
            report.seconds, report.encoder
        );
        assert!(
            f64::from(counted as u32) > 3.0 * mode.frames_per_second() * 0.5,
            "at least half the frame rate arrived"
        );
        let input = ffmpeg_next::format::input(&path).expect("the file opens");
        let stream = input
            .streams()
            .best(ffmpeg_next::media::Type::Video)
            .expect("it has a picture");
        let decoder = ffmpeg_next::codec::context::Context::from_parameters(stream.parameters())
            .and_then(|context| context.decoder().video())
            .expect("its picture can be decoded");
        assert_eq!(
            (decoder.width(), decoder.height()),
            (mode.width & !1, mode.height & !1)
        );
        drop(input);
        let _ = std::fs::remove_file(&path);
    }
}
