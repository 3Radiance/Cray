//! Tests for `transport::xhttp2::reorder::Reorder`.
//!
//! The API contract (see `layers/src/transport/xhttp2/reorder.rs`):
//! `push(seq, chunk)` returns `Ok(())`/`Err(ReorderError)` and puts the
//! chunks released by *this* push into the public `buf`, clearing it at
//! the start of every push. The caller reads `buf` after `push` and
//! before the next one (the server iterates `guard.buf` right after the
//! push, sending chunks one by one with awaits in between).

#[cfg(test)]
mod tests {
    use std::mem::take;

    use bytes::Bytes;
    use cray_layers::transport::xhttp2::reorder::{Reorder, ReorderError};

    fn b(n: u8) -> Bytes {
        Bytes::from(vec![n])
    }

    /// Push and take the chunks this push released from `buf`.
    fn push(r: &mut Reorder, seq: u64, chunk: Bytes) -> Result<Vec<Bytes>, ReorderError> {
        r.push(seq, chunk)?;
        Ok(take(&mut r.buf))
    }

    #[test]
    fn in_order() {
        let mut r = Reorder::new();
        assert_eq!(push(&mut r, 0, b(0)), Ok(vec![b(0)]));
        assert_eq!(push(&mut r, 1, b(1)), Ok(vec![b(1)]));
        assert_eq!(push(&mut r, 2, b(2)), Ok(vec![b(2)]));
    }

    #[test]
    fn out_of_order() {
        let mut r = Reorder::new();
        assert_eq!(push(&mut r, 2, b(2)), Ok(vec![]));
        assert_eq!(push(&mut r, 0, b(0)), Ok(vec![b(0)]));
        assert_eq!(push(&mut r, 1, b(1)), Ok(vec![b(1), b(2)]));
    }

    #[test]
    fn long_gap_released_at_once() {
        let mut r = Reorder::new();
        assert_eq!(push(&mut r, 3, b(3)), Ok(vec![]));
        assert_eq!(push(&mut r, 2, b(2)), Ok(vec![]));
        assert_eq!(push(&mut r, 1, b(1)), Ok(vec![]));
        assert_eq!(push(&mut r, 0, b(0)), Ok(vec![b(0), b(1), b(2), b(3)]));
    }

    #[test]
    fn old_seq_is_ignored() {
        let mut r = Reorder::new();
        push(&mut r, 0, b(0)).unwrap();
        assert_eq!(push(&mut r, 0, b(0)), Ok(vec![]));
    }

    #[test]
    fn duplicate_in_pending_is_ignored() {
        let mut r = Reorder::new();
        push(&mut r, 2, b(2)).unwrap();
        assert_eq!(push(&mut r, 2, b(9)), Ok(vec![]));
    }

    #[test]
    fn any_order_gives_sorted_output() {
        let orders: [[u64; 5]; 4] = [
            [0, 1, 2, 3, 4],
            [4, 3, 2, 1, 0],
            [3, 1, 0, 4, 2],
            [1, 0, 3, 2, 4],
        ];
        for order in orders {
            let mut r = Reorder::new();
            let mut out = Vec::new();
            for seq in order {
                out.extend(push(&mut r, seq, b(seq as u8)).unwrap());
            }
            assert_eq!(out, (0..5u8).map(b).collect::<Vec<_>>());
        }
    }

    #[test]
    fn duplicate_keeps_original() {
        let mut r = Reorder::new();
        push(&mut r, 2, b(2)).unwrap();
        assert_eq!(push(&mut r, 2, b(9)), Ok(vec![]));
        push(&mut r, 0, b(0)).unwrap();
        assert_eq!(push(&mut r, 1, b(1)), Ok(vec![b(1), b(2)]));
    }

    #[test]
    fn ignored_duplicate_does_not_disturb_the_stream() {
        let mut r = Reorder::new();
        assert_eq!(push(&mut r, 0, b(0)), Ok(vec![b(0)]));
        assert_eq!(push(&mut r, 0, b(9)), Ok(vec![])); // already delivered
        assert_eq!(push(&mut r, 1, b(1)), Ok(vec![b(1)]));
    }

    #[test]
    fn duplicate_of_a_packet_released_from_pending_is_ignored() {
        let mut r = Reorder::new();
        assert_eq!(push(&mut r, 1, b(1)), Ok(vec![]));
        assert_eq!(push(&mut r, 0, b(0)), Ok(vec![b(0), b(1)]));
        assert_eq!(push(&mut r, 1, b(9)), Ok(vec![])); // was delivered from pending
        assert_eq!(push(&mut r, 2, b(2)), Ok(vec![b(2)]));
    }

    #[test]
    fn empty_chunk_is_ordered_like_data() {
        // FIN is an empty chunk with the next seq: it must not overtake data
        let mut r = Reorder::new();
        assert_eq!(push(&mut r, 1, Bytes::new()), Ok(vec![]));
        assert_eq!(push(&mut r, 0, b(0)), Ok(vec![b(0), Bytes::new()]));
    }

    #[test]
    fn fin_arriving_first_is_still_delivered_in_order() {
        let mut r = Reorder::new();
        assert_eq!(push(&mut r, 3, Bytes::new()), Ok(vec![]));
        assert_eq!(push(&mut r, 1, b(1)), Ok(vec![]));
        assert_eq!(push(&mut r, 2, b(2)), Ok(vec![]));
        assert_eq!(
            push(&mut r, 0, b(0)),
            Ok(vec![b(0), b(1), b(2), Bytes::new()])
        );
    }

    #[test]
    fn data_after_delivered_fin_is_error() {
        let mut r = Reorder::new();
        assert_eq!(push(&mut r, 0, b(0)), Ok(vec![b(0)]));
        assert_eq!(push(&mut r, 1, Bytes::new()), Ok(vec![Bytes::new()]));
        // seq == next after the fin was cleared
        assert_eq!(push(&mut r, 2, b(2)), Err(ReorderError::InvalidSeq));
    }

    #[test]
    fn data_beyond_pending_fin_is_error() {
        let mut r = Reorder::new();
        assert_eq!(push(&mut r, 2, Bytes::new()), Ok(vec![]));
        assert_eq!(push(&mut r, 5, b(5)), Err(ReorderError::InvalidSeq));
    }

    #[test]
    fn conflicting_fin_is_error() {
        let mut r = Reorder::new();
        assert_eq!(push(&mut r, 3, Bytes::new()), Ok(vec![]));
        assert_eq!(push(&mut r, 2, Bytes::new()), Err(ReorderError::InvalidSeq));
    }

    #[test]
    fn stale_empty_chunk_does_not_set_fin() {
        let mut r = Reorder::new();
        assert_eq!(push(&mut r, 0, b(0)), Ok(vec![b(0)]));
        assert_eq!(push(&mut r, 1, b(1)), Ok(vec![b(1)]));
        assert_eq!(push(&mut r, 0, Bytes::new()), Ok(vec![]));
        assert_eq!(push(&mut r, 2, b(2)), Ok(vec![b(2)]));
    }

    #[test]
    fn stale_empty_chunk_does_not_poison_out_of_order_data() {
        let mut r = Reorder::new();
        push(&mut r, 0, b(0)).unwrap();
        push(&mut r, 1, b(1)).unwrap();
        assert_eq!(push(&mut r, 0, Bytes::new()), Ok(vec![]));
        assert_eq!(push(&mut r, 3, b(3)), Ok(vec![]));
        assert_eq!(push(&mut r, 2, b(2)), Ok(vec![b(2), b(3)]));
    }

    #[test]
    fn duplicate_fin_is_ignored() {
        let mut r = Reorder::new();
        assert_eq!(push(&mut r, 1, Bytes::new()), Ok(vec![]));
        assert_eq!(push(&mut r, 1, Bytes::new()), Ok(vec![]));
        assert_eq!(push(&mut r, 0, b(0)), Ok(vec![b(0), Bytes::new()]));
        assert_eq!(push(&mut r, 1, Bytes::new()), Ok(vec![]));
    }

    #[test]
    fn empty_chunk_on_occupied_seq_does_not_set_fin() {
        let mut r = Reorder::new();
        assert_eq!(push(&mut r, 2, b(2)), Ok(vec![]));
        assert_eq!(push(&mut r, 2, Bytes::new()), Ok(vec![]));
        assert_eq!(push(&mut r, 3, b(3)), Ok(vec![]));
        assert_eq!(push(&mut r, 0, b(0)), Ok(vec![b(0)]));
        assert_eq!(push(&mut r, 1, b(1)), Ok(vec![b(1), b(2), b(3)]));
    }

    // --- buf semantics (the new shared-buffer API) ---

    #[test]
    fn buf_holds_chunks_until_the_next_push() {
        // The caller may keep `buf` in view across other work (the server
        // sends chunks one by one); `buf` persists until the next push.
        let mut r = Reorder::new();
        r.push(1, b(1)).unwrap();
        r.push(0, b(0)).unwrap();
        assert_eq!(r.buf, vec![b(0), b(1)]);
    }

    #[test]
    fn next_push_clears_buf() {
        let mut r = Reorder::new();
        r.push(0, b(0)).unwrap();
        assert_eq!(r.buf, vec![b(0)]);
        r.push(1, b(1)).unwrap();
        assert_eq!(r.buf, vec![b(1)], "each push releases only its own chunks");
    }

    #[test]
    fn push_that_releases_nothing_clears_buf() {
        let mut r = Reorder::new();
        r.push(0, b(0)).unwrap();
        assert_eq!(r.buf, vec![b(0)]);
        // ignored duplicate: nothing is released, but buf must be emptied
        r.push(0, b(9)).unwrap();
        assert!(
            r.buf.is_empty(),
            "an ignored push leaves stale chunks in buf"
        );
    }

    #[test]
    fn failing_push_clears_buf() {
        let mut r = Reorder::new();
        // open a pending fin, deliver a chunk, then make the next push fail
        push(&mut r, 2, Bytes::new()).unwrap();
        r.push(0, b(0)).unwrap();
        assert_eq!(r.buf, vec![b(0)]);
        assert_eq!(r.push(3, b(3)), Err(ReorderError::InvalidSeq)); // beyond the fin
        assert!(
            r.buf.is_empty(),
            "a failing push leaves stale chunks in buf"
        );
    }

    fn permutations(n: usize) -> Vec<Vec<u64>> {
        fn heap(k: usize, a: &mut Vec<u64>, out: &mut Vec<Vec<u64>>) {
            if k == 1 {
                out.push(a.clone());
                return;
            }
            heap(k - 1, a, out);
            for i in 0..k - 1 {
                if k.is_multiple_of(2) {
                    a.swap(i, k - 1);
                } else {
                    a.swap(0, k - 1);
                }
                heap(k - 1, a, out);
            }
        }
        let mut a: Vec<u64> = (0..n as u64).collect();
        let mut out = Vec::new();
        heap(n, &mut a, &mut out);
        out
    }

    #[test]
    fn every_arrival_order_gives_sorted_output_with_fin_last() {
        const N: usize = 6;
        let expected: Vec<Bytes> = (0..5u8).map(b).chain([Bytes::new()]).collect();
        for order in permutations(N) {
            let mut r = Reorder::new();
            let mut out = Vec::new();
            for &seq in &order {
                let chunk = if seq == 5 { Bytes::new() } else { b(seq as u8) };
                let got = push(&mut r, seq, chunk).unwrap_or_else(|e| panic!("{order:?}: {e:?}"));
                out.extend(got);
            }
            assert_eq!(out, expected, "arrival order {order:?}");
        }
    }

    #[test]
    fn every_arrival_order_survives_duplicates() {
        const N: usize = 6;
        let expected: Vec<Bytes> = (0..5u8).map(b).chain([Bytes::new()]).collect();
        for order in permutations(N) {
            let mut r = Reorder::new();
            let mut out = Vec::new();
            for &seq in &order {
                let (chunk, garbage) = if seq == 5 {
                    (Bytes::new(), Bytes::new())
                } else {
                    (b(seq as u8), b(200))
                };
                out.extend(push(&mut r, seq, chunk).unwrap());
                assert_eq!(
                    push(&mut r, seq, garbage),
                    Ok(vec![]),
                    "dup {seq} in {order:?}"
                );
            }
            assert_eq!(out, expected, "arrival order {order:?}");
        }
    }
}
