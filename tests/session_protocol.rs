use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::thread;

use agmux_native::session::{receive_attachment, send_attachment};

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
