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
        fastrand::u64(..)
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
    let mut reg = Registry::new();
    cray_layers::register(&mut reg);
    let factory = reg.get("xhttp2").expect("xhttp2 is registered");
    let toml_str = format!(
        r#"
host = "{host}"
path = "{path}"
method = "{method}"
config = "{config}"
"#
    );
    let params: toml::Table = toml_str.parse().unwrap();
    factory(&params, &BuildCx { side })
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
        let method = if fastrand::bool() { "POST" } else { "PUT" };

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
        fastrand::u64(..)
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
        let done = std::future::poll_fn(|cx| {
            loop {
                match body.poll_data(cx) {
                    std::task::Poll::Pending => return std::task::Poll::Pending,
                    std::task::Poll::Ready(None) => return std::task::Poll::Ready(true),
                    std::task::Poll::Ready(Some(Err(_))) => {
                        return std::task::Poll::Ready(true)
                    }
                    std::task::Poll::Ready(Some(Ok(chunk))) => {
                        let _ = body.flow_control().release_capacity(chunk.len());
                        out.extend_from_slice(&chunk);
                        return std::task::Poll::Ready(false);
                    }
                }
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
        fastrand::u64(..)
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
    let _server = spawn_layer_server(server_layer, server_wire);

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
    let _server = spawn_layer_server(server_layer, server_wire);

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
