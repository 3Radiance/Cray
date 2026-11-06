#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use cray_layers::transport::xhttp2::reorder::Reorder;

    fn b(n: u8) -> Bytes {
        Bytes::from(vec![n])
    }

    #[test]
    fn in_order() {
        let mut r = Reorder::new();
        assert_eq!(r.push(0, b(0)), Ok(vec![b(0)]));
        assert_eq!(r.push(1, b(1)), Ok(vec![b(1)]));
        assert_eq!(r.push(2, b(2)), Ok(vec![b(2)]));
    }

    #[test]
    fn out_of_order() {
        let mut r = Reorder::new();
        assert_eq!(r.push(2, b(2)), Ok(vec![]));
        assert_eq!(r.push(0, b(0)), Ok(vec![b(0)]));
        assert_eq!(r.push(1, b(1)), Ok(vec![b(1), b(2)]));
    }

    #[test]
    fn long_gap_released_at_once() {
        let mut r = Reorder::new();
        assert_eq!(r.push(3, b(3)), Ok(vec![]));
        assert_eq!(r.push(2, b(2)), Ok(vec![]));
        assert_eq!(r.push(1, b(1)), Ok(vec![]));
        assert_eq!(r.push(0, b(0)), Ok(vec![b(0), b(1), b(2), b(3)]));
    }

    #[test]
    fn old_seq_is_ignored() {
        let mut r = Reorder::new();
        r.push(0, b(0)).unwrap();
        assert_eq!(r.push(0, b(0)), Ok(vec![]));
    }

    #[test]
    fn duplicate_in_pending_is_ignored() {
        let mut r = Reorder::new();
        r.push(2, b(2)).unwrap();
        assert_eq!(r.push(2, b(9)), Ok(vec![]));
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
                out.extend(r.push(seq, b(seq as u8)).unwrap());
            }
            assert_eq!(out, (0..5u8).map(b).collect::<Vec<_>>());
        }
    }

    #[test]
    fn duplicate_keeps_original() {
        let mut r = Reorder::new();
        r.push(2, b(2)).unwrap();
        assert_eq!(r.push(2, b(9)), Ok(vec![]));
        r.push(0, b(0)).unwrap();
        assert_eq!(r.push(1, b(1)), Ok(vec![b(1), b(2)]));
    }

    #[test]
    fn ignored_duplicate_does_not_disturb_the_stream() {
        let mut r = Reorder::new();
        assert_eq!(r.push(0, b(0)), Ok(vec![b(0)]));
        assert_eq!(r.push(0, b(9)), Ok(vec![])); // already delivered
        assert_eq!(r.push(1, b(1)), Ok(vec![b(1)]));
    }

    #[test]
    fn duplicate_of_a_packet_released_from_pending_is_ignored() {
        let mut r = Reorder::new();
        assert_eq!(r.push(1, b(1)), Ok(vec![]));
        assert_eq!(r.push(0, b(0)), Ok(vec![b(0), b(1)]));
        assert_eq!(r.push(1, b(9)), Ok(vec![])); // was delivered from pending
        assert_eq!(r.push(2, b(2)), Ok(vec![b(2)]));
    }

    #[test]
    fn empty_chunk_is_ordered_like_data() {
        // FIN is an empty chunk with the next seq: it must not overtake data
        let mut r = Reorder::new();
        assert_eq!(r.push(1, Bytes::new()), Ok(vec![]));
        assert_eq!(r.push(0, b(0)), Ok(vec![b(0), Bytes::new()]));
    }
}
