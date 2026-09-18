use std::ffi::OsStr;
use std::io;
use std::mem::{offset_of, size_of};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use anyhow::{Result, anyhow, bail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SocketProbe {
    Active,
    Stale,
}

/// Probes a filesystem Unix socket without waiting for space in its accept queue.
/// A saturated queue is treated as active. Only errors that prove no listener is
/// present allow the caller to unlink the socket.
pub(crate) fn probe_socket(path: &Path) -> Result<SocketProbe> {
    // SAFETY: socket takes integer constants and returns a fresh descriptor or -1.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: the successful socket call returned an owned, valid descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let (address, address_len) = socket_address(path.as_os_str())?;

    // SAFETY: address is initialized and lives through the call; its checked
    // length fits sockaddr_un, and fd remains valid for the call.
    let result = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&raw const address).cast::<libc::sockaddr>(),
            address_len,
        )
    };
    if result == 0 {
        return Ok(SocketProbe::Active);
    }

    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ECONNREFUSED | libc::ENOENT) => Ok(SocketProbe::Stale),
        Some(libc::EAGAIN | libc::EINPROGRESS | libc::EALREADY | libc::EINTR | libc::EISCONN) => {
            Ok(SocketProbe::Active)
        }
        _ => Err(anyhow!("failed to probe Unix socket: {err}")),
    }
}

fn socket_address(path: &OsStr) -> Result<(libc::sockaddr_un, libc::socklen_t)> {
    let path = path.as_bytes();
    if path.contains(&0) {
        bail!("Unix socket path contains a NUL byte");
    }

    // SAFETY: sockaddr_un contains only integer fields and a byte array.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if path.len() >= address.sun_path.len() {
        bail!("Unix socket path is too long");
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, src) in address.sun_path.iter_mut().zip(path) {
        *dst = *src as libc::c_char;
    }

    let length = offset_of!(libc::sockaddr_un, sun_path)
        .checked_add(path.len() + 1)
        .ok_or_else(|| anyhow!("Unix socket address length overflow"))?;
    if length > size_of::<libc::sockaddr_un>() {
        bail!("Unix socket path is too long");
    }
    Ok((address, length as libc::socklen_t))
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn saturated_listener_is_active_and_probe_does_not_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        // SAFETY: listener owns a valid bound stream socket descriptor.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
        let _queued = std::os::unix::net::UnixStream::connect(&path).unwrap();

        let started = Instant::now();
        assert_eq!(probe_socket(&path).unwrap(), SocketProbe::Active);
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn orphaned_socket_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        drop(listener);

        assert_eq!(probe_socket(&path).unwrap(), SocketProbe::Stale);
    }
}
