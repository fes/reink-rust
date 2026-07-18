//! Narrow Windows FFI boundary for present USBPRINT interfaces and bounded I/O.
//!
//! SetupAPI enumeration is authoritative for interface discovery. Direct
//! `ReadFile`/`WriteFile` use is level-C observed behavior; Microsoft USBPRINT
//! documentation does not explicitly define this data plane.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::time::Duration;

use reink_platform::{ByteTransport, TransportError, TransportErrorKind};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_ALLCLASSES, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA, SPDRP_HARDWAREID,
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW, SetupDiGetDevicePropertyW,
    SetupDiGetDeviceRegistryPropertyW,
};
use windows_sys::Win32::Devices::Properties::{DEVPKEY_Device_ContainerId, DEVPROP_TYPE_GUID};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_DEVICE_NOT_CONNECTED, ERROR_FILE_NOT_FOUND,
    ERROR_GEN_FAILURE, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_DATA, ERROR_IO_PENDING,
    ERROR_NO_MORE_ITEMS, ERROR_NOT_FOUND, ERROR_OPERATION_ABORTED, ERROR_PATH_NOT_FOUND,
    ERROR_SEM_TIMEOUT, ERROR_SHARING_VIOLATION, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResultEx, OVERLAPPED};
use windows_sys::Win32::System::Registry::REG_MULTI_SZ;
use windows_sys::Win32::System::Threading::CreateEventW;
use windows_sys::core::GUID;

use crate::native::{
    NativeCandidateToken, ParsedUsbHardwareId, WindowsNativePrinterCandidate,
    parse_usb_hardware_id_inner,
};

const GUID_DEVINTERFACE_USBPRINT: GUID = GUID {
    data1: 0x28d78fad,
    data2: 0x5a12,
    data3: 0x11d1,
    data4: [0xae, 0x5b, 0x00, 0x00, 0xf8, 0x03, 0xa8, 0xc2],
};
const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(5);
const CANCEL_CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);

/// Lists present classic USBPRINT interfaces without opening them.
///
/// Each opaque path remains inside a redacted process-local token. VID/PID and
/// optional MI are correlated through the interface devnode's container ID and
/// documented USB hardware IDs; the interface path is never parsed.
pub fn list_windows_native_printer_candidates()
-> Result<Vec<WindowsNativePrinterCandidate>, TransportError> {
    let usb_devices = enumerate_usb_device_identities()?;
    let interfaces = DeviceInfoSet::usbprint_interfaces()?;
    let mut candidates = Vec::new();
    let mut index = 0;
    while let Some((device_path, devinfo)) = interfaces.interface_detail(index)? {
        index += 1;
        let container = interfaces.container_id(&devinfo)?.ok_or_else(|| {
            enumeration_error(
                "a USBPRINT interface has no container ID; reconnect the printer or update its installed driver",
            )
        })?;
        let related = usb_devices.get(&GuidKey::from(container)).ok_or_else(|| {
            enumeration_error(
                "a USBPRINT interface could not be correlated to a present USB devnode by container ID",
            )
        })?;
        let identity = correlate_hardware_ids(related).ok_or_else(|| {
            enumeration_error(
                "a USBPRINT interface has no unambiguous related USB VID/PID hardware ID",
            )
        })?;
        candidates.push(WindowsNativePrinterCandidate {
            vendor_id: identity.vendor_id,
            product_id: identity.product_id,
            interface_number: identity.interface_number,
            token: Arc::new(NativeCandidateToken { device_path }),
        });
    }
    Ok(candidates)
}

fn enumeration_error(message: &'static str) -> TransportError {
    TransportError::new(
        TransportErrorKind::DeviceUnavailable,
        "enumerate Windows USBPRINT interfaces",
        message,
    )
}

fn correlate_hardware_ids(ids: &[ParsedUsbHardwareId]) -> Option<ParsedUsbHardwareId> {
    let by_device = ids.iter().fold(
        BTreeMap::<(u16, u16), BTreeSet<Option<u8>>>::new(),
        |mut devices, id| {
            devices
                .entry((id.vendor_id, id.product_id))
                .or_default()
                .insert(id.interface_number);
            devices
        },
    );
    if by_device.len() != 1 {
        return None;
    }
    let (&(vendor_id, product_id), interfaces) =
        by_device.first_key_value().expect("length checked above");
    let explicit = interfaces
        .iter()
        .flatten()
        .copied()
        .collect::<BTreeSet<_>>();
    let interface_number = match explicit.len() {
        0 => None,
        1 => explicit.first().copied(),
        // A multifunction USB container can legitimately contain several MI
        // hardware IDs. The USBPRINT interface token remains unambiguous, but
        // attributing any one MI would be a guess, so omit it.
        _ => None,
    };
    Some(ParsedUsbHardwareId {
        vendor_id,
        product_id,
        interface_number,
    })
}

fn enumerate_usb_device_identities()
-> Result<BTreeMap<GuidKey, Vec<ParsedUsbHardwareId>>, TransportError> {
    let devices = DeviceInfoSet::all_present_devices()?;
    let mut result = BTreeMap::<GuidKey, Vec<ParsedUsbHardwareId>>::new();
    let mut index = 0;
    while let Some(devinfo) = devices.device_info(index)? {
        index += 1;
        let Some(container) = devices.container_id(&devinfo)? else {
            continue;
        };
        let ids = devices
            .hardware_ids(&devinfo)?
            .into_iter()
            .filter_map(|id| parse_usb_hardware_id_inner(&id))
            .collect::<Vec<_>>();
        if !ids.is_empty() {
            result
                .entry(GuidKey::from(container))
                .or_default()
                .extend(ids);
        }
    }
    Ok(result)
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct GuidKey([u8; 16]);

impl From<GUID> for GuidKey {
    fn from(value: GUID) -> Self {
        let mut bytes = [0; 16];
        bytes[..4].copy_from_slice(&value.data1.to_ne_bytes());
        bytes[4..6].copy_from_slice(&value.data2.to_ne_bytes());
        bytes[6..8].copy_from_slice(&value.data3.to_ne_bytes());
        bytes[8..].copy_from_slice(&value.data4);
        Self(bytes)
    }
}

struct DeviceInfoSet(HDEVINFO);

impl DeviceInfoSet {
    fn usbprint_interfaces() -> Result<Self, TransportError> {
        // SAFETY: the GUID pointer is valid for the call and no optional string
        // or parent window is supplied. The returned list is owned by `Self`.
        let handle = unsafe {
            SetupDiGetClassDevsW(
                &GUID_DEVINTERFACE_USBPRINT,
                null(),
                null_mut(),
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )
        };
        Self::from_handle(handle, "enumerate Windows USBPRINT interfaces")
    }

    fn all_present_devices() -> Result<Self, TransportError> {
        // SAFETY: null class/enumerator pointers with DIGCF_ALLCLASSES are the
        // documented way to request all present devnodes.
        let handle = unsafe {
            SetupDiGetClassDevsW(null(), null(), null_mut(), DIGCF_PRESENT | DIGCF_ALLCLASSES)
        };
        Self::from_handle(handle, "enumerate present Windows devices")
    }

    fn from_handle(handle: HDEVINFO, operation: &'static str) -> Result<Self, TransportError> {
        if handle == INVALID_HANDLE_VALUE as HDEVINFO {
            Err(last_transport_error(operation))
        } else {
            Ok(Self(handle))
        }
    }

    fn interface_detail(
        &self,
        index: u32,
    ) -> Result<Option<(Vec<u16>, SP_DEVINFO_DATA)>, TransportError> {
        // SAFETY: zero is a valid initial representation for this C POD and its
        // required size field is set before SetupAPI observes it.
        let mut interface: SP_DEVICE_INTERFACE_DATA = unsafe { zeroed() };
        interface.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        // SAFETY: `self` and `interface` remain valid for the duration of the
        // call; the interface class pointer is valid.
        let found = unsafe {
            SetupDiEnumDeviceInterfaces(
                self.0,
                null(),
                &GUID_DEVINTERFACE_USBPRINT,
                index,
                &mut interface,
            )
        };
        if found == 0 {
            // SAFETY: GetLastError has no pointer or ownership requirements.
            return match unsafe { GetLastError() } {
                ERROR_NO_MORE_ITEMS => Ok(None),
                _ => Err(last_transport_error(
                    "enumerate Windows USBPRINT interfaces",
                )),
            };
        }

        let mut required = 0;
        // SAFETY: the first documented detail call intentionally supplies no
        // output buffer and asks only for its required byte count.
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                self.0,
                &interface,
                null_mut(),
                0,
                &mut required,
                null_mut(),
            );
        }
        // SAFETY: GetLastError has no pointer or ownership requirements.
        if required == 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
            return Err(last_transport_error(
                "size Windows USBPRINT interface detail",
            ));
        }
        let storage_words = (required as usize).div_ceil(size_of::<usize>());
        let mut storage = vec![0usize; storage_words];
        let storage_size = storage.len() * size_of::<usize>();
        let detail = storage
            .as_mut_ptr()
            .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        // SAFETY: `detail` points into an aligned-enough Vec allocation with
        // `required` bytes, and the C structure's variable path follows cbSize.
        unsafe {
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        }
        // SAFETY: zero is valid C POD initialization; cbSize is set before use.
        let mut devinfo: SP_DEVINFO_DATA = unsafe { zeroed() };
        devinfo.cbSize = size_of::<SP_DEVINFO_DATA>() as u32;
        // SAFETY: all pointers reference live buffers of their declared sizes.
        let succeeded = unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                self.0,
                &interface,
                detail,
                required,
                null_mut(),
                &mut devinfo,
            )
        };
        if succeeded == 0 {
            return Err(last_transport_error(
                "read Windows USBPRINT interface detail",
            ));
        }
        // SAFETY: SetupAPI initialized DevicePath as a terminated UTF-16 string
        // inside `storage`; the scan is bounded to the remaining allocation.
        let path = unsafe {
            let start = std::ptr::addr_of!((*detail).DevicePath).cast::<u16>();
            let offset = start
                .cast::<u8>()
                .offset_from(storage.as_ptr().cast::<u8>()) as usize;
            let capacity = (storage_size - offset) / size_of::<u16>();
            let raw = std::slice::from_raw_parts(start, capacity);
            let end = raw.iter().position(|&unit| unit == 0).ok_or_else(|| {
                TransportError::new(
                    TransportErrorKind::Io,
                    "read Windows USBPRINT interface detail",
                    "SetupAPI returned an unterminated interface path",
                )
            })?;
            raw[..=end].to_vec()
        };
        Ok(Some((path, devinfo)))
    }

    fn device_info(&self, index: u32) -> Result<Option<SP_DEVINFO_DATA>, TransportError> {
        // SAFETY: zero is valid C POD initialization; cbSize is set before use.
        let mut devinfo: SP_DEVINFO_DATA = unsafe { zeroed() };
        devinfo.cbSize = size_of::<SP_DEVINFO_DATA>() as u32;
        // SAFETY: `devinfo` is a valid writable structure for this call.
        if unsafe { SetupDiEnumDeviceInfo(self.0, index, &mut devinfo) } != 0 {
            return Ok(Some(devinfo));
        }
        // SAFETY: GetLastError has no pointer or ownership requirements.
        match unsafe { GetLastError() } {
            ERROR_NO_MORE_ITEMS => Ok(None),
            _ => Err(last_transport_error("enumerate present Windows devices")),
        }
    }

    fn container_id(&self, devinfo: &SP_DEVINFO_DATA) -> Result<Option<GUID>, TransportError> {
        let mut property_type = 0;
        let mut required = 0;
        // SAFETY: the first property call intentionally requests only its size.
        unsafe {
            SetupDiGetDevicePropertyW(
                self.0,
                devinfo,
                &DEVPKEY_Device_ContainerId,
                &mut property_type,
                null_mut(),
                0,
                &mut required,
                0,
            );
        }
        // SAFETY: GetLastError has no pointer or ownership requirements.
        let error = unsafe { GetLastError() };
        if required == 0 {
            return if matches!(error, ERROR_NOT_FOUND | ERROR_INVALID_DATA) {
                Ok(None)
            } else {
                Err(last_transport_error("query Windows device container ID"))
            };
        }
        if error != ERROR_INSUFFICIENT_BUFFER || required as usize != size_of::<GUID>() {
            return Err(TransportError::new(
                TransportErrorKind::Io,
                "query Windows device container ID",
                "SetupAPI returned an invalid container-ID property size",
            ));
        }
        // SAFETY: zero is a valid GUID representation.
        let mut container: GUID = unsafe { zeroed() };
        // SAFETY: the output buffer is exactly one GUID and remains live.
        if unsafe {
            SetupDiGetDevicePropertyW(
                self.0,
                devinfo,
                &DEVPKEY_Device_ContainerId,
                &mut property_type,
                (&mut container as *mut GUID).cast::<u8>(),
                size_of::<GUID>() as u32,
                null_mut(),
                0,
            )
        } == 0
        {
            return Err(last_transport_error("read Windows device container ID"));
        }
        if property_type != DEVPROP_TYPE_GUID {
            return Err(TransportError::new(
                TransportErrorKind::Io,
                "read Windows device container ID",
                "SetupAPI returned an unexpected container-ID property type",
            ));
        }
        Ok(Some(container))
    }

    fn hardware_ids(&self, devinfo: &SP_DEVINFO_DATA) -> Result<Vec<String>, TransportError> {
        let mut data_type = 0;
        let mut required = 0;
        // SAFETY: the first registry-property call intentionally requests size.
        unsafe {
            SetupDiGetDeviceRegistryPropertyW(
                self.0,
                devinfo,
                SPDRP_HARDWAREID,
                &mut data_type,
                null_mut(),
                0,
                &mut required,
            );
        }
        // SAFETY: GetLastError has no pointer or ownership requirements.
        let error = unsafe { GetLastError() };
        if required == 0 {
            return if matches!(error, ERROR_NOT_FOUND | ERROR_INVALID_DATA) {
                Ok(Vec::new())
            } else {
                Err(last_transport_error("query Windows USB hardware IDs"))
            };
        }
        if error != ERROR_INSUFFICIENT_BUFFER {
            return Err(last_transport_error("query Windows USB hardware IDs"));
        }
        let mut units = vec![0u16; (required as usize).div_ceil(size_of::<u16>())];
        // SAFETY: `units` is writable for at least the declared byte size.
        if unsafe {
            SetupDiGetDeviceRegistryPropertyW(
                self.0,
                devinfo,
                SPDRP_HARDWAREID,
                &mut data_type,
                units.as_mut_ptr().cast::<u8>(),
                required,
                null_mut(),
            )
        } == 0
        {
            return Err(last_transport_error("read Windows USB hardware IDs"));
        }
        if data_type != REG_MULTI_SZ {
            return Err(TransportError::new(
                TransportErrorKind::Io,
                "read Windows USB hardware IDs",
                "SetupAPI returned an unexpected hardware-ID property type",
            ));
        }
        if !(required as usize).is_multiple_of(size_of::<u16>()) {
            return Err(TransportError::new(
                TransportErrorKind::Io,
                "read Windows USB hardware IDs",
                "SetupAPI returned malformed UTF-16 hardware IDs",
            ));
        }
        units.truncate(required as usize / size_of::<u16>());
        Ok(split_multi_sz(&units))
    }
}

impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        // SAFETY: this handle is owned by `Self` and is destroyed exactly once.
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

fn split_multi_sz(units: &[u16]) -> Vec<String> {
    units
        .split(|unit| *unit == 0)
        .take_while(|value| !value.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// Bounded read-only-at-the-application-level D4 byte transport over USBPRINT.
pub struct WindowsNativeReadOnlyTransport {
    handle: Option<HANDLE>,
    vendor_id: u16,
    product_id: u16,
    interface_number: Option<u8>,
    timeout: Duration,
}

// SAFETY: the owned Windows handle supports concurrent-thread ownership
// transfer, while all I/O requires `&mut self` and is serialized by Rust.
unsafe impl Send for WindowsNativeReadOnlyTransport {}

impl WindowsNativeReadOnlyTransport {
    pub fn open(candidate: &WindowsNativePrinterCandidate) -> Result<Self, TransportError> {
        // SAFETY: the process-local token owns a valid terminated UTF-16 path
        // returned by SetupAPI; no pointer escapes this call.
        let handle = unsafe {
            CreateFileW(
                candidate.token.device_path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_transport_error(
                "open Windows stock-driver USBPRINT interface",
            ));
        }
        Ok(Self {
            handle: Some(handle),
            vendor_id: candidate.vendor_id,
            product_id: candidate.product_id,
            interface_number: candidate.interface_number,
            timeout: DEFAULT_IO_TIMEOUT,
        })
    }

    pub fn close(&mut self) -> Result<(), TransportError> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        // SAFETY: this transport owns `handle`, has no surviving normal pending
        // operation, and closes it exactly once.
        if unsafe { CloseHandle(handle) } == 0 {
            Err(last_transport_error(
                "close Windows stock-driver USBPRINT interface",
            ))
        } else {
            Ok(())
        }
    }

    fn handle(&self) -> Result<HANDLE, TransportError> {
        self.handle.ok_or_else(|| {
            TransportError::new(
                TransportErrorKind::DeviceUnavailable,
                "use Windows stock-driver USBPRINT interface",
                "the interface handle is closed",
            )
        })
    }

    fn abandon_stalled_io(&mut self, pending: PendingIo) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: closing the owned file handle requests cancellation of all
            // remaining operations. The pending state is intentionally leaked
            // below if the driver did not acknowledge cancellation, keeping the
            // OVERLAPPED and event storage valid without blocking this thread.
            unsafe {
                CloseHandle(handle);
            }
        }
        std::mem::forget(pending);
    }

    fn overlapped_io(
        &mut self,
        operation: &'static str,
        buffer: Vec<u8>,
        write: bool,
    ) -> Result<(usize, Vec<u8>), TransportError> {
        let handle = self.handle()?;
        let mut pending = PendingIo::new(operation, buffer)?;
        let length = u32::try_from(pending.buffer.len()).map_err(|_| {
            TransportError::new(
                TransportErrorKind::Unsupported,
                operation,
                "buffer exceeds the Windows transfer length limit",
            )
        })?;
        // SAFETY: `pending.buffer` is valid for `length` bytes and both it and
        // `pending.overlapped` remain stable until completion. If cancellation
        // does not complete, the whole pending state is intentionally leaked.
        let started = unsafe {
            if write {
                WriteFile(
                    handle,
                    pending.buffer.as_ptr(),
                    length,
                    null_mut(),
                    pending.overlapped.as_mut(),
                )
            } else {
                ReadFile(
                    handle,
                    pending.buffer.as_mut_ptr(),
                    length,
                    null_mut(),
                    pending.overlapped.as_mut(),
                )
            }
        };
        if started == 0 {
            // SAFETY: GetLastError has no pointer or ownership requirements.
            let error = unsafe { GetLastError() };
            if error != ERROR_IO_PENDING {
                return Err(win32_transport_error(operation, error));
            }
        }

        let mut transferred = 0;
        // SAFETY: handle and OVERLAPPED remain live, and transferred is writable.
        if unsafe {
            GetOverlappedResultEx(
                handle,
                pending.overlapped.as_ref(),
                &mut transferred,
                duration_millis(self.timeout),
                0,
            )
        } != 0
        {
            return Ok(pending.into_result(transferred as usize));
        }
        // SAFETY: GetLastError has no pointer or ownership requirements.
        let wait_error = unsafe { GetLastError() };
        if !matches!(wait_error, ERROR_SEM_TIMEOUT | WAIT_TIMEOUT) {
            return Err(win32_transport_error(operation, wait_error));
        }

        // SAFETY: this exact OVERLAPPED belongs to the owned handle.
        let cancelled = unsafe { CancelIoEx(handle, pending.overlapped.as_ref()) };
        if cancelled == 0 {
            // ERROR_NOT_FOUND means completion won the race; final retrieval
            // below still observes and cleans up that completion.
            // SAFETY: GetLastError has no pointer or ownership requirements.
            let cancel_error = unsafe { GetLastError() };
            if cancel_error != ERROR_NOT_FOUND {
                self.abandon_stalled_io(pending);
                return Err(win32_transport_error(operation, cancel_error));
            }
        }
        // SAFETY: all state remains valid for this final bounded cleanup wait.
        let cleaned = unsafe {
            GetOverlappedResultEx(
                handle,
                pending.overlapped.as_ref(),
                &mut transferred,
                duration_millis(CANCEL_CLEANUP_TIMEOUT),
                0,
            )
        };
        if cleaned == 0 {
            // SAFETY: GetLastError has no pointer or ownership requirements.
            let cleanup_error = unsafe { GetLastError() };
            if matches!(cleanup_error, ERROR_SEM_TIMEOUT | WAIT_TIMEOUT) {
                self.abandon_stalled_io(pending);
                return Err(TransportError::new(
                    TransportErrorKind::Timeout,
                    operation,
                    "timed out; cancellation did not complete and the interface was closed",
                ));
            }
            if cleanup_error != ERROR_OPERATION_ABORTED {
                return Err(win32_transport_error(operation, cleanup_error));
            }
        }
        Err(TransportError::new(
            TransportErrorKind::Timeout,
            operation,
            "timed out and was cancelled",
        ))
    }
}

impl ByteTransport for WindowsNativeReadOnlyTransport {
    fn write_all(&mut self, data: &[u8]) -> Result<(), TransportError> {
        let (transferred, _) = self.overlapped_io(
            "write Windows stock-driver USBPRINT interface",
            data.to_vec(),
            true,
        )?;
        if transferred != data.len() {
            return Err(TransportError::new(
                TransportErrorKind::Io,
                "write Windows stock-driver USBPRINT interface",
                format!(
                    "partial write rejected: transferred {transferred} of {} bytes",
                    data.len()
                ),
            ));
        }
        Ok(())
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, TransportError> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let (transferred, data) = self.overlapped_io(
            "read Windows stock-driver USBPRINT interface",
            vec![0; buffer.len()],
            false,
        )?;
        if transferred > buffer.len() {
            return Err(TransportError::new(
                TransportErrorKind::Io,
                "read Windows stock-driver USBPRINT interface",
                "Windows reported more bytes than the requested read buffer",
            ));
        }
        buffer[..transferred].copy_from_slice(&data[..transferred]);
        Ok(transferred)
    }

    fn description(&self) -> String {
        format!(
            "windows-native-usbprint:{:04x}:{:04x}{}:read-only",
            self.vendor_id,
            self.product_id,
            self.interface_number
                .map(|value| format!(":interface={value}"))
                .unwrap_or_default()
        )
    }
}

impl Drop for WindowsNativeReadOnlyTransport {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

struct PendingIo {
    overlapped: Box<OVERLAPPED>,
    event: HANDLE,
    buffer: Vec<u8>,
}

impl PendingIo {
    fn new(operation: &'static str, buffer: Vec<u8>) -> Result<Self, TransportError> {
        // SAFETY: null security/name pointers request a private unnamed event.
        let event = unsafe { CreateEventW(null(), 0, 0, null()) };
        if event.is_null() {
            return Err(last_transport_error(operation));
        }
        // SAFETY: zero is the documented initial state for OVERLAPPED.
        let mut overlapped: Box<OVERLAPPED> = Box::new(unsafe { zeroed() });
        overlapped.hEvent = event;
        Ok(Self {
            overlapped,
            event,
            buffer,
        })
    }

    fn into_result(mut self, transferred: usize) -> (usize, Vec<u8>) {
        (transferred, std::mem::take(&mut self.buffer))
    }
}

impl Drop for PendingIo {
    fn drop(&mut self) {
        // SAFETY: `event` is owned by this pending state and closed once, after
        // normal completion or acknowledged cancellation.
        unsafe {
            CloseHandle(self.event);
        }
    }
}

fn duration_millis(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX - 1)
}

fn last_transport_error(operation: &'static str) -> TransportError {
    // SAFETY: GetLastError has no pointer or ownership requirements.
    win32_transport_error(operation, unsafe { GetLastError() })
}

fn win32_transport_error(operation: &'static str, code: u32) -> TransportError {
    let kind = match code {
        ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION => TransportErrorKind::PermissionDenied,
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_DEVICE_NOT_CONNECTED => {
            TransportErrorKind::DeviceUnavailable
        }
        ERROR_SEM_TIMEOUT | WAIT_TIMEOUT => TransportErrorKind::Timeout,
        _ => TransportErrorKind::Io,
    };
    let message = match code {
        ERROR_ACCESS_DENIED => "access denied by the installed driver or system policy",
        ERROR_SHARING_VIOLATION => "the USBPRINT interface is in exclusive use",
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_DEVICE_NOT_CONNECTED => {
            "the selected USBPRINT interface is no longer available"
        }
        ERROR_SEM_TIMEOUT | WAIT_TIMEOUT => "the bounded operation timed out",
        ERROR_OPERATION_ABORTED => "the operation was cancelled",
        ERROR_GEN_FAILURE => "the USBPRINT driver reported a general I/O failure",
        _ => "Windows reported an I/O failure",
    };
    TransportError::new(kind, operation, format!("{message} (Win32 error {code})"))
}

impl fmt::Debug for WindowsNativeReadOnlyTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowsNativeReadOnlyTransport")
            .field("handle", &self.handle.map(|_| "<open redacted handle>"))
            .field("vendor_id", &format_args!("{:04x}", self.vendor_id))
            .field("product_id", &format_args!("{:04x}", self.product_id))
            .field("interface_number", &self.interface_number)
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use reink_platform::TransportErrorKind;

    use super::{correlate_hardware_ids, split_multi_sz, win32_transport_error};
    use crate::native::ParsedUsbHardwareId;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_DEVICE_NOT_CONNECTED, ERROR_SEM_TIMEOUT,
    };

    #[test]
    fn correlates_one_usb_device_and_prefers_its_explicit_mi() {
        let result = correlate_hardware_ids(&[
            ParsedUsbHardwareId {
                vendor_id: 0x04b8,
                product_id: 0x1234,
                interface_number: None,
            },
            ParsedUsbHardwareId {
                vendor_id: 0x04b8,
                product_id: 0x1234,
                interface_number: Some(0),
            },
        ])
        .unwrap();
        assert_eq!(result.interface_number, Some(0));
        assert!(
            correlate_hardware_ids(&[
                result,
                ParsedUsbHardwareId {
                    product_id: 0x5678,
                    ..result
                },
            ])
            .is_none()
        );
        assert_eq!(
            correlate_hardware_ids(&[
                result,
                ParsedUsbHardwareId {
                    interface_number: Some(1),
                    ..result
                },
            ])
            .unwrap()
            .interface_number,
            None
        );
    }

    #[test]
    fn parses_multisz_without_retaining_trailing_storage() {
        let units = "first\0second\0\0ignored"
            .encode_utf16()
            .collect::<Vec<_>>();
        assert_eq!(split_multi_sz(&units), ["first", "second"]);
    }

    #[test]
    fn maps_native_errors_to_transport_kinds_without_identifiers() {
        assert_eq!(
            win32_transport_error("open", ERROR_ACCESS_DENIED).kind,
            TransportErrorKind::PermissionDenied
        );
        assert_eq!(
            win32_transport_error("read", ERROR_DEVICE_NOT_CONNECTED).kind,
            TransportErrorKind::DeviceUnavailable
        );
        assert_eq!(
            win32_transport_error("read", ERROR_SEM_TIMEOUT).kind,
            TransportErrorKind::Timeout
        );
    }
}
