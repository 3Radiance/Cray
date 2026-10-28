use cray_core::*;
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

[http_headers]
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
