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
type = "fragment"
size = "500-1000"

[[outbound.layers]]
type = "jitter"
delay = "10-20"

[[outbound.layers]]
type = "tsf"
magic = 432
```

## `jitter`

Sleeps for a random time before every write is passed on.

```toml
[[outbound.layers]]
type = "jitter"
delay = "10-20"
```

`delay` is a string of milliseconds:

| Value | Meaning |
|---|---|
| `"15"` | fixed delay of 15 ms |
| `"10-20"` | random delay for every write, from 10 ms up to but not including 20 ms |

`min` must not exceed `max`. If `delay` is omitted, the delay is `"0"`.

How it behaves:

- The delay is applied **per write call**, not per byte. A single large write is
  delayed once, a tunnel that forwards many small chunks is delayed once per
  chunk.
- The caller waits during the delay (backpressure). Nothing is buffered.
- Reads are never delayed.
- Throughput is bounded by `write size / delay`. With 16 KiB writes and an
  average delay of 25 ms, one direction cannot exceed roughly 640 KiB/s. Keep
  delays small, or accept the cost on bulk traffic.
- The random numbers come from a fast process-wide generator seeded from the
  clock. The distribution is uniform and not cryptographic.

What it does and does not do: jitter changes **when** packets leave. It does not
change their sizes or directions, and a uniform random delay does not imitate
any real application. Treat it as a way to break naive timing correlation, not
as a disguise.

## `fragment`

Cuts every write into smaller pieces.

```toml
[[outbound.layers]]
type = "fragment"
size = "1000-1400"
```

`size` is a string:

| Value | Meaning |
|---|---|
| `"1400"` | pieces of at most 1400 bytes |
| `"500-1000"` | each piece is random in `[500, 1000)`, sampled for every write; `min` must be above 0 |
| `"50%"` | each piece is the given percentage of the write (1 to 100) |

If `size` is omitted, it is `"1400"`.

How it behaves:

- Every write call is truncated to a sampled prefix, and the layer reports a
  partial write. Callers such as `write_all` repeat the call with the rest, so
  one big write becomes a series of smaller consecutive writes.
- It applies to **every write for the whole lifetime of the connection**, not
  only to the first bytes. Small pieces on bulk traffic cost throughput.
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

## Logging

Both layers log every write at `trace` level under the tags `JITTER` and `FRAG`:

```bash
RUST_LOG=info,JITTER=trace,FRAG=trace ./cray --config client.toml
```

## Gotchas

- An invalid `delay` or `size` is reported when the first connection reaches the
  layer, not at startup.
- `fragment` with a size above your MTU has no visible effect, because the
  kernel splits large writes anyway.
- The upper bound of a range is exclusive. `"1000-1400"` never produces exactly
  1400, and `"10-10"` always gives 10.
- Using both layers on every connection costs throughput and adds latency.
  Measure before you enable them for bulk traffic.
