//! Bounded Linux kernel buffering for the helper's high-volume frame pipe.
use std::io;
use std::os::fd::{AsFd, AsRawFd};

const FRAME_PIPE_CAPACITY: libc::c_int = 1024 * 1024;

/// Request at most 1 MiB of kernel buffering, retaining the original capacity
/// when resource limits or container policy deny growth. Never changes blocking
/// mode, packet framing or the application queue's backpressure limits.
pub fn configure_frame_pipe(pipe: &impl AsFd) -> io::Result<usize> {
    let descriptor = pipe.as_fd();
    let current = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETPIPE_SZ) };
    if current < 0 {
        return Err(io::Error::last_os_error());
    }
    if current >= FRAME_PIPE_CAPACITY {
        return Ok(current as usize);
    }
    let resized = unsafe {
        libc::fcntl(
            descriptor.as_raw_fd(),
            libc::F_SETPIPE_SZ,
            FRAME_PIPE_CAPACITY,
        )
    };
    // EPERM/ENOMEM and per-user pipe page limits are ordinary on shared hosts.
    // The existing pipe remains usable; tuning must never reject a connection.
    Ok(if resized < 0 { current } else { resized } as usize)
}

#[cfg(test)]
mod tests {
    use super::configure_frame_pipe;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    #[test]
    fn pipe_tuning_preserves_data_eof_and_blocking_flags() {
        let mut descriptors = [-1; 2];
        assert_eq!(
            unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        let reader = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
        let writer = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
        let flags = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) };
        let capacity = configure_frame_pipe(&writer).unwrap();
        assert!(capacity > 0);
        assert_eq!(
            unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_GETPIPE_SZ) } as usize,
            capacity
        );
        assert_eq!(
            unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) },
            flags
        );
        let mut output = std::fs::File::from(writer);
        output.write_all(b"unchanged framing").unwrap();
        drop(output);
        let mut bytes = Vec::new();
        std::fs::File::from(reader).read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"unchanged framing");
    }

    #[test]
    fn a_non_pipe_descriptor_is_rejected_without_modifying_it() {
        let file = std::fs::File::open("/dev/null").unwrap();
        assert!(configure_frame_pipe(&file).is_err());
    }
}
