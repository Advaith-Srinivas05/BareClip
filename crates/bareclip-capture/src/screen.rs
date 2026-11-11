//! Captures a screen or a window through Windows' own capture system, the one the Xbox Game Bar
//! and OBS's newer capture use. Each new picture is copied out of the graphics chip into memory,
//! and only the latest is kept, for a recording to take at its own pace.

use std::sync::{Arc, Mutex};

use windows::{
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession},
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
        SizeInt32,
    },
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
                D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
                ID3D11Texture2D,
            },
            Dxgi::IDXGIDevice,
            Gdi::HMONITOR,
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
        },
    },
    core::{IInspectable, Interface},
};

use crate::Source;

/// One picture of what is being recorded.
#[derive(Clone, Debug)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// Blue, green, red and alpha, a row of `width` pixels at a time with nothing between rows.
    pub bgra: Vec<u8>,
    /// When Windows says it was shown, in 100-nanosecond steps of the performance counter.
    pub shown_at: i64,
}

/// What the capture's thread and the recording share.
#[derive(Default)]
struct Latest {
    picture: Option<Picture>,
    /// How many pictures have arrived, so the recording can tell a new one from a repeat.
    count: u64,
}

/// The graphics chip's side of copying a picture out, used one picture at a time.
struct Copier {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

// SAFETY: the device can be used from any thread, and the context and staging texture are only
// ever used by whoever holds the mutex around this.
unsafe impl Send for Copier {}

/// A capture under way. Dropping it stops it.
pub struct ScreenCapture {
    session: GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    latest: Arc<Mutex<Latest>>,
    size: (u32, u32),
}

impl ScreenCapture {
    /// Starts capturing, with or without the mouse pointer.
    pub fn start(source: Source, show_pointer: bool) -> Result<Self, String> {
        let failed =
            |what: &str, error: windows::core::Error| format!("{what}: {}", error.message());
        let (device, context) = open_device()
            .map_err(|error| failed("The graphics chip could not be used for capturing", error))?;
        let item = item_for(source).map_err(|error| failed("That can't be recorded", error))?;
        let size = item
            .Size()
            .map_err(|error| failed("Its size could not be read", error))?;
        let shared_device = winrt_device(&device).map_err(|error| {
            failed(
                "The graphics chip could not be shared with the capture",
                error,
            )
        })?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &shared_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(|error| failed("The capture could not start", error))?;
        let latest = Arc::new(Mutex::new(Latest::default()));
        let copier = Arc::new(Mutex::new(Copier {
            device,
            context,
            staging: None,
        }));
        let shared = latest.clone();
        let pool_size = Arc::new(Mutex::new(size));
        pool.FrameArrived(
            &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |pool, _| {
                let Some(pool) = pool.as_ref() else {
                    return Ok(());
                };
                let frame = pool.TryGetNextFrame()?;
                let content = frame.ContentSize()?;
                let shown_at = frame.SystemRelativeTime()?.Duration;
                let surface = frame.Surface()?;
                let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
                // SAFETY: the surface is a Direct3D 11 texture, which is what is asked for.
                let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
                let Ok(mut copier) = copier.lock() else {
                    return Ok(());
                };
                if let Some(picture) = copier.copy_out(&texture, content, shown_at)?
                    && let Ok(mut latest) = shared.lock()
                {
                    latest.picture = Some(picture);
                    latest.count += 1;
                }
                // A window that changed size needs pictures of its new size.
                if let Ok(mut current) = pool_size.lock()
                    && (content.Width != current.Width || content.Height != current.Height)
                    && content.Width > 0
                    && content.Height > 0
                {
                    *current = content;
                    let device = winrt_device(&copier.device)?;
                    pool.Recreate(
                        &device,
                        DirectXPixelFormat::B8G8R8A8UIntNormalized,
                        2,
                        content,
                    )?;
                }
                Ok(())
            }),
        )
        .map_err(|error| failed("The capture could not start", error))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|error| failed("The capture could not start", error))?;
        let _ = session.SetIsCursorCaptureEnabled(show_pointer);
        // Windows 11 draws a yellow border around what is captured unless told not to, which
        // older versions can't be told.
        let _ = session.SetIsBorderRequired(false);
        session
            .StartCapture()
            .map_err(|error| failed("The capture could not start", error))?;
        Ok(Self {
            session,
            pool,
            latest,
            size: (size.Width.max(0) as u32, size.Height.max(0) as u32),
        })
    }

    /// The size it started at. A window can change size later, which its pictures say.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The newest picture, and how many have arrived so far, if any has.
    pub fn latest(&self) -> Option<(Picture, u64)> {
        let latest = self.latest.lock().ok()?;
        latest
            .picture
            .clone()
            .map(|picture| (picture, latest.count))
    }

    /// How many pictures have arrived so far.
    pub fn count(&self) -> u64 {
        self.latest.lock().map_or(0, |latest| latest.count)
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

impl Copier {
    /// Copies a texture's content into memory.
    fn copy_out(
        &mut self,
        texture: &ID3D11Texture2D,
        content: SizeInt32,
        shown_at: i64,
    ) -> windows::core::Result<Option<Picture>> {
        let mut description = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: the texture is live and the description is filled in by the call.
        unsafe { texture.GetDesc(&mut description) };
        let width = (content.Width.max(0) as u32).min(description.Width);
        let height = (content.Height.max(0) as u32).min(description.Height);
        if width == 0 || height == 0 {
            return Ok(None);
        }
        let fits = self
            .staging
            .as_ref()
            .is_some_and(|(_, w, h)| *w == description.Width && *h == description.Height);
        if !fits {
            let staging_description = D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
                ..description
            };
            let mut staging = None;
            // SAFETY: the description is complete and `staging` receives the new texture.
            unsafe {
                self.device
                    .CreateTexture2D(&staging_description, None, Some(&mut staging))?;
            }
            self.staging = staging.map(|staging| (staging, description.Width, description.Height));
        }
        let Some((staging, ..)) = &self.staging else {
            return Ok(None);
        };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: both textures are live and the same size and format. The mapped memory is read
        // only up to `height` rows of `width` pixels, inside what the map covers, and unmapped
        // straight after.
        let bgra = unsafe {
            self.context.CopyResource(staging, texture);
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let pitch = mapped.RowPitch as usize;
            let row_bytes = width as usize * 4;
            let start = mapped.pData.cast::<u8>();
            let mut bgra = Vec::with_capacity(row_bytes * height as usize);
            for row in 0..height as usize {
                let line = std::slice::from_raw_parts(start.add(row * pitch), row_bytes);
                bgra.extend_from_slice(line);
            }
            self.context.Unmap(staging, 0);
            bgra
        };
        Ok(Some(Picture {
            width,
            height,
            bgra,
            shown_at,
        }))
    }
}

fn open_device() -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    // SAFETY: the out parameters receive the new device and its context.
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    match (device, context) {
        (Some(device), Some(context)) => Ok((device, context)),
        _ => Err(windows::core::Error::empty()),
    }
}

/// The same graphics chip as the capture system wants it.
fn winrt_device(device: &ID3D11Device) -> windows::core::Result<IDirect3DDevice> {
    let dxgi: IDXGIDevice = device.cast()?;
    // SAFETY: the device is live, and the call wraps it rather than taking it over.
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? };
    inspectable.cast()
}

fn item_for(source: Source) -> windows::core::Result<GraphicsCaptureItem> {
    let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    // SAFETY: the handle came from Windows' own list. One that has gone since is refused.
    unsafe {
        match source {
            Source::Screen(monitor) => {
                interop.CreateForMonitor(HMONITOR(monitor as *mut std::ffi::c_void))
            }
            Source::Window(window) => {
                interop.CreateForWindow(HWND(window as *mut std::ffi::c_void))
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        thread,
        time::{Duration, Instant},
    };
    use windows::{
        Win32::{
            Foundation::{COLORREF, LPARAM, LRESULT, WPARAM},
            Graphics::Gdi::{CreateSolidBrush, HBRUSH, InvalidateRect},
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
                DispatchMessageW, LWA_ALPHA, MSG, PM_REMOVE, PeekMessageW, RegisterClassW,
                SW_SHOWNOACTIVATE, SetLayeredWindowAttributes, ShowWindow, TranslateMessage,
                WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
                WS_POPUP,
            },
        },
        core::w,
    };

    unsafe extern "system" fn plain(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: Windows' own handling, for a window that does nothing of its own.
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    /// A window of one colour, almost entirely see-through so nobody sees it, kept alive until
    /// `stop` is set. Sends back its handle once it shows.
    pub(crate) fn coloured_window(
        [red, green, blue]: [u8; 3],
        stop: Arc<AtomicBool>,
    ) -> std::sync::mpsc::Receiver<isize> {
        let (sender, handle) = std::sync::mpsc::channel();
        thread::spawn(move || {
            // SAFETY: plain Win32 calls making and running a window on this thread, which is the
            // only one that touches it.
            unsafe {
                let instance = GetModuleHandleW(None).expect("the program has a module");
                let brush: HBRUSH = CreateSolidBrush(COLORREF(
                    u32::from(red) | u32::from(green) << 8 | u32::from(blue) << 16,
                ));
                let class = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(plain),
                    hInstance: instance.into(),
                    hbrBackground: brush,
                    lpszClassName: w!("BareClipCaptureTest"),
                    ..Default::default()
                };
                RegisterClassW(&class);
                let window = CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
                    w!("BareClipCaptureTest"),
                    w!("Capture test"),
                    WS_POPUP,
                    0,
                    0,
                    320,
                    180,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .expect("the window opens");
                // Almost entirely see-through, and clicks go through it, so nobody notices it.
                let _ = SetLayeredWindowAttributes(window, COLORREF(0), 1, LWA_ALPHA);
                let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
                let _ = sender.send(window.0 as isize);
                let mut message = MSG::default();
                while !stop.load(Ordering::Acquire) {
                    while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                    // Painting again gives the capture something new to send.
                    let _ = InvalidateRect(Some(window), None, true);
                    thread::sleep(Duration::from_millis(20));
                }
                let _ = DestroyWindow(window);
            }
        });
        handle
    }

    #[test]
    #[ignore = "needs a desktop session, which GitHub's runners may not give"]
    fn a_window_is_captured_in_its_own_colours() {
        let stop = Arc::new(AtomicBool::new(false));
        let window = coloured_window([200, 40, 90], stop.clone())
            .recv_timeout(Duration::from_secs(10))
            .expect("the window shows");
        let capture = ScreenCapture::start(Source::Window(window), false).expect("it starts");
        // The first picture can come before the window has painted itself.
        let middle_of = |picture: &Picture| {
            let at = ((picture.height / 2 * picture.width + picture.width / 2) * 4) as usize;
            [picture.bgra[at + 2], picture.bgra[at + 1], picture.bgra[at]]
        };
        let started = Instant::now();
        let picture = loop {
            if let Some((picture, _)) = capture.latest()
                && middle_of(&picture) != [0, 0, 0]
            {
                break picture;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "no painted picture came, only {:?}",
                capture
                    .latest()
                    .map(|(picture, count)| (middle_of(&picture), count))
            );
            thread::sleep(Duration::from_millis(10));
        };
        stop.store(true, Ordering::Release);
        assert!(
            picture.width >= 300 && picture.height >= 160,
            "{}x{}",
            picture.width,
            picture.height
        );
        assert_eq!(
            picture.bgra.len(),
            (picture.width * picture.height * 4) as usize
        );
        let [r, g, b] = middle_of(&picture);
        assert_eq!([r, g, b], [200, 40, 90]);
        assert!(picture.shown_at > 0);
    }
}
