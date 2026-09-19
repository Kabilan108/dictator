use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

const STARTUP_CHECK_DELAY: Duration = Duration::from_millis(60);

pub struct AudioPlayer {
    child: Option<Child>,
    path: Option<PathBuf>,
    position: Duration,
    started_at: Option<Instant>,
    paused: bool,
    program: OsString,
}

impl Default for AudioPlayer {
    fn default() -> Self {
        Self {
            child: None,
            path: None,
            position: Duration::ZERO,
            started_at: None,
            paused: false,
            program: OsString::from("ffplay"),
        }
    }
}

impl AudioPlayer {
    pub fn play(&mut self, path: &Path, position: Duration) -> Result<()> {
        self.terminate_child();
        self.path = None;
        self.position = Duration::ZERO;
        self.started_at = None;
        self.paused = false;
        if !path.is_file() {
            bail!("recording is no longer available");
        }

        let mut child = Command::new(&self.program)
            .arg("-nodisp")
            .arg("-autoexit")
            .arg("-loglevel")
            .arg("error")
            .arg("-ss")
            .arg(format!("{:.3}", position.as_secs_f64()))
            .arg("--")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("could not start ffplay; make sure FFmpeg is installed")?;

        // Spawn can succeed even when ffplay rejects the file or cannot open an
        // audio device. Catch that before the GUI shows a false playing state.
        thread::sleep(STARTUP_CHECK_DELAY);
        let startup_status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error).context("could not check ffplay status");
            }
        };
        if let Some(status) = startup_status {
            let detail = read_child_stderr(&mut child);
            let status = status.code().map_or_else(
                || "from a signal".to_string(),
                |code| format!("with status {code}"),
            );
            if detail.is_empty() {
                bail!("ffplay exited {status} before playback started");
            }
            bail!("ffplay could not play the recording: {detail}");
        }
        // Keep ffplay from blocking if it reports a later device error. The UI
        // already learned that startup succeeded, so later status is reaped by
        // refresh, stop, or Drop.
        if let Some(mut stderr) = child.stderr.take() {
            thread::spawn(move || {
                let _ = std::io::copy(&mut stderr, &mut std::io::sink());
            });
        }

        self.child = Some(child);
        self.path = Some(path.to_path_buf());
        self.position = position;
        self.started_at = Some(Instant::now());
        Ok(())
    }

    pub fn pause(&mut self) -> Result<()> {
        self.refresh();
        let Some(child) = self.child.as_ref() else {
            bail!("no recording is playing");
        };
        if self.paused {
            return Ok(());
        }
        let position = self.current_position();
        send_signal(child.id(), "STOP").context("could not pause ffplay")?;
        self.position = position;
        self.started_at = None;
        self.paused = true;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<()> {
        self.refresh();
        let Some(child) = self.child.as_ref() else {
            bail!("no paused recording is available");
        };
        if !self.paused {
            return Ok(());
        }
        send_signal(child.id(), "CONT").context("could not resume ffplay")?;
        self.started_at = Some(Instant::now());
        self.paused = false;
        Ok(())
    }

    /// Restarts ffplay at the requested position. Seeking a paused recording
    /// leaves it paused; seeking an active or completed recording starts it.
    pub fn seek(&mut self, position: Duration) -> Result<()> {
        self.refresh();
        let path = self
            .path
            .clone()
            .ok_or_else(|| anyhow!("no recording is loaded"))?;
        let remain_paused = self.child.is_some() && self.paused;
        self.play(&path, position)?;
        if remain_paused {
            self.pause()?;
        }
        Ok(())
    }

    pub fn stop(&mut self) {
        self.terminate_child();
        self.path = None;
        self.position = Duration::ZERO;
        self.started_at = None;
        self.paused = false;
    }

    pub fn is_playing(&mut self) -> bool {
        self.refresh();
        self.child.is_some() && !self.paused
    }

    pub fn is_paused(&mut self) -> bool {
        self.refresh();
        self.child.is_some() && self.paused
    }

    pub fn position(&mut self) -> Duration {
        self.refresh();
        self.current_position()
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn current_position(&self) -> Duration {
        self.started_at.map_or(self.position, |started_at| {
            self.position.saturating_add(started_at.elapsed())
        })
    }

    fn refresh(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(None) => {}
            Ok(Some(_)) => {
                self.position = self.current_position();
                self.started_at = None;
                self.paused = false;
                self.child = None;
            }
            Err(_) => {}
        }
    }

    fn terminate_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }

    #[cfg(test)]
    fn with_program(program: impl Into<OsString>) -> Self {
        Self {
            child: None,
            path: None,
            position: Duration::ZERO,
            started_at: None,
            paused: false,
            program: program.into(),
        }
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn send_signal(pid: u32, signal: &str) -> Result<()> {
    let status = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("could not run kill")?;
    if !status.success() {
        bail!("kill -{signal} failed for process {pid}");
    }
    Ok(())
}

fn read_child_stderr(child: &mut Child) -> String {
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    stderr.trim().to_string()
}

const EMPTY_PEAK: f32 = 0.08;
const MAX_BYTES_PER_BUCKET: usize = 16 * 1024;

#[derive(Clone, Copy)]
struct WavData {
    offset: u64,
    len: u64,
    channels: u16,
    block_align: u16,
    bits_per_sample: u16,
}

/// Parses RIFF chunks and reads at most 16 KiB per bucket, bounding waveform
/// work for long recordings.
pub fn waveform(path: &Path, buckets: usize) -> Vec<f32> {
    if buckets == 0 {
        return Vec::new();
    }
    waveform_from_file(path, buckets).unwrap_or_else(|_| vec![EMPTY_PEAK; buckets])
}

fn waveform_from_file(path: &Path, buckets: usize) -> Result<Vec<f32>> {
    let mut file = File::open(path)?;
    let wav = parse_wav(&mut file)?;
    if wav.bits_per_sample != 16 || wav.channels == 0 {
        bail!("waveform supports 16-bit PCM WAV files");
    }

    let frame_bytes = u64::from(wav.block_align);
    let frames = wav.len / frame_bytes;
    if frames == 0 {
        return Ok(vec![EMPTY_PEAK; buckets]);
    }

    let mut peaks = Vec::with_capacity(buckets);
    let mut buffer = vec![0_u8; MAX_BYTES_PER_BUCKET];
    for bucket in 0..buckets {
        let start_frame = frames.saturating_mul(bucket as u64) / buckets as u64;
        let end_frame = frames.saturating_mul((bucket + 1) as u64) / buckets as u64;
        let available_bytes = end_frame
            .saturating_sub(start_frame)
            .saturating_mul(frame_bytes);
        let read_len = available_bytes.min(MAX_BYTES_PER_BUCKET as u64) as usize;
        let read_len = read_len - read_len % usize::from(wav.block_align);
        if read_len == 0 {
            peaks.push(EMPTY_PEAK);
            continue;
        }

        let skip = (available_bytes - read_len as u64) / 2;
        let offset = wav
            .offset
            .saturating_add(start_frame.saturating_mul(frame_bytes))
            .saturating_add(skip - skip % frame_bytes);
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut buffer[..read_len])?;
        let peak = buffer[..read_len]
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]).unsigned_abs() as f32)
            .fold(0.0_f32, f32::max);
        peaks.push((peak / i16::MAX as f32).clamp(0.04, 1.0));
    }
    Ok(peaks)
}

fn parse_wav(file: &mut File) -> Result<WavData> {
    let file_len = file.metadata()?.len();
    let mut header = [0_u8; 12];
    file.read_exact(&mut header)?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
        bail!("not a RIFF/WAVE file");
    }
    let riff_end = u64::from(u32::from_le_bytes(header[4..8].try_into()?))
        .saturating_add(8)
        .min(file_len);

    let mut format = None;
    let mut data = None;
    let mut cursor = 12_u64;
    while cursor.saturating_add(8) <= riff_end {
        file.seek(SeekFrom::Start(cursor))?;
        let mut chunk = [0_u8; 8];
        file.read_exact(&mut chunk)?;
        let chunk_len = u64::from(u32::from_le_bytes(chunk[4..8].try_into()?));
        let chunk_start = cursor + 8;
        let chunk_end = chunk_start
            .checked_add(chunk_len)
            .ok_or_else(|| anyhow!("WAV chunk length overflow"))?;
        if chunk_end > riff_end {
            bail!("WAV chunk extends past the RIFF boundary");
        }

        match &chunk[..4] {
            b"fmt " if chunk_len >= 16 => {
                let mut fmt = [0_u8; 16];
                file.read_exact(&mut fmt)?;
                if u16::from_le_bytes(fmt[0..2].try_into()?) != 1 {
                    bail!("waveform supports PCM WAV files");
                }
                format = Some((
                    u16::from_le_bytes(fmt[2..4].try_into()?),
                    u16::from_le_bytes(fmt[12..14].try_into()?),
                    u16::from_le_bytes(fmt[14..16].try_into()?),
                ));
            }
            b"data" => data = Some((chunk_start, chunk_len)),
            _ => {}
        }
        if format.is_some() && data.is_some() {
            break;
        }
        cursor = chunk_end.saturating_add(chunk_len % 2);
    }

    let (channels, block_align, bits_per_sample) =
        format.ok_or_else(|| anyhow!("missing WAV fmt chunk"))?;
    let (offset, len) = data.ok_or_else(|| anyhow!("missing WAV data chunk"))?;
    let expected_align = channels
        .checked_mul(bits_per_sample / 8)
        .ok_or_else(|| anyhow!("invalid WAV block alignment"))?;
    if block_align == 0 || block_align != expected_align {
        bail!("invalid WAV block alignment");
    }
    Ok(WavData {
        offset,
        len,
        channels,
        block_align,
        bits_per_sample,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dictator-playback-{}-{name}", std::process::id()))
    }

    fn write_script(name: &str, body: &str) -> PathBuf {
        let path = temp_path(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn reports_an_immediate_player_failure() {
        let script = write_script("fail.sh", "#!/bin/sh\necho broken audio >&2\nexit 9\n");
        let audio = temp_path("audio.wav");
        fs::write(&audio, b"audio").unwrap();
        let error = AudioPlayer::with_program(&script)
            .play(&audio, Duration::ZERO)
            .unwrap_err();
        assert!(error.to_string().contains("broken audio"));
        let _ = fs::remove_file(script);
        let _ = fs::remove_file(audio);
    }

    #[test]
    fn pause_freezes_the_clock_and_resume_advances_it() {
        let script = write_script("sleep.sh", "#!/bin/sh\nexec sleep 10\n");
        let audio = temp_path("clock.wav");
        fs::write(&audio, b"audio").unwrap();
        let mut player = AudioPlayer::with_program(&script);
        player.play(&audio, Duration::from_secs(2)).unwrap();
        player.pause().unwrap();
        let paused_at = player.position();
        thread::sleep(Duration::from_millis(25));
        assert_eq!(player.position(), paused_at);
        assert!(player.is_paused());
        player.resume().unwrap();
        thread::sleep(Duration::from_millis(25));
        assert!(player.position() > paused_at);
        player.stop();
        assert!(!player.is_playing());
        let _ = fs::remove_file(script);
        let _ = fs::remove_file(audio);
    }

    #[test]
    fn parses_riff_chunks_instead_of_searching_for_data_text() {
        let path = temp_path("chunks.wav");
        let samples = [0_i16, i16::MAX, -1200, 300];
        let data: Vec<u8> = samples.into_iter().flat_map(i16::to_le_bytes).collect();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF\0\0\0\0WAVEJUNK");
        wav.extend_from_slice(&4_u32.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&16_000_u32.to_le_bytes());
        wav.extend_from_slice(&32_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);
        let riff_len = (wav.len() - 8) as u32;
        wav[4..8].copy_from_slice(&riff_len.to_le_bytes());
        fs::write(&path, wav).unwrap();

        let peaks = waveform(&path, 2);
        assert_eq!(peaks.len(), 2);
        assert_eq!(peaks[0], 1.0);
        assert!(peaks[1] >= 1200.0 / i16::MAX as f32);
        let _ = fs::remove_file(path);
    }
}
