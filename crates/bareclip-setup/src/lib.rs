//! The installer's Browse button. Windows Installer's own Browse window only shows the folder
//! clicked on, so this opens Windows' Browse For Folder window instead, whose box on top shows
//! the whole path BareClip will go in, with `BareClip` on the end, as each folder is clicked.
//!
//! The installer calls [`BrowseForFolder`], which reads `INSTALLFOLDER` to start from and, on
//! OK, puts the chosen path in `BROWSED_FOLDER` for the page to take.

/// The folder BareClip installs into, inside whichever folder is chosen.
pub const APP_FOLDER: &str = "BareClip";

/// Whether a path's last folder is already BareClip's own.
fn ends_in_app_folder(path: &str) -> bool {
    path.trim_end_matches('\\')
        .rsplit('\\')
        .next()
        .is_some_and(|last| last.eq_ignore_ascii_case(APP_FOLDER))
}

/// The whole path BareClip goes in when `folder` is chosen: `folder\BareClip`, unless `folder`
/// already ends in it. With no trailing backslash, as the box shows it.
pub fn install_path(folder: &str) -> String {
    let folder = folder.trim().trim_end_matches('\\');
    if ends_in_app_folder(folder) {
        folder.to_owned()
    } else if folder.is_empty() {
        APP_FOLDER.to_owned()
    } else {
        format!("{folder}\\{APP_FOLDER}")
    }
}

/// The folder to show chosen when the window opens: the one BareClip would go inside.
pub fn starting_folder(install_folder: &str) -> String {
    let path = install_folder.trim().trim_end_matches('\\');
    if !ends_in_app_folder(path) {
        return path.to_owned();
    }
    let parent = &path[..path.rfind('\\').unwrap_or(0)];
    // A drive's own folder keeps its backslash, so `D:` doesn't mean the drive's current folder.
    if parent.len() == 2 && parent.ends_with(':') {
        format!("{parent}\\")
    } else {
        parent.to_owned()
    }
}

/// What was typed in the box, as a whole path: as it is if it names a drive or a network
/// share, or else inside the folder chosen in the list.
pub fn typed_path(typed: &str, chosen: &str) -> String {
    let typed = typed.trim();
    let whole = typed.starts_with("\\\\") || typed.as_bytes().get(1) == Some(&b':');
    if whole || chosen.is_empty() {
        typed.to_owned()
    } else {
        format!("{}\\{typed}", chosen.trim_end_matches('\\'))
    }
}

/// The value for Windows Installer: the whole path with BareClip's folder and a backslash.
pub fn property_value(path: &str) -> String {
    format!("{}\\", install_path(path))
}

#[cfg(windows)]
mod browse {
    use super::{install_path, property_value, starting_folder, typed_path};
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, POINT, RECT, WPARAM},
        Graphics::Gdi::MapWindowPoints,
        System::{
            ApplicationInstallationAndServicing::{MSIHANDLE, MsiGetPropertyW, MsiSetPropertyW},
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize},
        },
        UI::{
            Shell::{
                BFFM_INITIALIZED, BFFM_SELCHANGED, BFFM_SETSELECTIONW, BFFM_VALIDATEFAILEDW,
                BIF_EDITBOX, BIF_RETURNONLYFSDIRS, BIF_VALIDATE, BROWSEINFOW, Common::ITEMIDLIST,
                SHBrowseForFolderW, SHGetPathFromIDListW,
            },
            WindowsAndMessaging::{
                GetDlgItem, GetForegroundWindow, GetWindowRect, SWP_NOACTIVATE, SWP_NOZORDER,
                SendMessageW, SetWindowPos, SetWindowTextW,
            },
        },
    };

    /// The box above the list in Windows' Browse For Folder window.
    const PATH_BOX: i32 = 0x3744;
    /// The line of words above the box, which Windows sets a little left of the box and list.
    const HEADING: i32 = 0x3742;
    /// How far inside the box's edge its text starts.
    const BOX_TEXT_INSET: i32 = 3;
    const ERROR_SUCCESS: u32 = 0;
    const ERROR_MORE_DATA: u32 = 234;
    const MAX_PATH: usize = 260;

    /// What the window's callback keeps between calls.
    struct Browsing {
        start: Vec<u16>,
        /// The folder last clicked in the list.
        chosen: String,
        /// A path typed into the box that isn't a folder yet, which is fine: it will be made.
        typed: Option<String>,
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn from_wide(text: &[u16]) -> String {
        let end = text
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(text.len());
        String::from_utf16_lossy(&text[..end])
    }

    /// The folder a shell item is, if it is one on disk.
    ///
    /// # Safety
    /// `item` must be null or a valid item list.
    unsafe fn path_of(item: *const ITEMIDLIST) -> Option<String> {
        if item.is_null() {
            return None;
        }
        let mut path = [0u16; MAX_PATH];
        // SAFETY: the buffer is MAX_PATH long, as this call needs.
        let found = unsafe { SHGetPathFromIDListW(item, path.as_mut_ptr()) } != 0;
        found.then(|| from_wide(&path))
    }

    /// Where a control sits inside the window.
    ///
    /// # Safety
    /// `control` must be a child of `window`.
    unsafe fn place_of(window: HWND, control: HWND) -> RECT {
        let mut place = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: the rectangle is two points, as MapWindowPoints reads it.
        unsafe {
            GetWindowRect(control, &mut place);
            MapWindowPoints(
                std::ptr::null_mut(),
                window,
                (&raw mut place).cast::<POINT>(),
                2,
            );
        }
        place
    }

    /// Moves the heading in so it starts where the path in the box below does, rather than a
    /// few pixels nearer the window's edge than the box and list.
    ///
    /// # Safety
    /// `window` must be the Browse For Folder window.
    unsafe fn line_up_heading(window: HWND) {
        // SAFETY: both are looked up in the window they belong to.
        let (heading, path_box) =
            unsafe { (GetDlgItem(window, HEADING), GetDlgItem(window, PATH_BOX)) };
        if heading.is_null() || path_box.is_null() {
            return;
        }
        // SAFETY: both are children of the window.
        let (text, boxed) = unsafe { (place_of(window, heading), place_of(window, path_box)) };
        let left = boxed.left + BOX_TEXT_INSET;
        if left <= text.left {
            return;
        }
        // SAFETY: moves a child of the window within it, keeping its right edge.
        unsafe {
            SetWindowPos(
                heading,
                std::ptr::null_mut(),
                left,
                text.top,
                text.right - left,
                text.bottom - text.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    unsafe extern "system" fn on_window(
        window: HWND,
        message: u32,
        detail: LPARAM,
        data: LPARAM,
    ) -> i32 {
        // SAFETY: `data` is the `Browsing` that `browse` passed, which outlives the window.
        let browsing = unsafe { &mut *(data as *mut Browsing) };
        match message {
            BFFM_INITIALIZED => {
                // SAFETY: this is the Browse For Folder window, just made.
                unsafe { line_up_heading(window) };
                // SAFETY: the window copies the path during the call.
                unsafe {
                    SendMessageW(
                        window,
                        BFFM_SETSELECTIONW,
                        1 as WPARAM,
                        browsing.start.as_ptr() as LPARAM,
                    );
                }
            }
            BFFM_SELCHANGED => {
                // SAFETY: the window passes the chosen item, valid during the call.
                if let Some(folder) = unsafe { path_of(detail as *const ITEMIDLIST) } {
                    let shown = wide(&install_path(&folder));
                    browsing.chosen = folder;
                    // SAFETY: the box is a child of this window, and copies the text.
                    unsafe { SetWindowTextW(GetDlgItem(window, PATH_BOX), shown.as_ptr()) };
                }
            }
            BFFM_VALIDATEFAILEDW => {
                // SAFETY: the window passes the box's text, valid during the call.
                let typed = unsafe { read_text(detail as *const u16) };
                browsing.typed = Some(typed_path(&typed, &browsing.chosen));
                // Closes the window, taking what was typed.
                return 0;
            }
            _ => {}
        }
        0
    }

    /// # Safety
    /// `text` must point to a nul-terminated UTF-16 string.
    unsafe fn read_text(text: *const u16) -> String {
        if text.is_null() {
            return String::new();
        }
        let mut length = 0;
        // SAFETY: the string is nul-terminated.
        while unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        // SAFETY: `length` units were just read.
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) })
    }

    /// Shows the window starting at `install_folder`, and gives the whole path chosen, or
    /// nothing if it was cancelled.
    fn browse(install_folder: &str) -> Option<String> {
        let mut browsing = Browsing {
            start: wide(&starting_folder(install_folder)),
            chosen: String::new(),
            typed: None,
        };
        let title = wide("Select a folder in the list below, then click OK.");
        let mut name = [0u16; MAX_PATH];
        let info = BROWSEINFOW {
            // The installer's page is in front, having just had Browse pressed.
            // SAFETY: no arguments.
            hwndOwner: unsafe { GetForegroundWindow() },
            pidlRoot: std::ptr::null_mut(),
            pszDisplayName: name.as_mut_ptr(),
            lpszTitle: title.as_ptr(),
            ulFlags: BIF_RETURNONLYFSDIRS | BIF_EDITBOX | BIF_VALIDATE,
            lpfn: Some(on_window),
            lParam: &raw mut browsing as LPARAM,
            iImage: 0,
        };
        // SAFETY: the window needs COM on this thread; undone below if this started it.
        let started = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        // SAFETY: `info` and everything it points to outlive the call.
        let item = unsafe { SHBrowseForFolderW(&info) };
        // SAFETY: the item came from the shell, and is freed once read.
        let picked = unsafe { path_of(item) };
        if !item.is_null() {
            // SAFETY: the shell allocated it with the task allocator.
            unsafe { CoTaskMemFree(item as *const _) };
        }
        if started >= 0 {
            // SAFETY: matches the successful start above.
            unsafe { CoUninitialize() };
        }
        // A path typed into the box wins, because OK was pressed with it there.
        browsing.typed.or(picked)
    }

    fn property(install: MSIHANDLE, name: &str) -> String {
        let name = wide(name);
        let mut value = vec![0u16; MAX_PATH];
        let mut length = value.len() as u32;
        // SAFETY: the buffer is `length` units long, and the call says how many it needs.
        let mut result =
            unsafe { MsiGetPropertyW(install, name.as_ptr(), value.as_mut_ptr(), &mut length) };
        if result == ERROR_MORE_DATA {
            length += 1;
            value = vec![0u16; length as usize];
            // SAFETY: as above, with room for the whole value.
            result =
                unsafe { MsiGetPropertyW(install, name.as_ptr(), value.as_mut_ptr(), &mut length) };
        }
        if result == ERROR_SUCCESS {
            from_wide(&value)
        } else {
            String::new()
        }
    }

    /// The installer's Browse button. Returns success whether or not a folder was chosen, so
    /// cancelling just leaves the page as it was.
    ///
    /// # Safety
    /// Only Windows Installer calls this, with the handle of the install in progress.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn BrowseForFolder(install: MSIHANDLE) -> u32 {
        let current = property(install, "INSTALLFOLDER");
        let value = browse(&current).map_or_else(String::new, |path| property_value(&path));
        let (name, value) = (wide("BROWSED_FOLDER"), wide(&value));
        // SAFETY: both strings are nul-terminated and live through the call.
        unsafe { MsiSetPropertyW(install, name.as_ptr(), value.as_ptr()) };
        ERROR_SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_shows_bareclip_inside_the_chosen_folder() {
        assert_eq!(install_path(r"D:\Apps"), r"D:\Apps\BareClip");
        assert_eq!(install_path(r"D:\Apps\"), r"D:\Apps\BareClip");
        assert_eq!(install_path(r"D:\"), r"D:\BareClip");
        assert_eq!(
            install_path(r"C:\Program Files"),
            r"C:\Program Files\BareClip"
        );
    }

    #[test]
    fn a_folder_already_called_bareclip_gets_no_second_one() {
        assert_eq!(install_path(r"D:\Apps\BareClip"), r"D:\Apps\BareClip");
        assert_eq!(install_path(r"D:\Apps\bareclip\"), r"D:\Apps\bareclip");
        assert_eq!(
            install_path(r"D:\BareClipStuff"),
            r"D:\BareClipStuff\BareClip",
            "only a folder named exactly BareClip counts"
        );
    }

    #[test]
    fn the_window_opens_on_the_folder_bareclip_would_go_inside() {
        assert_eq!(starting_folder(r"D:\Apps\BareClip\"), r"D:\Apps");
        assert_eq!(starting_folder(r"D:\BareClip\"), r"D:\");
        assert_eq!(
            starting_folder(r"C:\Users\me\AppData\Local\Programs\BareClip\"),
            r"C:\Users\me\AppData\Local\Programs"
        );
        assert_eq!(starting_folder(r"E:\Tools\"), r"E:\Tools");
    }

    #[test]
    fn a_typed_path_is_whole_or_inside_the_chosen_folder() {
        assert_eq!(typed_path(r"E:\Tools", r"D:\Apps"), r"E:\Tools");
        assert_eq!(
            typed_path(r"\\server\share\Apps", r"D:\Apps"),
            r"\\server\share\Apps"
        );
        assert_eq!(typed_path("Video", r"D:\Apps"), r"D:\Apps\Video");
        assert_eq!(typed_path(" Video ", r"D:\"), r"D:\Video");
    }

    #[test]
    fn the_installer_gets_the_path_with_a_backslash() {
        assert_eq!(property_value(r"D:\Apps\BareClip"), r"D:\Apps\BareClip\");
        assert_eq!(property_value(r"D:\Apps"), r"D:\Apps\BareClip\");
        assert_eq!(property_value(r"D:\New folder"), r"D:\New folder\BareClip\");
    }

    #[test]
    fn the_installers_browse_button_calls_this_dll() {
        let installer = include_str!("../../../installer/bareclip.wxs");
        assert!(installer.contains(r#"DllEntry="BrowseForFolder""#));
        assert!(installer.contains(r#"Event="DoAction" Value="BrowseForFolder""#));
        assert!(installer.contains(r#"Property="INSTALLFOLDER" Value="[BROWSED_FOLDER]""#));
        assert!(
            !installer.contains("BrowseDlg"),
            "Windows Installer's own Browse window is no longer used"
        );
    }
}
