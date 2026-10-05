use std::io::{IoSlice, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::UnixStream;
use std::thread;

use agtk::session::{receive_attachment, send_attachment};
use nix::sys::socket::{ControlMessage, MsgFlags, sendmsg};

#[test]
fn attachment_rejects_extra_descriptors_without_leaking_them() {
    let (mut server, client) = UnixStream::pair().expect("create socket pair");
    let source = tempfile::NamedTempFile::new().expect("create descriptor source");
    let count_source_descriptors = || {
        std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                std::fs::read_link(entry.path()).is_ok_and(|path| path == source.path())
            })
            .count()
    };
    let before = count_source_descriptors();
    server.write_all(&0_u32.to_be_bytes()).unwrap();
    let descriptors = [source.as_raw_fd(); 253];
    sendmsg::<()>(
        server.as_raw_fd(),
        &[IoSlice::new(b"P")],
        &[ControlMessage::ScmRights(&descriptors)],
        MsgFlags::empty(),
        None,
    )
    .unwrap();

    assert!(receive_attachment(&client).is_err());
    assert_eq!(count_source_descriptors(), before);
}

#[test]
fn attachment_transfers_replay_bytes_and_the_pty_descriptor() {
    let (server, client) = UnixStream::pair().expect("create socket pair");
    let mut source = tempfile::tempfile().expect("create temporary descriptor");
    source
        .write_all(b"pty data")
        .expect("write descriptor data");
    source.seek(SeekFrom::Start(0)).expect("rewind descriptor");

    let sender = thread::spawn(move || {
        send_attachment(&server, b"detached output", source.as_fd()).expect("send attachment");
    });

    let attachment = receive_attachment(&client).expect("receive attachment");
    let descriptor_flags = nix::fcntl::fcntl(&attachment.pty, nix::fcntl::FcntlArg::F_GETFD)
        .expect("read descriptor flags");
    assert_ne!(descriptor_flags & libc::FD_CLOEXEC, 0);
    sender.join().expect("sender completes");

    assert_eq!(attachment.replay, b"detached output");

    let mut received = std::fs::File::from(attachment.pty);
    let mut contents = String::new();
    received
        .read_to_string(&mut contents)
        .expect("read transferred descriptor");
    assert_eq!(contents, "pty data");
}
