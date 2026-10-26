//! Downloads the release archive, swaps it into the per-user install folder and adds shortcuts.

use controllin_setup::{ARCHIVE, CLIENT, download_url};
use launcher::PRODUCT_NAME;
use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub(crate) type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// Which shortcuts to point at the installed client.
#[derive(Clone, Copy)]
pub(crate) struct Shortcuts {
    pub(crate) desktop: bool,
    pub(crate) start_menu: bool,
}

/// What the installer is doing, for its window.
pub(crate) enum Step {
    /// Received and total bytes; the total is absent when the server does not send one.
    Downloading(u64, Option<u64>),
    Unpacking,
    Shortcuts,
}

/// `%LOCALAPPDATA%\Programs\<product>`, which needs no elevation.
pub(crate) fn target_dir() -> Result<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|base| PathBuf::from(base).join("Programs").join(PRODUCT_NAME))
        .ok_or_else(|| "переменная LOCALAPPDATA не задана".into())
}

/// Installs the latest release and returns the installed client executable.
pub(crate) fn install(shortcuts: Shortcuts, report: &dyn Fn(Step)) -> Result<PathBuf> {
    let target = target_dir()?;
    let programs = target.parent().ok_or("нет родительской папки")?.to_owned();
    let client = target.join(CLIENT);
    // A running executable refuses write access, which also guards the folder swap below.
    if client.is_file() && OpenOptions::new().write(true).open(&client).is_err() {
        return Err(format!(
            "{PRODUCT_NAME} сейчас запущен. Закройте игру и нажмите «Установить» снова."
        )
        .into());
    }
    fs::create_dir_all(&programs)?;

    let archive = std::env::temp_dir().join(ARCHIVE);
    download(&download_url(), &archive, report)?;

    report(Step::Unpacking);
    let staging = programs.join(format!("{PRODUCT_NAME}.new"));
    remove_dir(&staging)?;
    zip::ZipArchive::new(File::open(&archive)?)?.extract(&staging)?;
    let _ = fs::remove_file(&archive);
    if !staging.join(CLIENT).is_file() {
        return Err(format!("в архиве нет {CLIENT}").into());
    }
    let previous = programs.join(format!("{PRODUCT_NAME}.old"));
    remove_dir(&previous)?;
    if target.exists() {
        fs::rename(&target, &previous)?;
    }
    fs::rename(&staging, &target)?;
    remove_dir(&previous)?;

    report(Step::Shortcuts);
    crate::shortcuts::create(&client, &target, shortcuts)?;
    Ok(client)
}

/// Streams `url` to `path`, reporting each received chunk.
fn download(url: &str, path: &Path, report: &dyn Fn(Step)) -> Result<()> {
    let mut response = reqwest::blocking::Client::builder()
        .timeout(None)
        .build()?
        .get(url)
        .send()?
        .error_for_status()?;
    let total = response.content_length();
    let mut file = File::create(path)?;
    let mut buffer = vec![0; 1 << 16];
    let mut done = 0u64;
    report(Step::Downloading(0, total));
    loop {
        let read = response.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])?;
        done += read as u64;
        report(Step::Downloading(done, total));
    }
    Ok(())
}

fn remove_dir(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}
