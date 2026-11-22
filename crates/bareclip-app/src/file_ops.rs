//! Changes to files on disk: renaming, the Recycle Bin, Windows' Created and Modified dates, and
//! swapping a rewritten copy in for the original.

use std::{
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS,
        ERROR_SHARING_VIOLATION, FILETIME, GetLastError, INVALID_HANDLE_VALUE, SYSTEMTIME,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, GetDiskFreeSpaceExW, MoveFileExW, OPEN_EXISTING,
        REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW, SetFileTime,
    },
    System::Time::{
        FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
        TzSpecificLocalTimeToSystemTime,
    },
};

use crate::components::DateTime;

/// How long a change waits for readers that are still letting go of the file.
const WAIT_FOR_READERS: Duration = Duration::from_secs(2);
/// Names Windows keeps for devices, which no file can have, with or without an extension.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

/// Why Windows won't take a name, in plain words, or `None` if it will.
pub fn name_problem(name: &str) -> Option<String> {
    if name.trim().is_empty() {
        return Some("A name needs at least one letter or number.".to_owned());
    }
    if let Some(bad) = name
        .chars()
        .find(|c| r#"<>:"/\|?*"#.contains(*c) || c.is_control())
    {
        let shown = if bad.is_control() {
            "an invisible character".to_owned()
        } else {
            bad.to_string()
        };
        return Some(format!(
            "Windows doesn't allow {shown} in a name. Leave out < > : \" / \\ | ? *"
        ));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Some("Windows doesn't allow a name to end with a dot or a space.".to_owned());
    }
    let stem = name.split('.').next().unwrap_or(name).trim();
    if RESERVED
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(stem))
    {
        return Some(format!("Windows keeps the name {stem} for itself."));
    }
    None
}

/// Runs a change, trying again while another reader is still letting go of the file.
fn retrying<T>(mut change: impl FnMut() -> Result<T, u32>) -> Result<T, u32> {
    let started = Instant::now();
    loop {
        match change() {
            Err(code)
                if matches!(code, ERROR_SHARING_VIOLATION | ERROR_ACCESS_DENIED)
                    && started.elapsed() < WAIT_FOR_READERS =>
            {
                thread::sleep(Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

/// Renames a file in its own folder, keeping its extension, without replacing anything.
/// Gives the new path.
pub fn rename(path: &Path, new_stem: &str) -> Result<PathBuf, String> {
    let new_stem = new_stem.trim();
    if let Some(problem) = name_problem(new_stem) {
        return Err(problem);
    }
    let name = match path.extension() {
        Some(extension) => format!("{new_stem}.{}", extension.to_string_lossy()),
        None => new_stem.to_owned(),
    };
    let target = path.with_file_name(&name);
    if target == path {
        return Ok(target);
    }
    // Only the case differs, which Windows sees as the same file, so it is renamed in place.
    let same_file = target
        .to_string_lossy()
        .eq_ignore_ascii_case(&path.to_string_lossy());
    if !same_file && target.exists() {
        return Err(format!("There's already a file called {name} here."));
    }
    let (from, to) = (wide(path), wide(&target));
    // SAFETY: both names are nul-terminated and live until the call returns.
    let moved = retrying(|| {
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } != 0 {
            Ok(())
        } else {
            Err(unsafe { GetLastError() })
        }
    });
    match moved {
        Ok(()) => Ok(target),
        Err(ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS) => {
            Err(format!("There's already a file called {name} here."))
        }
        Err(code) => Err(format!(
            "Windows wouldn't rename it. {}",
            std::io::Error::from_raw_os_error(code as i32)
        )),
    }
}

/// Moves a file to the Recycle Bin, quietly, so it can be brought back from there.
pub fn move_to_recycle_bin(path: &Path) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    if !absolute.exists() {
        return Err("The file isn't there any more.".to_owned());
    }
    // The list of files ends in two zeros.
    let from: Vec<u16> = absolute.as_os_str().encode_wide().chain([0, 0]).collect();
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16,
        ..Default::default()
    };
    // SAFETY: the structure and the list it points to live until the call returns.
    let result = unsafe { SHFileOperationW(&mut operation) };
    if result != 0 || operation.fAnyOperationsAborted != 0 {
        return Err(format!("Windows gave error {result}."));
    }
    if absolute.exists() {
        return Err("It is still in use.".to_owned());
    }
    Ok(())
}

fn to_file_time(time: SystemTime) -> FILETIME {
    // File times count tenths of a microsecond from 1601, which is this long before 1970.
    const FROM_1601: u64 = 11_644_473_600 * 10_000_000;
    let since_1970 = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let ticks = FROM_1601 + since_1970.as_nanos() as u64 / 100;
    FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    }
}

fn from_file_time(time: FILETIME) -> SystemTime {
    const FROM_1601: u64 = 11_644_473_600 * 10_000_000;
    let ticks = u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime);
    UNIX_EPOCH + Duration::from_nanos(ticks.saturating_sub(FROM_1601) * 100)
}

fn to_parts(time: SYSTEMTIME) -> DateTime {
    DateTime {
        year: time.wYear,
        month: time.wMonth as u8,
        day: time.wDay as u8,
        hour: time.wHour as u8,
        minute: time.wMinute as u8,
        second: time.wSecond as u8,
    }
}

fn from_parts(parts: DateTime) -> SYSTEMTIME {
    SYSTEMTIME {
        wYear: parts.year,
        wMonth: u16::from(parts.month),
        wDayOfWeek: 0,
        wDay: u16::from(parts.day),
        wHour: u16::from(parts.hour),
        wMinute: u16::from(parts.minute),
        wSecond: u16::from(parts.second),
        wMilliseconds: 0,
    }
}

/// A moment as the date and time in UTC, to the second.
pub fn utc_parts(time: SystemTime) -> DateTime {
    let file_time = to_file_time(time);
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    // SAFETY: both are locals that live until the call returns.
    unsafe { FileTimeToSystemTime(&file_time, &mut utc) };
    to_parts(utc)
}

/// The moment a UTC date and time names.
pub fn from_utc(parts: DateTime) -> Option<SystemTime> {
    let utc = from_parts(parts);
    let mut file_time = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    // SAFETY: both are locals that live until the call returns.
    (unsafe { SystemTimeToFileTime(&utc, &mut file_time) } != 0).then(|| from_file_time(file_time))
}

/// A moment as the date and time on this PC's clock, to the second.
pub fn local_parts(time: SystemTime) -> DateTime {
    let utc = from_parts(utc_parts(time));
    let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    // SAFETY: a null time zone means this PC's, and both are locals.
    unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) };
    to_parts(local)
}

/// The moment a date and time on this PC's clock names.
pub fn from_local(parts: DateTime) -> Option<SystemTime> {
    let local = from_parts(parts);
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    // SAFETY: a null time zone means this PC's, and both are locals.
    if unsafe { TzSpecificLocalTimeToSystemTime(std::ptr::null(), &local, &mut utc) } == 0 {
        return None;
    }
    from_utc(to_parts(utc))
}

/// When Windows says a file was created and last changed.
pub fn file_times(path: &Path) -> std::io::Result<(SystemTime, SystemTime)> {
    let metadata = std::fs::metadata(path)?;
    Ok((metadata.created()?, metadata.modified()?))
}

/// Sets Windows' Created and Modified dates. `None` leaves one as it is.
pub fn set_file_times(
    path: &Path,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
) -> std::io::Result<()> {
    let name = wide(path);
    let opened = retrying(|| {
        // SAFETY: the name is nul-terminated and lives until the call returns.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_WRITE_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            Err(unsafe { GetLastError() })
        } else {
            Ok(handle)
        }
    })
    .map_err(|code| std::io::Error::from_raw_os_error(code as i32))?;
    let created = created.map(to_file_time);
    let modified = modified.map(to_file_time);
    let pointer = |time: &Option<FILETIME>| {
        time.as_ref()
            .map_or(std::ptr::null(), |time| time as *const FILETIME)
    };
    // SAFETY: the handle is open, and each time is null or a local that outlives the call.
    let set = unsafe {
        SetFileTime(
            opened,
            pointer(&created),
            std::ptr::null(),
            pointer(&modified),
        )
    };
    let error = (set == 0).then(std::io::Error::last_os_error);
    unsafe { CloseHandle(opened) };
    error.map_or(Ok(()), Err)
}

/// Puts a rewritten copy in place of the original. Windows keeps the original's Created date
/// and other details, and the copy's name is gone afterwards.
pub fn swap_in(original: &Path, copy: &Path) -> std::io::Result<()> {
    let (original_name, copy_name) = (wide(original), wide(copy));
    retrying(|| {
        // SAFETY: both names are nul-terminated and live until the call returns.
        let swapped = unsafe {
            ReplaceFileW(
                original_name.as_ptr(),
                copy_name.as_ptr(),
                std::ptr::null(),
                REPLACEFILE_IGNORE_MERGE_ERRORS,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if swapped != 0 {
            Ok(())
        } else {
            Err(unsafe { GetLastError() })
        }
    })
    .map_err(|code| std::io::Error::from_raw_os_error(code as i32))
}

/// How much room is free on the drive holding a folder.
pub fn free_space(folder: &Path) -> Option<u64> {
    let name = wide(folder);
    let mut free = 0_u64;
    // SAFETY: the name is nul-terminated, and the count is a local.
    (unsafe {
        GetDiskFreeSpaceExW(
            name.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } != 0)
        .then_some(free)
}

/// The page of Windows' Default apps for BareClip, where one press makes it the player for
/// every type. It is listed for everyone when installed for everyone, and otherwise for the
/// person who installed it.
pub fn default_apps_link() -> &'static str {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let key: Vec<u16> = r"Software\RegisteredApplications"
        .encode_utf16()
        .chain([0])
        .collect();
    let name: Vec<u16> = "BareClip".encode_utf16().chain([0]).collect();
    let mut size = 0_u32;
    // SAFETY: the names are nul-terminated, and asking for the size only reads no data.
    let for_everyone = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    } == 0;
    if for_everyone {
        "ms-settings:defaultapps?registeredAppMachine=BareClip"
    } else {
        "ms-settings:defaultapps?registeredAppUser=BareClip"
    }
}

/// Opens Windows' Default apps page at BareClip.
pub fn open_default_apps() {
    let _ = std::process::Command::new("explorer.exe")
        .arg(default_apps_link())
        .spawn();
}

/// Where a rewritten copy of a file goes while it is made: beside it, so swapping it in is
/// instant, with its own kind's extension so it is written the same way.
pub fn copy_path_for(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    let name = match path.extension() {
        Some(extension) => format!("~{stem}.bareclip-copy.{}", extension.to_string_lossy()),
        None => format!("~{stem}.bareclip-copy"),
    };
    path.with_file_name(name)
}

#[cfg(test)]
mod tests;
