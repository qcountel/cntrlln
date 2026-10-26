//! Names shared by the Controllin release archive and the web installer that unpacks it.

/// The GitHub repository whose latest release carries [`ARCHIVE`].
pub const REPOSITORY: &str = "qcountel/cntrlln";
/// The release asset holding the portable client, its files at the archive root.
pub const ARCHIVE: &str = "Controllin-windows-x64.zip";
/// The client executable that shortcuts start; the build names it `cinnabar.exe`.
pub const CLIENT: &str = "controllin.exe";

/// The always-current download link GitHub resolves to the newest release's [`ARCHIVE`].
pub fn download_url() -> String {
    format!("https://github.com/{REPOSITORY}/releases/latest/download/{ARCHIVE}")
}

/// Archive entries besides [`CLIENT`]: the runtime helpers from
/// `packaging/common/runtime-binaries.txt`, the shader compiler and the resources tree.
pub fn payload() -> Vec<String> {
    include_str!("../../../common/runtime-binaries.txt")
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| *name != "cinnabar")
        .map(|name| format!("{name}.exe"))
        .chain(["dxcompiler.dll".to_owned(), "resources".to_owned()])
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn payload_carries_the_core_and_never_user_data() {
        let payload = super::payload();
        assert!(payload.iter().any(|name| name == "bedrock-core.exe"));
        assert!(payload.iter().any(|name| name == "resources"));
        assert!(
            !payload
                .iter()
                .any(|name| name == "userdata" || name == "cinnabar.exe")
        );
    }
}
