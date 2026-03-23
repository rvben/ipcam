use std::sync::Arc;

use anyhow::{Result, bail};
use futures::StreamExt;
use retina::client::{SessionGroup, SetupOptions};
use openh264::formats::YUVSource;
use retina::codec::CodecItem;

/// Grab a single JPEG frame from an RTSP stream using native Rust (retina + openh264).
pub async fn grab_frame(rtsp_url: &str) -> Result<Vec<u8>> {
    let parsed = url::Url::parse(rtsp_url)?;
    let session_group = Arc::new(SessionGroup::default());
    let mut session = retina::client::Session::describe(
        parsed,
        retina::client::SessionOptions::default()
            .session_group(session_group)
            .user_agent("ipcam".to_owned()),
    )
    .await?;

    let video_idx = find_h264_stream(&session)?;

    session
        .setup(video_idx, SetupOptions::default()
            .transport(retina::client::Transport::Tcp(
                retina::client::TcpTransportOptions::default(),
            ))
            .frame_format(retina::codec::FrameFormat::SIMPLE))
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
pub async fn grab_frames_continuous(
    rtsp_url: &str,
    on_frame: tokio::sync::mpsc::Sender<Vec<u8>>,
) -> Result<()> {
    let parsed = url::Url::parse(rtsp_url)?;
    let session_group = Arc::new(SessionGroup::default());
    let mut session = retina::client::Session::describe(
        parsed,
        retina::client::SessionOptions::default()
            .session_group(session_group)
            .user_agent("ipcam".to_owned()),
    )
    .await?;

    let video_idx = find_h264_stream(&session)?;

    session
        .setup(video_idx, SetupOptions::default()
            .transport(retina::client::Transport::Tcp(
                retina::client::TcpTransportOptions::default(),
            ))
            .frame_format(retina::codec::FrameFormat::SIMPLE))
        .await?;
    let session = session.play(retina::client::PlayOptions::default()).await?;
    let mut demuxed = session.demuxed()?;

    let mut decoder = openh264::decoder::Decoder::new()?;
    let mut got_keyframe = false;

    let mut last_frame = tokio::time::Instant::now();
    let frame_interval = std::time::Duration::from_millis(500);

    loop {
        let item = tokio::time::timeout(std::time::Duration::from_secs(10), demuxed.next())
            .await
            .map_err(|_| anyhow::anyhow!("connection timed out"))?;

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
                Err(_) => continue, // skip decode errors
            }
        }
    }
}

fn find_h264_stream<S: retina::client::State>(session: &retina::client::Session<S>) -> Result<usize> {
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

    let img: image::RgbImage =
        image::ImageBuffer::from_raw(w as u32, h as u32, rgb_buf)
            .ok_or_else(|| anyhow::anyhow!("failed to create image buffer"))?;

    let mut jpeg_buf = std::io::Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_buf, 80);
    img.write_with_encoder(encoder)?;

    Ok(Some(jpeg_buf.into_inner()))
}
