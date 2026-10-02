//! Windows backend: WASAPI loopback of the default render endpoint, and
//! per-process loopback (Windows 10 2004+, build 19041) via
//! `ActivateAudioInterfaceAsync` with `VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK`.
//! Loopback needs no permission. The capture thread owns all COM objects.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::core::{implement, Interface, GUID, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eConsole, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;

use super::convert::{decode_pcm_packet, PcmLayout};
use super::{
    exe_name_matches, tree_roots, AppSelector, CaptureTarget, ChunkSink, NativeFormat, SystemAudioPermission,
    SystemAudioStatus,
};
use crate::error::DictationError;

const IEEE_FLOAT_SUBTYPE: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM: u32 = 0x8000_0000;
const AUDCLNT_STREAMFLAGS_EVENTCALLBACK: u32 = 0x0004_0000;
const BUFFERFLAGS_SILENT: u32 = 2;

pub(super) fn status() -> SystemAudioStatus {
    SystemAudioStatus {
        supported: true,
        per_app_capture: true,
        permission: SystemAudioPermission::NotRequired,
        detail: "WASAPI loopback of the default output device; no permission needed. \
                 Per-app capture requires Windows 10 version 2004 (build 19041) or later."
            .into(),
        backend: "wasapi-loopback",
    }
}

fn err(context: &str, e: impl std::fmt::Display) -> DictationError {
    DictationError::AudioCaptureError(format!("{context}: {e}"))
}

fn layout_of(fmt: &WAVEFORMATEX) -> Result<PcmLayout, DictationError> {
    let tag = { fmt.wFormatTag };
    let bits = { fmt.wBitsPerSample };
    let float = match tag {
        WAVE_FORMAT_IEEE_FLOAT => true,
        WAVE_FORMAT_PCM => false,
        WAVE_FORMAT_EXTENSIBLE => {
            // SAFETY: the tag says the allocation is a WAVEFORMATEXTENSIBLE.
            let ext = unsafe { &*(fmt as *const WAVEFORMATEX as *const WAVEFORMATEXTENSIBLE) };
            let sub = { ext.SubFormat };
            sub == IEEE_FLOAT_SUBTYPE
        }
        other => return Err(err("Unsupported loopback format tag", other)),
    };
    match (float, bits) {
        (true, 32) => Ok(PcmLayout::F32),
        (false, 16) => Ok(PcmLayout::I16),
        _ => Err(err("Unsupported loopback sample format", format!("float={float} bits={bits}"))),
    }
}

/// (pid, parent pid) of every running process whose executable matches `name`.
fn processes_for_exe(name: &str) -> Vec<(u32, u32)> {
    let mut found = Vec::new();
    // SAFETY: standard Toolhelp snapshot walk.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return found };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            let len = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
            if exe_name_matches(&String::from_utf16_lossy(&entry.szExeFile[..len]), name) {
                found.push((entry.th32ProcessID, entry.th32ParentProcessID));
            }
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    found
}

// ActivateAudioInterfaceAsync requires an agile completion handler (it is called
// back on an MTA worker thread). windows-implement 0.60.2 (src/gen.rs:313) answers
// IAgileObject (and IMarshal) queries when `Agile = true`, which is also its default;
// IAgileObject is not nameable in the interface list, so the flag is spelled out.
#[implement(IActivateAudioInterfaceCompletionHandler, Agile = true)]
struct Handler {
    tx: mpsc::Sender<()>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for Handler_Impl {
    fn ActivateCompleted(&self, _op: windows::core::Ref<'_, IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
        let _ = self.tx.send(());
        Ok(())
    }
}

/// Activate a process-loopback IAudioClient for `pid` (and its process tree).
unsafe fn activate_process_client(pid: u32) -> Result<IAudioClient, DictationError> {
    let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    let mut pv = PROPVARIANT::default();
    pv.Anonymous = PROPVARIANT_0 {
        Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
            vt: VT_BLOB,
            wReserved1: 0,
            wReserved2: 0,
            wReserved3: 0,
            Anonymous: PROPVARIANT_0_0_0 {
                blob: windows::Win32::System::Com::BLOB {
                    cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                    pBlobData: (&mut params as *mut AUDIOCLIENT_ACTIVATION_PARAMS).cast(),
                },
            },
        }),
    };
    // windows 0.61.3 DOES implement `Drop for PROPVARIANT` and it calls
    // `PropVariantClear` (windows-0.61.3/src/extensions/Win32/System/StructuredStorage.rs:32-36).
    // This VT_BLOB points at the stack `params`, so clearing it would
    // CoTaskMemFree a stack address. `pv` is therefore wrapped in ManuallyDrop and
    // never dropped; `params` outlives the (synchronous) ActivateAudioInterfaceAsync call.
    let pv = std::mem::ManuallyDrop::new(pv);
    let (tx, rx) = mpsc::channel();
    let handler: IActivateAudioInterfaceCompletionHandler = Handler { tx }.into();
    let op = ActivateAudioInterfaceAsync(
        VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        &IAudioClient::IID,
        Some(&*pv),
        &handler,
    )
    .map_err(|e| err("Process loopback activation failed (needs Windows 10 2004+)", e))?;
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| err("Process loopback activation", "timed out"))?;
    let mut hr = windows::core::HRESULT(0);
    let mut iface: Option<windows::core::IUnknown> = None;
    op.GetActivateResult(&mut hr, &mut iface).map_err(|e| err("GetActivateResult", e))?;
    hr.ok().map_err(|e| err("Process loopback activation result", e))?;
    iface
        .ok_or_else(|| err("Process loopback activation", "no interface returned"))?
        .cast::<IAudioClient>()
        .map_err(|e| err("Process loopback client cast", e))
}

struct Opened {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    layout: PcmLayout,
    format: NativeFormat,
    /// Event signalled per buffer (process loopback is event-driven).
    event: Option<HANDLE>,
}

/// Candidate process-tree roots to try, in order. `--app Teams.exe` matches many
/// processes; `INCLUDE_TARGET_PROCESS_TREE` on a root covers its children.
unsafe fn open(target: &CaptureTarget) -> Result<Opened, DictationError> {
    let candidates: Vec<u32> = match target {
        CaptureTarget::All => Vec::new(),
        CaptureTarget::App(AppSelector::Pid(pid)) => vec![*pid],
        CaptureTarget::App(AppSelector::Exe(name)) => {
            let roots = tree_roots(&processes_for_exe(name));
            if roots.is_empty() {
                return Err(DictationError::AudioCaptureError(format!("No running process matches '{name}'")));
            }
            roots
        }
        CaptureTarget::App(AppSelector::BundleId(id)) => {
            return Err(DictationError::AudioCaptureError(format!(
                "'{id}' looks like a macOS bundle id; on Windows pass a pid or an executable name (e.g. Teams.exe)"
            )))
        }
    };
    if candidates.is_empty() {
        return open_default_loopback();
    }
    let mut last = None;
    for pid in candidates {
        match open_process_loopback(pid) {
            Ok(o) => return Ok(o),
            Err(e) => last = Some(e),
        }
    }
    Err(last.expect("at least one candidate"))
}

unsafe fn open_default_loopback() -> Result<Opened, DictationError> {
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| err("Device enumerator", e))?;
    let device = enumerator
        .GetDefaultAudioEndpoint(eRender, eConsole)
        .map_err(|e| err("No default output device", e))?;
    let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(|e| err("Activating audio client", e))?;
    let mix = client.GetMixFormat().map_err(|e| err("GetMixFormat", e))?;
    let layout = layout_of(&*mix);
    let (channels, rate) = ((*mix).nChannels, (*mix).nSamplesPerSec);
    // `mix` points at the full (possibly extensible) format block.
    let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 2_000_000, 0, mix, None);
    windows::Win32::System::Com::CoTaskMemFree(Some(mix as *const _));
    let layout = layout?;
    init.map_err(|e| err("Initializing loopback", e))?;
    let capture: IAudioCaptureClient = client.GetService().map_err(|e| err("IAudioCaptureClient", e))?;
    Ok(Opened { client, capture, layout, format: NativeFormat { sample_rate: rate, channels }, event: None })
}

/// Follows Microsoft's "ApplicationLoopback" sample (microsoft/windows-classic-samples,
/// Samples/ApplicationLoopback/cpp/LoopbackCapture.cpp, recalled from memory, not
/// re-checked offline): activate `VAD\Process_Loopback`, then Initialize with
/// `AUDCLNT_STREAMFLAGS_LOOPBACK | EVENTCALLBACK | AUTOCONVERTPCM`, zero buffer
/// duration and periodicity, 16-bit PCM 44.1 kHz stereo, with SetEventHandle.
unsafe fn open_process_loopback(pid: u32) -> Result<Opened, DictationError> {
    let client = activate_process_client(pid)?;
    let fmt = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM,
        nChannels: 2,
        nSamplesPerSec: 44_100,
        nAvgBytesPerSec: 44_100 * 4,
        nBlockAlign: 4,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    client
        .Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
            0,
            0,
            &fmt,
            None,
        )
        .map_err(|e| err(&format!("Initializing process loopback for pid {pid}"), e))?;
    let event = CreateEventW(None, false, false, PCWSTR::null()).map_err(|e| err("CreateEvent", e))?;
    if let Err(e) = client.SetEventHandle(event) {
        let _ = CloseHandle(event);
        return Err(err("SetEventHandle", e));
    }
    let capture: IAudioCaptureClient = match client.GetService() {
        Ok(c) => c,
        Err(e) => {
            let _ = CloseHandle(event);
            return Err(err("IAudioCaptureClient", e));
        }
    };
    Ok(Opened {
        client,
        capture,
        layout: PcmLayout::I16,
        format: NativeFormat { sample_rate: fmt.nSamplesPerSec, channels: fmt.nChannels },
        event: Some(event),
    })
}

pub(super) struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    pub(super) fn start(target: &CaptureTarget, mut sink: ChunkSink) -> Result<(Self, NativeFormat), DictationError> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let target = target.clone();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<NativeFormat, DictationError>>();
        let thread = thread::Builder::new()
            .name("sagascript-system-audio".into())
            .spawn(move || {
                // SAFETY: COM is initialized and uninitialized on this thread; all
                // COM objects live and die inside this closure.
                unsafe {
                    if let Err(e) = CoInitializeEx(None, COINIT_MULTITHREADED).ok() {
                        let _ = ready_tx.send(Err(err("CoInitializeEx", e)));
                        return;
                    }
                    match open(&target).and_then(|o| o.client.Start().map(|_| o).map_err(|e| err("Starting loopback", e))) {
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                        }
                        Ok(o) => {
                            let _ = ready_tx.send(Ok(o.format));
                            let bytes_per_sample = if o.layout == PcmLayout::F32 { 4 } else { 2 };
                            while !stop_thread.load(Ordering::Relaxed) {
                                match o.event {
                                    // Event-driven (process loopback): wake per buffer, 100 ms cap so stop is prompt.
                                    Some(ev) => {
                                        WaitForSingleObject(ev, 100);
                                    }
                                    None => thread::sleep(Duration::from_millis(10)),
                                }
                                while let Ok(n) = o.capture.GetNextPacketSize() {
                                    if n == 0 {
                                        break;
                                    }
                                    let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                                    if o.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None).is_err() {
                                        break;
                                    }
                                    let samples = frames as usize * o.format.channels as usize;
                                    let silent = flags & BUFFERFLAGS_SILENT != 0;
                                    let bytes = if data.is_null() || silent {
                                        &[][..]
                                    } else {
                                        std::slice::from_raw_parts(data, samples * bytes_per_sample)
                                    };
                                    sink(&decode_pcm_packet(bytes, o.layout, silent || data.is_null(), samples));
                                    let _ = o.capture.ReleaseBuffer(frames);
                                }
                            }
                            let _ = o.client.Stop();
                            if let Some(ev) = o.event {
                                let _ = CloseHandle(ev);
                            }
                        }
                    }
                    CoUninitialize();
                }
            })
            .map_err(|e| err("Spawning capture thread", e))?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(format)) => Ok((Capture { stop, thread: Some(thread) }, format)),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                Err(err("System audio start", "timed out"))
            }
        }
    }

    pub(super) fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.shutdown();
    }
}
