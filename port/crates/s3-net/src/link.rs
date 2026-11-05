//! Per-direction sequencing for resumable links (used by both client and relay).
//!
//! Every non-control message gets `seq = 1, 2, ...`; each frame carries `ack` = highest seq received from the
//! peer. The sender keeps messages until acked and retransmits the ones after the peer's `last_seq` when a
//! connection is resumed. The transport (WebSocket over TCP) is ordered, so a gap means a protocol bug.

use std::collections::VecDeque;

use crate::protocol::{Frame, Message};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accept {
    /// Control frame or next frame in order: process it.
    New,
    /// Already processed (retransmission after resume): ignore.
    Duplicate,
    /// Missing frames before this one: the link is broken.
    Gap { expected: u64, got: u64 },
}

#[derive(Debug)]
pub struct Link<M> {
    next_seq: u64,
    unacked: VecDeque<(u64, M)>,
    /// Highest in-order seq received from the peer.
    pub last_recv: u64,
    /// Highest seq we told the peer about (to decide when an explicit Ack is due).
    pub last_ack_sent: u64,
}

impl<M: Message + Clone> Default for Link<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: Message + Clone> Link<M> {
    pub fn new() -> Self {
        Self { next_seq: 1, unacked: VecDeque::new(), last_recv: 0, last_ack_sent: 0 }
    }

    /// Assign a sequence number (control messages get 0 and are not kept) and build the frame.
    pub fn stamp(&mut self, msg: M) -> Frame<M> {
        let seq = if msg.is_control() {
            0
        } else {
            let s = self.next_seq;
            self.next_seq += 1;
            self.unacked.push_back((s, msg.clone()));
            s
        };
        self.last_ack_sent = self.last_recv;
        Frame { seq, ack: self.last_recv, msg }
    }

    /// Peer acknowledged everything up to `ack`.
    pub fn on_ack(&mut self, ack: u64) {
        while self.unacked.front().is_some_and(|(s, _)| *s <= ack) {
            self.unacked.pop_front();
        }
    }

    pub fn accept(&mut self, seq: u64) -> Accept {
        if seq == 0 {
            return Accept::New;
        }
        if seq <= self.last_recv {
            return Accept::Duplicate;
        }
        if seq != self.last_recv + 1 {
            return Accept::Gap { expected: self.last_recv + 1, got: seq };
        }
        self.last_recv = seq;
        Accept::New
    }

    /// Frames to resend after a resume where the peer reported `peer_last` as its last received seq.
    pub fn retransmit_after(&mut self, peer_last: u64) -> Vec<Frame<M>> {
        self.on_ack(peer_last);
        self.last_ack_sent = self.last_recv;
        self.unacked.iter().map(|(s, m)| Frame { seq: *s, ack: self.last_recv, msg: m.clone() }).collect()
    }

    pub fn unacked_len(&self) -> usize {
        self.unacked.len()
    }

    pub fn ack_due(&self) -> bool {
        self.last_recv > self.last_ack_sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ClientMsg;

    fn send(n: u32) -> ClientMsg {
        ClientMsg::Send { from: 1, to: 0, flags: 0, payload: vec![n as u8] }
    }

    #[test]
    fn sequencing_ack_and_resume() {
        let mut a: Link<ClientMsg> = Link::new();
        let mut b: Link<ClientMsg> = Link::new();
        let f1 = a.stamp(send(1));
        let ping = a.stamp(ClientMsg::Ping { nonce: 0 });
        let f2 = a.stamp(send(2));
        let f3 = a.stamp(send(3));
        assert_eq!((f1.seq, ping.seq, f2.seq, f3.seq), (1, 0, 2, 3));
        assert_eq!(a.unacked_len(), 3);
        assert_eq!(b.accept(f1.seq), Accept::New);
        assert_eq!(b.accept(ping.seq), Accept::New);
        assert_eq!(b.accept(f2.seq), Accept::New);
        // connection drops before f3 arrives; b reports last_recv = 2 on resume
        let again = a.retransmit_after(b.last_recv);
        assert_eq!(again.iter().map(|f| f.seq).collect::<Vec<_>>(), vec![3]);
        assert_eq!(b.accept(1), Accept::Duplicate);
        assert_eq!(b.accept(3), Accept::New);
        assert_eq!(b.accept(5), Accept::Gap { expected: 4, got: 5 });
        a.on_ack(3);
        assert_eq!(a.unacked_len(), 0);
        assert!(b.ack_due());
    }
}
