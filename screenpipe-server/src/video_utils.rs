use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use chrono::NaiveDateTime;
use chrono::{DateTime, Utc};
use image::DynamicImage;
use oasgen::OaSchema;
use screenpipe_core::find_ffmpeg_path;
use screenpipe_db::VideoMetadata as DBVideoMetadata;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::path::PathBuf;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::{debug, error, info};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct FFprobeOutput {
    format: Format,
    streams: Vec<Stream>,
}

#[derive(Debug, Deserialize)]
struct Format {
    duration: Option<String>,
    tags: Option<Tags>,
}

#[derive(Debug, Deserialize)]
struct Tags {
    creation_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Stream {
    r_frame_rate: String,
}

pub async fn extract_frame(file_path: &str, offset_index: i64) -> Result<String> {
    let ffmpeg_path = find_ffmpeg_path().expect("failed to find ffmpeg path");

    let offset_seconds = offset_index as f64 / 1000.0;
    let offset_str = format!("{:.3}", offset_seconds);

    debug!(
        "extracting frame from {} at offset {}",
        file_path, offset_str
    );

    let mut command = Command::new(ffmpeg_path);
    command
        .args([
            "-ss",
            &offset_str,
            "-i",
            file_path,
            "-vf",
            "scale=iw*0.75:ih*0.75", // Scale down to 75% of original size
            "-vframes",
            "1",
            "-f",
            "image2pipe",
            "-c:v",
            "mjpeg", // Use JPEG instead of PNG for smaller size
            "-q:v",
            "10", // Compression quality (2-31, lower is better quality)
            "-",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    debug!("ffmpeg command: {:?}", command);

    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().expect("failed to open stdout");
    let mut stderr = child.stderr.take().expect("failed to open stderr");

    let mut frame_data = Vec::new();
    stdout.read_to_end(&mut frame_data).await?;

    let status = child.wait().await?;
    if !status.success() {
        let mut error_message = String::new();
        stderr.read_to_string(&mut error_message).await?;
        info!("ffmpeg error: {}", error_message);
        return Err(anyhow::anyhow!("ffmpeg process failed: {}", error_message));
    }

    if frame_data.is_empty() {
        return Err(anyhow::anyhow!("failed to extract frame: no data received"));
    }

    Ok(general_purpose::STANDARD.encode(frame_data))
}

#[derive(OaSchema, Deserialize)]
pub struct MergeVideosRequest {
    pub video_paths: Vec<String>,
}

#[derive(OaSchema, Serialize)]
pub struct MergeVideosResponse {
    video_path: String,
}

#[derive(OaSchema, Deserialize)]
pub struct ValidateMediaParams {
    pub file_path: String,
}

pub async fn validate_media(file_path: &str) -> Result<()> {
    use tokio::fs::try_exists;

    if !try_exists(file_path).await? {
        return Err(anyhow::anyhow!("media file does not exist: {}", file_path));
    }

    let ffmpeg_path = find_ffmpeg_path().expect("failed to find ffmpeg path");
    let status = Command::new(ffmpeg_path)
        .args(["-v", "error", "-i", file_path, "-f", "null", "-"])
        .output()
        .await?;

    if status.status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("invalid media file: {}", file_path))
    }
}

pub async fn merge_videos(
    request: MergeVideosRequest,
    output_dir: PathBuf,
) -> Result<MergeVideosResponse> {
    info!("merging videos: {:?}", request.video_paths);

    if let Err(e) = tokio::fs::create_dir_all(&output_dir).await {
        error!("failed to create output directory: {:?}", e);
        return Err(anyhow::anyhow!(
            "failed to create output directory: {:?}",
            e
        ));
    }

    let output_filename = format!("output_{}.mp4", Uuid::new_v4());
    let output_path = output_dir.join(&output_filename);

    // create a temporary file to store the list of input videos
    let temp_file = output_dir.join("input_list.txt");
    let mut file = tokio::fs::File::create(&temp_file).await?;
    for video_path in &request.video_paths {
        // video validation before writing in txt
        if let Err(e) = validate_media(video_path).await {
            error!("invalid file in merging, skipping: {:?}", e);
            continue;
        }
        // Escape single quotes in the file path
        let escaped_path = video_path.replace("'", "'\\''");
        tokio::io::AsyncWriteExt::write_all(
            &mut file,
            format!("file '{}'\n", escaped_path).as_bytes(),
        )
        .await?;
    }

    let ffmpeg_path = find_ffmpeg_path().expect("failed to find ffmpeg path");
    let status = Command::new(ffmpeg_path)
        .args([
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            temp_file.to_str().unwrap(),
            "-c",
            "copy",
            "-y",
            output_path.to_str().unwrap(),
        ])
        .output()
        .await?;

    // clean up the temporary file
    tokio::fs::remove_file(temp_file).await?;

    // log ffmpeg's output
    let stdout = String::from_utf8_lossy(&status.stdout);
    let stderr = String::from_utf8_lossy(&status.stderr);
    debug!("ffmpeg stdout: {}", stdout);
    debug!("ffmpeg stderr: {}", stderr);

    if status.status.success() {
        match output_path.try_exists() {
            Ok(true) => {
                info!("videos merged successfully: {:?}", output_path);
                Ok(MergeVideosResponse {
                    video_path: output_path.to_string_lossy().into_owned(),
                })
            }
            Ok(false) => Err(anyhow::anyhow!(
                "ffmpeg reported success, but output file not found: {:?}",
                output_path
            )),
            Err(e) => Err(anyhow::anyhow!(
                "failed to check if output file exists: {:?}",
                e
            )),
        }
    } else {
        Err(anyhow::anyhow!(
            "ffmpeg failed to merge videos. error: {}",
            stderr
        ))
    }
}

pub async fn extract_frames_from_video(
    video_path: &std::path::Path,
    output_path: Option<PathBuf>,
) -> Result<Vec<DynamicImage>> {
    let ffmpeg_path = find_ffmpeg_path().expect("failed to find ffmpeg path");
    let temp_dir = tempfile::tempdir()?;
    let output_pattern = temp_dir.path().join("frame%d.jpg");

    debug!(
        "extracting frames from {} to {}",
        video_path.display(),
        output_pattern.display()
    );

    // Ensure video file exists
    if !video_path.exists() {
        return Err(anyhow::anyhow!(
            "video file does not exist: {}",
            video_path.display()
        ));
    }

    // Get source FPS and calculate target FPS
    let source_fps = match get_video_fps(&ffmpeg_path, video_path.to_str().unwrap()).await {
        Ok(fps) => fps,
        Err(e) => {
            debug!("failed to get video fps, using default 1fps: {}", e);
            1.0
        }
    };

    let target_fps = if source_fps > 10.0 { 1.0 } else { source_fps };
    let fps_filter = format!("fps={}", target_fps);

    // Extract frames using ffmpeg
    let status = Command::new(&ffmpeg_path)
        .args([
            "-i",
            video_path.to_str().unwrap(),
            "-vf",
            &fps_filter,
            "-strict",
            "unofficial",
            "-c:v",
            "mjpeg",
            "-q:v",
            "2",
            "-qmin",
            "2",
            "-qmax",
            "4",
            "-vsync",
            "0",
            "-threads",
            "2",
            "-y",
            output_pattern.to_str().unwrap(),
        ])
        .output()
        .await?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        return Err(anyhow::anyhow!("ffmpeg failed: {}", stderr));
    }

    // Collect all frames into a vector
    let mut frames = Vec::new();
    let mut entries = tokio::fs::read_dir(&temp_dir.path()).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let frame_data = tokio::fs::read(&path).await?;
        let img = image::load_from_memory(&frame_data)?;

        if let Some(out_dir) = &output_path {
            let frame_name = entry.file_name();
            let dest_path = out_dir.join(frame_name);
            debug!("saving frame to disk: {}", dest_path.display());
            img.save(&dest_path)?;
        }

        frames.push(img);
    }

    if frames.is_empty() {
        return Err(anyhow::anyhow!("no frames were extracted"));
    }

    debug!("extracted {} frames", frames.len());
    Ok(frames)
}

async fn get_video_fps(ffmpeg_path: &PathBuf, video_path: &str) -> Result<f64> {
    let ffprobe_path = ffmpeg_path.with_file_name("ffprobe");

    let output = Command::new(&ffprobe_path)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-select_streams",
            "v:0", // Select first video stream
            "-show_entries",
            "stream=r_frame_rate", // Only request frame rate information
            video_path,
        ])
        .output()
        .await?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("ffprobe failed: {}", error));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    debug!("ffprobe output: {}", stdout);

    // Parse the simplified JSON output
    let parsed: serde_json::Value = serde_json::from_str(&stdout)?;

    let fps = parsed
        .get("streams")
        .and_then(|streams| streams.as_array())
        .and_then(|streams| streams.first())
        .and_then(|stream| stream.get("r_frame_rate"))
        .and_then(|rate| rate.as_str())
        .and_then(|rate| {
            let parts: Vec<f64> = rate.split('/').filter_map(|n| n.parse().ok()).collect();
            if parts.len() == 2 && parts[1] != 0.0 {
                Some(parts[0] / parts[1])
            } else {
                None
            }
        })
        .unwrap_or(1.0);

    debug!("Video FPS: {}", fps);
    Ok(fps)
}

fn parse_time_from_filename(path: &str) -> Option<DateTime<Utc>> {
    let path = Path::new(path);
    let filename = path.file_name()?.to_str()?;

    // Assuming format: monitor_1_2024-10-19_02-51-20.mp4
    let parts: Vec<&str> = filename.split('_').collect();
    if parts.len() >= 4 {
        let date = parts[2];
        let time = parts[3].split('.').next()?;
        let datetime_str = format!("{} {}", date, time.replace('-', ":"));

        // Parse with format "2024-10-19 02:51:20"
        NaiveDateTime::parse_from_str(&datetime_str, "%Y-%m-%d %H:%M:%S")
            .ok()?
            .and_local_timezone(Utc)
            .earliest()
    } else {
        None
    }
}

pub async fn get_video_metadata(video_path: &str) -> Result<VideoMetadata> {
    let ffmpeg_path = find_ffmpeg_path().expect("failed to find ffmpeg path");
    let ffprobe_path = ffmpeg_path.with_file_name("ffprobe");

    // Try ffprobe first
    let creation_time = match Command::new(&ffprobe_path)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-show_entries",
            "format_tags=creation_time",
            video_path,
        ])
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let metadata: FFprobeOutput = serde_json::from_str(&stdout)?;

            metadata
                .format
                .tags
                .and_then(|t| t.creation_time)
                .and_then(|t| {
                    DateTime::parse_from_rfc3339(&t)
                        .or_else(|_| DateTime::parse_from_str(&t, "%Y-%m-%d %H:%M:%S%.f %z"))
                        .or_else(|_| DateTime::parse_from_str(&t, "%Y-%m-%d %H:%M:%S"))
                        .ok()
                })
                .map(|t| t.with_timezone(&Utc))
        }
        _ => None,
    };

    // Try filename if ffprobe failed
    let creation_time = creation_time.or_else(|| parse_time_from_filename(video_path));

    // Try filesystem metadata if everything else failed
    let creation_time = match creation_time {
        Some(time) => time,
        None => {
            if let Ok(metadata) = tokio::fs::metadata(video_path).await {
                if let Ok(created) = metadata.created() {
                    DateTime::<Utc>::from(created)
                } else {
                    debug!("falling back to current time for creation_time");
                    Utc::now()
                }
            } else {
                debug!("falling back to current time for creation_time");
                Utc::now()
            }
        }
    };

    // Rest of the metadata gathering (fps, duration) remains the same...
    let (fps, duration) = get_video_technical_metadata(&ffprobe_path, video_path).await?;

    Ok(VideoMetadata {
        creation_time,
        fps,
        duration,
        device_name: None,
        name: Some(video_path.to_string()),
    })
}

// Helper function to get fps and duration
async fn get_video_technical_metadata(ffprobe_path: &Path, video_path: &str) -> Result<(f64, f64)> {
    let output = Command::new(ffprobe_path)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            video_path,
        ])
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let metadata: FFprobeOutput = serde_json::from_str(&stdout)?;

    let fps = metadata
        .streams
        .first()
        .and_then(|s| {
            let parts: Vec<f64> = s
                .r_frame_rate
                .split('/')
                .filter_map(|n| n.parse().ok())
                .collect();
            if parts.len() == 2 && parts[1] != 0.0 {
                Some(parts[0] / parts[1])
            } else {
                None
            }
        })
        .unwrap_or(30.0);

    let duration = metadata
        .format
        .duration
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(0.0);

    Ok((fps, duration))
}

#[derive(Debug, Clone)]
pub struct VideoMetadata {
    pub creation_time: DateTime<Utc>,
    pub fps: f64,
    pub duration: f64,
    pub device_name: Option<String>,
    pub name: Option<String>,
}

impl From<VideoMetadata> for DBVideoMetadata {
    fn from(metadata: VideoMetadata) -> Self {
        DBVideoMetadata {
            creation_time: metadata.creation_time,
            fps: metadata.fps,
            duration: metadata.duration,
            device_name: metadata.device_name,
            name: metadata.name,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct VideoMetadataOverrides {
    pub overrides: Vec<VideoMetadataItem>,
}

#[derive(Debug, Deserialize)]
pub struct VideoMetadataItem {
    pub file_path: String, // Direct file path
    pub metadata: VideoMetadataOverride,
}

#[derive(Debug, Deserialize)]
pub struct VideoMetadataOverride {
    pub creation_time: Option<DateTime<Utc>>,
    pub fps: Option<f64>,
    pub duration: Option<f64>,
    pub device_name: Option<String>,
    pub name: Option<String>,
}

impl VideoMetadataOverride {
    pub fn apply_to(&self, metadata: &mut VideoMetadata) {
        if let Some(creation_time) = self.creation_time {
            metadata.creation_time = creation_time;
        }
        if let Some(fps) = self.fps {
            metadata.fps = fps;
        }
        if let Some(duration) = self.duration {
            metadata.duration = duration;
        }
        if let Some(ref device_name) = self.device_name {
            metadata.device_name = Some(device_name.clone());
        }
        if let Some(ref name) = self.name {
            metadata.name = Some(name.clone());
        }
    }
}

pub async fn extract_frame_from_video(file_path: &str, offset_index: i64) -> Result<String> {
    let ffmpeg_path = find_ffmpeg_path().expect("failed to find ffmpeg path");
    let temp_dir = tempfile::tempdir()?;
    let output_filename = format!("frame_{}_{}.jpg", Utc::now().timestamp_nanos_opt().unwrap_or_default(), offset_index);
    let output_pattern = temp_dir.path().join(output_filename);

    let duration = get_video_duration(&ffmpeg_path, file_path).await.unwrap_or(0.0);
    let offset_seconds = offset_index as f64 * (duration / get_total_frames(file_path, &ffmpeg_path).await.unwrap_or(1) as f64); // Calculate offset based on total frames if possible
    let offset_seconds_str = format!("{:.3}", offset_seconds);

    debug!(
        target: "ffmpeg_extract",
        video_path = %file_path,
        offset_index = %offset_index,
        calculated_offset_seconds = %offset_seconds_str,
        output_path = %output_pattern.display()
    );

    let mut command = Command::new(&ffmpeg_path);
    command
        .arg("-ss")
        .arg(&offset_seconds_str)
        .arg("-i")
        .arg(file_path)
        .arg("-vframes")
        .arg("1")
        .arg("-q:v") // for high quality
        .arg("2") // for high quality (1-5 is good, 2 is often visually lossless for jpg)
        .arg(output_pattern.as_os_str())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    info!(target: "ffmpeg_extract", "Executing ffmpeg command: {:?}", command);

    let child = command.spawn().map_err(|e| {
        error!(target: "ffmpeg_extract", "Failed to spawn ffmpeg: {}. Command: {:?}", e, command);
        anyhow::anyhow!("Failed to spawn ffmpeg: {}", e)
    })?;

    let output = child.wait_with_output().await.map_err(|e| {
        error!(target: "ffmpeg_extract", "ffmpeg command failed to run: {}. Command: {:?}", e, command);
        anyhow::anyhow!("ffmpeg command failed to run: {}", e)
    })?;

    let ffmpeg_stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        error!(
            target: "ffmpeg_extract",
            "ffmpeg process failed. Status: {}. Stderr: {}. Command: {:?}",
            output.status,
            ffmpeg_stderr,
            command
        );
        return Err(anyhow::anyhow!(
            "ffmpeg process failed with status {}: {}",
            output.status,
            ffmpeg_stderr
        ));
    }

    if !output_pattern.exists() {
        error!(
            target: "ffmpeg_extract",
            "ffmpeg command succeeded but output file was not created: {}. Stderr: {}. Command: {:?}",
            output_pattern.display(),
            ffmpeg_stderr,
            command
        );
        return Err(anyhow::anyhow!(
            "failed to extract frame: file not created. Stderr: {}",
            ffmpeg_stderr
        ));
    }

    info!(target: "ffmpeg_extract", "Successfully extracted frame to {}", output_pattern.display());
    Ok(output_pattern.to_string_lossy().into_owned())
}

async fn get_total_frames(video_path: &str, ffmpeg_path: &Path) -> Result<i64> {
    let mut cmd = Command::new(ffmpeg_path);
    cmd.arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-show_entries")
        .arg("stream=nb_frames")
        .arg("-of")
        .arg("default=nokey=1:noprint_wrappers=1")
        .arg(video_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = cmd.output().await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("ffprobe for nb_frames failed: {}", stderr));
    }
    let nb_frames_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if nb_frames_str == "N/A" {
        // Fallback for streams that don't report nb_frames directly, like some live streams or weird formats
        // Try to get duration and r_frame_rate
        debug!(target: "ffmpeg_extract", "nb_frames is N/A for {}, trying duration/fps fallback", video_path);
        let (duration, fps) = get_video_duration_and_fps(ffmpeg_path, video_path).await?;
        if duration > 0.0 && fps > 0.0 {
            return Ok((duration * fps).round() as i64);
        }
        return Err(anyhow::anyhow!("nb_frames is N/A and could not calculate from duration/fps"));
    }
    nb_frames_str.parse::<i64>().map_err(anyhow::Error::from)
}

async fn get_video_duration(ffmpeg_path: &Path, video_path: &str) -> Result<f64> {
    let mut cmd = Command::new(ffmpeg_path);
    cmd.arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(video_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    
    let output = cmd.output().await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("ffprobe for duration failed: {}", stderr));
    }
    let duration_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    duration_str.parse::<f64>().map_err(anyhow::Error::from)
}

async fn get_video_duration_and_fps(ffmpeg_path: &Path, video_path: &str) -> Result<(f64, f64)> {
    let mut cmd = Command::new(ffmpeg_path);
    cmd.arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("v:0") // Select only video stream
        .arg("-show_entries")
        .arg("stream=r_frame_rate,duration") // Get frame rate and duration
        .arg("-of")
        .arg("json") // Output as JSON for easier parsing
        .arg(video_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = cmd.output().await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "ffprobe for duration/fps failed: {}. Command: {:?}",
            stderr,
            cmd
        ));
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let ffprobe_out: serde_json::Value = serde_json::from_str(&stdout_str).map_err(|e| {
        anyhow::anyhow!(
            "Failed to parse ffprobe JSON output: {}. Output: {}",
            e,
            stdout_str
        )
    })?;

    let streams = ffprobe_out.get("streams").and_then(|s| s.as_array());
    if let Some(streams_arr) = streams {
        if !streams_arr.is_empty() {
            let stream = &streams_arr[0];
            let r_frame_rate_str = stream.get("r_frame_rate").and_then(|r| r.as_str()).unwrap_or("0/0");
            let duration_str = stream.get("duration").and_then(|d| d.as_str());

            let fps = if r_frame_rate_str.contains('/') {
                let parts: Vec<&str> = r_frame_rate_str.split('/').collect();
                if parts.len() == 2 {
                    let num = parts[0].parse::<f64>().unwrap_or(0.0);
                    let den = parts[1].parse::<f64>().unwrap_or(1.0);
                    if den != 0.0 { num / den } else { 0.0 }
                } else {
                    0.0
                }
            } else {
                r_frame_rate_str.parse::<f64>().unwrap_or(0.0)
            };
            
            let duration = duration_str.and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            return Ok((duration, fps));
        }
    }
    Err(anyhow::anyhow!("Could not extract duration/fps from video: {}", video_path))
}

async fn cleanup_old_frames(frames_dir: &PathBuf) -> Result<()> {
    use std::time::{Duration, SystemTime};

    let one_hour_ago = SystemTime::now() - Duration::from_secs(3600);
    let mut read_dir = tokio::fs::read_dir(frames_dir).await?;

    while let Some(entry) = read_dir.next_entry().await? {
        if let Ok(metadata) = entry.metadata().await {
            if let Ok(modified) = metadata.modified() {
                if modified < one_hour_ago {
                    if let Err(e) = tokio::fs::remove_file(entry.path()).await {
                        error!("Failed to remove old frame: {}", e);
                    }
                }
            }
        }
    }

    Ok(())
}

pub async fn extract_high_quality_frame(
    file_path: &str,
    offset_index: i64,
    output_dir: &Path,
) -> Result<String> {
    let ffmpeg_path = find_ffmpeg_path().expect("failed to find ffmpeg path");

    let source_fps = match get_video_fps(&ffmpeg_path, file_path).await {
        Ok(fps) => fps,
        Err(e) => {
            error!("failed to get video fps, using default 1fps: {}", e);
            1.0
        }
    };

    let frame_time = offset_index as f64 * source_fps;

    let frame_filename = format!(
        "frame_{}_{}.png",
        chrono::Utc::now().timestamp_micros(),
        offset_index
    );
    let output_path = output_dir.join(frame_filename);

    let mut command = Command::new(&ffmpeg_path);
    command.args([
        "-y",
        "-loglevel",
        "error",
        "-ss",
        &frame_time.to_string(),
        "-i",
        file_path,
        "-vframes",
        "1",
        "-vf",
        "scale=3840:2160:flags=lanczos",
        "-c:v",
        "png",
        "-compression_level",
        "0",
        "-preset",
        "veryslow",
        "-qscale:v",
        "1",
        output_path.to_str().unwrap(),
    ]);

    let output = command.output().await?;
    if !output.status.success() {
        let error_msg = String::from_utf8_lossy(&output.stderr);
        error!("FFmpeg failed: {}", error_msg);
        return Err(anyhow::anyhow!("FFmpeg failed: {}", error_msg));
    }

    Ok(output_path.to_str().unwrap().to_string())
}
