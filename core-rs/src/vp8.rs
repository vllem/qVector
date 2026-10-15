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
    #[cfg(feature = "simd")]
    return i420_to_rgba_simd(y, ys, u, v, cs, w, h);
    #[cfg(not(feature = "simd"))]
    i420_to_rgba_scalar(y, ys, u, v, cs, w, h)
}

/// One pixel of `i420_to_rgba`: red, green, blue.
#[inline]
fn yuv_pixel(y: u8, u: u8, v: u8) -> [u8; 3] {
    let yy = (y as i32 - 16).max(0) * 298;
    let (uu, vv) = (u as i32 - 128, v as i32 - 128);
    [((yy + 409 * vv + 128) >> 8).clamp(0, 255) as u8, ((yy - 100 * uu - 208 * vv + 128) >> 8).clamp(0, 255) as u8, ((yy + 516 * uu + 128) >> 8).clamp(0, 255) as u8]
}

#[cfg_attr(feature = "simd", allow(dead_code))] /* the reference the simd version is tested against */
fn i420_to_rgba_scalar(y: &[u8], ys: usize, u: &[u8], v: &[u8], cs: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![255u8; w * h * 4];
    for row in 0..h {
        for col in 0..w {
            let c = (row / 2) * cs + col / 2;
            let i = (row * w + col) * 4;
            out[i..i + 3].copy_from_slice(&yuv_pixel(y[row * ys + col], u[c], v[c]));
        }
    }
    out
}

/// `i420_to_rgba` eight pixels at a time (the same integer arithmetic, so the same bytes); the last `w % 8` of a row go one by one.
#[cfg(feature = "simd")]
fn i420_to_rgba_simd(y: &[u8], ys: usize, u: &[u8], v: &[u8], cs: usize, w: usize, h: usize) -> Vec<u8> {
    use std::simd::{prelude::*, simd_swizzle, ToBytes};
    let k = i32x8::splat;
    let channel = |x: i32x8| ((x + k(128)) >> k(8)).simd_clamp(k(0), k(255)).cast::<u32>();
    let mut out = vec![255u8; w * h * 4];
    let whole = w - w % 8;
    for row in 0..h {
        let (yr, cr) = (&y[row * ys..row * ys + w], (row / 2) * cs);
        let line = &mut out[row * w * 4..(row + 1) * w * 4];
        for col in (0..whole).step_by(8) {
            let yy = (u8x8::from_slice(&yr[col..]).cast::<i32>() - k(16)).simd_max(k(0)) * k(298);
            let chroma = |p: &[u8]| { let four = u8x4::from_slice(&p[cr + col / 2..]).cast::<i32>() - i32x4::splat(128); simd_swizzle!(four, [0, 0, 1, 1, 2, 2, 3, 3]) }; /* one sample for two pixels */
            let (uu, vv) = (chroma(u), chroma(v));
            let (r, g, b) = (channel(yy + k(409) * vv), channel(yy - k(100) * uu - k(208) * vv), channel(yy + k(516) * uu));
            let px = r | (g << u32x8::splat(8)) | (b << u32x8::splat(16)) | u32x8::splat(0xff00_0000);
            px.to_le_bytes().copy_to_slice(&mut line[col * 4..col * 4 + 32]);
        }
        for col in whole..w {
            let c = cr + col / 2;
            line[col * 4..col * 4 + 3].copy_from_slice(&yuv_pixel(yr[col], u[c], v[c]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Planes of `w` x `h` with rows `pad` bytes longer than the picture (as decoders hand them out), filled from a seed.
    fn planes(w: usize, h: usize, pad: usize, seed: u32) -> (Vec<u8>, usize, Vec<u8>, Vec<u8>, usize) {
        let mut s = seed;
        let mut next = || { s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (s >> 24) as u8 };
        let (ys, cs) = (w + pad, w.div_ceil(2) + pad);
        let ch = h.div_ceil(2);
        ((0..ys * h).map(|_| next()).collect(), ys, (0..cs * ch).map(|_| next()).collect(), (0..cs * ch).map(|_| next()).collect(), cs)
    }

    #[test]
    fn video_yuv_becomes_the_colours_it_stands_for() {
        let px = |y, u, v| { let o = i420_to_rgba(&[y; 4], 2, &[u], &[v], 1, 2, 2); assert!(o.chunks(4).all(|p| p == &o[..4])); [o[0], o[1], o[2], o[3]] };
        assert_eq!(px(16, 128, 128), [0, 0, 0, 255], "black");
        assert_eq!(px(235, 128, 128), [255, 255, 255, 255], "white");
        assert_eq!(px(81, 90, 240), [255, 0, 0, 255], "red");
        assert_eq!(px(0, 0, 0)[3], 255);
        let (y, ys, u, v, cs) = planes(37, 11, 5, 1);
        let out = i420_to_rgba(&y, ys, &u, &v, cs, 37, 11);
        assert_eq!(out.len(), 37 * 11 * 4);
        let (row, col) = (9, 36); /* the last pixel of an odd-sized picture takes the last chroma sample */
        assert_eq!(&out[(row * 37 + col) * 4..][..3], &yuv_pixel(y[row * ys + col], u[(row / 2) * cs + col / 2], v[(row / 2) * cs + col / 2]));
    }

    #[cfg(feature = "simd")]
    #[test]
    fn the_simd_yuv_conversion_gives_the_same_bytes_as_the_plain_one() {
        for (n, &(w, h, pad)) in [(8, 2, 0), (16, 16, 0), (17, 9, 3), (960, 540, 32), (641, 361, 7), (7, 5, 1)].iter().enumerate() {
            let (y, ys, u, v, cs) = planes(w, h, pad, n as u32 + 7);
            assert_eq!(i420_to_rgba_simd(&y, ys, &u, &v, cs, w, h), i420_to_rgba_scalar(&y, ys, &u, &v, cs, w, h), "{w}x{h}, rows {pad} longer");
        }
    }
}
