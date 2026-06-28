/// D8+D9: RTL-SDR FFI binding with ring buffer decoupling
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// FFI to librtlsdr
#[allow(non_camel_case_types)]
type rtlsdr_dev_t = c_void;
type ReadAsyncCb = unsafe extern "C" fn(buf: *mut u8, len: u32, ctx: *mut c_void);

extern "C" {
    fn rtlsdr_get_device_count() -> u32;
    fn rtlsdr_open(dev: *mut *mut rtlsdr_dev_t, index: u32) -> i32;
    fn rtlsdr_close(dev: *mut rtlsdr_dev_t) -> i32;
    fn rtlsdr_set_center_freq(dev: *mut rtlsdr_dev_t, freq: u32) -> i32;
    fn rtlsdr_set_sample_rate(dev: *mut rtlsdr_dev_t, rate: u32) -> i32;
    fn rtlsdr_set_tuner_gain_mode(dev: *mut rtlsdr_dev_t, manual: i32) -> i32;
    fn rtlsdr_set_tuner_gain(dev: *mut rtlsdr_dev_t, gain: i32) -> i32;
    fn rtlsdr_set_freq_correction(dev: *mut rtlsdr_dev_t, ppm: i32) -> i32;
    fn rtlsdr_reset_buffer(dev: *mut rtlsdr_dev_t) -> i32;
    fn rtlsdr_read_async(
        dev: *mut rtlsdr_dev_t,
        cb: ReadAsyncCb,
        ctx: *mut c_void,
        buf_num: u32,
        buf_len: u32,
    ) -> i32;
    fn rtlsdr_cancel_async(dev: *mut rtlsdr_dev_t) -> i32;
    fn rtlsdr_set_agc_mode(dev: *mut rtlsdr_dev_t, on: i32) -> i32;
}

pub struct RtlSdr {
    dev: *mut rtlsdr_dev_t,
}

unsafe impl Send for RtlSdr {}

impl RtlSdr {
    pub fn open(index: u32) -> Result<Self, String> {
        let count = unsafe { rtlsdr_get_device_count() };
        if count == 0 {
            return Err("No RTL-SDR devices found".into());
        }
        if index >= count {
            return Err(format!("Device index {} not found ({} devices)", index, count));
        }
        let mut dev: *mut rtlsdr_dev_t = std::ptr::null_mut();
        let ret = unsafe { rtlsdr_open(&mut dev, index) };
        if ret != 0 {
            return Err(format!("rtlsdr_open failed: {}", ret));
        }
        Ok(Self { dev })
    }

    pub fn configure(&self, freq: u32, sample_rate: u32, gain: i32, ppm: i32) {
        unsafe {
            rtlsdr_set_center_freq(self.dev, freq);
            rtlsdr_set_sample_rate(self.dev, sample_rate);
            if gain == 0 {
                // AGC mode
                rtlsdr_set_tuner_gain_mode(self.dev, 0);
                rtlsdr_set_agc_mode(self.dev, 1);
            } else {
                rtlsdr_set_tuner_gain_mode(self.dev, 1);
                rtlsdr_set_tuner_gain(self.dev, gain);
            }
            if ppm != 0 {
                rtlsdr_set_freq_correction(self.dev, ppm);
            }
            rtlsdr_reset_buffer(self.dev);
        }
    }

    /// Start async reading. Calls `callback` for each USB chunk.
    /// Blocks the calling thread until cancel_async is called.
    pub fn read_async(
        &self,
        tx: crossbeam_channel::Sender<Vec<u8>>,
        running: Arc<AtomicBool>,
    ) {
        struct Ctx {
            tx: crossbeam_channel::Sender<Vec<u8>>,
            running: Arc<AtomicBool>,
        }

        unsafe extern "C" fn cb(buf: *mut u8, len: u32, ctx: *mut c_void) {
            let ctx = &*(ctx as *const Ctx);
            if !ctx.running.load(Ordering::Relaxed) { return; }
            let slice = std::slice::from_raw_parts(buf, len as usize);
            let _ = ctx.tx.try_send(slice.to_vec());
        }

        let ctx = Box::new(Ctx { tx, running });
        let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

        unsafe {
            // 12 buffers of 256KB each
            rtlsdr_read_async(self.dev, cb, ctx_ptr, 12, 256 * 1024);
            // Cleanup
            let _ = Box::from_raw(ctx_ptr as *mut Ctx);
        }
    }

    pub fn cancel(&self) {
        unsafe { rtlsdr_cancel_async(self.dev); }
    }
}

impl Drop for RtlSdr {
    fn drop(&mut self) {
        unsafe { rtlsdr_close(self.dev); }
    }
}
