# Traffic Shaping Layers: `jitter` and `fragment`

Both layers are plain wrappers. They take any stream and return a stream: no
sockets, no profile files, and no knowledge of the transport or the protocol
below and above them. They work on the client and on the server, in
`inbound.layers` or in `outbound.layers`, anywhere between the source layer
(`tcp` or `unix`) and the protocol.

They only touch **writes**. Reads pass through untouched, so each side shapes the
traffic it sends, and the two ends do not have to be configured the same way.

```toml
[[outbound.layers]]
type = "tcp"
addr = "127.0.0.1:10811"

[[outbound.layers]]
type = "jitter"
stages = [
    { bytes = 2000, delay = "0" },
    { delay = "1-10" },
]

[[outbound.layers]]
type = "fragment"
stages = [
    { bytes = 2000, size = "1-1" },
    { size = "600-1000" },
]

[[outbound.layers]]
type = "vless"
uuid = "11111111-1111-4111-8111-111111111111"
```

Both layers work in **stages** (`stages`): each stage covers its own prefix of
the connection, and the final stage without `bytes` covers all remaining
traffic. `bytes` is the total number of bytes already written through the
layer at which it advances to the next stage.

## `jitter`

Sleeps for a random time before every write is passed on.

```toml
[[outbound.layers]]
type = "jitter"
stages = [
    { bytes = 2000, delay = "0" },
    { delay = "10-20" },
]
```

Each stage is a `{ bytes, delay }` pair:

- `bytes` (optional) is the stage's upper bound in bytes, counted as the total
  bytes already written through the layer. Once the counter reaches `bytes`,
  the layer moves to the next stage. The last stage omits `bytes`: it runs
  until the connection ends.
- `delay` is a string of milliseconds:

| Value | Meaning |
|---|---|
| `"15"` | fixed delay of 15 ms |
| `"10-20"` | random delay for every write, from 10 ms to 20 ms inclusive |
| `"0"` | no delay |

`min` must not exceed `max`. If `stages` is omitted, the delay is `"0"` for the
whole connection.

How it behaves:

- The delay is applied **per write call**, not per byte. A single large write is
  delayed once, a tunnel that forwards many small chunks is delayed once per
  chunk.
- The caller waits during the delay (backpressure). Nothing is buffered.
- Reads are never delayed.
- Throughput is bounded by `write size / delay`. With 16 KiB writes and an
  average delay of 25 ms, one direction cannot exceed roughly 640 KiB/s. Keep
  delays small, or accept the cost on bulk traffic.
- The random numbers come from a per-connection seeded generator (one
  `fastrand::Rng` per stream). The distribution is uniform and not
  cryptographic.

What it does and does not do: jitter changes **when** packets leave. It does not
change their sizes or directions, and a uniform random delay does not imitate
any real application. Treat it as a way to break naive timing correlation, not
as a disguise.

## `fragment`

Cuts every write into smaller pieces.

```toml
[[outbound.layers]]
type = "fragment"
stages = [
    { bytes = 2000, size = "1-1" },
    { size = "1000-1400" },
]
```

Each stage is a `{ bytes, size }` pair:

- `bytes` (optional) is the same upper bound in total written bytes as in
  `jitter`. The last stage without `bytes` runs until the connection ends.
- `size` is a string:

| Value | Meaning |
|---|---|
| `"1400"` | pieces of at most 1400 bytes |
| `"500-1000"` | each piece is random in `[500, 1000)`, sampled for every piece; `min` must be above 0 |
| `"50%"` | each piece is the given percentage of the write (1 to 100) |

If `stages` is omitted, the size is `"1400"` for the whole connection.

How it behaves:

- Every write call is sliced into sampled-size pieces within a single call: the
  layer keeps writing chunks to the inner stream until the buffer is consumed.
  The caller sees one completed write, while a series of smaller consecutive
  writes travels down the chain.
- Stages let you cut only the start of the connection (for example, the first
  bytes carrying the ClientHello) and cut the rest coarser or not at all.
- Writes that are already smaller than the sampled size pass through unchanged.
- The layer adds no pauses. Whether neighboring pieces become separate TCP
  segments is up to the kernel: with `TCP_NODELAY` each write normally leaves
  at once, but writes made while the send queue is busy may be merged.
  Put a `jitter` layer in the same chain to separate them in time.

### Where to put it

The position decides what gets cut:

| Chain | Effect |
|---|---|
| `tcp → fragment → tls → ...` | the fragmenter sees TLS output, including the **ClientHello**, and splits it across several TCP writes |
| `tcp → tls → fragment → ...` | plaintext writes are cut before TLS, so TLS produces smaller records |
| `tcp → fragment → protocol` | splits the protocol's own bytes, for chains without TLS |

This is a splitting technique only. It cannot send fake packets, change the TTL,
reorder segments or touch TCP options, so it does not replace tools that work on
raw packets.

### Important: fine-grained cuts and `jitter`

If you cut the TLS ClientHello (or just the first bytes) finely, for example
with `1-1`, **jitter must be disabled on that byte range** (`delay = "0"`).
When `jitter` sits below `fragment` in the chain
(`tcp → jitter → fragment → ...`), every cut piece goes through its own delay:
2000 bytes cut 1 byte at a time is about 2000 writes, and even a 1–10 ms delay
(~5 ms on average) adds roughly 10 seconds to the ClientHello alone — the
handshake times out and the socket drops.

The rule: fine cuts go with zero jitter, jitter kicks in only later on large
chunks. Reference setup from `test.toml`:

```toml
[[outbound.layers]]
type = "jitter"
stages = [
    { bytes = 2000, delay = "0" },
    { delay = "1-10" },
]

[[outbound.layers]]
type = "fragment"
stages = [
    { bytes = 2000, size = "1-1" },
    { size = "600-1000" },
]
```

Here the first 2000 bytes (the ClientHello) are cut 1 byte at a time with no
delays, and the rest goes in 600–1000 byte chunks with 1–10 ms of jitter.

## Logging

Both layers log every write at `trace` level under the tags `JITTER` and `FRAG`:

```bash
RUST_LOG=info,JITTER=trace,FRAG=trace ./cray --config client.toml
```

## Gotchas

- An invalid `delay` or `size` inside `stages` is reported when the config is
  loaded, so the proxy refuses to start with such a configuration. Validate
  the config before deploying.
- `fragment` with a size above your MTU has no visible effect, because the
  kernel splits large writes anyway.
- The upper bound of a range is exclusive in `fragment` (`"1000-1400"` never
  produces exactly 1400) but inclusive in `jitter` (`"10-20"` can produce 20).
  `"10-10"` always gives 10 in both.
- `stages` must hold at least one stage: an empty list panics the connection on
  the first write.
- Using both layers on every connection costs throughput and adds latency.
  Measure before you enable them for bulk traffic.
