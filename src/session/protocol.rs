use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

use nix::cmsg_space;
use nix::sys::socket::{ControlMessage, ControlMessageOwned, MsgFlags, recvmsg, sendmsg};

const ATTACH_MARKER: &[u8] = b"P";
const MAX_REPLAY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct Attachment {
    pub replay: Vec<u8>,
    pub pty: OwnedFd,
}

pub fn send_attachment(
    mut socket: &UnixStream,
    replay: &[u8],
    pty: BorrowedFd<'_>,
) -> io::Result<()> {
    let replay_len = u32::try_from(replay.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "replay is too large"))?;
    socket.write_all(&replay_len.to_be_bytes())?;
    socket.write_all(replay)?;

    let iov = [IoSlice::new(ATTACH_MARKER)];
    let descriptors = [pty.as_raw_fd()];
    let control = [ControlMessage::ScmRights(&descriptors)];
    sendmsg::<()>(socket.as_raw_fd(), &iov, &control, MsgFlags::empty(), None)
        .map_err(io::Error::from)?;
    Ok(())
}

pub fn receive_attachment(mut socket: &UnixStream) -> io::Result<Attachment> {
    let mut length = [0_u8; 4];
    socket.read_exact(&mut length)?;
    let replay_len = u32::from_be_bytes(length) as usize;
    if replay_len > MAX_REPLAY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "session replay exceeds protocol limit",
        ));
    }

    let mut replay = vec![0_u8; replay_len];
    socket.read_exact(&mut replay)?;

    let mut marker = [0_u8; 1];
    let mut iov = [IoSliceMut::new(&mut marker)];
    let mut control_space = cmsg_space!([RawFd; 1]);
    let (message_bytes, mut descriptors) = {
        let message = recvmsg::<()>(
            socket.as_raw_fd(),
            &mut iov,
            Some(&mut control_space),
            MsgFlags::empty(),
        )
        .map_err(io::Error::from)?;

        let descriptors = message
            .cmsgs()
            .map_err(io::Error::from)?
            .filter_map(|control| match control {
                ControlMessageOwned::ScmRights(descriptors) => Some(descriptors),
                _ => None,
            })
            .flatten()
            .map(|raw_fd| {
                // SAFETY: SCM_RIGHTS created a new descriptor owned by this process.
                unsafe { OwnedFd::from_raw_fd(raw_fd) }
            })
            .collect::<Vec<_>>();
        (message.bytes, descriptors)
    };

    if message_bytes != ATTACH_MARKER.len() || marker != ATTACH_MARKER {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid session attachment marker",
        ));
    }

    if descriptors.len() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "session attachment must contain one PTY descriptor",
        ));
    }

    let pty = descriptors.pop().expect("descriptor count checked");
    Ok(Attachment { replay, pty })
}
