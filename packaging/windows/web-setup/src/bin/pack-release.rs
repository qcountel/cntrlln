//! Packs a built client folder into the release archive the web installer downloads.
//!
//! Usage: `pack-release <bundle dir> <output dir>`. Only the client, its runtime helpers and
//! `resources` are packed, never user data or local backups beside them.
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool reports to its console"
)]

use controllin_setup::{ARCHIVE, CLIENT, payload};
use std::fs::File;
use std::io;
use std::path::Path;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [bundle, out] = args.as_slice() else {
        eprintln!("usage: pack-release <bundle dir> <output dir>");
        std::process::exit(2);
    };
    if let Err(error) = pack(Path::new(bundle), Path::new(out)) {
        eprintln!("pack-release: {error}");
        std::process::exit(1);
    }
}

fn pack(bundle: &Path, out: &Path) -> io::Result<()> {
    std::fs::create_dir_all(out)?;
    let archive = out.join(ARCHIVE);
    let mut zip = ZipWriter::new(File::create(&archive)?);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .large_file(true);
    for name in std::iter::once(CLIENT.to_owned()).chain(payload()) {
        let path = bundle.join(&name);
        if !path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("missing {}", path.display()),
            ));
        }
        add(&mut zip, &path, &name, options)?;
    }
    zip.finish().map_err(io::Error::other)?;
    println!("{}", archive.display());
    Ok(())
}

/// Adds `path` as `name`, recursing into directories with `/`-separated entry names.
fn add(
    zip: &mut ZipWriter<File>,
    path: &Path,
    name: &str,
    options: SimpleFileOptions,
) -> io::Result<()> {
    if path.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(path)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let child = format!("{name}/{}", entry.file_name().to_string_lossy());
            add(zip, &entry.path(), &child, options)?;
        }
        return Ok(());
    }
    zip.start_file(name, options).map_err(io::Error::other)?;
    io::copy(&mut File::open(path)?, zip)?;
    Ok(())
}
