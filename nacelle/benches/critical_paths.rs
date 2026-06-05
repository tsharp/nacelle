use bytes::{Bytes, BytesMut};
use criterion::{BatchSize, BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use nacelle::{FrameRequest, LengthDelimitedProtocol, Protocol};

fn protocol_frame_benches(c: &mut Criterion) {
    let protocol = LengthDelimitedProtocol;
    let req = FrameRequest {
        request_id: 7,
        opcode: 42,
        flags: 0,
        body_len: 4096,
    };
    let small_body = [0xAB; 32];
    let large_body = vec![0xCD; 4096];
    let small_frame = protocol
        .encode_request_frame(7, 42, 0, &small_body)
        .expect("small frame should encode");
    let large_frame = protocol
        .encode_request_frame(7, 42, 0, &large_body)
        .expect("large frame should encode");
    let response_chunk = Bytes::copy_from_slice(&large_body);

    let mut group = c.benchmark_group("protocol_frames");
    group.bench_function("encode_request_small", |b| {
        b.iter(|| {
            black_box(
                protocol
                    .encode_request_frame(7, 42, 0, black_box(&small_body))
                    .expect("frame should encode"),
            )
        })
    });
    group.bench_function("encode_request_large", |b| {
        b.iter(|| {
            black_box(
                protocol
                    .encode_request_frame(7, 42, 0, black_box(large_body.as_slice()))
                    .expect("frame should encode"),
            )
        })
    });
    group.bench_function("decode_head_small", |b| {
        b.iter_batched(
            || BytesMut::from(small_frame.as_ref()),
            |mut buf| {
                black_box(
                    protocol
                        .decode_head(&mut buf, 1024)
                        .expect("decode should succeed")
                        .expect("head should be present"),
                )
            },
            BatchSize::SmallInput,
        )
    });
    group.bench_function("decode_head_large", |b| {
        b.iter_batched(
            || BytesMut::from(large_frame.as_ref()),
            |mut buf| {
                black_box(
                    protocol
                        .decode_head(&mut buf, 8192)
                        .expect("decode should succeed")
                        .expect("head should be present"),
                )
            },
            BatchSize::SmallInput,
        )
    });
    group.bench_function(BenchmarkId::new("encode_response_chunk", "4k"), |b| {
        b.iter_batched(
            || {
                (
                    protocol.response_context(&req),
                    BytesMut::with_capacity(8192),
                )
            },
            |(mut context, mut dst)| {
                protocol
                    .encode_response_chunk(&mut context, response_chunk.clone(), &mut dst)
                    .expect("response chunk should encode");
                black_box(dst)
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

criterion_group!(critical_paths, protocol_frame_benches);
criterion_main!(critical_paths);
