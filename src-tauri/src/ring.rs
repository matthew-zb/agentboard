use std::collections::VecDeque;

/// Fixed-capacity byte ring: keeps only the most recent `cap` bytes.
pub struct RingBuffer {
    cap: usize,
    buf: VecDeque<u8>,
}

impl RingBuffer {
    pub fn new(cap: usize) -> Self {
        Self { cap, buf: VecDeque::new() }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        if self.cap == 0 {
            return;
        }
        if bytes.len() >= self.cap {
            self.buf.clear();
            self.buf.extend(&bytes[bytes.len() - self.cap..]);
            return;
        }
        let overflow = (self.buf.len() + bytes.len()).saturating_sub(self.cap);
        self.buf.drain(..overflow);
        self.buf.extend(bytes);
    }

    pub fn snapshot(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn ring_keeps_last_capacity_bytes() {
        let mut r = RingBuffer::new(1_048_576);
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        r.push(&data);
        assert_eq!(r.len(), 1_048_576);
        assert_eq!(r.snapshot(), data[data.len()-1_048_576..].to_vec());
    }
    #[test] fn ring_small_pushes_concatenate() {
        let mut r = RingBuffer::new(8);
        r.push(b"abc"); r.push(b"defgh"); r.push(b"ij");
        assert_eq!(r.snapshot(), b"cdefghij".to_vec());
    }
}
