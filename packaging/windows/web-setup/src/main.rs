//! Downloads the latest Controllin release, installs it for the current user under
//! `%LOCALAPPDATA%\Programs` and adds Desktop and Start menu shortcuts.
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the installer talks to the user through its console window"
)]

use controllin_setup::{CLIENT, download_url};
use launcher::PRODUCT_NAME;
use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() {
    println!("Установка {PRODUCT_NAME}\n");
    match install() {
        Ok(client) => {
            println!("\nГотово! {PRODUCT_NAME} установлен, ярлыки на рабочем столе и в меню Пуск.");
            if ask("Запустить сейчас? [Y/n] ") {
                let directory = client.parent().unwrap_or(Path::new("."));
                if let Err(error) = std::process::Command::new(&client)
                    .current_dir(directory)
                    .spawn()
                {
                    eprintln!("Не удалось запустить: {error}");
                    pause();
                }
            }
        }
        Err(error) => {
            eprintln!("\nОшибка: {error}");
            pause();
        }
    }
}

/// Installs the downloaded release and returns the installed client executable.
fn install() -> Result<PathBuf> {
    let programs = std::env::var_os("LOCALAPPDATA")
        .map(|base| PathBuf::from(base).join("Programs"))
        .ok_or("переменная LOCALAPPDATA не задана")?;
    let target = programs.join(PRODUCT_NAME);
    let client = target.join(CLIENT);
    // A running executable refuses write access, which also guards the folder swap below.
    if client.is_file() && OpenOptions::new().write(true).open(&client).is_err() {
        return Err(format!("{PRODUCT_NAME} сейчас запущен. Закройте игру и запустите установщик снова.").into());
    }
    fs::create_dir_all(&programs)?;

    let archive = std::env::temp_dir().join(controllin_setup::ARCHIVE);
    download(&download_url(), &archive)?;

    println!("Распаковка...");
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

    println!("Создание ярлыков...");
    shortcuts::create(&client, &target)?;
    Ok(client)
}

/// Streams `url` to `path`, printing whole-percent progress when the size is known.
fn download(url: &str, path: &Path) -> Result<()> {
    println!("Скачивание {url}");
    let mut response = reqwest::blocking::Client::builder()
        .timeout(None)
        .build()?
        .get(url)
        .send()?
        .error_for_status()?;
    let total = response.content_length();
    let mut file = File::create(path)?;
    let mut buffer = vec![0; 1 << 16];
    let (mut done, mut shown) = (0u64, None);
    loop {
        let read = response.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])?;
        done += read as u64;
        let megabytes = done / (1 << 20);
        let percent = total.map(|total| done * 100 / total.max(1));
        if shown != Some((megabytes, percent)) {
            shown = Some((megabytes, percent));
            match percent {
                Some(percent) => print!("\r  {percent}% ({megabytes} МБ)   "),
                None => print!("\r  {megabytes} МБ   "),
            }
            let _ = io::stdout().flush();
        }
    }
    println!();
    Ok(())
}

fn remove_dir(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Asks a yes/no question where Enter means yes.
fn ask(question: &str) -> bool {
    print!("{question}");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    let _ = io::stdin().lock().read_line(&mut answer);
    !matches!(answer.trim().to_lowercase().as_str(), "n" | "no" | "н" | "нет")
}

fn pause() {
    print!("Нажмите Enter, чтобы закрыть окно...");
    let _ = io::stdout().flush();
    let _ = io::stdin().lock().read_line(&mut String::new());
}

#[cfg(windows)]
mod shortcuts {
    use super::{PRODUCT_NAME, Result};
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

    /// Points a Desktop and a Start menu shortcut at `client`, replacing earlier ones.
    pub(super) fn create(client: &Path, directory: &Path) -> Result<()> {
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
            for folder in [FOLDERID_Desktop, FOLDERID_Programs] {
                let path = known_folder(&folder)?.join(format!("{PRODUCT_NAME}.lnk"));
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
}

#[cfg(not(windows))]
mod shortcuts {
    use super::Result;
    use std::path::Path;

    pub(super) fn create(_client: &Path, _directory: &Path) -> Result<()> {
        Err("ярлыки поддерживаются только в Windows".into())
    }
}
