//! Source-picture geometry from complete coded access units.
//!
//! The host's negotiated Mode is the requested presentation mode. A WGC
//! mirror can encode at its existing monitor's native resolution instead.
//! This narrow parser reads only AV1 sequence headers and H.264 SPS NALs;
//! it does not modify the encoded AU or interpret video colour.

struct Bits<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(bytes: &'a [u8]) -> Self { Self { bytes, pos: 0 } }

    fn get(&mut self, n: usize) -> Option<u32> {
        if n > 32 || self.pos.checked_add(n)? > self.bytes.len().checked_mul(8)? {
            return None;
        }
        let mut value = 0u32;
        for _ in 0..n {
            let byte = self.bytes[self.pos / 8];
            value = (value << 1) | u32::from((byte >> (7 - self.pos % 8)) & 1);
            self.pos += 1;
        }
        Some(value)
    }

    fn ue(&mut self) -> Option<u32> {
        let mut zeroes = 0usize;
        while self.get(1)? == 0 {
            zeroes += 1;
            if zeroes > 30 { return None; }
        }
        Some((1u32 << zeroes).checked_sub(1)?.checked_add(self.get(zeroes)?)?)
    }

    fn se(&mut self) -> Option<i32> {
        let v = self.ue()?;
        let magnitude = i32::try_from(v.div_ceil(2)).ok()?;
        Some(if v & 1 != 0 { magnitude } else { -magnitude })
    }

    fn uvlc(&mut self) -> Option<u32> {
        let mut leading_zeroes = 0usize;
        while self.get(1)? == 0 {
            leading_zeroes += 1;
            if leading_zeroes >= 32 { return Some(u32::MAX); }
        }
        Some(((1u64 << leading_zeroes) - 1 + u64::from(self.get(leading_zeroes)?)) as u32)
    }
}

fn leb128(data: &[u8], at: &mut usize) -> Option<usize> {
    let mut result = 0usize;
    for shift in (0..56).step_by(7) {
        let value = *data.get(*at)?;
        *at += 1;
        result = result.checked_add(usize::from(value & 0x7f).checked_shl(shift)?)?;
        if value & 0x80 == 0 { return Some(result); }
    }
    None
}

fn av1_sequence(payload: &[u8]) -> Option<(u32, u32)> {
    let mut r = Bits::new(payload);
    r.get(3)?; // seq_profile
    r.get(1)?; // still_picture
    let reduced = r.get(1)? != 0;
    if reduced {
        r.get(5)?; // seq_level_idx[0]
    } else {
        let mut delay_length = 0usize;
        let timing = r.get(1)? != 0;
        let mut decoder_model = false;
        if timing {
            r.get(32)?; // num_units_in_display_tick
            r.get(32)?; // time_scale
            if r.get(1)? != 0 { r.uvlc()?; }
            decoder_model = r.get(1)? != 0;
            if decoder_model {
                delay_length = r.get(5)? as usize + 1;
                r.get(32)?; // num_units_in_decoding_tick
                r.get(5)?; // buffer_removal_time_length_minus_1
                r.get(5)?; // frame_presentation_time_length_minus_1
            }
        }
        let initial_delay = r.get(1)? != 0;
        let operating_points = r.get(5)? as usize + 1;
        for _ in 0..operating_points {
            r.get(12)?; // operating_point_idc
            let level = r.get(5)?;
            if level > 7 { r.get(1)?; } // seq_tier
            if decoder_model && r.get(1)? != 0 {
                r.get(delay_length)?; // decoder_buffer_delay
                r.get(delay_length)?; // encoder_buffer_delay
                r.get(1)?; // low_delay_mode_flag
            }
            if initial_delay && r.get(1)? != 0 { r.get(4)?; }
        }
    }
    let width_bits = r.get(4)? as usize + 1;
    let height_bits = r.get(4)? as usize + 1;
    let width = r.get(width_bits)?.checked_add(1)?;
    let height = r.get(height_bits)?.checked_add(1)?;
    valid_size(width, height)
}

fn valid_size(w: u32, h: u32) -> Option<(u32, u32)> {
    if w >= 320 && h >= 200 && w <= 3840 && h <= 2160 && w % 2 == 0 && h % 2 == 0 {
        Some((w, h))
    } else { None }
}

/// Width and height advertised by the first AV1 sequence header in this AU.
/// Returns None if an AU has no complete, safely parsed sequence header.
pub(super) fn av1_dimensions(au: &[u8]) -> Option<(u32, u32)> {
    let mut at = 0usize;
    while at < au.len() {
        let header = *au.get(at)?;
        at += 1;
        if header & 0x81 != 0 || header & 0x02 == 0 { return None; }
        let kind = (header >> 3) & 0x0f;
        if header & 0x04 != 0 { at = at.checked_add(1)?; }
        let size = leb128(au, &mut at)?;
        let end = at.checked_add(size)?;
        let payload = au.get(at..end)?;
        if kind == 1 { return av1_sequence(payload); }
        at = end;
    }
    None
}

fn start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            if data[i + 2] == 1 { return Some((i, 3)); }
            if i + 4 <= data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                return Some((i, 4));
            }
        }
        i += 1;
    }
    None
}

fn h264_sps(nal: &[u8]) -> Option<(u32, u32)> {
    if nal.first()? & 0x1f != 7 { return None; }
    let mut rbsp = Vec::with_capacity(nal.len());
    let mut zeroes = 0usize;
    for &b in &nal[1..] {
        if zeroes >= 2 && b == 3 {
            zeroes = 0;
            continue; // emulation-prevention byte
        }
        rbsp.push(b);
        zeroes = if b == 0 { zeroes + 1 } else { 0 };
    }
    let mut r = Bits::new(&rbsp);
    let profile = r.get(8)? as u8;
    r.get(8)?; // constraint flags
    r.get(8)?; // level_idc
    r.ue()?; // seq_parameter_set_id
    let mut chroma_format_idc = 1u32;
    if matches!(profile, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
        chroma_format_idc = r.ue()?;
        if chroma_format_idc > 3 { return None; }
        if chroma_format_idc == 3 { r.get(1)?; } // separate_colour_plane_flag
        r.ue()?; // bit_depth_luma_minus8
        r.ue()?; // bit_depth_chroma_minus8
        r.get(1)?; // qpprime_y_zero_transform_bypass_flag
        if r.get(1)? != 0 { // seq_scaling_matrix_present_flag
            let n = if chroma_format_idc == 3 { 12 } else { 8 };
            for i in 0..n {
                if r.get(1)? == 0 { continue; }
                let mut next = 8i32;
                let mut last = 8i32;
                for _ in 0..(if i < 6 { 16 } else { 64 }) {
                    if next != 0 { next = (last + r.se()? + 256) % 256; }
                    last = if next == 0 { last } else { next };
                }
            }
        }
    }
    r.ue()?; // log2_max_frame_num_minus4
    let order = r.ue()?;
    if order == 0 {
        r.ue()?; // log2_max_pic_order_cnt_lsb_minus4
    } else if order == 1 {
        r.get(1)?;
        r.se()?;
        r.se()?;
        let cycle = usize::try_from(r.ue()?).ok()?;
        if cycle > 256 { return None; }
        for _ in 0..cycle { r.se()?; }
    } else if order != 2 { return None; }
    r.ue()?; // max_num_ref_frames
    r.get(1)?; // gaps_in_frame_num_value_allowed_flag
    let mb_width = r.ue()?.checked_add(1)?;
    let map_height = r.ue()?.checked_add(1)?;
    let frame_only = r.get(1)? != 0;
    if !frame_only { r.get(1)?; } // mb_adaptive_frame_field_flag
    r.get(1)?; // direct_8x8_inference_flag
    let (left, right, top, bottom) = if r.get(1)? != 0 {
        (r.ue()?, r.ue()?, r.ue()?, r.ue()?)
    } else { (0, 0, 0, 0) };
    let frame_factor = if frame_only { 1 } else { 2 };
    let (sub_w, sub_h) = match chroma_format_idc {
        0 | 3 => (1u32, 1u32),
        1 => (2, 2),
        2 => (2, 1),
        _ => return None,
    };
    let width = mb_width.checked_mul(16)?
        .checked_sub(sub_w.checked_mul(left.checked_add(right)?)?)?;
    let height = map_height.checked_mul(16)?.checked_mul(frame_factor)?
        .checked_sub(sub_h.checked_mul(frame_factor)?.checked_mul(top.checked_add(bottom)?)?)?;
    valid_size(width, height)
}

/// Annex-B H.264 SPS dimensions, using its conformance-window crop.
/// The bootstrap parses cached SPS directly; keep this scan only for tests.
#[cfg(test)]
pub(super) fn h264_dimensions(au: &[u8]) -> Option<(u32, u32)> {
    let mut from = 0usize;
    while let Some((start, prefix)) = start_code(au, from) {
        let nal_start = start + prefix;
        let end = start_code(au, nal_start).map_or(au.len(), |p| p.0);
        if (*au.get(nal_start)? & 0x1f) == 7 {
            return h264_sps(au.get(nal_start..end)?);
        }
        from = end;
    }
    None
}

/// H.264 Annex-B bootstrap state. Host encoders can emit SPS, PPS and IDR
/// in separate complete access units; libounce needs SPS+PPS before Create.
/// Retain only bounded parameter sets, never entire compressed frames.
#[derive(Default)]
pub(super) struct H264Bootstrap {
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    active: Option<Vec<u8>>,
}

pub(super) enum H264Decision {
    Continue,
    NeedKeyframe,
    Reopen { config: Vec<u8>, dimensions: Option<(u32, u32)> },
}

impl H264Bootstrap {
    const MAX_CONFIG_NAL: usize = 2048;

    /// Every call sees one *complete* Annex-B AU. An SPS/PPS change is
    /// activated only on IDR so the new decoder never begins on a P-frame.
    pub(super) fn observe(&mut self, au: &[u8], decoder_open: bool) -> H264Decision {
        let mut from = 0usize;
        let mut has_idr = false;
        while let Some((start, prefix)) = start_code(au, from) {
            let begin = start + prefix;
            let end = start_code(au, begin).map_or(au.len(), |p| p.0);
            if let Some(nal) = au.get(begin..end).filter(|nal| !nal.is_empty()) {
                let kind = nal[0] & 0x1f;
                if kind == 5 { has_idr = true; }
                if (kind == 7 || kind == 8) && nal.len() <= Self::MAX_CONFIG_NAL {
                    let slot = if kind == 7 { &mut self.sps } else { &mut self.pps };
                    if slot.as_deref() != Some(nal) { *slot = Some(nal.to_vec()); }
                }
            }
            from = end;
        }
        let (Some(sps), Some(pps)) = (&self.sps, &self.pps) else {
            return H264Decision::NeedKeyframe;
        };
        let mut config = Vec::with_capacity(sps.len() + pps.len() + 8);
        config.extend_from_slice(&[0, 0, 0, 1]);
        config.extend_from_slice(sps);
        config.extend_from_slice(&[0, 0, 0, 1]);
        config.extend_from_slice(pps);
        if decoder_open && self.active.as_deref() == Some(config.as_slice()) {
            return H264Decision::Continue;
        }
        if !has_idr { return H264Decision::NeedKeyframe; }
        H264Decision::Reopen { dimensions: h264_sps(sps), config }
    }

    /// Commit only after the hardware decoder actually opens successfully.
    pub(super) fn activated(&mut self, config: Vec<u8>) { self.active = Some(config); }

    /// Flush invalidates reference pictures. Keep the parameter sets, but
    /// require the next IDR to reanchor the decoder before decoding deltas.
    pub(super) fn invalidate(&mut self) { self.active = None; }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_false_av1_sequence_on_mid_gop_frame() {
        let mut au = vec![0x12, 0, 0x32, 0xf0, 0xea, 0x02];
        au.resize(46_454, 0);
        assert_eq!(av1_dimensions(&au), None);
    }

    #[test]
    fn h264_bootstrap_collects_separate_sps_pps_and_starts_on_idr() {
        let mut init = H264Bootstrap::default();
        let sps = [0, 0, 0, 1, 0x67, 0x42, 0, 0x1e];
        let pps = [0, 0, 1, 0x68, 0xcc, 0x10];
        let idr = [0, 0, 1, 0x65, 0x80];
        assert!(matches!(init.observe(&sps, false), H264Decision::NeedKeyframe));
        assert!(matches!(init.observe(&pps, false), H264Decision::NeedKeyframe));
        assert!(matches!(init.observe(&[0, 0, 1, 0x41, 0x80], false), H264Decision::NeedKeyframe));
        let H264Decision::Reopen { config, .. } = init.observe(&idr, false) else {
            panic!("an IDR after cached SPS/PPS must open the decoder")
        };
        assert_eq!(config.as_slice(), &[0, 0, 0, 1, 0x67, 0x42, 0, 0x1e,
                            0, 0, 0, 1, 0x68, 0xcc, 0x10]);
        init.activated(config);
        assert!(matches!(init.observe(&[0, 0, 1, 0x41, 0x80], true), H264Decision::Continue));
        init.invalidate();
        assert!(matches!(init.observe(&[0, 0, 1, 0x41, 0x80], true), H264Decision::NeedKeyframe));
        assert!(matches!(init.observe(&idr, true), H264Decision::Reopen { .. }));
    }

    #[test]
    fn h264_parameter_change_does_not_reopen_on_inter_frame() {
        let mut init = H264Bootstrap::default();
        let old = [0, 0, 1, 0x67, 0x42, 0, 0x1e,
                   0, 0, 1, 0x68, 0xcc, 0x10, 0, 0, 1, 0x65, 0x80];
        let H264Decision::Reopen { config, .. } = init.observe(&old, false) else { panic!() };
        init.activated(config);
        let new_sps = [0, 0, 1, 0x67, 0x42, 0, 0x1f];
        assert!(matches!(init.observe(&new_sps, true), H264Decision::NeedKeyframe));
        assert!(matches!(init.observe(&[0, 0, 1, 0x41, 0x80], true), H264Decision::NeedKeyframe));
        assert!(matches!(init.observe(&[0, 0, 1, 0x65, 0x80], true), H264Decision::Reopen { .. }));
    }

    #[test]
    fn h264_bootstrap_caps_parameter_memory() {
        let mut init = H264Bootstrap::default();
        let mut huge_sps = vec![0, 0, 1, 0x67];
        huge_sps.resize(H264Bootstrap::MAX_CONFIG_NAL + 4, 0x55);
        assert!(matches!(init.observe(&huge_sps, false), H264Decision::NeedKeyframe));
        assert!(init.sps.is_none());
    }

    #[test]
    fn invalid_inputs_are_rejected() {
        for au in [&[][..], &[0x0a, 0x80][..], &[0x0a, 5, 0][..]] {
            assert_eq!(av1_dimensions(au), None);
        }
        assert_eq!(h264_dimensions(&[0, 0, 1, 0x41, 0, 0]), None);
    }
}
