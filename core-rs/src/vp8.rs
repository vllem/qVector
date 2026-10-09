//! VP8 through libvpx: the codec every WebRTC client offers, and royalty-free. Raw bindings (`env-libvpx-sys`), I420 pictures.

use std::os::raw::{c_int, c_long, c_ulong};

use vpx_sys as vpx;

/// A contiguous I420 picture (`w * h` luma, then two quarter-size chroma planes), `w` and `h` even.
pub struct I420 {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

fn check(code: vpx::vpx_codec_err_t, what: &str) -> Result<(), String> {
    if code == vpx::vpx_codec_err_t::VPX_CODEC_OK {
        Ok(())
    } else {
        Err(format!("libvpx: {what} failed ({code:?})"))
    }
}

pub struct Vp8Encoder {
    ctx: vpx::vpx_codec_ctx_t,
    frames: i64,
}

// The context is only ever used from the thread that owns the encoder.
unsafe impl Send for Vp8Encoder {}

impl Vp8Encoder {
    pub fn new(w: usize, h: usize, fps: u32, bitrate_kbps: u32, key_every: u32) -> Result<Vp8Encoder, String> {
        unsafe {
            let iface = vpx::vpx_codec_vp8_cx();
            let mut cfg = std::mem::MaybeUninit::<vpx::vpx_codec_enc_cfg_t>::uninit();
            check(vpx::vpx_codec_enc_config_default(iface, cfg.as_mut_ptr(), 0), "default config")?;
            let mut cfg = cfg.assume_init();
            cfg.g_w = w as u32;
            cfg.g_h = h as u32;
            cfg.g_timebase = vpx::vpx_rational { num: 1, den: fps as c_int };
            cfg.g_error_resilient = 1;
            cfg.g_lag_in_frames = 0;
            cfg.g_threads = 2;
            cfg.rc_end_usage = vpx::vpx_rc_mode::VPX_CBR;
            cfg.rc_target_bitrate = bitrate_kbps;
            cfg.kf_mode = vpx::vpx_kf_mode::VPX_KF_AUTO;
            cfg.kf_max_dist = key_every;
            let mut ctx: vpx::vpx_codec_ctx_t = std::mem::zeroed();
            check(vpx::vpx_codec_enc_init_ver(&mut ctx, iface, &cfg, 0, vpx::VPX_ENCODER_ABI_VERSION as c_int), "encoder init")?;
            let enc = Vp8Encoder { ctx, frames: 0 };
            let mut enc = enc;
            // Real-time speed setting; a failure here only costs quality, so it is ignored.
            let _ = vpx::vpx_codec_control_(&mut enc.ctx, vpx::vp8e_enc_control_id::VP8E_SET_CPUUSED as c_int, -6 as c_int);
            Ok(enc)
        }
    }

    /// One picture in, one frame out (empty when the encoder dropped it).
    pub fn encode(&mut self, pic: &I420, key: bool) -> Result<Vec<u8>, String> {
        unsafe {
            let mut img: vpx::vpx_image_t = std::mem::zeroed();
            let ok = vpx::vpx_img_wrap(&mut img, vpx::vpx_img_fmt::VPX_IMG_FMT_I420, pic.w as u32, pic.h as u32, 1, pic.data.as_ptr() as *mut u8);
            if ok.is_null() {
                return Err("libvpx: cannot wrap the picture".into());
            }
            let flags = if key { vpx::VPX_EFLAG_FORCE_KF as c_long } else { 0 };
            check(vpx::vpx_codec_encode(&mut self.ctx, &img, self.frames, 1, flags, vpx::VPX_DL_REALTIME as c_ulong), "encode")?;
            self.frames += 1;
            let mut out = Vec::new();
            let mut iter: vpx::vpx_codec_iter_t = std::ptr::null();
            loop {
                let pkt = vpx::vpx_codec_get_cx_data(&mut self.ctx, &mut iter);
                if pkt.is_null() {
                    break;
                }
                if (*pkt).kind == vpx::vpx_codec_cx_pkt_kind::VPX_CODEC_CX_FRAME_PKT {
                    let f = (*pkt).data.frame;
                    out.extend_from_slice(std::slice::from_raw_parts(f.buf as *const u8, f.sz));
                }
            }
            Ok(out)
        }
    }
}

impl Drop for Vp8Encoder {
    fn drop(&mut self) {
        unsafe {
            vpx::vpx_codec_destroy(&mut self.ctx);
        }
    }
}

pub struct Vp8Decoder {
    ctx: vpx::vpx_codec_ctx_t,
}

unsafe impl Send for Vp8Decoder {}

impl Vp8Decoder {
    pub fn new() -> Result<Vp8Decoder, String> {
        unsafe {
            let cfg = vpx::vpx_codec_dec_cfg { threads: 2, w: 0, h: 0 };
            let mut ctx: vpx::vpx_codec_ctx_t = std::mem::zeroed();
            check(vpx::vpx_codec_dec_init_ver(&mut ctx, vpx::vpx_codec_vp8_dx(), &cfg, 0, vpx::VPX_DECODER_ABI_VERSION as c_int), "decoder init")?;
            Ok(Vp8Decoder { ctx })
        }
    }

    /// One frame in; the picture out as RGBA (`None` while the decoder has nothing to show, e.g. before a key frame).
    pub fn decode(&mut self, frame: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
        unsafe {
            if vpx::vpx_codec_decode(&mut self.ctx, frame.as_ptr(), frame.len() as u32, std::ptr::null_mut(), 0) != vpx::vpx_codec_err_t::VPX_CODEC_OK {
                return None;
            }
            let mut iter: vpx::vpx_codec_iter_t = std::ptr::null();
            let img = vpx::vpx_codec_get_frame(&mut self.ctx, &mut iter);
            if img.is_null() {
                return None;
            }
            let img = &*img;
            let (w, h) = (img.d_w as usize, img.d_h as usize);
            let plane = |i: usize, rows: usize| std::slice::from_raw_parts(img.planes[i], img.stride[i] as usize * rows);
            Some((w as u32, h as u32, i420_to_rgba(plane(0, h), img.stride[0] as usize, plane(1, h.div_ceil(2)), plane(2, h.div_ceil(2)), img.stride[1] as usize, w, h)))
        }
    }
}

impl Drop for Vp8Decoder {
    fn drop(&mut self) {
        unsafe {
            vpx::vpx_codec_destroy(&mut self.ctx);
        }
    }
}

/// BT.601 limited-range YUV to RGBA, the conversion cameras and browsers use for video this size.
pub fn i420_to_rgba(y: &[u8], ys: usize, u: &[u8], v: &[u8], cs: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![255u8; w * h * 4];
    for row in 0..h {
        for col in 0..w {
            let yy = (y[row * ys + col] as i32 - 16).max(0) * 298;
            let uu = u[(row / 2) * cs + col / 2] as i32 - 128;
            let vv = v[(row / 2) * cs + col / 2] as i32 - 128;
            let i = (row * w + col) * 4;
            out[i] = ((yy + 409 * vv + 128) >> 8).clamp(0, 255) as u8;
            out[i + 1] = ((yy - 100 * uu - 208 * vv + 128) >> 8).clamp(0, 255) as u8;
            out[i + 2] = ((yy + 516 * uu + 128) >> 8).clamp(0, 255) as u8;
        }
    }
    out
}
