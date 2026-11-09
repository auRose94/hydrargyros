//! HTTPS tests: the `sslCert`/`sslKey` config pair switches the vhost to
//! TLS on the same port, verified with a real handshake against a
//! self-signed certificate.

#![cfg(feature = "web")]

use std::time::Duration;

use hyg::web::Server;
use hyg::o;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test(flavor = "multi_thread")]
async fn https_serves_with_pem_pair() {
    // both ring and aws-lc-rs land in the feature tree, so rustls refuses to
    // guess a provider — install one before the server task builds its config
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let dir = std::env::temp_dir().join("hyg_tls_test");
    std::fs::create_dir_all(&dir).unwrap();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    std::fs::write(&cert_path, cert.cert.pem()).unwrap();
    std::fs::write(&key_path, cert.key_pair.serialize_pem()).unwrap();

    let mut serv = Server::with_config(o!{
        "host": "127.0.0.1",
        "port": 0,
        "sslCert": cert_path.to_string_lossy().as_ref(),
        "sslKey": key_path.to_string_lossy().as_ref(),
    });
    serv.get("/", |_req| "over tls");
    let s = serv.clone();
    tokio::spawn(async move { let _ = s.start().await; });
    let mut addr = String::new();
    for _ in 0..100 {
        addr = serv.local_addr();
        if !addr.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!addr.is_empty(), "server did not bind in time");

    // a client that trusts exactly this self-signed cert — a real
    // certificate verification, no dangerous-skip mode
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.cert.der().clone()).unwrap();
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));

    let tcp = TcpStream::connect(&addr).await.unwrap();
    let mut tls = connector
        .connect("localhost".try_into().unwrap(), tcp)
        .await
        .unwrap();
    tls.write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), tls.read_to_end(&mut buf))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&buf);
    assert!(text.contains("200 OK"), "{text}");
    assert!(text.contains("over tls"), "{text}");
    serv.stop();
}
