use dolby_vision::rpu::ConversionMode;
use dolby_vision::rpu::dovi_rpu::DoviRpu;
use nuvio_dovi::{probe_profile, process_access_unit};

// Test RPUs come from a dovi_tool checkout: DOVI_TOOL_DIR=/path/to/dovi_tool cargo test
fn assets() -> String { format!("{}/assets/tests", std::env::var("DOVI_TOOL_DIR").unwrap_or_else(|_| "../../../dovi_tool".into())) }

/// dovi_tool's RPU .bin format: 00 00 00 01 + RPU payload (no 0x7C01 NAL header).
fn rpu_bin_to_nal(name: &str) -> Vec<u8> {
    let data = std::fs::read(format!("{}/{name}", assets())).unwrap();
    assert_eq!(&data[..4], &[0, 0, 0, 1]);
    let mut nal = vec![0x7C, 0x01];
    nal.extend_from_slice(&data[4..]);
    nal
}

fn sc4(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nal);
}
fn sc3(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&[0, 0, 1]);
    out.extend_from_slice(nal);
}

fn nal_header(t: u8) -> [u8; 2] {
    [t << 1, 0x01]
}

/// Fake BL slice / EL NALs with emulation-prevention bytes and a trailing zero byte.
fn fake_nal(t: u8, len: usize, seed: u8) -> Vec<u8> {
    let mut v = nal_header(t).to_vec();
    for i in 0..len {
        v.push(seed.wrapping_add((i * 37) as u8) | 0x04);
    }
    v.extend_from_slice(&[0x00, 0x00, 0x03, 0x01, 0x80]);
    v
}

/// Build an access unit as Matroska -> Media3 would emit it (4-byte start codes), plus a 3-byte one.
fn build_au(rpu_nal: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
    let aud = vec![0x46, 0x01, 0x50];
    let slice_idr = fake_nal(19, 300, 7);
    let suffix_sei = fake_nal(40, 20, 3);
    let el1 = {
        // UNSPEC63 wrapping an EL slice
        let mut v = nal_header(63).to_vec();
        v.extend_from_slice(&fake_nal(1, 200, 9));
        v
    };
    let el2 = {
        let mut v = nal_header(63).to_vec();
        v.extend_from_slice(&fake_nal(34, 10, 1));
        v
    };
    let mut au = Vec::new();
    sc4(&mut au, &aud);
    sc4(&mut au, &slice_idr);
    sc3(&mut au, &suffix_sei);
    sc4(&mut au, &el2);
    sc4(&mut au, &el1);
    sc4(&mut au, rpu_nal);
    (au, vec![aud, slice_idr, suffix_sei])
}

fn split_annexb(data: &[u8]) -> Vec<Vec<u8>> {
    let mut nals = Vec::new();
    let mut i = 0;
    let mut start: Option<usize> = None;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            if let Some(s) = start {
                let mut e = i;
                if e > s && data[e - 1] == 0 {
                    e -= 1;
                }
                nals.push(data[s..e].to_vec());
            }
            start = Some(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    if let Some(s) = start {
        nals.push(data[s..].to_vec());
    }
    nals
}

#[test]
fn mel_access_unit_matches_dovi_tool_reference() {
    let rpu = rpu_bin_to_nal("mel_orig.bin");
    let expected = rpu_bin_to_nal("mel_to_81.bin");
    let (au, kept) = build_au(&rpu);
    assert_eq!(probe_profile(&au), Some(7));

    let (out, stats) = process_access_unit(&au, 2, true);
    let out = out.expect("changed");
    assert_eq!(stats.rpus_converted, 1);
    assert_eq!(stats.rpus_failed, 0);
    assert_eq!(stats.el_nals_dropped, 2);
    assert_eq!(stats.source_profile, 7);

    let nals = split_annexb(&out);
    assert_eq!(nals.len(), 4, "AUD + IDR + SEI + RPU");
    assert_eq!(&nals[..3], &kept[..], "base-layer NALs must be byte-identical");
    assert_eq!(nals[3], expected, "RPU must equal dovi_tool's mel_to_81 reference");
    assert!(nals.iter().all(|n| (n[0] >> 1) & 0x3F != 63));
    assert_eq!(probe_profile(&out), Some(8));
    // Base layer bytes before the RPU are kept verbatim, including the 3-byte start code.
    let prefix_len = 4 + kept[0].len() + 4 + kept[1].len() + 3 + kept[2].len();
    assert_eq!(&out[..prefix_len], &au[..prefix_len]);
}

#[test]
fn fel_access_unit_is_profile_81_with_identity_mapping() {
    let rpu = rpu_bin_to_nal("fel_orig.bin");
    let (au, kept) = build_au(&rpu);
    let (out, stats) = process_access_unit(&au, 2, true);
    let out = out.unwrap();
    assert_eq!((stats.rpus_converted, stats.el_nals_dropped, stats.source_profile), (1, 2, 7));

    let nals = split_annexb(&out);
    assert_eq!(&nals[..3], &kept[..]);
    let converted = DoviRpu::parse_unspec62_nalu(&nals[3]).unwrap();
    assert_eq!(converted.dovi_profile, 8);
    assert!(converted.header.disable_residual_flag);
    assert!(converted.rpu_data_mapping.as_ref().is_none_or(|m| m.nlq.is_none()));

    // Same bytes as dovi_tool's own `-m 2` conversion of this RPU.
    let mut reference = DoviRpu::parse_unspec62_nalu(&rpu).unwrap();
    reference.convert_with_mode(ConversionMode::To81).unwrap();
    assert_eq!(nals[3], reference.write_hevc_unspec62_nalu().unwrap());
}

#[test]
fn non_dolby_vision_access_unit_is_untouched() {
    let mut au = Vec::new();
    sc4(&mut au, &[0x46, 0x01, 0x50]);
    sc4(&mut au, &fake_nal(1, 500, 2));
    let (out, stats) = process_access_unit(&au, 2, true);
    assert!(out.is_none());
    assert_eq!(stats.rpus_converted, 0);
}

#[test]
fn corrupt_rpu_is_kept_and_counted() {
    let mut bad = vec![0x7C, 0x01, 0x19, 0x08, 0x09];
    bad.extend_from_slice(&[0xFF; 40]);
    let mut au = Vec::new();
    sc4(&mut au, &fake_nal(1, 50, 2));
    sc4(&mut au, &bad);
    let (out, stats) = process_access_unit(&au, 2, true);
    assert_eq!(stats.rpus_failed, 1);
    assert_eq!(out.unwrap(), au);
}
