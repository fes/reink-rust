use std::fs;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

use reink_platform::{
    DeviceDiscovery, DeviceLocation, DiscoveredDevice, DiscoveryError, DiscoveryRequest,
    PrinterIdentityHint,
};

const PRINTER_DIRECTORIES: [&str; 2] = ["/dev", "/dev/usb"];

/// Lists Linux USB-printer device nodes without opening them.
///
/// Returned paths are selection candidates only. Discovery neither communicates
/// with a printer nor changes kernel-driver state.
#[derive(Clone, Debug, Default)]
pub struct LinuxDeviceFileDiscovery;

impl DeviceDiscovery for LinuxDeviceFileDiscovery {
    fn discover(
        &self,
        _request: DiscoveryRequest,
    ) -> Result<Vec<DiscoveredDevice>, DiscoveryError> {
        device_files_from_directories(PRINTER_DIRECTORIES.iter().map(Path::new))
    }
}

fn device_files_from_directories(
    directories: impl IntoIterator<Item = impl AsRef<Path>>,
) -> Result<Vec<DiscoveredDevice>, DiscoveryError> {
    let mut paths = Vec::new();
    for directory in directories {
        let directory = directory.as_ref();
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(discovery_error("list device directory", error)),
        };
        for entry in entries {
            let entry = entry.map_err(|error| discovery_error("read device directory", error))?;
            if !is_printer_device_name(&entry.file_name()) {
                continue;
            }
            let file_type = entry
                .file_type()
                .map_err(|error| discovery_error("inspect device node", error))?;
            if file_type.is_char_device() {
                paths.push(entry.path());
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths.into_iter().map(device_for_path).collect())
}

fn is_printer_device_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_prefix("lp"))
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn device_for_path(path: PathBuf) -> DiscoveredDevice {
    DiscoveredDevice {
        display_name: path.display().to_string(),
        location: DeviceLocation::DeviceFile(path),
        identity_hint: PrinterIdentityHint::default(),
    }
}

fn discovery_error(operation: &'static str, error: io::Error) -> DiscoveryError {
    let message = if error.kind() == io::ErrorKind::PermissionDenied {
        format!("{error}; check access permissions without changing printer drivers")
    } else {
        error.to_string()
    };
    DiscoveryError::Failed { operation, message }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::io;
    use std::path::PathBuf;

    use reink_platform::DeviceLocation;

    use super::{device_for_path, discovery_error, is_printer_device_name};

    #[test]
    fn accepts_only_linux_printer_device_names() {
        assert!(is_printer_device_name(OsStr::new("lp0")));
        assert!(is_printer_device_name(OsStr::new("lp123")));
        assert!(!is_printer_device_name(OsStr::new("lp")));
        assert!(!is_printer_device_name(OsStr::new("lp0.backup")));
        assert!(!is_printer_device_name(OsStr::new("usb")));
    }

    #[test]
    fn produces_an_explicit_device_file_selection() {
        let path = PathBuf::from("/dev/usb/lp0");
        let device = device_for_path(path.clone());

        assert_eq!(device.display_name, "/dev/usb/lp0");
        assert_eq!(device.location, DeviceLocation::DeviceFile(path));
    }

    #[test]
    fn permission_failures_explain_the_non_destructive_remedy() {
        let error = discovery_error(
            "list device directory",
            io::Error::from(io::ErrorKind::PermissionDenied),
        );

        assert!(
            error
                .to_string()
                .contains("without changing printer drivers")
        );
    }
}
