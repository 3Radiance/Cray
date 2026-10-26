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
