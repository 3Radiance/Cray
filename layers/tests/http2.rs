use cray_core::*;
use http::Request;
use std::io::Write;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

fn write_profile(tag: &str, content: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cray-h2-test-{tag}-{}-{}",
        std::process::id(),
        fastrand::u64(..)
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let profile_path = dir.join("profile.toml");
    let mut file = std::fs::File::create(&profile_path).expect("create profile file");
    file.write_all(content.as_bytes()).expect("write profile");
    profile_path
}

fn make_flat_profile(tag: &str) -> PathBuf {
    write_profile(
        tag,
        r#"
[settings]
header_table_size = 4096
enable_push = false
initial_window_size = 65535
max_frame_size = 16384

[http_headers]
"user-agent" = "cray-test"
"#,
    )
}

fn make_response_reject_profile(tag: &str, body_path: &str) -> PathBuf {
    write_profile(
        tag,
        &format!(
            r#"
reject_status = 505

[settings]
header_table_size = 4096
enable_push = false
initial_window_size = 65535
max_frame_size = 16384

[response]
headers_order = ["server", "content-type"]

[response.http_headers]
server = "cray-test"
content-type = "application/grpc"

[reject]
headers_order = ["x-reject", "content-type"]
body_path = "{body_path}"

[reject.http_headers]
x-reject = "cray-page"
content-type = "text/html"
"#
        ),
    )
}

fn build_http2(
    host: &str,
    path: &str,
    method: &str,
    config: Option<&str>,
    side: Side,
) -> anyhow::Result<Box<dyn Layer>> {
    let mut reg = Registry::new();
    cray_layers::register(&mut reg);
    let factory = reg.get("http2").expect("http2 is registered");
    let mut toml_str = format!(
        r#"
host = "{host}"
path = "{path}"
method = "{method}"
"#
    );
    if let Some(config) = config {
        toml_str.push_str(&format!("config = \"{config}\"\n"));
    }
    let params: toml::Table = toml_str.parse().unwrap();
    factory(&params, &BuildCx { side })
}

async fn read_body(mut body: http2::RecvStream) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let done = std::future::poll_fn(|cx| {
            loop {
                match body.poll_data(cx) {
                    std::task::Poll::Pending => return std::task::Poll::Pending,
                    std::task::Poll::Ready(None) => return std::task::Poll::Ready(true),
                    std::task::Poll::Ready(Some(Err(_))) => return std::task::Poll::Ready(true),
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

fn assert_header(resp: &http::Response<http2::RecvStream>, name: &str, expected: &str) {
    let got = resp
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_else(|| panic!("missing header '{name}'"));
    assert_eq!(got, expected, "unexpected value of header '{name}'");
}

#[tokio::test]
async fn test_http2_roundtrip_data() {
    let profile = make_flat_profile("roundtrip");
    let profile_str = profile.to_str().unwrap();

    let server_layer = build_http2(
        "127.0.0.1",
        "/tun",
        "POST",
        Some(profile_str),
        Side::Inbound,
    )
    .expect("server build");
    let client_layer = build_http2(
        "127.0.0.1",
        "/tun",
        "POST",
        Some(profile_str),
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
            Output::Streams(inc) => inc,
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
            Output::Stream(s) => s,
            _ => panic!("expected Output::Stream"),
        }
    });

    let (mut incoming, mut client_stream) = tokio::try_join!(
        async { server_handle.await.map_err(|e| anyhow::anyhow!(e)) },
        async { client_handle.await.map_err(|e| anyhow::anyhow!(e)) }
    )
    .expect("handshakes finished");

    let mut server_stream =
        tokio::time::timeout(std::time::Duration::from_secs(5), incoming.accept())
            .await
            .expect("accept timed out")
            .expect("accepted stream");

    let msg_from_client = b"hello from client over h2!";
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

    let msg_from_server = b"hello back from server over h2!";
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
async fn test_http2_response_headers_and_reject_page() {
    let html = "<html><body>cray says no</body></html>";
    let body_path = std::env::temp_dir().join(format!(
        "cray-h2-reject-{}-{}.html",
        std::process::id(),
        fastrand::u64(..)
    ));
    std::fs::write(&body_path, html).expect("write reject page");

    let profile = make_response_reject_profile("resp-reject", body_path.to_str().unwrap());
    let profile_str = profile.to_str().unwrap().to_string();

    let server_layer = build_http2(
        "127.0.0.1",
        "/tun",
        "POST",
        Some(&profile_str),
        Side::Inbound,
    )
    .expect("server build");

    let (client_wire, server_wire) = duplex(1 << 20);
    tokio::spawn(async move {
        let mut cx = Cx::default();
        let output = server_layer
            .call(Input::Stream(Box::pin(server_wire)), &mut cx)
            .await
            .expect("server handshake");
        match output {
            Output::Streams(mut incoming) => {
                // Keep the acceptor alive so the server keeps answering probes.
                while incoming.accept().await.is_some() {}
            }
            _ => panic!("expected Output::Streams"),
        }
    });

    let (send_req, conn) = http2::client::handshake(client_wire)
        .await
        .expect("client handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sr = send_req.ready().await.expect("client ready");

    // Matching request -> 200 with the [response] headers.
    let req = Request::builder()
        .method("POST")
        .uri("https://127.0.0.1/tun")
        .body(())
        .unwrap();
    let (resp, _stream) = sr.send_request(req, true).expect("send matching probe");
    let resp = resp.await.expect("success response");

    assert_eq!(resp.status().as_u16(), 200);
    assert_header(&resp, "server", "cray-test");
    assert_header(&resp, "content-type", "application/grpc");

    // Non-matching request -> reject_status with the [reject] headers and body.
    let req = Request::builder()
        .method("GET")
        .uri("https://127.0.0.1/wrong")
        .body(())
        .unwrap();
    let (resp2, _stream) = sr.send_request(req, true).expect("send reject probe");
    let resp2 = resp2.await.expect("reject response");

    assert_eq!(resp2.status().as_u16(), 505);
    assert_header(&resp2, "x-reject", "cray-page");
    assert_header(&resp2, "content-type", "text/html");
    assert_eq!(read_body(resp2.into_body()).await, html.as_bytes());
}
