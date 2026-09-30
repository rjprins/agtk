use agtk::session::ReplayBuffer;

#[test]
fn replay_buffer_keeps_the_most_recent_bytes() {
    let mut buffer = ReplayBuffer::new(5);

    buffer.push(b"abc");
    buffer.push(b"def");

    assert_eq!(buffer.snapshot(), b"bcdef");
}

#[test]
fn replay_buffer_can_be_cleared_after_attachment() {
    let mut buffer = ReplayBuffer::new(8);
    buffer.push(b"waiting");

    assert_eq!(buffer.take(), b"waiting");
    assert!(buffer.snapshot().is_empty());
}
