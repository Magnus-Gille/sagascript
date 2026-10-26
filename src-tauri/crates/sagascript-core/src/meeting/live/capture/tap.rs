//! Core Audio process tap (macOS 14.2+), using a tap-only private aggregate.
//! No physical input device is added to the system track, including on headsets.
//! API layouts/constants checked against AudioHardware.h/AudioHardwareTapping.h.
use super::Sink;
use objc2::{
    msg_send,
    rc::{Allocated, Retained},
    runtime::{AnyClass, AnyObject},
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};
use std::{
    cell::UnsafeCell,
    ffi::{c_char, c_void, CStr},
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        OnceLock,
    },
};

const fn cc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}
#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Format {
    rate: f64,
    id: u32,
    flags: u32,
    bytes_packet: u32,
    frames_packet: u32,
    bytes_frame: u32,
    channels: u32,
    bits: u32,
    reserved: u32,
}
#[repr(C)]
struct Buffer {
    channels: u32,
    bytes: u32,
    data: *const f32,
}
#[repr(C)]
struct BufferList {
    count: u32,
    buffers: [Buffer; 1],
}
// Keep the callback timestamp ABI tied to the SDK-generated binding. In
// particular, SMPTETime is 24 bytes and mFlags is at byte 56, not byte 48.
type Timestamp = coreaudio_sys::AudioTimeStamp;
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Timebase {
    numer: u32,
    denom: u32,
}
type Proc = extern "C" fn(
    u32,
    *const Timestamp,
    *const BufferList,
    *const Timestamp,
    *mut BufferList,
    *const Timestamp,
    *mut c_void,
) -> i32;
#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(
        id: u32,
        addr: *const Address,
        qsize: u32,
        q: *const c_void,
        size: *mut u32,
        value: *mut c_void,
    ) -> i32;
    fn AudioObjectSetPropertyData(
        id: u32,
        addr: *const Address,
        qsize: u32,
        q: *const c_void,
        size: u32,
        value: *const c_void,
    ) -> i32;
    fn AudioHardwareCreateAggregateDevice(desc: *const c_void, out: *mut u32) -> i32;
    fn AudioHardwareDestroyAggregateDevice(id: u32) -> i32;
    fn AudioDeviceCreateIOProcID(
        device: u32,
        proc_: Proc,
        ctx: *mut c_void,
        out: *mut *mut c_void,
    ) -> i32;
    fn AudioDeviceDestroyIOProcID(device: u32, id: *mut c_void) -> i32;
    fn AudioDeviceStart(device: u32, id: *mut c_void) -> i32;
    fn AudioDeviceStop(device: u32, id: *mut c_void) -> i32;
}
extern "C" {
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}
type Create = unsafe extern "C" fn(*mut AnyObject, *mut u32) -> i32;
type Destroy = unsafe extern "C" fn(u32) -> i32;
fn symbol<T>(name: &CStr) -> Option<T> {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
    // SAFETY: valid symbol string, only called for the two SDK function signatures above.
    let p = unsafe { dlsym(-2isize as *mut c_void, name.as_ptr()) };
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy(&p) })
}
fn timebase() -> Timebase {
    static BASE: OnceLock<Timebase> = OnceLock::new();
    *BASE.get_or_init(|| {
        let mut b = Timebase::default();
        // SAFETY: valid out pointer. Failure leaves denominator zero and fails timestamps closed.
        unsafe {
            mach_timebase_info(&mut b);
        }
        b
    })
}
fn to_ns(ticks: u64) -> u64 {
    let b = timebase();
    if b.denom == 0 {
        return 0;
    }
    ((u128::from(ticks) * u128::from(b.numer)) / u128::from(b.denom)).min(u128::from(u64::MAX))
        as u64
}
pub fn supported() -> bool {
    symbol::<Create>(c"AudioHardwareCreateProcessTap").is_some()
        && symbol::<Destroy>(c"AudioHardwareDestroyProcessTap").is_some()
        && AnyClass::get(c"CATapDescription").is_some()
}
pub fn host_ns() -> u64 {
    to_ns(unsafe { mach_absolute_time() })
}
fn property<T: Default + Copy>(id: u32, selector: u32) -> Result<T, String> {
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    let addr = Address {
        selector,
        scope: cc(b"glob"),
        element: 0,
    };
    // SAFETY: address and correctly sized writable result per SDK.
    let code = unsafe {
        AudioObjectGetPropertyData(
            id,
            &addr,
            0,
            ptr::null(),
            &mut size,
            (&mut value as *mut T).cast(),
        )
    };
    if code == 0 && size == std::mem::size_of::<T>() as u32 {
        Ok(value)
    } else {
        Err(format!("Core Audio property failed ({code})"))
    }
}
pub fn default_devices() -> Result<(u32, u32), String> {
    Ok((property(1, cc(b"dIn "))?, property(1, cc(b"dOut"))?))
}
fn any<T: objc2::Message>(value: Retained<T>) -> Retained<AnyObject> {
    unsafe { Retained::cast_unchecked(value) }
}
fn dict(items: &[(&str, Retained<AnyObject>)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<_> = items
        .iter()
        .map(|(key, _)| NSString::from_str(key))
        .collect();
    let refs: Vec<_> = keys.iter().map(|key| &**key).collect();
    let values: Vec<_> = items.iter().map(|(_, value)| value.clone()).collect();
    NSDictionary::from_retained_objects(&refs, &values)
}
struct Context {
    closed: AtomicBool,
    readers: AtomicUsize,
    sink: UnsafeCell<Option<Sink>>,
    channels: u32,
    planar: bool,
    rate: u32,
}
// SAFETY: callbacks only borrow Sink immutably while counted. Teardown closes first,
// waits for counted readers, then takes it. Closed late callbacks never access Sink.
unsafe impl Sync for Context {}
extern "C" fn callback(
    _device: u32,
    _now: *const Timestamp,
    input: *const BufferList,
    time: *const Timestamp,
    _output: *mut BufferList,
    _outtime: *const Timestamp,
    client: *mut c_void,
) -> i32 {
    if client.is_null() {
        return 0;
    }
    // SAFETY: context is retained after unregistering to cover late callbacks.
    let ctx = unsafe { &*client.cast::<Context>() };
    ctx.readers.fetch_add(1, Ordering::SeqCst);
    if !ctx.closed.load(Ordering::SeqCst) {
        // SAFETY: counted active reader and not closed, so teardown cannot take Sink.
        if let Some(sink) = unsafe { &*ctx.sink.get() } {
            copy_input(ctx, sink, input, time);
        }
    }
    ctx.readers.fetch_sub(1, Ordering::SeqCst);
    0
}
fn copy_input(ctx: &Context, sink: &Sink, input: *const BufferList, time: *const Timestamp) {
    let fail = || {
        sink.health.failed.store(true, Ordering::Release);
    };
    if input.is_null() || time.is_null() {
        fail();
        return;
    }
    // SAFETY: pointers and buffer list are Core Audio callback arguments.
    let (list, time) = unsafe { (&*input, &*time) };
    if time.mFlags & coreaudio_sys::kAudioTimeStampHostTimeValid == 0 {
        fail();
        return;
    }
    if list.count == 0 {
        return;
    }
    let count = if ctx.planar { ctx.channels } else { 1 };
    if list.count != count || count > 32 {
        fail();
        return;
    }
    let buffers = unsafe { std::slice::from_raw_parts(list.buffers.as_ptr(), count as usize) };
    let channels = if ctx.planar { 1 } else { ctx.channels };
    if buffers
        .iter()
        .any(|b| b.channels != channels || b.bytes % 4 != 0)
    {
        fail();
        return;
    }
    // A null data pointer with zero bytes is a valid idle callback.
    if buffers.iter().all(|b| b.bytes == 0) {
        return;
    }
    if buffers
        .iter()
        .any(|b| b.data.is_null() || b.bytes != buffers[0].bytes)
    {
        fail();
        return;
    }
    let samples = buffers[0].bytes as usize / 4;
    if !samples.is_multiple_of(channels as usize) {
        fail();
        return;
    }
    let frames = samples / channels as usize;
    sink.push(to_ns(time.mHostTime), ctx.rate, frames, |i| {
        let mut sum = 0.;
        for channel in 0..ctx.channels as usize {
            // SAFETY: every plane/interleaved stride is checked above against reported bytes.
            sum += unsafe {
                if ctx.planar {
                    *buffers[channel].data.add(i)
                } else {
                    *buffers[0].data.add(i * ctx.channels as usize + channel)
                }
            };
        }
        sum / ctx.channels as f32
    });
}

pub struct Tap {
    tap: u32,
    aggregate: u32,
    proc: *mut c_void,
    context: *mut Context,
    destroy: Destroy,
}
impl Tap {
    pub fn start(sink: Sink) -> Result<Self, String> {
        let _ = host_ns(); // initialize timebase off the realtime callback
        let create: Create = symbol(c"AudioHardwareCreateProcessTap")
            .ok_or("System audio needs macOS 14.2 or later")?;
        let destroy: Destroy =
            symbol(c"AudioHardwareDestroyProcessTap").ok_or("Process tap API unavailable")?;
        let class = AnyClass::get(c"CATapDescription").ok_or("Process tap class unavailable")?;
        // Exclude this process when it already owns an audio object (CPAL has opened above).
        let pid = std::process::id() as i32;
        let mut own = 0u32;
        let mut size = 4;
        let addr = Address {
            selector: cc(b"id2p"),
            scope: cc(b"glob"),
            element: 0,
        };
        let status = unsafe {
            AudioObjectGetPropertyData(
                1,
                &addr,
                4,
                (&pid as *const i32).cast(),
                &mut size,
                (&mut own as *mut u32).cast(),
            )
        };
        let excluded = if status == 0 && own != 0 {
            NSArray::from_retained_slice(&[NSNumber::new_u32(own)])
        } else {
            NSArray::new()
        };
        let description: Retained<AnyObject> = unsafe {
            let allocated: Allocated<AnyObject> = msg_send![class, alloc];
            msg_send![allocated,initStereoGlobalTapButExcludeProcesses:&*excluded]
        };
        let uuid = NSUUID::new();
        unsafe {
            let _: () = msg_send![&*description,setUUID:&*uuid];
            let _: () =
                msg_send![&*description,setName:&*NSString::from_str("SagaScript meeting audio")];
            let _: () = msg_send![&*description,setPrivate:true];
            let _: () = msg_send![&*description,setMuteBehavior:0isize];
        }
        let mut tap = 0;
        let result = unsafe { create(Retained::as_ptr(&description) as *mut AnyObject, &mut tap) };
        if result != 0 {
            return Err(format!(
                "System audio could not start ({result}). Check System Audio Recording permission."
            ));
        }
        let mut owned = Self {
            tap,
            aggregate: 0,
            proc: ptr::null_mut(),
            context: ptr::null_mut(),
            destroy,
        };
        let format: Format = property(tap, cc(b"tfmt"))?;
        if format.id != cc(b"lpcm")
            || format.bits != 32
            || format.flags & 1 == 0
            || !(8_000.0..=192_000.0).contains(&format.rate)
            || format.channels == 0
            || format.channels > 32
        {
            return Err("Unsupported system-audio format".into());
        }
        let tap_desc = dict(&[
            ("uid", any(uuid.UUIDString())),
            ("drift", any(NSNumber::new_bool(true))),
        ]);
        let aggregate_description = dict(&[
            ("uid", any(NSUUID::new().UUIDString())),
            (
                "name",
                any(NSString::from_str("SagaScript private meeting tap")),
            ),
            ("private", any(NSNumber::new_bool(true))),
            ("taps", any(NSArray::from_retained_slice(&[tap_desc]))),
        ]);
        // A tap-only aggregate cannot include headset microphone inputs. No physical subdevice.
        let result = unsafe {
            AudioHardwareCreateAggregateDevice(
                Retained::as_ptr(&aggregate_description).cast(),
                &mut owned.aggregate,
            )
        };
        if result != 0 {
            return Err(format!("System audio aggregate failed ({result})"));
        }
        let context = Box::new(Context {
            closed: AtomicBool::new(false),
            readers: AtomicUsize::new(0),
            sink: UnsafeCell::new(Some(sink)),
            channels: format.channels,
            planar: format.flags & 32 != 0,
            rate: format.rate as u32,
        });
        owned.context = Box::into_raw(context);
        let result = unsafe {
            AudioDeviceCreateIOProcID(
                owned.aggregate,
                callback,
                owned.context.cast(),
                &mut owned.proc,
            )
        };
        if result != 0 {
            return Err(format!(
                "Registering system audio callback failed ({result})"
            ));
        }
        // A successfully created IOProc can still have no capture permission.
        // Reapplying this tap's own description exercises the permission boundary.
        let tap_description = Retained::as_ptr(&description);
        let address = Address {
            selector: cc(b"tdsc"),
            scope: cc(b"glob"),
            element: 0,
        };
        let permission = unsafe {
            AudioObjectSetPropertyData(
                owned.tap,
                &address,
                0,
                ptr::null(),
                std::mem::size_of_val(&tap_description) as u32,
                (&tap_description as *const *const AnyObject).cast(),
            )
        };
        if permission != 0 {
            return Err(format!("System Audio Recording permission is unavailable ({permission}). Enable it in System Settings, then start again."));
        }
        let result = unsafe { AudioDeviceStart(owned.aggregate, owned.proc) };
        if result != 0 {
            return Err(format!("Starting system audio failed ({result})"));
        }
        Ok(owned)
    }
}
impl Drop for Tap {
    fn drop(&mut self) {
        // SAFETY: reverse construction order. Context remains a small inert tombstone:
        // SDK provides no callback quiescence guarantee. It never retains PCM or a sender.
        unsafe {
            if !self.context.is_null() {
                (*self.context).closed.store(true, Ordering::SeqCst);
            }
            if !self.proc.is_null() {
                let stopped = AudioDeviceStop(self.aggregate, self.proc);
                let removed = AudioDeviceDestroyIOProcID(self.aggregate, self.proc);
                if stopped != 0 || removed != 0 {
                    tracing::warn!(stopped, removed, "Live tap stop/unregister failed");
                }
            }
            if self.aggregate != 0 {
                let status = AudioHardwareDestroyAggregateDevice(self.aggregate);
                if status != 0 {
                    tracing::warn!(status, "Live aggregate cleanup failed");
                }
            }
            if self.tap != 0 {
                let status = (self.destroy)(self.tap);
                if status != 0 {
                    tracing::warn!(status, "Live process tap cleanup failed");
                }
            }
            if !self.context.is_null() {
                while (*self.context).readers.load(Ordering::SeqCst) != 0 {
                    std::thread::yield_now();
                }
                drop((*(*self.context).sink.get()).take());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::live::{capture::Health, document::Source};
    use std::sync::{mpsc, Arc};

    #[test]
    fn other_callback_layouts_match_sdk_bindings() {
        assert_eq!(
            std::mem::size_of::<Address>(),
            std::mem::size_of::<coreaudio_sys::AudioObjectPropertyAddress>()
        );
        assert_eq!(
            std::mem::offset_of!(Address, element),
            std::mem::offset_of!(coreaudio_sys::AudioObjectPropertyAddress, mElement)
        );
        assert_eq!(
            std::mem::size_of::<Format>(),
            std::mem::size_of::<coreaudio_sys::AudioStreamBasicDescription>()
        );
        assert_eq!(
            std::mem::offset_of!(Format, channels),
            std::mem::offset_of!(
                coreaudio_sys::AudioStreamBasicDescription,
                mChannelsPerFrame
            )
        );
        assert_eq!(
            std::mem::size_of::<Buffer>(),
            std::mem::size_of::<coreaudio_sys::AudioBuffer>()
        );
        assert_eq!(
            std::mem::offset_of!(Buffer, data),
            std::mem::offset_of!(coreaudio_sys::AudioBuffer, mData)
        );
        assert_eq!(
            std::mem::size_of::<BufferList>(),
            std::mem::size_of::<coreaudio_sys::AudioBufferList>()
        );
        assert_eq!(
            std::mem::offset_of!(BufferList, buffers),
            std::mem::offset_of!(coreaudio_sys::AudioBufferList, mBuffers)
        );
    }

    #[test]
    fn sdk_timestamp_layout_accepts_host_time_from_callback() {
        // CoreAudioBaseTypes.h: sizeof(AudioTimeStamp)=64, mHostTime=8,
        // mSMPTETime=32 (24 bytes), mFlags=56. Use the SDK byte layout,
        // independent of the Rust struct, to catch field-offset regressions.
        assert_eq!(std::mem::size_of::<Timestamp>(), 64);
        assert_eq!(std::mem::offset_of!(Timestamp, mHostTime), 8);
        assert_eq!(std::mem::offset_of!(Timestamp, mSMPTETime), 32);
        assert_eq!(std::mem::offset_of!(Timestamp, mFlags), 56);
        let mut timestamp = [0u64; 8];
        timestamp[1] = 1_000_000;
        timestamp[7] = 2; // kAudioTimeStampHostTimeValid at byte 56 (little endian).
        let (sender, receiver) = mpsc::sync_channel(1);
        let health = Arc::new(Health::default());
        let sink = Sink::new(Source::System, sender, health.clone());
        let context = Context {
            closed: AtomicBool::new(false),
            readers: AtomicUsize::new(0),
            sink: UnsafeCell::new(None),
            channels: 1,
            planar: false,
            rate: 16_000,
        };
        let samples = [0.25f32; 160];
        let buffers = BufferList {
            count: 1,
            buffers: [Buffer {
                channels: 1,
                bytes: 640,
                data: samples.as_ptr(),
            }],
        };
        copy_input(&context, &sink, &buffers, timestamp.as_ptr().cast());
        assert!(!health.failed.load(Ordering::Acquire));
        let block = receiver
            .try_recv()
            .expect("valid SDK timestamp must deliver audio");
        assert_eq!(block.host_ns, to_ns(1_000_000));
        assert_eq!(block.len, 160);
        assert_eq!(block.samples[0], 0.25);
    }
}
