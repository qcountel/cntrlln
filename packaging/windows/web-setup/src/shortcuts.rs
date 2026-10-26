//! Desktop and Start menu shortcuts through the shell's link object.

use crate::install::{Result, Shortcuts};
use launcher::PRODUCT_NAME;
use std::path::{Path, PathBuf};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, IPersistFile,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    ShellLink,
};
use windows::core::{GUID, HSTRING, Interface};

/// Points the chosen shortcuts at `client`, replacing earlier ones.
pub(crate) fn create(client: &Path, directory: &Path, shortcuts: Shortcuts) -> Result<()> {
    let folders = [
        (shortcuts.desktop, FOLDERID_Desktop),
        (shortcuts.start_menu, FOLDERID_Programs),
    ];
    // SAFETY: COM is initialized on this thread before any interface is created, and the
    // shell link only reads the strings it is given during each call.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(client))?;
        link.SetWorkingDirectory(&HSTRING::from(directory))?;
        link.SetIconLocation(&HSTRING::from(client), 0)?;
        link.SetDescription(&HSTRING::from(PRODUCT_NAME))?;
        let file: IPersistFile = link.cast()?;
        for (_, folder) in folders.iter().filter(|(wanted, _)| *wanted) {
            let path = known_folder(folder)?.join(format!("{PRODUCT_NAME}.lnk"));
            file.Save(&HSTRING::from(path.as_path()), true)?;
        }
    }
    Ok(())
}

fn known_folder(id: &GUID) -> Result<PathBuf> {
    // SAFETY: the returned buffer is copied before it is freed with the COM allocator.
    unsafe {
        let raw = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None)?;
        let path = raw.to_string();
        CoTaskMemFree(Some(raw.0.cast()));
        Ok(PathBuf::from(path?))
    }
}
