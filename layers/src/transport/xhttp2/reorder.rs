use std::collections::HashMap;
use std::collections::hash_map::Entry;

use bytes::Bytes;

use crate::transport::xhttp2::reorder::ReorderError::InvalidSeq;
pub struct Reorder {
    next: u64,
    fin: Option<u64>,
    pending: HashMap<u64, Bytes>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReorderError {
    InvalidSeq,
}

impl Reorder {
    pub fn new() -> Self {
        Self {
            next: 0,
            fin: None,
            pending: HashMap::new(),
        }
    }

    pub fn push(&mut self, seq: u64, buf: Bytes) -> Result<Vec<Bytes>, ReorderError> {
        let mut vec = Vec::new();
        if seq < self.next {
            return Ok(vec);
        }

        if self.pending.contains_key(&seq) {
            return Ok(vec);
        }

        if buf.is_empty() {
            match self.fin {
                Some(f) if f != seq => return Err(InvalidSeq),
                _ => self.fin = Some(seq),
            }
        }

        if let Some(f) = self.fin {
            if seq > f {
                return Err(InvalidSeq);
            }
        }

        match seq {
            seq if seq == self.next => {
                self.next += 1;
                vec.push(buf);
                while let Some(b) = self.pending.remove(&self.next) {
                    self.next += 1;
                    vec.push(b);
                }
                return Ok(vec);
            }
            seq if seq > self.next => {
                match self.pending.entry(seq) {
                    Entry::Occupied(_) => {
                        return Ok(vec);
                    }
                    Entry::Vacant(slot) => slot.insert(buf),
                };
                return Ok(vec);
            }
            _ => return Err(InvalidSeq),
        }
    }
}
