//! OpenTelemetry telemetry integration for Asterisk.
//!
//! Provides distributed tracing capabilities for SIP calls and transactions,
//! enabling observability across hops and systems. Uses OTLP to export traces
//! to observability backends like Jaeger, Zipkin, or cloud providers.

use opentelemetry::{
    global, 
    trace::{Span, SpanKind, Tracer},
    KeyValue,
};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    trace::{self, RandomIdGenerator, Sampler},
    Resource,
};
use opentelemetry_semantic_conventions::resource;
use std::time::Duration;
use tracing::{error, info};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::layer::SubscriberExt;

/// Initialize OpenTelemetry tracing with OTLP exporter.
///
/// Sets up the global tracer provider with OTLP export to the configured
/// endpoint (defaults to http://localhost:4317). Should be called early
/// in application startup before any spans are created.
///
/// # Configuration
/// 
/// Configuration is done via environment variables:
/// - `OTEL_EXPORTER_OTLP_ENDPOINT`: OTLP endpoint (default: http://localhost:4317)
/// - `OTEL_SERVICE_NAME`: Service name (default: asterisk-rs)
/// - `OTEL_SERVICE_VERSION`: Service version (default: 0.1.0)
/// - `OTEL_RESOURCE_ATTRIBUTES`: Additional resource attributes
///
/// # Returns
/// 
/// Returns a guard that should be kept alive for the duration of the application.
/// When dropped, it will flush any remaining spans and shutdown the tracer provider.
pub fn init_telemetry() -> Result<TracingGuard, Box<dyn std::error::Error + Send + Sync>> {
    info!("Initializing OpenTelemetry telemetry");

    // Get configuration from environment
    let service_name = std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "asterisk-rs".to_string());
    let service_version = std::env::var("OTEL_SERVICE_VERSION").unwrap_or_else(|_| "0.1.0".to_string());
    let otlp_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| "http://localhost:4317".to_string());

    info!(
        service_name = %service_name,
        service_version = %service_version,
        otlp_endpoint = %otlp_endpoint,
        "Setting up OpenTelemetry OTLP exporter"
    );

    // Build resource with service information
    let resource = Resource::new([
        resource::SERVICE_NAME.string(service_name.clone()),
        resource::SERVICE_VERSION.string(service_version),
        KeyValue::new("service.instance.id", uuid::Uuid::new_v4().to_string()),
        KeyValue::new("telemetry.sdk.language", "rust"),
        KeyValue::new("telemetry.sdk.name", "opentelemetry"),
    ]);

    // Create OTLP trace exporter
    let otlp_exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(&otlp_endpoint)
        .with_timeout(Duration::from_secs(3));

    // Build tracer provider
    let tracer_provider = trace::TracerProvider::builder()
        .with_batch_exporter(otlp_exporter, trace::BatchConfig::default())
        .with_resource(resource)
        .with_id_generator(RandomIdGenerator::default())
        .with_sampler(Sampler::TraceIdRatioBased(1.0)) // Sample all traces
        .build();

    // Set global tracer provider
    global::set_tracer_provider(tracer_provider.clone());

    // Create OpenTelemetry tracing layer
    let tracer = global::tracer("asterisk-rs");
    let telemetry_layer = OpenTelemetryLayer::new(tracer);

    info!("OpenTelemetry telemetry initialized successfully");

    Ok(TracingGuard {
        _tracer_provider: tracer_provider,
    })
}

/// Guard that ensures proper shutdown of the tracer provider.
/// Keep this alive for the duration of the application.
pub struct TracingGuard {
    _tracer_provider: trace::TracerProvider,
}

impl Drop for TracingGuard {
    fn drop(&mut self) {
        info!("Shutting down OpenTelemetry telemetry");
        global::shutdown_tracer_provider();
    }
}

/// Creates a span for a SIP transaction.
///
/// # Arguments
/// 
/// * `transaction_id` - Unique transaction identifier (branch parameter)
/// * `method` - SIP method (INVITE, ACK, BYE, etc.)
/// * `call_id` - SIP Call-ID header value
/// * `from_tag` - From tag for transaction correlation
/// * `to_tag` - To tag for transaction correlation (may be empty for client transactions)
/// * `remote_addr` - Remote endpoint address
///
/// # Returns
/// 
/// Returns an active span that should be kept in scope while processing the transaction.
pub fn create_sip_transaction_span(
    transaction_id: &str,
    method: &str,
    call_id: &str,
    from_tag: Option<&str>,
    to_tag: Option<&str>,
    remote_addr: &std::net::SocketAddr,
) -> impl Span {
    let tracer = global::tracer("asterisk-sip");
    
    let mut span_builder = tracer
        .span_builder(format!("sip.transaction.{}", method.to_lowercase()))
        .with_kind(SpanKind::Server)
        .with_attributes(vec![
            KeyValue::new("sip.method", method.to_string()),
            KeyValue::new("sip.call_id", call_id.to_string()),
            KeyValue::new("sip.transaction.id", transaction_id.to_string()),
            KeyValue::new("net.peer.ip", remote_addr.ip().to_string()),
            KeyValue::new("net.peer.port", remote_addr.port() as i64),
        ]);

    if let Some(from_tag) = from_tag {
        span_builder = span_builder.with_attributes([KeyValue::new("sip.from.tag", from_tag.to_string())]);
    }
    
    if let Some(to_tag) = to_tag {
        span_builder = span_builder.with_attributes([KeyValue::new("sip.to.tag", to_tag.to_string())]);
    }

    span_builder.start(&tracer)
}

/// Creates a span for a SIP call (multiple transactions).
///
/// # Arguments
/// 
/// * `call_id` - SIP Call-ID header value
/// * `from_uri` - From URI
/// * `to_uri` - To URI
/// * `user_agent` - User-Agent header value
///
/// # Returns
/// 
/// Returns an active span that should be kept in scope while processing the call.
pub fn create_sip_call_span(
    call_id: &str,
    from_uri: &str,
    to_uri: &str,
    user_agent: Option<&str>,
) -> impl Span {
    let tracer = global::tracer("asterisk-sip");
    
    let mut span_builder = tracer
        .span_builder("sip.call")
        .with_kind(SpanKind::Server)
        .with_attributes(vec![
            KeyValue::new("sip.call_id", call_id.to_string()),
            KeyValue::new("sip.from.uri", from_uri.to_string()),
            KeyValue::new("sip.to.uri", to_uri.to_string()),
        ]);

    if let Some(user_agent) = user_agent {
        span_builder = span_builder.with_attributes([KeyValue::new("sip.user_agent", user_agent.to_string())]);
    }

    span_builder.start(&tracer)
}

/// Extract trace context from SIP headers.
///
/// Looks for trace context in custom SIP headers following W3C trace context format.
/// The trace context is typically carried in X-Trace-Id and X-Span-Id headers.
///
/// # Arguments
/// 
/// * `headers` - SIP message headers as key-value pairs
///
/// # Returns
/// 
/// Returns the extracted trace context that can be used to continue a distributed trace.
pub fn extract_sip_trace_context(
    headers: &std::collections::HashMap<String, String>
) -> Option<opentelemetry::Context> {
    use opentelemetry::trace::TraceContextExt;

    // Look for W3C trace context in X-Trace-* headers
    let trace_id_header = headers.get("X-Trace-Id").or_else(|| headers.get("x-trace-id"))?;
    let span_id_header = headers.get("X-Span-Id").or_else(|| headers.get("x-span-id"))?;

    // Parse trace ID and span ID
    let trace_id = opentelemetry::trace::TraceId::from_hex(trace_id_header).ok()?;
    let span_id = opentelemetry::trace::SpanId::from_hex(span_id_header).ok()?;
    
    // Get trace flags (default to sampled)
    let trace_flags = headers
        .get("X-Trace-Flags")
        .or_else(|| headers.get("x-trace-flags"))
        .and_then(|flags| flags.parse().ok())
        .unwrap_or(0x01); // Default to sampled

    let trace_state = headers
        .get("X-Trace-State")
        .or_else(|| headers.get("x-trace-state"))
        .and_then(|state| state.parse().ok())
        .unwrap_or_default();

    // Create span context
    let span_context = opentelemetry::trace::SpanContext::new(
        trace_id,
        span_id,
        opentelemetry::trace::TraceFlags::new(trace_flags),
        false, // is_remote
        trace_state,
    );

    // Create context with span
    let context = opentelemetry::Context::current();
    Some(context.with_span(opentelemetry::trace::NoopSpan::new(span_context)))
}

/// Inject trace context into SIP headers.
///
/// Adds trace context to SIP headers in W3C trace context format.
/// This allows trace context to be propagated across SIP hops.
///
/// # Arguments
/// 
/// * `headers` - Mutable reference to SIP message headers
/// * `span` - Current span whose context should be injected
pub fn inject_sip_trace_context(
    headers: &mut std::collections::HashMap<String, String>,
    span: &dyn Span,
) {
    let span_context = span.span_context();
    
    if span_context.is_valid() {
        headers.insert("X-Trace-Id".to_string(), span_context.trace_id().to_hex());
        headers.insert("X-Span-Id".to_string(), span_context.span_id().to_hex());
        headers.insert("X-Trace-Flags".to_string(), span_context.trace_flags().to_u8().to_string());
        
        if !span_context.trace_state().is_empty() {
            headers.insert("X-Trace-State".to_string(), span_context.trace_state().to_string());
        }
    }
}

/// Add SIP transaction outcome to span.
///
/// Records the final outcome of a SIP transaction (success, timeout, error).
///
/// # Arguments
/// 
/// * `span` - The transaction span
/// * `status_code` - Final SIP response status code (if any)
/// * `reason` - Human readable outcome reason
pub fn record_sip_transaction_outcome(
    span: &mut dyn Span,
    status_code: Option<u16>,
    reason: &str,
) {
    if let Some(code) = status_code {
        span.set_attribute(KeyValue::new("sip.response.status_code", code as i64));
        
        // Set span status based on SIP status code
        if (200..300).contains(&code) {
            span.set_status(opentelemetry::trace::Status::Ok);
        } else if (400..500).contains(&code) {
            span.set_status(opentelemetry::trace::Status::error("Client error"));
        } else if (500..600).contains(&code) {
            span.set_status(opentelemetry::trace::Status::error("Server error"));
        }
    }
    
    span.set_attribute(KeyValue::new("sip.transaction.outcome", reason.to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_trace_context_roundtrip() {
        // Create a mock span with trace context
        let tracer = global::tracer("test");
        let span = tracer.span_builder("test").start(&tracer);
        
        let mut headers = HashMap::new();
        inject_sip_trace_context(&mut headers, &span);
        
        // Should have trace headers
        assert!(headers.contains_key("X-Trace-Id"));
        assert!(headers.contains_key("X-Span-Id"));
        assert!(headers.contains_key("X-Trace-Flags"));
        
        // Extract should work
        let extracted_context = extract_sip_trace_context(&headers);
        assert!(extracted_context.is_some());
    }
}