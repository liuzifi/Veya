//! Native file pickers for database backup and restore, and the local time used to
//! name backup files. The pickers block until the user answers, so callers run them on a
//! worker thread, never on the UI thread.

use std::path::PathBuf;

/// File name suggested for a new backup, e.g. `veya-backup-20261023-153012.db`.
pub fn default_backup_file_name(timestamp: &str) -> String {
    format!("veya-backup-{timestamp}.db")
}

/// Local time as `YYYYMMDD-HHMMSS`.
#[cfg(windows)]
pub fn local_timestamp() -> String {
    native::local_timestamp()
}

#[cfg(not(windows))]
pub fn local_timestamp() -> String {
    "00000000-000000".into()
}

/// Ask where to save a backup. `owner_title` is the title of the window the dialog
/// belongs to. Returns `None` when the user cancels or the dialog cannot open.
#[cfg(windows)]
pub fn pick_save_file(title: &str, default_name: &str, owner_title: &str) -> Option<PathBuf> {
    native::run_dialog(true, title, default_name, owner_title)
}

/// Ask which backup file to restore from. Returns `None` on cancel.
#[cfg(windows)]
pub fn pick_open_file(title: &str, owner_title: &str) -> Option<PathBuf> {
    native::run_dialog(false, title, "", owner_title)
}

/// Ask the user to pick an existing folder (the database storage directory).
/// `owner_title` is the title of the window the dialog belongs to. Returns
/// `None` when the user cancels or the dialog cannot open.
#[cfg(windows)]
pub fn pick_folder(title: &str, owner_title: &str) -> Option<PathBuf> {
    native::browse_for_folder(title, owner_title)
}

#[cfg(not(windows))]
pub fn pick_save_file(_title: &str, _default_name: &str, _owner_title: &str) -> Option<PathBuf> {
    None
}

#[cfg(not(windows))]
pub fn pick_open_file(_title: &str, _owner_title: &str) -> Option<PathBuf> {
    None
}

#[cfg(not(windows))]
pub fn pick_folder(_title: &str, _owner_title: &str) -> Option<PathBuf> {
    None
}

#[cfg(windows)]
mod native {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};

    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::System::SystemInformation::GetLocalTime;
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, GetSaveFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY,
        OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;

    /// Room for long paths; the legacy dialog writes the chosen path into this buffer.
    const PATH_BUFFER: usize = 4096;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Filter pairs are NUL separated and the list ends with a double NUL.
    fn filter() -> Vec<u16> {
        let mut out = Vec::new();
        for part in ["Veya 数据库备份 (*.db)", "*.db", "所有文件 (*.*)", "*.*"] {
            out.extend(part.encode_utf16());
            out.push(0);
        }
        out.push(0);
        out
    }

    pub fn local_timestamp() -> String {
        let now = unsafe { GetLocalTime() };
        format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond
        )
    }

    pub fn run_dialog(
        save: bool,
        title: &str,
        default_name: &str,
        owner_title: &str,
    ) -> Option<PathBuf> {
        let title = wide(title);
        let owner_title = wide(owner_title);
        let extension = wide("db");
        let filter = filter();
        let mut file = vec![0u16; PATH_BUFFER];
        for (slot, unit) in file
            .iter_mut()
            .zip(default_name.encode_utf16().take(PATH_BUFFER - 1))
        {
            *slot = unit;
        }
        let owner = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(owner_title.as_ptr())) }
            .unwrap_or_default();
        let flags = if save {
            OFN_EXPLORER
                | OFN_OVERWRITEPROMPT
                | OFN_PATHMUSTEXIST
                | OFN_NOCHANGEDIR
                | OFN_HIDEREADONLY
        } else {
            OFN_EXPLORER
                | OFN_FILEMUSTEXIST
                | OFN_PATHMUSTEXIST
                | OFN_NOCHANGEDIR
                | OFN_HIDEREADONLY
        };
        let mut dialog = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: owner,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            nFilterIndex: 1,
            lpstrFile: PWSTR(file.as_mut_ptr()),
            nMaxFile: PATH_BUFFER as u32,
            lpstrTitle: PCWSTR(title.as_ptr()),
            Flags: flags,
            lpstrDefExt: PCWSTR(extension.as_ptr()),
            ..Default::default()
        };
        let accepted = unsafe {
            if save {
                GetSaveFileNameW(&mut dialog)
            } else {
                GetOpenFileNameW(&mut dialog)
            }
        };
        if !accepted.as_bool() {
            return None;
        }
        let length = file
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(file.len());
        (length > 0).then(|| PathBuf::from(OsString::from_wide(&file[..length])))
    }

    /// Folder picker backed by `SHBrowseForFolderW`. The legacy dialog is used
    /// because it is available on every supported Windows version (including
    /// Win7 without SP1) and matches the look-and-feel of the file pickers
    /// above. `title` is shown above the tree as the banner text; `owner_title`
    /// locates the owning window so the dialog is properly modal.
    ///
    /// `pidlRoot` is left null so browsing starts at the desktop. The calling
    /// thread runs on a plain OS thread (see `veya-desktop`'s `run_blocking`), so
    /// it initializes a COM apartment itself — `BIF_USENEWUI` requires one. The
    /// returned PIDL is a raw pointer freed with `CoTaskMemFree`; our apartment
    /// (when we created it) is released on every exit path.
    pub fn browse_for_folder(title: &str, owner_title: &str) -> Option<PathBuf> {
        use windows::Win32::System::Com::{
            CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
        };
        use windows::Win32::UI::Shell::{
            SHBrowseForFolderW, SHGetPathFromIDListW, BIF_RETURNONLYFSDIRS,
            BIF_USENEWUI, BROWSEINFOW,
        };

        let title = wide(title);
        let owner_title = wide(owner_title);
        let owner = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(owner_title.as_ptr())) }
            .unwrap_or_default();

        let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

        let info = BROWSEINFOW {
            hwndOwner: owner,
            pidlRoot: std::ptr::null_mut(),
            pszDisplayName: PWSTR::null(),
            lpszTitle: PCWSTR(title.as_ptr()),
            ulFlags: BIF_RETURNONLYFSDIRS | BIF_USENEWUI,
            lpfn: None,
            lParam: windows::Win32::Foundation::LPARAM(0),
            iImage: 0,
        };

        let pidl = unsafe { SHBrowseForFolderW(&info) };
        let picked = if pidl.is_null() {
            None
        } else {
            // `SHGetPathFromIDListW` writes into a fixed 260-unit buffer.
            let mut buffer = [0u16; 260];
            let ok = unsafe { SHGetPathFromIDListW(pidl, &mut buffer) };
            unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };
            if ok.as_bool() {
                let length = buffer
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(buffer.len());
                (length > 0).then(|| PathBuf::from(OsString::from_wide(&buffer[..length])))
            } else {
                None
            }
        };

        // Balance our own successful initialization (S_OK); leave foreign
        // apartments (S_FALSE) and refused ones (RPC_E_CHANGED_MODE) alone.
        if com.is_ok() {
            unsafe { CoUninitialize() };
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_file_names_carry_the_timestamp_and_the_db_extension() {
        assert_eq!(
            default_backup_file_name("20261023-153012"),
            "veya-backup-20261023-153012.db"
        );
    }

    #[test]
    fn timestamp_has_the_documented_shape() {
        let stamp = local_timestamp();
        assert_eq!(stamp.len(), 15);
        assert_eq!(stamp.as_bytes()[8], b'-');
        assert!(stamp
            .bytes()
            .filter(|b| *b != b'-')
            .all(|b| b.is_ascii_digit()));
    }
}
