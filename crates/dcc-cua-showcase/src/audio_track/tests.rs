use super::*;
use dcc_cua_protocol::recording_audio::ProcessAudioTarget;
use rstest::rstest;
use std::io::Cursor;

fn grant() -> ProcessAudioGrant {
    ProcessAudioGrant {
        target: ProcessAudioTarget {
            process_id: 42,
            process_creation_time_100ns: 123,
            window_handle: 7,
            window_thread_id: 9,
            window_class_hash: 11,
        },
        max_duration_ms: 1000,
        max_output_bytes: 2_000_000,
    }
}

#[rstest]
fn audio_output_budget_covers_headers_seek_patches_and_audio_payload() {
    let mut out = BoundedRecordingOutput::new(Cursor::new(Vec::<u8>::new()), 16).unwrap();
    out.write_all(&[1; 16]).unwrap();
    out.seek(SeekFrom::Start(0)).unwrap();
    out.write_all(&[2; 4]).unwrap();
    assert_eq!(out.length(), 16);
    assert!(out.seek(SeekFrom::End(1)).is_err());
    out.seek(SeekFrom::End(0)).unwrap();
    assert!(out.write_all(&[3]).is_err());
    assert_eq!(out.into_inner().into_inner().len(), 16);
}

#[rstest]
fn audio_track_refuses_overlapping_or_unbounded_pcm_before_growth() {
    let mut b = AacTrackBuffer::new(grant()).unwrap();
    b.push(100_000, &[1; 960]).unwrap();
    assert_eq!(b.push(90_000, &[1; 960]), Err(AudioLifecycleError::Packet));
    assert_eq!(
        b.push(u64::MAX, &[1; 960]),
        Err(AudioLifecycleError::Budget)
    );
    assert_eq!(b.push(999_000, &[1; 960]), Err(AudioLifecycleError::Budget));
}

#[rstest]
fn generated_mp4_contains_decodable_aac_and_existing_h264_tracks() {
    // All pixels and samples are generated locally; no capture or audio device.
    let mut encoder = crate::Encoder::with_api_config(
        crate::OpenH264API::from_source(),
        crate::EncoderConfig::new(),
    )
    .unwrap();
    let stream = crate::encode_bgra(&mut encoder, &vec![80; 16 * 16 * 4], 16, 16).unwrap();
    let (sps, pps) = crate::parameter_sets(&stream).unwrap();
    let video = crate::avcc_sample(&stream).unwrap();
    let output =
        BoundedRecordingOutput::new(Cursor::new(Vec::new()), grant().max_output_bytes).unwrap();
    let mut writer = Mp4Writer::write_start(
        output,
        &mp4::Mp4Config {
            major_brand: "isom".parse().unwrap(),
            minor_version: 512,
            compatible_brands: vec!["isom".parse().unwrap(), "avc1".parse().unwrap()],
            timescale: 1000,
        },
    )
    .unwrap();
    writer
        .add_track(
            &mp4::AvcConfig {
                width: 16,
                height: 16,
                seq_param_set: sps,
                pic_param_set: pps,
            }
            .into(),
        )
        .unwrap();
    writer
        .write_sample(
            1,
            &Mp4Sample {
                start_time: 0,
                duration: 300,
                rendering_offset: 0,
                is_sync: true,
                bytes: video.into(),
            },
        )
        .unwrap();
    let mut audio = AacTrackBuffer::new(grant()).unwrap();
    for block in 0..20 {
        let samples: Vec<i16> = (0..480)
            .flat_map(|i| {
                let t = (block * 480 + i) as f32 / 48000.0;
                let v = (0.3 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin() * 32767.0) as i16;
                [v, v]
            })
            .collect();
        audio.push(50_000 + block * 10_000, &samples).unwrap();
    }
    let receipt = audio.append_after_single_video_track(&mut writer).unwrap();
    assert!(receipt.nonzero_input_samples > 0);
    writer.write_end().unwrap();
    let data = writer.into_writer().into_inner().into_inner();
    let mut reader =
        mp4::Mp4Reader::read_header(Cursor::new(data.clone()), data.len() as u64).unwrap();
    assert_eq!(reader.tracks().len(), 2);
    assert_eq!(
        reader.tracks()[&1].media_type().unwrap(),
        mp4::MediaType::H264
    );
    assert_eq!(
        reader.tracks()[&2].media_type().unwrap(),
        mp4::MediaType::AAC
    );
    assert_eq!(reader.tracks()[&2].timescale(), 48000);
    let mut decoder =
        rusty_aac::AacDecoder::with_config_bytes(&rusty_aac::audio_specific_config_bytes(48000, 2))
            .unwrap();
    let mut decoded = Vec::new();
    for n in 1..=reader.sample_count(2).unwrap() {
        let packet = reader.read_sample(2, n).unwrap().unwrap();
        let pcm = decoder.decode(&packet.bytes, None).unwrap();
        decoded.extend(pcm.samples);
    }
    let energy: f64 =
        decoded.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / decoded.len() as f64;
    assert!(energy > 0.001, "encoded track decoded to silence: {energy}");
    let duration_ms = receipt.encoded_samples * 1000 / 48000;
    assert!(
        (250..=300).contains(&duration_ms),
        "audio timeline drifted: {duration_ms}"
    );
    let path =
        std::env::temp_dir().join(format!("dcc-cua-synthetic-av-{}.mp4", uuid::Uuid::new_v4()));
    std::fs::write(&path, &data).unwrap();
    println!(
        "synthetic_av_fixture={} bytes={} audio_duration_ms={} decoded_energy={} native_capture=false",
        path.display(),
        data.len(),
        duration_ms,
        energy
    );
}
