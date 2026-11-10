//! Synthetic send-window and loss-recovery conformance cases.

use std::num::NonZeroU8;
use std::time::Duration;

use garmin_gfdi::{MlrReceiver, MlrSendError, MlrSender};

const HANDLE: NonZeroU8 = NonZeroU8::new(0x86).unwrap();
const ZERO: Duration = Duration::ZERO;
const NO_PACKETS: [Vec<u8>; 0] = [];

fn seconds(value: u64) -> Duration {
    Duration::from_secs(value)
}

#[test]
fn fragmentation_piggyback_and_cumulative_ack_match_wire_bytes() {
    let mut sender = MlrSender::new(HANDLE, 5, 9, 2).unwrap();
    sender.enqueue(&[1, 2, 3, 4, 5, 6, 7]).unwrap();
    assert_eq!(sender.buffered_bytes(), 7);
    assert_eq!(
        sender.poll(ZERO, 63).unwrap(),
        [
            vec![0xef, 0xc0, 1, 2, 3],
            vec![0xef, 0xc1, 4, 5, 6],
            vec![0xef, 0xc2, 7],
        ]
    );
    assert_eq!(sender.acknowledge(2, ZERO).unwrap(), 6);
    assert_eq!(sender.buffered_bytes(), 1);
    assert_eq!(sender.acknowledged_sequence(), 2);
    assert_eq!(sender.acknowledge(3, ZERO).unwrap(), 1);
    assert_eq!(sender.poll(seconds(100), 0).unwrap(), Vec::<Vec<u8>>::new());
    sender.enqueue(&[]).unwrap();
    assert_eq!(sender.poll(seconds(100), 0).unwrap(), NO_PACKETS);
}

#[test]
fn limits_and_invalid_acks_leave_pending_data_intact() {
    for size in [0, 1, 2, 513, usize::MAX] {
        assert!(matches!(
            MlrSender::new(HANDLE, size, 10, 0),
            Err(MlrSendError::PacketSize)
        ));
    }
    let mut empty = MlrSender::new(HANDLE, 512, 0, 0).unwrap();
    empty.enqueue(&[]).unwrap();
    assert_eq!(empty.enqueue(&[1]), Err(MlrSendError::BufferLimit));
    let mut sender = MlrSender::new(HANDLE, 3, 2, 1).unwrap();
    assert_eq!(sender.acknowledge(1, ZERO), Err(MlrSendError::InvalidAck));
    sender.enqueue(&[1, 2]).unwrap();
    assert_eq!(sender.enqueue(&[3]), Err(MlrSendError::BufferLimit));
    assert_eq!(sender.poll(ZERO, 64), Err(MlrSendError::InvalidAck));
    let original = sender.poll(ZERO, 0).unwrap();
    assert_eq!(sender.acknowledge(64, ZERO), Err(MlrSendError::InvalidAck));
    assert_eq!(sender.acknowledge(3, ZERO), Err(MlrSendError::InvalidAck));
    assert_eq!(sender.buffered_bytes(), 2);
    assert_eq!(sender.poll(seconds(1), 0).unwrap(), original);
}

#[test]
fn duplicate_and_stale_acks_do_not_postpone_retransmission() {
    let mut sender = MlrSender::new(HANDLE, 3, 3, 2).unwrap();
    sender.enqueue(&[1, 2, 3]).unwrap();
    let original = sender.poll(ZERO, 0).unwrap();
    assert_eq!(
        sender.acknowledge(1, Duration::from_millis(500)).unwrap(),
        1
    );
    assert_eq!(sender.acknowledge(0, seconds(1)).unwrap(), 0);
    assert_eq!(sender.acknowledge(1, seconds(1)).unwrap(), 0);
    assert_eq!(
        sender.poll(Duration::from_millis(1499), 1).unwrap(),
        NO_PACKETS
    );
    assert_eq!(
        sender.poll(Duration::from_millis(1500), 1).unwrap(),
        original[1..]
    );
    assert_eq!(sender.acknowledge(3, seconds(2)).unwrap(), 2);
    sender.enqueue(&[4]).unwrap();
    assert_eq!(sender.poll(seconds(2), 1).unwrap(), [vec![0xe0, 0x43, 4]]);
}

#[test]
fn retries_back_off_cap_and_exhaust_with_no_revival() {
    let mut sender = MlrSender::new(HANDLE, 20, 1, 7).unwrap();
    sender.enqueue(&[1]).unwrap();
    let original = sender.poll(ZERO, 0).unwrap();
    let mut deadline = 1;
    for delay in [2, 4, 8, 16, 20, 20, 20] {
        assert_eq!(
            sender
                .poll(Duration::from_millis(deadline * 1000 - 1), 0)
                .unwrap(),
            NO_PACKETS
        );
        assert_eq!(sender.poll(seconds(deadline), 63).unwrap(), original);
        deadline += delay;
    }
    assert_eq!(
        sender.poll(seconds(deadline), 0),
        Err(MlrSendError::RetriesExhausted)
    );
    assert_eq!(sender.buffered_bytes(), 0);
    assert_eq!(sender.enqueue(&[2]), Err(MlrSendError::Closed));
    assert_eq!(
        sender.acknowledge(1, seconds(deadline)),
        Err(MlrSendError::Closed)
    );
    assert_eq!(sender.poll(seconds(deadline), 0), Err(MlrSendError::Closed));
    sender.cancel();
}

#[test]
fn initial_window_is_32_and_backoff_does_not_admit_unsent_data() {
    let mut sender = MlrSender::new(HANDLE, 3, 64, 1).unwrap();
    sender.enqueue(&[1; 64]).unwrap();
    let initial = sender.poll(ZERO, 0).unwrap();
    assert_eq!(initial.len(), 32);
    assert_eq!(sender.poll(ZERO, 0).unwrap(), NO_PACKETS);
    assert_eq!(sender.poll(seconds(1), 0).unwrap(), initial);
    assert_eq!(sender.acknowledge(1, seconds(1)).unwrap(), 1);
    assert_eq!(sender.poll(seconds(1), 0).unwrap(), NO_PACKETS);
    assert_eq!(sender.acknowledge(32, seconds(1)).unwrap(), 31);
    assert_eq!(sender.poll(seconds(1), 0).unwrap().len(), 32);
    assert_eq!(sender.acknowledge(0, seconds(1)).unwrap(), 32);
    assert_eq!(sender.buffered_bytes(), 0);
}

#[test]
fn zero_retry_budget_and_cancellation_discard_queued_and_sent_bytes() {
    let mut sender = MlrSender::new(HANDLE, 3, 64, 0).unwrap();
    sender.enqueue(&[1; 64]).unwrap();
    assert_eq!(sender.poll(ZERO, 0).unwrap().len(), 32);
    assert_eq!(
        sender.poll(seconds(1), 0),
        Err(MlrSendError::RetriesExhausted)
    );
    assert_eq!(sender.buffered_bytes(), 0);
    let mut sender = MlrSender::new(HANDLE, 3, 64, 1).unwrap();
    sender.enqueue(&[2; 64]).unwrap();
    sender.poll(ZERO, 0).unwrap();
    sender.cancel();
    assert_eq!(sender.buffered_bytes(), 0);
    assert_eq!(sender.poll(ZERO, 0), Err(MlrSendError::Closed));
}

#[test]
fn loss_and_ack_loss_recover_exactly_once_across_sequence_wraps() {
    let input: Vec<u8> = (0..200).collect();
    let mut sender = MlrSender::new(HANDLE, 3, input.len(), 2).unwrap();
    let mut receiver = MlrReceiver::new(HANDLE, 3).unwrap();
    assert_eq!(receiver.expected_sequence(), 0);
    sender.enqueue(&input).unwrap();
    let mut output = Vec::new();
    let mut now = ZERO;
    let mut lost_data = false;
    let mut lost_ack = false;
    for _ in 0..100 {
        let packets = sender.poll(now, 0).unwrap();
        let mut pending_ack = None;
        for packet in packets {
            if !lost_data && packet[1] == 2 {
                lost_data = true;
                continue;
            }
            let update = receiver.receive(&packet, now).unwrap();
            if let Some(data) = update.data {
                output.extend_from_slice(data);
            }
            pending_ack = update.ack.or(pending_ack);
        }
        if let Some(ack) = receiver.flush_ack().unwrap().or(pending_ack) {
            let next = (ack[0] & 15) << 2 | ack[1] >> 6;
            if lost_ack {
                sender.acknowledge(next, now).unwrap();
            } else {
                lost_ack = true;
            }
        }
        if sender.buffered_bytes() == 0 {
            break;
        }
        now += seconds(1);
    }
    assert!(lost_data && lost_ack);
    assert_eq!(sender.buffered_bytes(), 0);
    assert_eq!(output, input);
    assert_eq!(receiver.expected_sequence(), 8);
}
