//! Windows backend: WASAPI loopback of the default render endpoint, and
//! per-process loopback (Windows 10 2004+, build 19041) via
//! `ActivateAudioInterfaceAsync` with `VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK`.
//! Loopback needs no permission. The capture thread owns all COM objects.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::core::{implement, Interface, GUID};
use windows::Win32::Foundation::CloseHandle;
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
use windows::Win32::System::Variant::VT_BLOB;

use super::convert::{decode_pcm_packet, PcmLayout};
use super::{
    exe_name_matches, AppSelector, CaptureTarget, ChunkSink, NativeFormat, SystemAudioPermission,
    SystemAudioStatus,
};
use crate::error::DictationError;

const IEEE_FLOAT_SUBTYPE: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM: u32 = 0x8000_0000;
const AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
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

fn pids_for_exe(name: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    // SAFETY: standard Toolhelp snapshot walk.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return pids };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            let len = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
            if exe_name_matches(&String::from_utf16_lossy(&entry.szExeFile[..len]), name) {
                pids.push(entry.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    pids
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
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
    let (tx, rx) = mpsc::channel();
    let handler: IActivateAudioInterfaceCompletionHandler = Handler { tx }.into();
    let op = ActivateAudioInterfaceAsync(
        VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        &IAudioClient::IID,
        Some(&pv),
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
}

unsafe fn open(target: &CaptureTarget) -> Result<Opened, DictationError> {
    let process_pid = match target {
        CaptureTarget::All => None,
        CaptureTarget::App(AppSelector::Pid(pid)) => Some(*pid),
        CaptureTarget::App(AppSelector::Exe(name)) => Some(*pids_for_exe(name).first().ok_or_else(|| {
            DictationError::AudioCaptureError(format!("No running process matches '{name}'"))
        })?),
        CaptureTarget::App(AppSelector::BundleId(id)) => {
            return Err(DictationError::AudioCaptureError(format!(
                "'{id}' looks like a macOS bundle id; on Windows pass a pid or an executable name (e.g. Teams.exe)"
            )))
        }
    };
    let (client, fmt_owned, flags) = if let Some(pid) = process_pid {
        let client = activate_process_client(pid)?;
        // Process loopback has no mix format; request 32-bit float stereo 48 kHz.
        let fmt = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: 2,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 48_000 * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        (
            client,
            fmt,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
        )
    } else {
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
        return Ok(Opened { client, capture, layout, format: NativeFormat { sample_rate: rate, channels } });
    };
    let layout = layout_of(&fmt_owned)?;
    client
        .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &fmt_owned, None)
        .map_err(|e| err("Initializing process loopback", e))?;
    let capture: IAudioCaptureClient = client.GetService().map_err(|e| err("IAudioCaptureClient", e))?;
    Ok(Opened {
        client,
        capture,
        layout,
        format: NativeFormat { sample_rate: fmt_owned.nSamplesPerSec, channels: fmt_owned.nChannels },
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
                                thread::sleep(Duration::from_millis(10));
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
