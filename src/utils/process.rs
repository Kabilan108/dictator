use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug)]
pub struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

/// Runs a child with bounded wait time and captured output.
///
/// Both pipes are drained concurrently so a noisy child cannot block on a full
/// pipe. Once `timeout` expires, the child is killed and reaped before this
/// function returns. At most `max_output_bytes` from each pipe is retained.
pub fn bounded_output(
    command: &mut Command,
    timeout: Duration,
    max_output_bytes: usize,
) -> io::Result<BoundedOutput> {
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = kill_group_and_reap(&mut child);
            return Err(io::Error::other("child stdout was unavailable"));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = kill_group_and_reap(&mut child);
            return Err(io::Error::other("child stderr was unavailable"));
        }
    };

    let stdout_reader = match spawn_reader("bounded-stdout", stdout, max_output_bytes) {
        Ok(reader) => reader,
        Err(err) => {
            let _ = kill_group_and_reap(&mut child);
            return Err(err);
        }
    };
    let stderr_reader = match spawn_reader("bounded-stderr", stderr, max_output_bytes) {
        Ok(reader) => reader,
        Err(err) => {
            let _ = kill_group_and_reap(&mut child);
            let _ = stdout_reader.join();
            return Err(err);
        }
    };

    let deadline = Instant::now() + timeout;
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status, false),
            Ok(None) if Instant::now() >= deadline => {
                let status = kill_group_and_reap(&mut child);
                break (status?, true);
            }
            Ok(None) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                std::thread::sleep(remaining.min(WAIT_POLL_INTERVAL));
            }
            Err(err) => {
                let _ = kill_group_and_reap(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(err);
            }
        }
    };

    // The direct child can exit while a helper keeps its pipes open. Kill the
    // isolated process group before joining readers so completion stays bounded.
    kill_process_group(child.id())?;

    let stdout = join_reader(stdout_reader)?;
    let stderr = join_reader(stderr_reader)?;
    Ok(BoundedOutput {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        timed_out,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    })
}

fn kill_group_and_reap(child: &mut std::process::Child) -> io::Result<ExitStatus> {
    let group_result = kill_process_group(child.id());
    let status = child.wait();
    match (group_result, status) {
        (_, Ok(status)) => Ok(status),
        (Err(group_err), Err(_)) => Err(group_err),
        (Ok(()), Err(wait_err)) => Err(wait_err),
    }
}

fn kill_process_group(leader_pid: u32) -> io::Result<()> {
    let pid = i32::try_from(leader_pid).map_err(|_| io::Error::other("child PID exceeds i32"))?;
    let result = unsafe { libc::kill(-pid, libc::SIGKILL) };
    if result == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(err)
    }
}

#[derive(Debug)]
struct CapturedPipe {
    bytes: Vec<u8>,
    truncated: bool,
}

fn spawn_reader(
    name: &str,
    reader: impl Read + Send + 'static,
    limit: usize,
) -> io::Result<JoinHandle<io::Result<CapturedPipe>>> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || drain_bounded(reader, limit))
}

fn drain_bounded(mut reader: impl Read, limit: usize) -> io::Result<CapturedPipe> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        let keep = read.min(limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    Ok(CapturedPipe { bytes, truncated })
}

fn join_reader(reader: JoinHandle<io::Result<CapturedPipe>>) -> io::Result<CapturedPipe> {
    reader
        .join()
        .map_err(|_| io::Error::other("child output reader panicked"))?
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::Path;

    use super::*;

    fn write_script(path: &Path, body: &str) {
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(body.as_bytes()).unwrap();
        file.sync_all().unwrap();
        drop(file);
    }

    fn process_is_gone_or_zombie(pid: i32) -> bool {
        let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return true,
            Err(err) => panic!("failed to inspect descendant process: {err}"),
        };
        stat.rsplit_once(") ")
            .and_then(|(_, fields)| fields.split_whitespace().next())
            == Some("Z")
    }

    #[test]
    fn timed_out_child_is_killed_reaped_and_output_is_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("hung-command");
        let pid_file = directory.path().join("pid");
        write_script(
            &executable,
            "#!/bin/sh\nprintf '%s' \"$$\" > \"$1\"\nprintf 'started'\nprintf 'problem' >&2\nwhile :; do :; done\n",
        );
        let started = Instant::now();
        let output = bounded_output(
            Command::new("/bin/sh").arg(&executable).arg(&pid_file),
            Duration::from_millis(100),
            4,
        )
        .unwrap();

        assert!(output.timed_out);
        assert!(!output.status.success());
        assert_eq!(output.stdout, b"star");
        assert_eq!(output.stderr, b"prob");
        assert!(output.stdout_truncated);
        assert!(output.stderr_truncated);
        assert!(started.elapsed() < Duration::from_secs(2));

        let pid: i32 = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[test]
    fn successful_child_returns_complete_output() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("finite-command");
        write_script(
            &executable,
            "#!/bin/sh\nprintf 'ready'\nprintf 'clear' >&2\n",
        );

        let output = bounded_output(
            Command::new("/bin/sh").arg(&executable),
            Duration::from_secs(1),
            64,
        )
        .unwrap();

        assert!(!output.timed_out);
        assert!(output.status.success());
        assert_eq!(output.stdout, b"ready");
        assert_eq!(output.stderr, b"clear");
        assert!(!output.stdout_truncated);
        assert!(!output.stderr_truncated);
    }

    #[test]
    fn successful_parent_cannot_leave_a_descendant_holding_pipes_open() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("background-command");
        let pid_file = directory.path().join("descendant-pid");
        write_script(
            &executable,
            "#!/bin/sh\nsleep 60 &\nprintf '%s' \"$!\" > \"$1\"\nprintf 'parent done'\n",
        );
        let started = Instant::now();
        let output = bounded_output(
            Command::new("/bin/sh").arg(&executable).arg(&pid_file),
            Duration::from_secs(1),
            64,
        )
        .unwrap();

        assert!(!output.timed_out);
        assert!(output.status.success());
        assert_eq!(output.stdout, b"parent done");
        assert!(started.elapsed() < Duration::from_secs(2));

        let pid: i32 = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if process_is_gone_or_zombie(pid) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "descendant was neither gone nor a terminated zombie"
            );
            std::thread::yield_now();
        }
    }
}
