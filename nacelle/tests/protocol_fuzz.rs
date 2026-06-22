#![cfg(feature = "reference_protocol")]

use bytes::BytesMut;
use nacelle::{LengthDelimitedProtocol, Protocol};
use proptest::prelude::*;

proptest! {
    #[test]
    fn random_frame_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let protocol = LengthDelimitedProtocol;
        let mut buf = BytesMut::from(bytes.as_slice());
        let _ = protocol.decode_head(&mut buf, 16 * 1024 * 1024);
    }

    #[test]
    fn valid_fragmented_frames_decode_only_after_complete_body(
        request_id in any::<u64>(),
        opcode in any::<u64>(),
        flags in any::<u32>(),
        body in proptest::collection::vec(any::<u8>(), 0..512),
        split in 0usize..512,
    ) {
        let protocol = LengthDelimitedProtocol;
        let frame = protocol
            .encode_request_frame(request_id, opcode, flags, &body)
            .expect("valid frame should encode");
        let split = split.min(frame.len());
        let mut buf = BytesMut::from(&frame[..split]);

        if split < 24 {
            prop_assert!(protocol
                .decode_head(&mut buf, 16 * 1024 * 1024)
                .expect("partial valid frame should not error")
                .is_none());
        }

        buf.extend_from_slice(&frame[split..]);
        let decoded = protocol
            .decode_head(&mut buf, 16 * 1024 * 1024)
            .expect("complete valid frame should decode")
            .expect("complete frame head should decode");

        prop_assert_eq!(decoded.request.request_id, request_id);
        prop_assert_eq!(decoded.request.opcode, opcode);
        prop_assert_eq!(decoded.request.flags, flags);
        prop_assert_eq!(decoded.body_len, body.len());
        prop_assert_eq!(buf.len(), body.len());
    }

    #[test]
    fn malformed_frame_lengths_are_rejected(length in 0u32..20) {
        let protocol = LengthDelimitedProtocol;
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&length.to_le_bytes());
        buf.extend_from_slice(&[0_u8; 20]);

        let result = protocol.decode_head(&mut buf, 16 * 1024 * 1024);
        prop_assert!(result.is_err());
    }
}
