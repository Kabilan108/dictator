use std::ffi::{CString, OsStr};
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

const PRIVATE_DIR_MODE: libc::mode_t = 0o700;

/// Creates or opens the socket's private parent without following the parent or
/// application-directory symlinks, then checks ownership and tightens its mode.
pub(crate) fn ensure_private_socket_parent(socket_path: &Path) -> Result<()> {
    private_socket_parent(socket_path, true)
}

/// Checks that a default client socket lives in the private directory created by
/// the daemon. This never creates or changes filesystem objects.
pub(crate) fn validate_private_socket_parent(socket_path: &Path) -> Result<()> {
    private_socket_parent(socket_path, false)
}

fn private_socket_parent(socket_path: &Path, create: bool) -> Result<()> {
    let directory = socket_path
        .parent()
        .ok_or_else(|| anyhow!("Unix socket path has no parent directory"))?;
    let root = directory
        .parent()
        .ok_or_else(|| anyhow!("Unix socket directory has no trusted parent"))?;
    let name = directory
        .file_name()
        .ok_or_else(|| anyhow!("Unix socket directory has no final component"))?;

    let root_fd = open_directory(root)?;
    validate_trusted_root(root, &root_fd)?;
    let name = c_string(name)?;
    let directory_fd = match open_directory_at(&root_fd, &name) {
        Ok(fd) => fd,
        Err(err) if create && err.raw_os_error() == Some(libc::ENOENT) => {
            // SAFETY: root_fd is an open directory, name is a single NUL-free
            // component, and mkdirat copies it during the call.
            let result =
                unsafe { libc::mkdirat(root_fd.as_raw_fd(), name.as_ptr(), PRIVATE_DIR_MODE) };
            if result != 0 {
                let mkdir_err = io::Error::last_os_error();
                if mkdir_err.kind() != io::ErrorKind::AlreadyExists {
                    return Err(anyhow!(
                        "failed to create private socket directory {}: {mkdir_err}",
                        directory.display()
                    ));
                }
            }
            open_directory_at(&root_fd, &name).map_err(|err| {
                anyhow!(
                    "failed to open private socket directory {}: {err}",
                    directory.display()
                )
            })?
        }
        Err(err) => {
            return Err(anyhow!(
                "failed to open private socket directory {}: {err}",
                directory.display()
            ));
        }
    };

    let metadata = descriptor_metadata(&directory_fd)?;
    if metadata.st_uid != current_uid() {
        bail!(
            "private socket directory is owned by uid {}, want {}: {}",
            metadata.st_uid,
            current_uid(),
            directory.display()
        );
    }
    let mode = metadata.st_mode & 0o777;
    if create && mode != PRIVATE_DIR_MODE {
        // SAFETY: directory_fd is an owned descriptor for the directory checked above.
        if unsafe { libc::fchmod(directory_fd.as_raw_fd(), PRIVATE_DIR_MODE) } != 0 {
            return Err(anyhow!(
                "failed to secure private socket directory {}: {}",
                directory.display(),
                io::Error::last_os_error()
            ));
        }
    } else if !create && mode != PRIVATE_DIR_MODE {
        bail!(
            "private socket directory mode is {:o}, want 700: {}",
            mode,
            directory.display()
        );
    }
    Ok(())
}

fn open_directory(path: &Path) -> Result<OwnedFd> {
    let path_string = c_string(path.as_os_str())?;
    // SAFETY: path_string is NUL-terminated and open returns a fresh descriptor or -1.
    let fd = unsafe {
        libc::open(
            path_string.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    owned_fd(fd).map_err(|err| anyhow!("failed to open directory {}: {err}", path.display()))
}

fn open_directory_at(root: &OwnedFd, name: &CString) -> io::Result<OwnedFd> {
    // SAFETY: root is an open directory and name is a NUL-terminated component.
    let fd = unsafe {
        libc::openat(
            root.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    owned_fd(fd)
}

fn owned_fd(fd: libc::c_int) -> io::Result<OwnedFd> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a nonnegative result from open/openat is a new owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn descriptor_metadata(fd: &OwnedFd) -> Result<libc::stat> {
    // SAFETY: stat is plain data and fstat initializes it for a valid descriptor.
    let mut metadata: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fd remains valid and metadata points to writable storage for the call.
    if unsafe { libc::fstat(fd.as_raw_fd(), &raw mut metadata) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(metadata)
}

fn validate_trusted_root(path: &Path, fd: &OwnedFd) -> Result<()> {
    let metadata = descriptor_metadata(fd)?;
    let mode = metadata.st_mode & 0o7777;
    if path == Path::new("/tmp") {
        if metadata.st_uid != 0 || mode != 0o1777 {
            bail!("/tmp must be root-owned with mode 1777");
        }
        return Ok(());
    }
    if metadata.st_uid != current_uid() || mode != 0o700 {
        bail!(
            "socket runtime root must be owned by uid {} with mode 700: {}",
            current_uid(),
            path.display()
        );
    }
    Ok(())
}

fn c_string(value: &OsStr) -> Result<CString> {
    CString::new(value.as_bytes()).map_err(|_| anyhow!("filesystem path contains a NUL byte"))
}

fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
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
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::{Duration, Instant};

    use super::*;

    fn private_tempdir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

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

    #[test]
    fn private_socket_parent_is_created_and_validated() {
        let root = private_tempdir();
        let socket = root.path().join("dictator").join("dictator.sock");

        ensure_private_socket_parent(&socket).unwrap();
        validate_private_socket_parent(&socket).unwrap();
        let mode = std::fs::metadata(socket.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn private_socket_parent_does_not_follow_application_symlink() {
        let root = private_tempdir();
        let target = root.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&target, root.path().join("dictator")).unwrap();
        let socket = root.path().join("dictator").join("dictator.sock");

        assert!(ensure_private_socket_parent(&socket).is_err());
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "symlink target permissions were changed");
    }

    #[test]
    fn private_socket_parent_does_not_follow_runtime_root_symlink() {
        let outer = private_tempdir();
        let target = outer.path().join("runtime");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
        let root_link = outer.path().join("runtime-link");
        symlink(&target, &root_link).unwrap();
        let socket = root_link.join("dictator").join("dictator.sock");

        assert!(ensure_private_socket_parent(&socket).is_err());
        assert!(!target.join("dictator").exists());
    }

    #[test]
    fn client_validation_rejects_a_permissive_socket_directory() {
        let root = private_tempdir();
        let directory = root.path().join("dictator");
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
        let socket = directory.join("dictator.sock");

        assert!(validate_private_socket_parent(&socket).is_err());
        ensure_private_socket_parent(&socket).unwrap();
        validate_private_socket_parent(&socket).unwrap();
    }
}
