//! macOS backend: Core Audio process tap + private aggregate device.
//!
//! Signatures verified against the macOS SDK headers (CoreAudio/AudioHardware.h,
//! AudioHardwareTapping.h, CATapDescription.h). The 14.2+ symbols are resolved
//! with `dlsym` so the binary still launches on older systems.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::ptr;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};

use super::{
    macos_version_supported, map_tcc_preflight, parse_macos_version, AppSelector, CaptureTarget,
    ChunkSink, NativeFormat, SystemAudioPermission, SystemAudioStatus, MACOS_MIN_VERSION,
};
use crate::error::DictationError;

type OSStatus = i32;
type AudioObjectID = u32;

const fn fourcc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | s[3] as u32
}

const SYSTEM_OBJECT: AudioObjectID = 1; // kAudioObjectSystemObject
const SCOPE_GLOBAL: u32 = fourcc(b"glob");
const ELEMENT_MAIN: u32 = 0;
// kAudioHardwarePropertyDefaultOutputDevice ('dOut'): where normal media/app audio
// plays. NOT 'sOut' (DefaultSystemOutputDevice), which is only the alert/system-sound
// device (AudioHardware.h:476-478, 610-611). Taps capture app output, so the
// aggregate's clock/sub-device must be the device that audio is actually rendered to.
const PROP_DEFAULT_OUTPUT: u32 = fourcc(b"dOut");
const PROP_PROCESS_LIST: u32 = fourcc(b"prs#");
const PROP_DEVICE_UID: u32 = fourcc(b"uid ");
const PROP_PROCESS_PID: u32 = fourcc(b"ppid");
const PROP_PROCESS_BUNDLE_ID: u32 = fourcc(b"pbid");
const PROP_TAP_FORMAT: u32 = fourcc(b"tfmt");
const PROP_STREAM_CONFIGURATION: u32 = fourcc(b"slay"); // kAudioDevicePropertyStreamConfiguration
const SCOPE_INPUT: u32 = fourcc(b"inpt");
const FLAG_NON_INTERLEAVED: u32 = 1 << 5; // kAudioFormatFlagIsNonInterleaved

#[repr(C)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct StreamDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    number_buffers: u32,
    buffers: [AudioBuffer; 1],
}

type IoProc = extern "C" fn(
    AudioObjectID,
    *const c_void,
    *const AudioBufferList,
    *const c_void,
    *mut AudioBufferList,
    *const c_void,
    *mut c_void,
) -> OSStatus;

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyDataSize(
        id: AudioObjectID,
        addr: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> OSStatus;
    fn AudioObjectGetPropertyData(
        id: AudioObjectID,
        addr: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> OSStatus;
    fn AudioHardwareCreateAggregateDevice(desc: *const c_void, out: *mut AudioObjectID) -> OSStatus;
    fn AudioHardwareDestroyAggregateDevice(id: AudioObjectID) -> OSStatus;
    fn AudioDeviceCreateIOProcID(
        device: AudioObjectID,
        proc_: IoProc,
        client: *mut c_void,
        out: *mut *mut c_void,
    ) -> OSStatus;
    fn AudioDeviceDestroyIOProcID(device: AudioObjectID, id: *mut c_void) -> OSStatus;
    fn AudioDeviceStart(device: AudioObjectID, id: *mut c_void) -> OSStatus;
    fn AudioDeviceStop(device: AudioObjectID, id: *mut c_void) -> OSStatus;
}

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlopen(path: *const c_char, flags: c_int) -> *mut c_void;
    fn sysctlbyname(
        name: *const c_char,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *mut c_void,
        newlen: usize,
    ) -> c_int;
    fn proc_pidpath(pid: c_int, buffer: *mut c_void, size: u32) -> c_int;
}

const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
const RTLD_LAZY: c_int = 1;

type CreateTapFn = unsafe extern "C" fn(*mut AnyObject, *mut AudioObjectID) -> OSStatus;
type DestroyTapFn = unsafe extern "C" fn(AudioObjectID) -> OSStatus;

fn resolve<T>(name: &CStr) -> Option<T> {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
    // SAFETY: dlsym with a valid NUL-terminated name; T is a fn pointer type.
    let p = unsafe { dlsym(RTLD_DEFAULT, name.as_ptr()) };
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
}

fn os_status_message(status: OSStatus) -> String {
    let bytes = (status as u32).to_be_bytes();
    if bytes.iter().all(|b| b.is_ascii_graphic()) {
        format!("OSStatus {status} ('{}')", String::from_utf8_lossy(&bytes))
    } else {
        format!("OSStatus {status}")
    }
}

fn capture_err(context: &str, status: OSStatus) -> DictationError {
    DictationError::AudioCaptureError(format!("{context}: {}", os_status_message(status)))
}

fn os_version() -> Option<(u32, u32)> {
    let mut buf = [0u8; 32];
    let mut len = buf.len();
    // SAFETY: valid buffer and length for a string sysctl.
    let rc = unsafe {
        sysctlbyname(c"kern.osproductversion".as_ptr(), buf.as_mut_ptr().cast(), &mut len, ptr::null_mut(), 0)
    };
    if rc != 0 {
        return None;
    }
    let s = CStr::from_bytes_until_nul(&buf).ok()?.to_str().ok()?;
    parse_macos_version(s)
}

/// Best-effort, non-prompting permission query through the private TCC
/// preflight. Returns `Unknown` if the symbol or its answer is unavailable.
fn tcc_permission() -> SystemAudioPermission {
    type Preflight = unsafe extern "C" fn(*const c_void, *const c_void) -> c_int;
    // SAFETY: dlopen/dlsym with valid names; the function takes (CFStringRef, CFDictionaryRef).
    unsafe {
        let handle = dlopen(c"/System/Library/PrivateFrameworks/TCC.framework/TCC".as_ptr(), RTLD_LAZY);
        if handle.is_null() {
            return SystemAudioPermission::Unknown;
        }
        let sym = dlsym(handle, c"TCCAccessPreflight".as_ptr());
        if sym.is_null() {
            return SystemAudioPermission::Unknown;
        }
        let f: Preflight = std::mem::transmute(sym);
        let service = NSString::from_str("kTCCServiceAudioCapture");
        let raw = f(Retained::as_ptr(&service).cast(), ptr::null());
        tracing::debug!(raw, "TCCAccessPreflight(kTCCServiceAudioCapture) raw result");
        map_tcc_preflight(raw)
    }
}

/// Capability only: OS version and API presence. Never queries permission.
pub(super) fn capability() -> SystemAudioStatus {
    let backend = "coreaudio-process-tap";
    let version = os_version();
    let (supported, detail) = match version {
        None => (false, "Could not determine the macOS version.".to_string()),
        Some((maj, min)) if !macos_version_supported(maj, min) => (
            false,
            format!(
                "System-audio capture needs macOS {}.{} or later (this Mac runs {maj}.{min}). \
                 macOS 13 is not supported for system audio; microphone recording still works.",
                MACOS_MIN_VERSION.0, MACOS_MIN_VERSION.1
            ),
        ),
        Some(_) => {
            let api = AnyClass::get(c"CATapDescription").is_some()
                && resolve::<CreateTapFn>(c"AudioHardwareCreateProcessTap").is_some();
            if api {
                (true, "Core Audio process taps available.".to_string())
            } else {
                (false, "Core Audio process-tap API not found on this system.".to_string())
            }
        }
    };
    let permission = if supported { SystemAudioPermission::Unknown } else { SystemAudioPermission::Unsupported };
    SystemAudioStatus { supported, per_app_capture: supported, permission, detail, backend }
}

/// Capability plus the private TCC preflight; `doctor` only.
pub(super) fn status() -> SystemAudioStatus {
    let mut s = capability();
    if !s.supported {
        return s;
    }
    s.permission = tcc_permission();
    s.detail = match s.permission {
        SystemAudioPermission::Denied => format!(
            "{} Permission denied: enable the app (or your terminal) under System Settings > Privacy & Security > \
             Screen & System Audio Recording > System Audio Recording Only.",
            s.detail
        ),
        SystemAudioPermission::Granted => format!("{} Permission granted.", s.detail),
        _ => format!(
            "{} Permission state cannot be determined without prompting; macOS asks for System Audio \
             Recording permission on the first capture.",
            s.detail
        ),
    };
    s
}

/// An audio process as seen by Core Audio.
#[derive(Debug, Clone)]
pub(super) struct ProcInfo {
    pub object_id: u32,
    pub pid: i32,
    pub bundle_id: Option<String>,
    pub exe: Option<String>,
}

/// Pure matching of `--app` against the process list. Helper processes whose
/// bundle id extends the requested one (`com.google.Chrome.helper`) match too.
pub(super) fn select_processes(procs: &[ProcInfo], app: &AppSelector) -> Vec<u32> {
    procs
        .iter()
        .filter(|p| match app {
            AppSelector::Pid(pid) => p.pid as i64 == *pid as i64,
            AppSelector::BundleId(id) => p.bundle_id.as_deref().is_some_and(|b| {
                b.eq_ignore_ascii_case(id)
                    || (b.len() > id.len()
                        && b[..id.len()].eq_ignore_ascii_case(id)
                        && b.as_bytes()[id.len()] == b'.')
            }),
            AppSelector::Exe(name) => {
                let want = name.to_ascii_lowercase();
                p.exe.as_deref().is_some_and(|e| {
                    let base = e.rsplit('/').next().unwrap_or(e).to_ascii_lowercase();
                    base == want
                })
            }
        })
        .map(|p| p.object_id)
        .collect()
}

fn any<T: objc2::Message>(r: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: every Objective-C object is an AnyObject.
    unsafe { Retained::cast_unchecked(r) }
}

fn dict(entries: &[(&str, Retained<AnyObject>)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<Retained<NSString>> = entries.iter().map(|(k, _)| NSString::from_str(k)).collect();
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<Retained<AnyObject>> = entries.iter().map(|(_, v)| v.clone()).collect();
    NSDictionary::from_retained_objects(&key_refs, &values)
}

fn get_data<T: Default + Copy>(id: AudioObjectID, selector: u32) -> Result<T, OSStatus> {
    let addr = PropertyAddress { selector, scope: SCOPE_GLOBAL, element: ELEMENT_MAIN };
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: value/size describe a valid buffer of T.
    let st = unsafe { AudioObjectGetPropertyData(id, &addr, 0, ptr::null(), &mut size, (&mut value as *mut T).cast()) };
    if st == 0 { Ok(value) } else { Err(st) }
}

fn get_string(id: AudioObjectID, selector: u32) -> Option<String> {
    let addr = PropertyAddress { selector, scope: SCOPE_GLOBAL, element: ELEMENT_MAIN };
    let mut cf: *mut c_void = ptr::null_mut();
    let mut size = std::mem::size_of::<*mut c_void>() as u32;
    // SAFETY: CFStringRef out-parameter; the property is "copy" so we own the result.
    let st = unsafe { AudioObjectGetPropertyData(id, &addr, 0, ptr::null(), &mut size, (&mut cf as *mut *mut c_void).cast()) };
    if st != 0 || cf.is_null() {
        return None;
    }
    // SAFETY: CFString is toll-free bridged to NSString; we own one retain.
    let s = unsafe { Retained::from_raw(cf.cast::<NSString>()) }?;
    Some(s.to_string())
}

fn list_processes() -> Result<Vec<ProcInfo>, OSStatus> {
    let addr = PropertyAddress { selector: PROP_PROCESS_LIST, scope: SCOPE_GLOBAL, element: ELEMENT_MAIN };
    let mut size = 0u32;
    // SAFETY: standard size-then-data property read.
    let st = unsafe { AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &addr, 0, ptr::null(), &mut size) };
    if st != 0 {
        return Err(st);
    }
    let mut ids = vec![0u32; size as usize / 4];
    let st = unsafe {
        AudioObjectGetPropertyData(SYSTEM_OBJECT, &addr, 0, ptr::null(), &mut size, ids.as_mut_ptr().cast())
    };
    if st != 0 {
        return Err(st);
    }
    ids.truncate(size as usize / 4);
    Ok(ids
        .into_iter()
        .map(|object_id| {
            let pid = get_data::<i32>(object_id, PROP_PROCESS_PID).unwrap_or(-1);
            let mut buf = [0u8; 4096];
            // SAFETY: buffer valid for 4096 bytes.
            let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
            let exe = (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned());
            ProcInfo { object_id, pid, bundle_id: get_string(object_id, PROP_PROCESS_BUNDLE_ID), exe }
        })
        .collect())
}

/// Sum the channels of the buffers in raw `AudioBufferList` bytes (as returned
/// for kAudioDevicePropertyStreamConfiguration). Reads fields from the byte slice
/// only, so a zero-buffer reply (8 bytes, smaller than the Rust struct) and
/// truncated replies are handled without forming any out-of-bounds reference.
fn parse_input_channels(bytes: &[u8]) -> u32 {
    let first = std::mem::offset_of!(AudioBufferList, buffers);
    let stride = std::mem::size_of::<AudioBuffer>();
    let Some(count) = bytes.get(..4).map(|b| u32::from_ne_bytes(b.try_into().unwrap())) else {
        return 0;
    };
    let mut total = 0u32;
    for i in 0..count as usize {
        let at = first + i * stride;
        let Some(ch) = bytes.get(at..at + 4) else { break }; // truncated: ignore the rest
        total = total.saturating_add(u32::from_ne_bytes(ch.try_into().unwrap()));
    }
    total
}

/// Total input channels a device exposes (0 for an output-only device).
fn input_channel_count(dev: AudioObjectID) -> Result<u32, OSStatus> {
    let addr = PropertyAddress { selector: PROP_STREAM_CONFIGURATION, scope: SCOPE_INPUT, element: ELEMENT_MAIN };
    let mut size = 0u32;
    // SAFETY: standard size-then-data property read.
    let st = unsafe { AudioObjectGetPropertyDataSize(dev, &addr, 0, ptr::null(), &mut size) };
    if st != 0 {
        return Err(st);
    }
    let mut raw = vec![0u8; size as usize];
    let st = unsafe { AudioObjectGetPropertyData(dev, &addr, 0, ptr::null(), &mut size, raw.as_mut_ptr().cast()) };
    if st != 0 {
        return Err(st);
    }
    raw.truncate(size as usize);
    Ok(parse_input_channels(&raw))
}

/// Pick the tap's audio out of an IOProc input list, or `None` when the list
/// does not have exactly the tap's shape. The aggregate also contains the
/// physical output device; `start` refuses devices with input streams, and this
/// check is the second line of defence so microphone input can never reach the
/// "system" track. `buffers` is `(channels_in_buffer, samples)` per buffer.
pub(super) fn select_tap_audio(buffers: &[(u32, &[f32])], tap_channels: u32, non_interleaved: bool) -> Option<Vec<f32>> {
    if tap_channels == 0 {
        return None;
    }
    if non_interleaved && tap_channels > 1 {
        if buffers.len() != tap_channels as usize || buffers.iter().any(|(c, _)| *c != 1) {
            return None;
        }
        let planes: Vec<&[f32]> = buffers.iter().map(|(_, d)| *d).collect();
        Some(planar_to_interleaved(&planes))
    } else {
        match buffers {
            [(c, d)] if *c == tap_channels => Some(d.to_vec()),
            _ => None,
        }
    }
}

/// Planar (one buffer per channel) to interleaved.
pub(super) fn planar_to_interleaved(planes: &[&[f32]]) -> Vec<f32> {
    let n = planes.iter().map(|p| p.len()).min().unwrap_or(0);
    let mut out = Vec::with_capacity(n * planes.len());
    for i in 0..n {
        for p in planes {
            out.push(p[i]);
        }
    }
    out
}

/// Shown when the selected output device also has microphone inputs.
const DUPLEX_OUTPUT_MESSAGE: &str = "System-audio capture is not supported while the output device also has a \
microphone input (USB headsets and audio interfaces, Bluetooth headsets that expose a microphone), because that \
input could end up in the system-audio track. Select an output-only device in System Settings > Sound > Output \
(the built-in speakers, or wired headphones on the headphone jack), then try again.";

struct IoContext {
    sink: ChunkSink,
    non_interleaved: bool,
    tap_channels: u32,
}

extern "C" fn io_proc(
    _device: AudioObjectID,
    _now: *const c_void,
    input: *const AudioBufferList,
    _input_time: *const c_void,
    _output: *mut AudioBufferList,
    _output_time: *const c_void,
    client: *mut c_void,
) -> OSStatus {
    if input.is_null() || client.is_null() {
        return 0;
    }
    // SAFETY: client is the IoContext leaked in `start` and alive until after
    // AudioDeviceStop; Core Audio calls the IOProc from one thread at a time.
    let ctx = unsafe { &mut *(client as *mut IoContext) };
    let list = unsafe { &*input };
    let count = list.number_buffers as usize;
    // SAFETY: the buffer array holds `count` entries.
    let bufs = unsafe { std::slice::from_raw_parts(list.buffers.as_ptr(), count) };
    let view = |b: &AudioBuffer| -> (u32, &[f32]) {
        if b.data.is_null() {
            (b.number_channels, &[])
        } else {
            // SAFETY: Core Audio guarantees `data_byte_size` valid bytes of float samples.
            (b.number_channels, unsafe { std::slice::from_raw_parts(b.data.cast::<f32>(), b.data_byte_size as usize / 4) })
        }
    };
    let views: Vec<(u32, &[f32])> = bufs.iter().map(view).collect();
    // Anything that is not exactly the tap's shape is dropped (silence is gap-filled).
    if let Some(chunk) = select_tap_audio(&views, ctx.tap_channels, ctx.non_interleaved) {
        (ctx.sink)(&chunk);
    }
    0
}

pub(super) struct Capture {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    proc_id: *mut c_void,
    ctx: *mut IoContext,
    destroy_tap: DestroyTapFn,
    stopped: bool,
}

// SAFETY: the raw pointers are only used from start/stop on the owning thread
// and by Core Audio's IO thread, which stop() joins via AudioDeviceStop.
unsafe impl Send for Capture {}

impl Capture {
    pub(super) fn start(target: &CaptureTarget, sink: ChunkSink) -> Result<(Self, NativeFormat), DictationError> {
        let st = capability();
        if !st.supported {
            return Err(DictationError::AudioCaptureError(st.detail));
        }
        let create: CreateTapFn = resolve(c"AudioHardwareCreateProcessTap")
            .ok_or_else(|| DictationError::AudioCaptureError("Core Audio process taps unavailable".into()))?;
        let destroy_tap: DestroyTapFn = resolve(c"AudioHardwareDestroyProcessTap")
            .ok_or_else(|| DictationError::AudioCaptureError("Core Audio process taps unavailable".into()))?;
        let cls = AnyClass::get(c"CATapDescription")
            .ok_or_else(|| DictationError::AudioCaptureError("CATapDescription class not found".into()))?;

        let (alloc, ids): (Allocated<AnyObject>, Retained<NSArray<NSNumber>>) = unsafe {
            let alloc: Allocated<AnyObject> = msg_send![cls, alloc];
            match target {
                CaptureTarget::All => (alloc, NSArray::new()),
                CaptureTarget::App(app) => {
                    let procs = list_processes().map_err(|s| capture_err("Listing audio processes failed", s))?;
                    let matched = select_processes(&procs, app);
                    if matched.is_empty() {
                        return Err(DictationError::AudioCaptureError(format!(
                            "No audio process matches {app:?}. The app must be running and have opened an audio device \
                             (join the call first); helper processes match by bundle-id prefix."
                        )));
                    }
                    let nums: Vec<Retained<NSNumber>> = matched.iter().map(|i| NSNumber::new_u32(*i)).collect();
                    (alloc, NSArray::from_retained_slice(&nums))
                }
            }
        };
        let desc: Retained<AnyObject> = unsafe {
            match target {
                CaptureTarget::All => msg_send![alloc, initStereoGlobalTapButExcludeProcesses: &*ids],
                CaptureTarget::App(_) => msg_send![alloc, initStereoMixdownOfProcesses: &*ids],
            }
        };
        let tap_uuid = NSUUID::new();
        unsafe {
            let _: () = msg_send![&*desc, setName: &*NSString::from_str("Sagascript system audio")];
            let _: () = msg_send![&*desc, setUUID: &*tap_uuid];
            let _: () = msg_send![&*desc, setPrivate: true];
            let _: () = msg_send![&*desc, setMuteBehavior: 0isize]; // CATapUnmuted
        }

        let mut tap: AudioObjectID = 0;
        // SAFETY: desc is a valid CATapDescription; tap is a valid out pointer.
        let status = unsafe { create(Retained::as_ptr(&desc) as *mut AnyObject, &mut tap) };
        if status != 0 {
            return Err(capture_err("Creating the process tap failed (permission denied or unsupported?)", status));
        }

        // From here on, clean up on every error path.
        let cleanup_tap = |tap| unsafe {
            destroy_tap(tap);
        };

        let format: StreamDescription = match get_data(tap, PROP_TAP_FORMAT) {
            Ok(f) => f,
            Err(s) => {
                cleanup_tap(tap);
                return Err(capture_err("Reading the tap format failed", s));
            }
        };
        let native = NativeFormat { sample_rate: format.sample_rate as u32, channels: format.channels_per_frame.max(1) as u16 };

        // Resolve the output device ONCE; its UID (aggregate clock/sub-device) and
        // its input configuration (duplex check) both come from this same ID.
        let Ok(output_dev) = get_data::<AudioObjectID>(SYSTEM_OBJECT, PROP_DEFAULT_OUTPUT) else {
            cleanup_tap(tap);
            return Err(DictationError::AudioCaptureError("No default output device found".into()));
        };
        let Some(output_uid) = get_string(output_dev, PROP_DEVICE_UID) else {
            cleanup_tap(tap);
            return Err(DictationError::AudioCaptureError("No default output device found".into()));
        };

        // The aggregate contains the physical output device. If that device has
        // input streams (USB headset/interface, Bluetooth headset with a
        // microphone), they would appear in the aggregate's input list next to the
        // tap; refuse rather than risk microphone audio in the "system" track.
        match input_channel_count(output_dev) {
            Ok(0) => {}
            Ok(_) => {
                cleanup_tap(tap);
                return Err(DictationError::AudioCaptureError(DUPLEX_OUTPUT_MESSAGE.into()));
            }
            Err(s) => {
                cleanup_tap(tap);
                return Err(capture_err("Inspecting the default output device failed", s));
            }
        }

        let agg_uid = NSUUID::new().UUIDString();
        let tap_uid_str = tap_uuid.UUIDString();
        let aggregate_desc = {
            let sub = dict(&[("uid", any(NSString::from_str(&output_uid)))]);
            let tapd = dict(&[("uid", any(tap_uid_str)), ("drift", any(NSNumber::new_bool(true)))]);
            let subs = NSArray::from_retained_slice(&[sub]);
            let taps = NSArray::from_retained_slice(&[tapd]);
            dict(&[
                ("uid", any(agg_uid)),
                ("name", any(NSString::from_str("Sagascript tap"))),
                ("private", any(NSNumber::new_bool(true))),
                ("master", any(NSString::from_str(&output_uid))),
                ("subdevices", any(subs)),
                ("taps", any(taps)),
                // No "tapautostart": it makes AudioDeviceStart block until a tapped
                // process first plays audio (AudioHardware.h, kAudioAggregateDeviceTapAutoStartKey),
                // which would hang startup, --duration and Ctrl+C while nothing plays.
                // Idle time is silence-filled by the recorder instead.
            ])
        };
        let mut aggregate: AudioObjectID = 0;
        // SAFETY: NSDictionary is toll-free bridged to CFDictionaryRef.
        let status = unsafe { AudioHardwareCreateAggregateDevice(Retained::as_ptr(&aggregate_desc).cast(), &mut aggregate) };
        if status != 0 {
            cleanup_tap(tap);
            return Err(capture_err("Creating the aggregate device failed", status));
        }

        let ctx = Box::into_raw(Box::new(IoContext {
            sink,
            non_interleaved: format.format_flags & FLAG_NON_INTERLEAVED != 0,
            tap_channels: format.channels_per_frame,
        }));
        let mut proc_id: *mut c_void = ptr::null_mut();
        let status = unsafe { AudioDeviceCreateIOProcID(aggregate, io_proc, ctx.cast(), &mut proc_id) };
        let started = if status == 0 { unsafe { AudioDeviceStart(aggregate, proc_id) } } else { status };
        if started != 0 {
            unsafe {
                if status == 0 {
                    AudioDeviceDestroyIOProcID(aggregate, proc_id);
                }
                AudioHardwareDestroyAggregateDevice(aggregate);
                drop(Box::from_raw(ctx));
            }
            cleanup_tap(tap);
            return Err(capture_err("Starting the tap failed", started));
        }
        Ok((Capture { tap, aggregate, proc_id, ctx, destroy_tap, stopped: false }, native))
    }

    pub(super) fn stop(mut self) -> Result<(), DictationError> {
        self.teardown();
        Ok(())
    }

    fn teardown(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        // SAFETY: reverse order of creation; AudioDeviceStop returns only after
        // the IOProc has finished, so freeing the context afterwards is sound.
        unsafe {
            AudioDeviceStop(self.aggregate, self.proc_id);
            AudioDeviceDestroyIOProcID(self.aggregate, self.proc_id);
            AudioHardwareDestroyAggregateDevice(self.aggregate);
            (self.destroy_tap)(self.tap);
            drop(Box::from_raw(self.ctx));
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.teardown();
    }
}

#[cfg(test)]
mod tests {
    fn buffer_list_bytes(channels: &[u32], declared: u32) -> Vec<u8> {
        let mut v = declared.to_ne_bytes().to_vec();
        v.extend_from_slice(&[0; 4]); // padding before the 8-aligned buffer array
        for c in channels {
            v.extend_from_slice(&c.to_ne_bytes());
            v.extend_from_slice(&[0; 12]); // data_byte_size + data pointer
        }
        v
    }

    #[test]
    fn input_channel_parsing_handles_empty_short_and_truncated_lists() {
        assert_eq!(parse_input_channels(&[]), 0);
        assert_eq!(parse_input_channels(&[1, 0]), 0);
        assert_eq!(parse_input_channels(&buffer_list_bytes(&[], 0)), 0); // output-only: 8 bytes
        assert_eq!(parse_input_channels(&buffer_list_bytes(&[2], 1)), 2);
        assert_eq!(parse_input_channels(&buffer_list_bytes(&[1, 2], 2)), 3);
        // declares 3 buffers but only 1 is present: count what is there
        assert_eq!(parse_input_channels(&buffer_list_bytes(&[2], 3)), 2);
        // a buffer cut off mid-header contributes nothing
        let mut cut = buffer_list_bytes(&[2], 1);
        cut.truncate(10);
        assert_eq!(parse_input_channels(&cut), 0);
    }

    #[test]
    fn tap_audio_selection_rejects_foreign_buffers() {
        let l = [1.0f32, 2.0];
        let r = [3.0f32, 4.0];
        let mic = [9.0f32, 9.0];
        // planar stereo tap
        assert_eq!(select_tap_audio(&[(1, &l), (1, &r)], 2, true).unwrap(), vec![1.0, 3.0, 2.0, 4.0]);
        // extra physical-input buffer (duplex device): rejected
        assert!(select_tap_audio(&[(1, &mic), (1, &l), (1, &r)], 2, true).is_none());
        // interleaved stereo tap
        assert_eq!(select_tap_audio(&[(2, &[1.0, 3.0, 2.0, 4.0])], 2, false).unwrap(), vec![1.0, 3.0, 2.0, 4.0]);
        assert!(select_tap_audio(&[(1, &mic), (2, &[1.0, 3.0])], 2, false).is_none());
        // wrong channel layout
        assert!(select_tap_audio(&[(1, &mic)], 2, false).is_none());
        assert!(select_tap_audio(&[], 2, true).is_none());
    }

    use super::*;

    fn procs() -> Vec<ProcInfo> {
        vec![
            ProcInfo { object_id: 10, pid: 100, bundle_id: Some("us.zoom.xos".into()), exe: Some("/Applications/zoom.us.app/Contents/MacOS/zoom.us".into()) },
            ProcInfo { object_id: 11, pid: 200, bundle_id: Some("com.google.Chrome.helper".into()), exe: Some("/x/Google Chrome Helper".into()) },
            ProcInfo { object_id: 12, pid: 201, bundle_id: Some("com.google.Chromevox".into()), exe: None },
            ProcInfo { object_id: 13, pid: 300, bundle_id: None, exe: Some("/usr/bin/afplay".into()) },
        ]
    }

    #[test]
    fn selects_by_bundle_prefix_pid_and_exe() {
        assert_eq!(select_processes(&procs(), &AppSelector::BundleId("us.zoom.xos".into())), vec![10]);
        assert_eq!(select_processes(&procs(), &AppSelector::BundleId("com.google.Chrome".into())), vec![11]);
        assert_eq!(select_processes(&procs(), &AppSelector::Pid(300)), vec![13]);
        assert_eq!(select_processes(&procs(), &AppSelector::Exe("AFPLAY".into())), vec![13]);
        assert!(select_processes(&procs(), &AppSelector::Pid(1)).is_empty());
    }

    #[test]
    fn planar_interleaves_and_truncates() {
        assert_eq!(planar_to_interleaved(&[&[1.0, 2.0, 3.0], &[10.0, 20.0]]), vec![1.0, 10.0, 2.0, 20.0]);
        assert!(planar_to_interleaved(&[]).is_empty());
    }

    #[test]
    fn status_never_panics_and_is_consistent() {
        let s = status();
        assert_eq!(s.backend, "coreaudio-process-tap");
        if !s.supported {
            assert_eq!(s.permission, SystemAudioPermission::Unsupported);
        }
    }

    #[test]
    fn fourcc_matches_core_audio_constants() {
        assert_eq!(PROP_PROCESS_LIST, 0x70727323); // 'prs#'
        assert_eq!(SCOPE_GLOBAL, 0x676c6f62); // 'glob'
    }

    /// Live test: creates a real tap (may show the System Audio Recording
    /// prompt). Run only with the owner's OK:
    ///   cargo test -p sagascript-core --features record macos_live -- --ignored --nocapture
    #[test]
    #[ignore = "live system-audio capture; requires owner approval"]
    fn macos_live_capture_one_second() {
        use std::sync::{Arc, Mutex};
        let got = Arc::new(Mutex::new(0usize));
        let g = got.clone();
        let (cap, fmt) = Capture::start(&CaptureTarget::All, Box::new(move |c| *g.lock().unwrap() += c.len())).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        cap.stop().unwrap();
        assert!(fmt.sample_rate > 0);
        assert!(*got.lock().unwrap() > 0);
    }
}
