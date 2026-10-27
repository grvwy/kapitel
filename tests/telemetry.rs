use axum::{Router, body::Bytes, http::HeaderMap, routing::post};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use std::{
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_server(directory: &std::path::Path, enabled: bool, endpoint: &str) -> Server {
    Server(
        Command::new(env!("CARGO_BIN_EXE_kapitel"))
            .arg("serve")
            .env_clear()
            .env("KAPITEL_DATA_DIRECTORY", directory.join("data"))
            .env("KAPITEL_LIBRARY_DIRECTORY", directory.join("library"))
            .env("KAPITEL_TELEMETRY_ENABLED", enabled.to_string())
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint)
            .env(
                "OTEL_EXPORTER_OTLP_HEADERS",
                "Authorization=Bearer%20synthetic-secret",
            )
            .env("OTEL_RESOURCE_ATTRIBUTES", "private.data=must-not-export")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

async fn wait_until_ready(client: &reqwest::Client) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(response) = client.get("http://127.0.0.1:8080/health").send().await
                && response.status().is_success()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("Kapitel must become ready even when telemetry is unavailable");
}

#[tokio::test]
async fn actual_otlp_export_is_opt_in_private_and_independent_of_collector_availability() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("library")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/prefix", listener.local_addr().unwrap());
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let collector = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/prefix/v1/traces",
                post(move |headers: HeaderMap, body: Bytes| {
                    let sender = sender.clone();
                    async move {
                        assert_eq!(headers["authorization"], "Bearer synthetic-secret");
                        assert_eq!(headers["content-type"], "application/x-protobuf");
                        sender
                            .send(ExportTraceServiceRequest::decode(body).unwrap())
                            .unwrap();
                        ([("content-type", "application/x-protobuf")], "")
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    {
        let _disabled_server = start_server(directory.path(), false, &endpoint);
        wait_until_ready(&client).await;
        client
            .get("http://127.0.0.1:8080/api/books?search=private-search")
            .send()
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(6), receiver.recv())
                .await
                .is_err(),
            "No export without opt-in, even with an endpoint configured"
        );
    }
    {
        let _enabled_server = start_server(directory.path(), true, &endpoint);
        wait_until_ready(&client).await;
        for path in [
            "/api/books?search=private-search",
            "/api/books/private-book-id",
            "/api/playback-sessions/private-session-id/position",
        ] {
            client
                .get(format!("http://127.0.0.1:8080{path}"))
                .header("cookie", "secret-cookie")
                .header("user-agent", "secret-device")
                .send()
                .await
                .unwrap();
        }
        let export = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        let decoded = format!("{export:?}");
        for forbidden in [
            "private-search",
            "private-book-id",
            "private-session-id",
            "secret-cookie",
            "secret-device",
            "synthetic-secret",
            "must-not-export",
            directory.path().to_str().unwrap(),
        ] {
            assert!(
                !decoded.contains(forbidden),
                "Private data leaked: {forbidden}"
            );
        }
        let resource = &export.resource_spans[0];
        let resource_keys: Vec<_> = resource
            .resource
            .as_ref()
            .unwrap()
            .attributes
            .iter()
            .map(|attribute| attribute.key.as_str())
            .collect();
        assert_eq!(resource_keys.len(), 2);
        assert!(
            resource_keys.contains(&"service.name") && resource_keys.contains(&"service.version")
        );
        let spans = &resource.scope_spans[0].spans;
        assert!(
            spans
                .iter()
                .any(|span| span.name == "GET /api/books/{book_id}")
        );
        assert!(spans.iter().any(|span| span.name == "library.scan"));
        assert!(!spans.iter().any(|span| span.name.contains("/health")));
        for span in spans {
            assert!(span.end_time_unix_nano >= span.start_time_unix_nano);
            for attribute in &span.attributes {
                assert!(
                    [
                        "http.request.method",
                        "http.route",
                        "http.response.status_code",
                        "scan.succeeded",
                        "library.book_count",
                        "scan.issue_count"
                    ]
                    .contains(&attribute.key.as_str())
                );
            }
        }
    }
    collector.abort();
    let _ = collector.await;
    let _unavailable_collector_server = start_server(directory.path(), true, &endpoint);
    wait_until_ready(&client).await;
    for _ in 0..300 {
        assert_eq!(
            client
                .get("http://127.0.0.1:8080/api/books")
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    assert!(
        client
            .get("http://127.0.0.1:8080/health")
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
}

#[test]
fn enabled_telemetry_without_endpoint_fails_before_serving() {
    let output = Command::new(env!("CARGO_BIN_EXE_kapitel"))
        .arg("serve")
        .env_clear()
        .env("KAPITEL_TELEMETRY_ENABLED", "true")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Telemetry requires OTEL_EXPORTER_OTLP_ENDPOINT")
    );
}
