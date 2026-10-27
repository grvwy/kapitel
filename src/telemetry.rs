//! Optional, allowlisted operational traces. Never bridge application logs or request data.
use crate::AppState;
use anyhow::{Context, ensure};
use axum::{
    extract::{MatchedPath, Request, State},
    middleware::Next,
    response::Response,
};
use opentelemetry::{
    KeyValue,
    trace::{Span, SpanKind, Status, Tracer, TracerProvider},
};
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    Resource,
    trace::{BatchConfigBuilder, BatchSpanProcessor, SdkTracer, SdkTracerProvider},
};
use std::time::{Duration, SystemTime};

pub struct Telemetry(SdkTracerProvider);

impl Telemetry {
    pub fn from_environment() -> anyhow::Result<Option<Self>> {
        let enabled = std::env::var("KAPITEL_TELEMETRY_ENABLED").ok();
        let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
        let Some(endpoint) = export_endpoint(enabled.as_deref(), endpoint.as_deref())? else {
            return Ok(None);
        };
        // Redirects must not forward telemetry to a different destination.
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .build()
            .context("Cannot initialize telemetry HTTP client")?;
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_http_client(client)
            .with_endpoint(endpoint)
            .with_timeout(Duration::from_secs(3))
            .build()
            .map_err(|_| anyhow::anyhow!("Cannot initialize OTLP trace exporter"))?;
        let processor = BatchSpanProcessor::builder(exporter)
            .with_batch_config(
                BatchConfigBuilder::default()
                    .with_max_queue_size(256)
                    .with_max_export_batch_size(64)
                    .with_scheduled_delay(Duration::from_secs(5))
                    .build(),
            )
            .build();
        let provider = SdkTracerProvider::builder()
            .with_span_processor(processor)
            // Do not auto-detect hostnames, process details or resource environment variables.
            .with_resource(
                Resource::builder_empty()
                    .with_service_name("kapitel")
                    .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
                    .build(),
            )
            .build();
        Ok(Some(Self(provider)))
    }

    pub fn tracer(&self) -> SdkTracer {
        self.0.tracer("kapitel")
    }

    pub fn shutdown(self) {
        if self
            .0
            .shutdown_with_timeout(Duration::from_secs(5))
            .is_err()
        {
            tracing::warn!("Telemetry shutdown could not flush all pending spans");
        }
    }
}

fn export_endpoint(
    enabled: Option<&str>,
    endpoint: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let enabled = enabled
        .unwrap_or("false")
        .parse::<bool>()
        .context("KAPITEL_TELEMETRY_ENABLED must be true or false")?;
    if !enabled {
        return Ok(None);
    }
    let endpoint = endpoint.context("Telemetry requires OTEL_EXPORTER_OTLP_ENDPOINT")?;
    let mut endpoint = url::Url::parse(endpoint)
        .map_err(|_| anyhow::anyhow!("OTEL_EXPORTER_OTLP_ENDPOINT must be an HTTP(S) base URL"))?;
    ensure!(
        matches!(endpoint.scheme(), "http" | "https")
            && endpoint.host_str().is_some()
            && endpoint.username().is_empty()
            && endpoint.password().is_none()
            && endpoint.query().is_none()
            && endpoint.fragment().is_none(),
        "OTEL_EXPORTER_OTLP_ENDPOINT must be an HTTP(S) base URL without credentials, query or fragment"
    );
    endpoint.set_path(&format!(
        "{}/v1/traces",
        endpoint.path().trim_end_matches('/')
    ));
    Ok(Some(endpoint.into()))
}

pub async fn request(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(tracer) = &state.telemetry else {
        return next.run(request).await;
    };
    let Some(route) = request.extensions().get::<MatchedPath>().cloned() else {
        return next.run(request).await;
    };
    if !route.as_str().starts_with("/api/") {
        return next.run(request).await;
    }
    let method = match request.method().as_str() {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "HEAD" => "HEAD",
        _ => "_OTHER",
    };
    let started_at = SystemTime::now();
    let response = next.run(request).await;
    // Successful progress saves, range requests and cover loads are high-volume noise.
    let routine = route.as_str().ends_with("/position")
        || route.as_str().starts_with("/api/media/")
        || route.as_str().ends_with("/cover");
    if routine && response.status().is_success() {
        return response;
    }
    let mut span = tracer
        .span_builder(format!("{method} {}", route.as_str()))
        .with_kind(SpanKind::Server)
        .with_start_time(started_at)
        .with_attributes([
            KeyValue::new("http.request.method", method),
            KeyValue::new("http.route", route.as_str().to_owned()),
            KeyValue::new(
                "http.response.status_code",
                i64::from(response.status().as_u16()),
            ),
        ])
        .start(tracer);
    if response.status().is_server_error() {
        span.set_status(Status::error("server_error"));
    }
    span.end();
    response
}

pub async fn scan_finished(state: &AppState, started_at: SystemTime, succeeded: bool) {
    let Some(tracer) = &state.telemetry else {
        return;
    };
    let mut span = tracer
        .span_builder("library.scan")
        .with_start_time(started_at)
        .with_attributes([KeyValue::new("scan.succeeded", succeeded)])
        .start(tracer);
    if !succeeded {
        span.set_status(Status::error("scan_failed"));
    }
    // Counts only. Error messages and paths stay in the administrator's library view.
    if let Ok((books, issues)) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT (SELECT count(*) FROM books), (SELECT count(*) FROM index_issues)",
    )
    .fetch_one(&state.database)
    .await
    {
        span.set_attribute(KeyValue::new("library.book_count", books));
        span.set_attribute(KeyValue::new("scan.issue_count", issues));
    }
    span.end();
}

#[cfg(test)]
mod tests {
    use super::export_endpoint;

    #[test]
    fn export_requires_opt_in_and_a_valid_explicit_destination() {
        assert_eq!(export_endpoint(None, None).unwrap(), None);
        assert_eq!(
            export_endpoint(None, Some("http://collector:4318")).unwrap(),
            None
        );
        assert_eq!(
            export_endpoint(Some("false"), Some("invalid")).unwrap(),
            None
        );
        assert!(export_endpoint(Some("yes"), None).is_err());
        assert!(export_endpoint(Some("true"), None).is_err());
        for endpoint in [
            "",
            "invalid",
            "ftp://collector",
            "http://user:secret@collector",
            "http://collector?q=secret",
            "http://collector/#secret",
        ] {
            let error = export_endpoint(Some("true"), Some(endpoint)).unwrap_err();
            assert!(!error.to_string().contains("secret"));
        }
        for endpoint in ["https://collector/prefix", "https://collector/prefix/"] {
            assert_eq!(
                export_endpoint(Some("true"), Some(endpoint)).unwrap(),
                Some("https://collector/prefix/v1/traces".into())
            );
        }
    }
}
