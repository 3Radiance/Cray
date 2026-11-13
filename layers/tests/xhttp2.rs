use bytes::Bytes;
use cray_core::*;
use http::Request;
use std::io::Write;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

fn make_h2_profile(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cray-xhttp2-test-{tag}-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let profile_path = dir.join("profile.toml");
    let mut file = std::fs::File::create(&profile_path).expect("create profile file");

    let profile_content = r#"
[settings]
header_table_size = 4096
enable_push = false
initial_window_size = 65535
max_frame_size = 16384

[uplink]
headers_order = ["x-session-id", "user-agent"]

[uplink.http_headers]
"x-session-id" = "id"
"user-agent" = "cray-test"

[downlink]
headers_order = ["x-session-id", "user-agent"]

[downlink.http_headers]
"x-session-id" = "id"
"user-agent" = "cray-test"
"#;
    file.write_all(profile_content.as_bytes())
        .expect("write profile");
    profile_path
}

fn build_xhttp2(
    host: &str,
    path: &str,
    method: &str,
    config: &str,
    side: Side,
) -> anyhow::Result<Box<dyn Layer>> {
    build_xhttp2_mode(host, path, method, config, side, "StreamUp")
}

#[tokio::test]
async fn test_xhttp2_build_validation() {
    let profile = make_h2_profile("validation");
    let profile_str = profile.to_str().unwrap();

    // Valid
    assert!(build_xhttp2("example.com", "/test", "POST", profile_str, Side::Inbound).is_ok());
    assert!(build_xhttp2("example.com", "/test", "POST", profile_str, Side::Outbound).is_ok());
    assert!(build_xhttp2("example.com", "/test", "GET", profile_str, Side::Inbound).is_err());

    // Missing fields
    let mut reg = Registry::new();
    cray_layers::register(&mut reg);
    let factory = reg.get("xhttp2").unwrap();

    let no_method: toml::Table = r#"host = "example.com"
path = "/test"
mode = "StreamUp"
config = "/tmp/fake""#
        .parse()
        .unwrap();
    assert!(
        factory(
            &no_method,
            &BuildCx {
                side: Side::Inbound
            }
        )
        .is_err()
    );

    let no_config: toml::Table = r#"host = "example.com"
path = "/test"
method = "POST""#
        .parse()
        .unwrap();
    assert!(
        factory(
            &no_config,
            &BuildCx {
                side: Side::Outbound
            }
        )
        .is_err()
    );
}

#[tokio::test]
async fn test_xhttp2_roundtrip_data() {
    let profile = make_h2_profile("roundtrip");
    let profile_str = profile.to_str().unwrap();

    let server_layer = build_xhttp2(
        "127.0.0.1",
        "/split-http",
        "POST",
        profile_str,
        Side::Inbound,
    )
    .expect("server build");

    let client_layer = build_xhttp2(
        "127.0.0.1",
        "/split-http",
        "POST",
        profile_str,
        Side::Outbound,
    )
    .expect("client build");

    let (client_wire, server_wire) = duplex(1 << 20);

    let server_handle = tokio::spawn(async move {
        let mut cx_in = Cx::default();
        let server_output = server_layer
            .call(Input::Stream(Box::pin(server_wire)), &mut cx_in)
            .await
            .expect("server handshake");

        match server_output {
            Output::Streams(inc) => inc,
            _ => panic!("expected Output::Streams"),
        }
    });

    let client_handle = tokio::spawn(async move {
        let mut cx_out = Cx::default();
        let client_output = client_layer
            .call(Input::Stream(Box::pin(client_wire)), &mut cx_out)
            .await
            .expect("client handshake");

        match client_output {
            Output::Stream(s) => s,
            _ => panic!("expected Output::Stream"),
        }
    });

    let (mut incoming, mut client_stream) = tokio::try_join!(
        async { server_handle.await.map_err(|e| anyhow::anyhow!(e)) },
        async { client_handle.await.map_err(|e| anyhow::anyhow!(e)) }
    )
    .expect("handshakes finished");

    // Server accepts the established logical duplex stream
    let mut server_stream =
        tokio::time::timeout(std::time::Duration::from_secs(5), incoming.accept())
            .await
            .expect("accept timed out")
            .expect("accepted stream");

    // Test client -> server
    let msg_from_client = b"hello from client over xhttp2!";
    client_stream
        .write_all(msg_from_client)
        .await
        .expect("client write");

    let mut buf = vec![0u8; msg_from_client.len()];
    server_stream
        .read_exact(&mut buf)
        .await
        .expect("server read");
    assert_eq!(buf, msg_from_client);

    // Test server -> client
    let msg_from_server = b"hello back from server over split get response!";
    server_stream
        .write_all(msg_from_server)
        .await
        .expect("server write");

    let mut buf2 = vec![0u8; msg_from_server.len()];
    client_stream
        .read_exact(&mut buf2)
        .await
        .expect("client read");
    assert_eq!(buf2, msg_from_server);
}

#[tokio::test]
async fn test_xhttp2_large_buffers_throughput() {
    use std::time::{Duration, Instant};
    use tokio::io::split;

    const ROUNDS: usize = 8;
    const PAYLOAD_SIZE: usize = 4 * 1024 * 1024;
    const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30);

    for round in 0..ROUNDS {
        let method = if rand::random::<bool>() {
            "POST"
        } else {
            "PUT"
        };

        let profile = make_h2_profile(&format!("stress-{round}"));
        let profile_str = profile.to_str().unwrap();

        let server_layer = build_xhttp2(
            "127.0.0.1",
            "/split-http",
            method,
            profile_str,
            Side::Inbound,
        )
        .expect("server build");

        let client_layer = build_xhttp2(
            "127.0.0.1",
            "/split-http",
            method,
            profile_str,
            Side::Outbound,
        )
        .expect("client build");

        let (client_wire, server_wire) = duplex(1 << 20);

        let server_handle = tokio::spawn(async move {
            let mut cx = Cx::default();

            let output = server_layer
                .call(Input::Stream(Box::pin(server_wire)), &mut cx)
                .await
                .expect("server handshake");

            match output {
                Output::Streams(incoming) => incoming,
                _ => panic!("expected Output::Streams"),
            }
        });

        let client_handle = tokio::spawn(async move {
            let mut cx = Cx::default();

            let output = client_layer
                .call(Input::Stream(Box::pin(client_wire)), &mut cx)
                .await
                .expect("client handshake");

            match output {
                Output::Stream(stream) => stream,
                _ => panic!("expected Output::Stream"),
            }
        });

        let (mut incoming, client_stream) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::try_join!(
                async {
                    server_handle
                        .await
                        .map_err(|e| anyhow::anyhow!("server task: {e}"))
                },
                async {
                    client_handle
                        .await
                        .map_err(|e| anyhow::anyhow!("client task: {e}"))
                }
            )
        })
        .await
        .expect("handshake timed out")
        .expect("handshake failed");

        let server_stream = tokio::time::timeout(Duration::from_secs(10), incoming.accept())
            .await
            .expect("accept timed out")
            .expect("accept failed");

        let client_payload: Vec<u8> = (0..PAYLOAD_SIZE)
            .map(|i| ((i * 31 + round * 17) % 251) as u8)
            .collect();

        let server_payload: Vec<u8> = (0..PAYLOAD_SIZE)
            .map(|i| ((i * 47 + round * 29 + 113) % 251) as u8)
            .collect();

        let (mut client_reader, mut client_writer) = split(client_stream);
        let (mut server_reader, mut server_writer) = split(server_stream);

        // Test client -> server
        let mut received_by_server = vec![0u8; PAYLOAD_SIZE];
        let started = Instant::now();

        let (write_result, read_result) = tokio::time::timeout(TRANSFER_TIMEOUT, async {
            tokio::join!(
                client_writer.write_all(&client_payload),
                server_reader.read_exact(&mut received_by_server)
            )
        })
        .await
        .expect("client -> server transfer timed out");

        write_result.expect("client write failed");
        read_result.expect("server read failed");

        let client_to_server_elapsed = started.elapsed();

        assert_eq!(
            received_by_server, client_payload,
            "client -> server data mismatch in round {round}"
        );

        let client_to_server_mib_s =
            PAYLOAD_SIZE as f64 / (1024.0 * 1024.0) / client_to_server_elapsed.as_secs_f64();

        // Test server -> client
        let mut received_by_client = vec![0u8; PAYLOAD_SIZE];
        let started = Instant::now();

        let (write_result, read_result) = tokio::time::timeout(TRANSFER_TIMEOUT, async {
            tokio::join!(
                server_writer.write_all(&server_payload),
                client_reader.read_exact(&mut received_by_client)
            )
        })
        .await
        .expect("server -> client transfer timed out");

        write_result.expect("server write failed");
        read_result.expect("client read failed");

        let server_to_client_elapsed = started.elapsed();

        assert_eq!(
            received_by_client, server_payload,
            "server -> client data mismatch in round {round}"
        );

        let server_to_client_mib_s =
            PAYLOAD_SIZE as f64 / (1024.0 * 1024.0) / server_to_client_elapsed.as_secs_f64();

        println!(
            "[XHTTP2 stress] round={round}, method={method}, size={} MiB",
            PAYLOAD_SIZE / (1024 * 1024)
        );

        println!(
            "  client -> server: {:.2} ms, {:.2} MiB/s",
            client_to_server_elapsed.as_secs_f64() * 1000.0,
            client_to_server_mib_s,
        );

        println!(
            "  server -> client: {:.2} ms, {:.2} MiB/s",
            server_to_client_elapsed.as_secs_f64() * 1000.0,
            server_to_client_mib_s,
        );
    }
}

fn write_profile(tag: &str, content: &str) -> PathBuf {
    use std::io::Write as _;

    let dir = std::env::temp_dir().join(format!(
        "cray-xhttp2-test-{tag}-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let profile_path = dir.join("profile.toml");
    let mut file = std::fs::File::create(&profile_path).expect("create profile file");
    file.write_all(content.as_bytes()).expect("write profile");
    profile_path
}

fn make_direction_profile(tag: &str) -> PathBuf {
    write_profile(
        tag,
        r#"
[settings]
header_table_size = 4096
enable_push = false
initial_window_size = 65535
max_frame_size = 16384

[uplink]
headers_order = ["x-up-id", "user-agent"]

[uplink.http_headers]
"x-up-id" = "id"
"user-agent" = "cray-up"

[downlink]
headers_order = ["x-down-id", "user-agent"]

[downlink.http_headers]
"x-down-id" = "id"
"user-agent" = "cray-down"

[uplink.response]
headers_order = ["server", "data"]

[uplink.response.http_headers]
server = "nginx-up"
data = "id"

[downlink.response]
headers_order = ["server", "data"]

[downlink.response.http_headers]
server = "nginx-down"
data = "id"
"#,
    )
}

fn make_reject_profile(tag: &str, body_path: &str) -> PathBuf {
    write_profile(
        tag,
        &format!(
            r#"
reject_status = 418

[settings]
header_table_size = 4096
enable_push = false
initial_window_size = 65535
max_frame_size = 16384

[http_headers]
"x-session-id" = "id"
"user-agent" = "cray-test"

[reject]
headers_order = ["server", "content-type"]
body_path = "{body_path}"

[reject.http_headers]
server = "cray-test"
content-type = "text/html; charset=utf-8"
"#
        ),
    )
}

async fn manual_h2_client(wire: tokio::io::DuplexStream) -> http2::client::SendRequest<Bytes> {
    let (send_req, conn) = http2::client::handshake(wire)
        .await
        .expect("client handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    send_req.ready().await.expect("client ready")
}

async fn read_body(mut body: http2::RecvStream) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let done = std::future::poll_fn(|cx| match body.poll_data(cx) {
            std::task::Poll::Pending => return std::task::Poll::Pending,
            std::task::Poll::Ready(None) => return std::task::Poll::Ready(true),
            std::task::Poll::Ready(Some(Err(_))) => return std::task::Poll::Ready(true),
            std::task::Poll::Ready(Some(Ok(chunk))) => {
                let _ = body.flow_control().release_capacity(chunk.len());
                out.extend_from_slice(&chunk);
                return std::task::Poll::Ready(false);
            }
        })
        .await;
        if done {
            break;
        }
    }
    out
}

fn spawn_layer_server(layer: Box<dyn Layer>, wire: tokio::io::DuplexStream) {
    tokio::spawn(async move {
        let mut cx = Cx::default();
        let output = layer
            .call(Input::Stream(Box::pin(wire)), &mut cx)
            .await
            .expect("server handshake");
        match output {
            Output::Streams(mut incoming) => {
                // Keep the acceptor alive so the server keeps answering probes.
                loop {
                    if incoming.accept().await.is_none() {
                        break;
                    }
                }
            }
            _ => panic!("expected Output::Streams"),
        }
    });
}

fn assert_header(resp: &http::Response<http2::RecvStream>, name: &str, expected: &str) {
    let got = resp
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_else(|| panic!("missing header '{name}'"));
    assert_eq!(got, expected, "unexpected value of header '{name}'");
}

#[tokio::test]
async fn test_xhttp2_legacy_flat_profile_still_works() {
    let profile = write_profile(
        "legacy-flat",
        r#"
[settings]
header_table_size = 4096
enable_push = false

[http_headers]
"x-session-id" = "id"
"user-agent" = "cray-test"
"#,
    );
    let profile_str = profile.to_str().unwrap();

    assert!(build_xhttp2("example.com", "/test", "POST", profile_str, Side::Inbound).is_ok());
    assert!(build_xhttp2("example.com", "/test", "POST", profile_str, Side::Outbound).is_ok());
}

#[tokio::test]
async fn test_xhttp2_reject_custom_page() {
    let html = "<html><body>cray says no</body></html>";
    let body_path = std::env::temp_dir().join(format!(
        "cray-xhttp2-reject-{}-{}.html",
        std::process::id(),
        rand::random::<u64>()
    ));
    std::fs::write(&body_path, html).expect("write reject page");

    let profile = make_reject_profile("reject-page", body_path.to_str().unwrap());
    let profile_str = profile.to_str().unwrap().to_string();

    let server_layer = build_xhttp2(
        "127.0.0.1",
        "/split-http",
        "POST",
        &profile_str,
        Side::Inbound,
    )
    .expect("server build");

    let (client_wire, server_wire) = duplex(1 << 20);
    spawn_layer_server(server_layer, server_wire);

    let mut sr = manual_h2_client(client_wire).await;

    let req = Request::builder()
        .method("GET")
        .uri("https://127.0.0.1/nope")
        .body(())
        .unwrap();
    let (resp, _stream) = sr.send_request(req, true).expect("send reject probe");
    let resp = resp.await.expect("reject response");

    assert_eq!(resp.status().as_u16(), 418);
    assert_header(&resp, "server", "cray-test");
    assert_header(&resp, "content-type", "text/html; charset=utf-8");
    assert_eq!(read_body(resp.into_body()).await, html.as_bytes());
}

#[tokio::test]
async fn test_xhttp2_per_direction_headers_and_responses() {
    let profile = make_direction_profile("per-dir");
    let profile_str = profile.to_str().unwrap().to_string();

    let server_layer = build_xhttp2(
        "127.0.0.1",
        "/split-http",
        "POST",
        &profile_str,
        Side::Inbound,
    )
    .expect("server build");

    let (client_wire, server_wire) = duplex(1 << 20);
    spawn_layer_server(server_layer, server_wire);

    let mut sr = manual_h2_client(client_wire).await;
    let sid = "a".repeat(32);
    let uri = "https://127.0.0.1/split-http";

    // Uplink: POST with the [uplink] id header, parked until GET arrives.
    let post_req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("x-up-id", sid.as_str())
        .header("user-agent", "cray-up")
        .body(())
        .unwrap();
    let (post_resp, mut post_stream) = sr.send_request(post_req, false).expect("send uplink");
    post_stream
        .send_data(Bytes::from_static(b"uplink payload"), true)
        .expect("uplink body");

    // Downlink: GET with the [downlink] id header, pairs with the parked POST.
    let get_req = Request::builder()
        .method("GET")
        .uri(uri)
        .header("x-down-id", sid.as_str())
        .header("user-agent", "cray-down")
        .body(())
        .unwrap();
    let (get_resp, _get_stream) = sr.send_request(get_req, true).expect("send downlink");

    let get_resp = get_resp.await.expect("downlink response");
    assert_eq!(get_resp.status().as_u16(), 200);
    assert_header(&get_resp, "server", "nginx-down");
    assert_header(&get_resp, "data", &sid);

    let post_resp = post_resp.await.expect("uplink response");
    assert_eq!(post_resp.status().as_u16(), 200);
    assert_header(&post_resp, "server", "nginx-up");
    assert_header(&post_resp, "data", &sid);
}

fn make_packet_profile(tag: &str) -> PathBuf {
    write_profile(
        tag,
        r#"
[settings]
header_table_size = 4096
enable_push = false
initial_window_size = 65535
max_frame_size = 16384

[uplink]
headers_order = ["x-session-id", "x-packet-seq", "user-agent"]

[uplink.http_headers]
"x-session-id" = "id"
"x-packet-seq" = "seq"
"user-agent" = "cray-test"

[downlink]
headers_order = ["x-session-id", "user-agent"]

[downlink.http_headers]
"x-session-id" = "id"
"user-agent" = "cray-test"
"#,
    )
}

fn build_xhttp2_mode(
    host: &str,
    path: &str,
    method: &str,
    config: &str,
    side: Side,
    mode: &str,
) -> anyhow::Result<Box<dyn Layer>> {
    let mut reg = Registry::new();
    cray_layers::register(&mut reg);
    let factory = reg.get("xhttp2").expect("xhttp2 is registered");
    let toml_str = format!(
        r#"
host = "{host}"
path = "{path}"
method = "{method}"
config = "{config}"
mode = "{mode}"
"#
    );
    let params: toml::Table = toml_str.parse().unwrap();
    factory(&params, &BuildCx { side })
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
}

// ============ packet-up ============
use std::future::Future;
use std::time::Duration;

const PACKET_MODE: &str = "PacketUp";

async fn within<F: Future>(what: &str, f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), f)
        .await
        .unwrap_or_else(|_| panic!("{what}: timed out"))
}

fn packet_layer(profile: &std::path::Path, side: Side) -> Box<dyn Layer> {
    build_xhttp2_mode(
        "127.0.0.1",
        "/split-http",
        "POST",
        profile.to_str().unwrap(),
        side,
        PACKET_MODE,
    )
    .expect("build layer")
}

fn spawn_server_handshake(
    layer: Box<dyn Layer>,
    wire: tokio::io::DuplexStream,
) -> tokio::task::JoinHandle<BoxedIncoming> {
    tokio::spawn(async move {
        let mut cx = Cx::default();
        match layer
            .call(Input::Stream(Box::pin(wire)), &mut cx)
            .await
            .expect("server handshake")
        {
            Output::Streams(incoming) => incoming,
            _ => panic!("expected Output::Streams"),
        }
    })
}

async fn packet_up_pair(tag: &str) -> (BoxedStream, BoxedStream, BoxedIncoming) {
    init_tracing();
    let profile = make_packet_profile(tag);
    let (client_wire, server_wire) = duplex(1 << 20);

    let server = spawn_server_handshake(packet_layer(&profile, Side::Inbound), server_wire);
    let client_layer = packet_layer(&profile, Side::Outbound);
    let client = tokio::spawn(async move {
        let mut cx = Cx::default();
        match client_layer
            .call(Input::Stream(Box::pin(client_wire)), &mut cx)
            .await
            .expect("client handshake")
        {
            Output::Stream(s) => s,
            _ => panic!("expected Output::Stream"),
        }
    });

    let mut incoming = within("server handshake", server).await.unwrap();
    let client_stream = within("client handshake", client).await.unwrap();
    let server_stream = within("accept", incoming.accept())
        .await
        .expect("accept returned None");
    (client_stream, server_stream, incoming)
}

fn first_diff(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y)
}

#[tokio::test]
async fn test_xhttp2_packet_up_roundtrip_and_fin() {
    let (mut client, mut server, _incoming) = packet_up_pair("pu-roundtrip").await;

    let parts: [&[u8]; 3] = [b"one|", b"two|", b"three"];
    for part in parts {
        client.write_all(part).await.expect("client write");
    }
    let mut buf = vec![0u8; 13];
    within("server read", server.read_exact(&mut buf))
        .await
        .expect("server read");
    assert_eq!(&buf, b"one|two|three");

    server.write_all(b"pong").await.expect("server write");
    let mut back = [0u8; 4];
    within("client read", client.read_exact(&mut back))
        .await
        .expect("client read");
    assert_eq!(&back, b"pong");

    client.shutdown().await.expect("client shutdown");
    let mut rest = Vec::new();
    let n = within("EOF after FIN", server.read_to_end(&mut rest))
        .await
        .expect("read_to_end");
    assert_eq!(n, 0, "server must see EOF right after FIN");
}

#[tokio::test]
async fn test_xhttp2_packet_up_large_payload_in_order() {
    const SIZE: usize = 4 * 1024 * 1024;
    let (client, server, _incoming) = packet_up_pair("pu-large").await;

    let up: Vec<u8> = (0..SIZE).map(|i| ((i * 31) % 251) as u8).collect();
    let down: Vec<u8> = (0..SIZE).map(|i| ((i * 47 + 113) % 251) as u8).collect();
    let (mut cr, mut cw) = tokio::io::split(client);
    let (mut sr, mut sw) = tokio::io::split(server);

    let mut got_up = vec![0u8; SIZE];
    let mut got_down = vec![0u8; SIZE];
    let (w1, r1, w2, r2) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(
            cw.write_all(&up),
            sr.read_exact(&mut got_up),
            sw.write_all(&down),
            cr.read_exact(&mut got_down),
        )
    })
    .await
    .expect("transfer timed out");
    w1.expect("client write");
    r1.expect("server read");
    w2.expect("server write");
    r2.expect("client read");

    assert!(
        got_up == up,
        "client -> server mismatch, first diff at {:?}",
        first_diff(&got_up, &up)
    );
    assert!(
        got_down == down,
        "server -> client mismatch, first diff at {:?}",
        first_diff(&got_down, &down)
    );
}

fn raw_req(method: &str, sid: &str, seq: Option<u64>) -> Request<()> {
    let mut b = Request::builder()
        .method(method)
        .uri("https://127.0.0.1/split-http")
        .header("x-session-id", sid)
        .header("user-agent", "cray-test");
    if let Some(s) = seq {
        b = b.header("x-packet-seq", s.to_string());
    }
    b.body(()).unwrap()
}

fn send_post(
    sr: &mut http2::client::SendRequest<Bytes>,
    sid: &str,
    seq: u64,
    body: &'static [u8],
) -> http2::client::ResponseFuture {
    let (resp, mut stream) = sr
        .send_request(raw_req("POST", sid, Some(seq)), false)
        .expect("send POST");
    stream
        .send_data(Bytes::from_static(body), true)
        .expect("send POST body");
    resp
}

async fn body_ends_cleanly(mut body: http2::RecvStream) -> bool {
    while let Some(chunk) = body.data().await {
        match chunk {
            Ok(c) => {
                let _ = body.flow_control().release_capacity(c.len());
            }
            Err(_) => return false,
        }
    }
    true
}

#[tokio::test]
async fn test_xhttp2_packet_up_server_reorders_raw_posts() {
    init_tracing();
    let profile = make_packet_profile("pu-server-raw");
    let (client_wire, server_wire) = duplex(1 << 20);
    let server = spawn_server_handshake(packet_layer(&profile, Side::Inbound), server_wire);
    let mut sr = manual_h2_client(client_wire).await;
    let mut incoming = within("server handshake", server).await.unwrap();
    let sid = "c".repeat(32);

    let (_get_resp, _get_body) = sr
        .send_request(raw_req("GET", &sid, None), true)
        .expect("send GET");
    let mut server_stream = within("accept", incoming.accept())
        .await
        .expect("accept returned None");

    let r2 = send_post(&mut sr, &sid, 2, b"cc");
    let r0 = send_post(&mut sr, &sid, 0, b"aa");
    let r1 = send_post(&mut sr, &sid, 1, b"bb");

    let mut buf = [0u8; 6];
    within("read reordered", server_stream.read_exact(&mut buf))
        .await
        .expect("read");
    assert_eq!(&buf, b"aabbcc", "server must deliver in seq order");

    for (name, resp) in [("seq 2", r2), ("seq 0", r0), ("seq 1", r1)] {
        let resp = within(name, resp).await.expect("POST response");
        assert_eq!(resp.status().as_u16(), 200, "{name}");
        assert!(
            within(name, body_ends_cleanly(resp.into_body())).await,
            "{name}: response stream was reset instead of closed"
        );
    }

    // duplicate of an already delivered seq: acknowledged and dropped, the
    // session stays alive (the client may have retried a POST)
    let dup = send_post(&mut sr, &sid, 1, b"zz");
    let resp = within("duplicate", dup).await.expect("duplicate response");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "duplicate seq must be acknowledged, not rejected"
    );
    assert!(
        within("duplicate body", body_ends_cleanly(resp.into_body())).await,
        "duplicate: response stream was reset instead of closed"
    );

    // the stream goes on in order and the duplicate payload never shows up
    let next = send_post(&mut sr, &sid, 3, b"dd");
    let mut tail = [0u8; 2];
    within("read after duplicate", server_stream.read_exact(&mut tail))
        .await
        .expect("read after duplicate");
    assert_eq!(&tail, b"dd", "duplicate payload must not be delivered");
    let resp = within("seq 3", next).await.expect("POST response");
    assert_eq!(resp.status().as_u16(), 200);
}

/// A rejected request (a reset stream counts too) must not be acknowledged.
async fn assert_rejected(what: &str, resp: http2::client::ResponseFuture) {
    if let Ok(r) = within(what, resp).await {
        assert_ne!(r.status().as_u16(), 200, "{what} must be rejected");
    }
}

/// Only the server: malformed POSTs are rejected, the session survives them.
#[tokio::test]
async fn test_xhttp2_packet_up_server_rejects_malformed_posts() {
    use cray_layers::transport::xhttp2::client::MAX_BODY;

    init_tracing();
    let profile = make_packet_profile("pu-server-malformed");
    let (client_wire, server_wire) = duplex(1 << 20);
    let server = spawn_server_handshake(packet_layer(&profile, Side::Inbound), server_wire);
    let mut sr = manual_h2_client(client_wire).await;
    let mut incoming = within("server handshake", server).await.unwrap();
    let sid = "d".repeat(32);

    let (_get_resp, _get_body) = sr
        .send_request(raw_req("GET", &sid, None), true)
        .expect("send GET");
    let mut server_stream = within("accept", incoming.accept())
        .await
        .expect("accept returned None");

    // no seq header at all
    let (resp, mut stream) = sr
        .send_request(raw_req("POST", &sid, None), false)
        .expect("send POST");
    stream
        .send_data(Bytes::from_static(b"xx"), true)
        .expect("send body");
    assert_rejected("missing seq", resp).await;

    // seq is not a number
    let req = Request::builder()
        .method("POST")
        .uri("https://127.0.0.1/split-http")
        .header("x-session-id", sid.as_str())
        .header("x-packet-seq", "not-a-number")
        .body(())
        .unwrap();
    let (resp, mut stream) = sr.send_request(req, false).expect("send POST");
    stream
        .send_data(Bytes::from_static(b"yy"), true)
        .expect("send body");
    assert_rejected("non-numeric seq", resp).await;

    // body larger than MAX_BODY
    let resp = send_post(&mut sr, &sid, 0, &[7u8; MAX_BODY + 1]);
    assert_rejected("oversized body", resp).await;

    // none of the above consumed seq 0 or leaked bytes: the session is intact
    let ok = send_post(&mut sr, &sid, 0, b"ok");
    let mut buf = [0u8; 2];
    within("read after rejects", server_stream.read_exact(&mut buf))
        .await
        .expect("read after rejects");
    assert_eq!(&buf, b"ok", "rejected requests must not reach the stream");
    let resp = within("valid POST", ok).await.expect("POST response");
    assert_eq!(resp.status().as_u16(), 200);
}

#[tokio::test]
async fn test_xhttp2_mode_validation() {
    let profile = make_packet_profile("mode-validation");
    let p = profile.to_str().unwrap();

    assert!(
        build_xhttp2_mode("example.com", "/test", "POST", p, Side::Inbound, "StreamUp").is_ok()
    );
    assert!(
        build_xhttp2_mode("example.com", "/test", "POST", p, Side::Inbound, "PacketUp").is_ok()
    );
    assert!(build_xhttp2_mode("example.com", "/test", "POST", p, Side::Inbound, "nope").is_err());

    let mut reg = Registry::new();
    cray_layers::register(&mut reg);
    let factory = reg.get("xhttp2").unwrap();
    let no_mode: toml::Table = format!(
        r#"host = "example.com"
path = "/test"
method = "POST"
config = "{p}""#
    )
    .parse()
    .unwrap();
    assert!(
        factory(
            &no_mode,
            &BuildCx {
                side: Side::Inbound
            }
        )
        .is_err()
    );
}
