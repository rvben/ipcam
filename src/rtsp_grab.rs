use std::sync::Arc;

use anyhow::{Result, bail};
use futures::StreamExt;
use openh264::formats::YUVSource;
use retina::client::{Credentials, SessionGroup, SetupOptions};
use retina::codec::CodecItem;

/// Parse an RTSP URL, extracting credentials and returning a clean URL + optional creds.
fn parse_rtsp_url(rtsp_url: &str) -> Result<(url::Url, Option<Credentials>)> {
    let mut parsed = url::Url::parse(rtsp_url)?;
    let creds = if !parsed.username().is_empty() {
        let username = percent_decode(parsed.username());
        let password = parsed.password().map(percent_decode).unwrap_or_default();
        parsed.set_username("").ok();
        parsed.set_password(None).ok();
        Some(Credentials { username, password })
    } else {
        None
    };
    Ok((parsed, creds))
}

fn percent_decode(s: &str) -> String {
    // url::Url::username() and password() return percent-encoded strings.
    // Decode them byte-by-byte, preserving `+` literally (unlike form_urlencoded).
    let mut result = String::with_capacity(s.len());
    let mut bytes = s.as_bytes().iter();
    while let Some(&b) = bytes.next() {
        if b == b'%' {
            let hi = bytes.next().copied().unwrap_or(0);
            let lo = bytes.next().copied().unwrap_or(0);
            if let (Some(h), Some(l)) = (hex_val(hi), hex_val(lo)) {
                result.push((h << 4 | l) as char);
            } else {
                result.push('%');
                result.push(hi as char);
                result.push(lo as char);
            }
        } else {
            result.push(b as char);
        }
    }
    result
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn session_options(creds: Option<Credentials>) -> retina::client::SessionOptions {
    retina::client::SessionOptions::default()
        .session_group(Arc::new(SessionGroup::default()))
        .creds(creds)
        .user_agent("ipcam".to_owned())
}

/// Grab a single JPEG frame from an RTSP stream using native Rust (retina + openh264).
pub async fn grab_frame(rtsp_url: &str) -> Result<Vec<u8>> {
    let (parsed, creds) = parse_rtsp_url(rtsp_url)?;
    let mut session = retina::client::Session::describe(parsed, session_options(creds)).await?;

    let video_idx = find_h264_stream(&session)?;

    session
        .setup(
            video_idx,
            SetupOptions::default()
                .transport(retina::client::Transport::Tcp(
                    retina::client::TcpTransportOptions::default(),
                ))
                .frame_format(retina::codec::FrameFormat::SIMPLE),
        )
        .await?;
    let session = session.play(retina::client::PlayOptions::default()).await?;
    let mut demuxed = session.demuxed()?;

    let mut decoder = openh264::decoder::Decoder::new()?;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if tokio::time::Instant::now() > deadline {
            bail!("timed out waiting for video frame");
        }

        let item = tokio::time::timeout(std::time::Duration::from_secs(5), demuxed.next())
            .await
            .map_err(|_| anyhow::anyhow!("timed out reading RTSP packet"))?;

        let item = match item {
            Some(Ok(item)) => item,
            Some(Err(e)) => bail!("RTSP error: {}", e),
            None => bail!("RTSP stream ended unexpectedly"),
        };

        if let CodecItem::VideoFrame(frame) = item {
            if !frame.is_random_access_point() {
                continue;
            }
            if let Some(jpeg) = decode_frame_to_jpeg(&mut decoder, frame.data())? {
                return Ok(jpeg);
            }
        }
    }
}

/// Continuously grab frames from an RTSP stream and send JPEG bytes to the channel.
/// Retries on connection failure with exponential backoff.
pub async fn grab_frames_continuous(
    rtsp_url: &str,
    on_frame: tokio::sync::mpsc::Sender<Vec<u8>>,
) -> Result<()> {
    let mut attempts = 0u32;
    loop {
        match stream_frames(rtsp_url, &on_frame).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                attempts += 1;
                if attempts >= 5 {
                    bail!("failed after {} attempts: {}", attempts, e);
                }
                let backoff = std::time::Duration::from_secs(u64::from(attempts).min(5));
                tokio::time::sleep(backoff).await;
            }
        }
    }
}

async fn stream_frames(
    rtsp_url: &str,
    on_frame: &tokio::sync::mpsc::Sender<Vec<u8>>,
) -> Result<()> {
    let connect_timeout = std::time::Duration::from_secs(10);

    let (parsed, creds) = parse_rtsp_url(rtsp_url)?;

    let mut session = tokio::time::timeout(
        connect_timeout,
        retina::client::Session::describe(parsed, session_options(creds)),
    )
    .await
    .map_err(|_| anyhow::anyhow!("connection timed out"))??;

    let video_idx = find_h264_stream(&session)?;

    tokio::time::timeout(
        connect_timeout,
        session.setup(
            video_idx,
            SetupOptions::default()
                .transport(retina::client::Transport::Tcp(
                    retina::client::TcpTransportOptions::default(),
                ))
                .frame_format(retina::codec::FrameFormat::SIMPLE),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("setup timed out"))??;

    let session = tokio::time::timeout(
        connect_timeout,
        session.play(retina::client::PlayOptions::default()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("play timed out"))??;

    let mut demuxed = session.demuxed()?;
    let mut decoder = openh264::decoder::Decoder::new()?;
    let mut got_keyframe = false;

    let mut last_frame = tokio::time::Instant::now();
    let frame_interval = std::time::Duration::from_millis(500);

    loop {
        let item = tokio::time::timeout(std::time::Duration::from_secs(10), demuxed.next())
            .await
            .map_err(|_| anyhow::anyhow!("read timed out"))?;

        let item = match item {
            Some(Ok(item)) => item,
            Some(Err(e)) => bail!("RTSP error: {}", e),
            None => bail!("stream ended"),
        };

        if let CodecItem::VideoFrame(frame) = item {
            if frame.is_random_access_point() {
                got_keyframe = true;
            }
            if !got_keyframe {
                continue;
            }

            // Feed all frames to the decoder to keep it in sync
            if last_frame.elapsed() < frame_interval {
                let _ = decoder.decode(frame.data());
                continue;
            }

            match decode_frame_to_jpeg(&mut decoder, frame.data()) {
                Ok(Some(jpeg)) => {
                    last_frame = tokio::time::Instant::now();
                    if on_frame.send(jpeg).await.is_err() {
                        return Ok(());
                    }
                }
                Ok(None) => {}
                Err(_) => continue,
            }
        }
    }
}

fn find_h264_stream<S: retina::client::State>(
    session: &retina::client::Session<S>,
) -> Result<usize> {
    session
        .streams()
        .iter()
        .position(|s| s.encoding_name().eq_ignore_ascii_case("h264"))
        .ok_or_else(|| anyhow::anyhow!("no H.264 video stream found"))
}

fn decode_frame_to_jpeg(
    decoder: &mut openh264::decoder::Decoder,
    data: &[u8],
) -> Result<Option<Vec<u8>>> {
    let yuv = match decoder.decode(data) {
        Ok(Some(yuv)) => yuv,
        Ok(None) => return Ok(None),
        Err(e) => bail!("H.264 decode error: {}", e),
    };

    let (w, h) = yuv.dimensions();
    let mut rgb_buf = vec![0u8; w * h * 3];
    yuv.write_rgb8(&mut rgb_buf);

    let img: image::RgbImage = image::ImageBuffer::from_raw(w as u32, h as u32, rgb_buf)
        .ok_or_else(|| anyhow::anyhow!("failed to create image buffer"))?;

    let mut jpeg_buf = std::io::Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_buf, 80);
    img.write_with_encoder(encoder)?;

    Ok(Some(jpeg_buf.into_inner()))
}
