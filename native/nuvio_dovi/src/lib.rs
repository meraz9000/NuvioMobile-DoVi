//! On-the-fly Dolby Vision Profile 7 -> Profile 8.1 conversion for HEVC Annex-B access units.
//!
//! This mirrors what Kodi does for Android: per access unit, the Dolby Vision enhancement layer
//! (NAL type 63, UNSPEC63) is dropped and every RPU (NAL type 62, UNSPEC62) is rewritten with
//! dovi_tool's conversion modes (the `dolby_vision` crate by quietvoid). All other NAL units are
//! copied byte-for-byte.

use dolby_vision::rpu::dovi_rpu::DoviRpu;

pub const NAL_UNSPEC62_RPU: u8 = 62;
pub const NAL_UNSPEC63_EL: u8 = 63;

const START_CODE: [u8; 4] = [0, 0, 0, 1];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub rpus_converted: u32,
    pub rpus_failed: u32,
    pub el_nals_dropped: u32,
    /// Profile of the last RPU seen before conversion (0 when none).
    pub source_profile: u8,
}

/// A NAL unit inside an Annex-B buffer.
/// `region_start..region_end` covers its start code (3 or 4 bytes) and payload, up to the next
/// start code; `payload_start` is the first byte after the start code.
#[derive(Debug, Clone, Copy)]
struct NalRegion {
    region_start: usize,
    payload_start: usize,
    region_end: usize,
}

fn find_start_code(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 3 <= data.len() {
        if data[i + 2] > 1 {
            i += 3;
        } else if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            return Some(i);
        } else {
            i += 1;
        }
    }
    None
}

fn split_nals(data: &[u8]) -> Vec<NalRegion> {
    let mut starts: Vec<(usize, usize)> = Vec::new(); // (region_start, payload_start)
    let mut pos = 0;
    while let Some(sc) = find_start_code(data, pos) {
        // A 4-byte start code's leading zero belongs to the start code, not the previous NAL.
        let region_start = if sc > 0 && data[sc - 1] == 0 { sc - 1 } else { sc };
        starts.push((region_start, sc + 3));
        pos = sc + 3;
    }
    let mut out = Vec::with_capacity(starts.len());
    for (idx, &(region_start, payload_start)) in starts.iter().enumerate() {
        let region_end = starts.get(idx + 1).map(|s| s.0).unwrap_or(data.len());
        out.push(NalRegion { region_start, payload_start, region_end });
    }
    out
}

#[inline]
fn nal_type(data: &[u8], nal: &NalRegion) -> Option<u8> {
    data.get(nal.payload_start).map(|b| (b >> 1) & 0x3F)
}

/// Converts a single UNSPEC62 NAL (header 0x7C01 included, no start code).
/// Returns the new NAL (header included) and the source profile.
pub fn convert_rpu_nal(nal: &[u8], mode: u8) -> Result<(Vec<u8>, u8), String> {
    let mut rpu = DoviRpu::parse_unspec62_nalu(nal).map_err(|e| e.to_string())?;
    let profile = rpu.dovi_profile;
    rpu.convert_with_mode(mode).map_err(|e| e.to_string())?;
    let out = rpu.write_hevc_unspec62_nalu().map_err(|e| e.to_string())?;
    Ok((out, profile))
}

/// Returns the Dolby Vision profile of the first RPU in an Annex-B access unit, if any.
pub fn probe_profile(data: &[u8]) -> Option<u8> {
    for nal in split_nals(data) {
        if nal_type(data, &nal) == Some(NAL_UNSPEC62_RPU) {
            if let Ok(rpu) = DoviRpu::parse_unspec62_nalu(&data[nal.payload_start..nal.region_end]) {
                return Some(rpu.dovi_profile);
            }
        }
    }
    None
}

/// Processes one Annex-B access unit. Returns `None` when nothing had to change.
pub fn process_access_unit(data: &[u8], mode: u8, drop_el: bool) -> (Option<Vec<u8>>, Stats) {
    let mut stats = Stats::default();
    let nals = split_nals(data);
    let touched = nals.iter().any(|n| {
        matches!(nal_type(data, n), Some(NAL_UNSPEC62_RPU))
            || (drop_el && nal_type(data, n) == Some(NAL_UNSPEC63_EL))
    });
    if !touched {
        return (None, stats);
    }

    let mut out = Vec::with_capacity(data.len());
    // Bytes before the first start code (should not exist in Annex-B, but keep them verbatim).
    let first = nals.first().map(|n| n.region_start).unwrap_or(data.len());
    out.extend_from_slice(&data[..first]);

    for nal in &nals {
        match nal_type(data, nal) {
            Some(NAL_UNSPEC63_EL) if drop_el => {
                stats.el_nals_dropped += 1;
            }
            Some(NAL_UNSPEC62_RPU) => {
                match convert_rpu_nal(&data[nal.payload_start..nal.region_end], mode) {
                    Ok((converted, profile)) => {
                        stats.rpus_converted += 1;
                        stats.source_profile = profile;
                        out.extend_from_slice(&START_CODE);
                        out.extend_from_slice(&converted);
                    }
                    Err(_) => {
                        stats.rpus_failed += 1;
                        out.extend_from_slice(&data[nal.region_start..nal.region_end]);
                    }
                }
            }
            _ => out.extend_from_slice(&data[nal.region_start..nal.region_end]),
        }
    }
    (Some(out), stats)
}

// ---------------------------------------------------------------------------------------------
// JNI: com.nuvio.app.features.player.dovi.DoviNative
// ---------------------------------------------------------------------------------------------

mod jni_bridge {
    use super::*;
    use jni::JNIEnv;
    use jni::objects::{JByteArray, JClass, JIntArray};
    use jni::sys::{jboolean, jbyteArray, jint};
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn read_bytes(env: &JNIEnv, array: &JByteArray, length: jint) -> Option<Vec<u8>> {
        let total = env.get_array_length(array).ok()? as usize;
        let len = (length.max(0) as usize).min(total);
        let mut buf = vec![0i8; len];
        env.get_byte_array_region(array, 0, &mut buf).ok()?;
        // i8 -> u8 without copying semantics change.
        Some(buf.into_iter().map(|b| b as u8).collect())
    }

    /// `processAccessUnit(data, length, mode, dropEl, statsOut[4])`: returns the rewritten access
    /// unit, or null when it was left unchanged (or on any error).
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_nuvio_app_features_player_dovi_DoviNative_processAccessUnit<
        'local,
    >(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        data: JByteArray<'local>,
        length: jint,
        mode: jint,
        drop_el: jboolean,
        stats_out: JIntArray<'local>,
    ) -> jbyteArray {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let bytes = read_bytes(&env, &data, length)?;
            let (out, stats) = process_access_unit(&bytes, mode as u8, drop_el != 0);
            if !stats_out.is_null() {
                let values = [
                    stats.rpus_converted as jint,
                    stats.rpus_failed as jint,
                    stats.el_nals_dropped as jint,
                    stats.source_profile as jint,
                ];
                if env.get_array_length(&stats_out).ok()? as usize >= values.len() {
                    let _ = env.set_int_array_region(&stats_out, 0, &values);
                }
            }
            let out = out?;
            let array = env.byte_array_from_slice(&out).ok()?;
            Some(array.into_raw())
        }));
        match result {
            Ok(Some(array)) => array,
            _ => std::ptr::null_mut(),
        }
    }

    /// `probeProfile(data, length)`: Dolby Vision profile of the first RPU, or -1.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_nuvio_app_features_player_dovi_DoviNative_probeProfile<'local>(
        env: JNIEnv<'local>,
        _class: JClass<'local>,
        data: JByteArray<'local>,
        length: jint,
    ) -> jint {
        catch_unwind(AssertUnwindSafe(|| {
            read_bytes(&env, &data, length)
                .and_then(|b| probe_profile(&b))
                .map(|p| p as jint)
                .unwrap_or(-1)
        }))
        .unwrap_or(-1)
    }
}
