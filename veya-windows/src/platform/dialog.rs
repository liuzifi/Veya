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

    /// Explorer-style folder picker, available since Windows Vista. It provides
    /// the navigation pane, address bar, search, and new-folder commands instead
    /// of the legacy tree-only dialog. Call on a blocking worker thread.
    pub fn browse_for_folder(title: &str, owner_title: &str) -> Option<PathBuf> {
        use windows::core::w;
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        };
        use windows::Win32::UI::Shell::{
            FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR,
            FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
        };

        let title = wide(title);
        let owner_title = wide(owner_title);
        let owner = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(owner_title.as_ptr())) }.ok();
        if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_err() {
            return None;
        }

        // The closure releases all COM interfaces before CoUninitialize, also
        // on cancellation or an API failure. No fixed MAX_PATH buffer is used.
        let picked = (|| {
            let dialog: IFileOpenDialog =
                unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }.ok()?;
            let options = unsafe { dialog.GetOptions() }.ok()?;
            unsafe {
                dialog
                    .SetOptions(
                        options
                            | FOS_PICKFOLDERS
                            | FOS_FORCEFILESYSTEM
                            | FOS_PATHMUSTEXIST
                            | FOS_NOCHANGEDIR,
                    )
                    .ok()?;
                dialog.SetTitle(PCWSTR(title.as_ptr())).ok()?;
                dialog.SetOkButtonLabel(w!("选择文件夹")).ok()?;
                dialog.Show(owner).ok()?;
            }
            let item = unsafe { dialog.GetResult() }.ok()?;
            let path = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }.ok()?;
            if path.is_null() {
                return None;
            }
            let picked = unsafe { OsString::from_wide(path.as_wide()) };
            unsafe { CoTaskMemFree(Some(path.0.cast())) };
            (!picked.is_empty()).then(|| PathBuf::from(picked))
        })();

        // Both S_OK and S_FALSE increment COM's initialization count.
        unsafe { CoUninitialize() };
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
