# TODO

## pb_http connection reuse

- Add a client-side connection pool for `pb_http` instead of opening a new TCP/QUIC connection for every proxied TCP stream.
- Pool keys should include at least:
  - endpoint
  - transport (`h2`, `h2c`, `h3`)
  - TLS server name
  - TLS verification / CA options
  - relevant h2/h3/QUIC tuning options
- For `h2`/`h2c`, reuse `h2::client::SendRequest<Bytes>` and open one HTTP/2 stream per proxied TCP stream.
- For `h3`, reuse the QUIC/H3 connection and `h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>`.
- Keep at least one H3 `SendRequest` handle alive for the lifetime of the pooled connection; dropping the last handle closes the H3 connection with `H3_NO_ERROR`.
- Track per-connection active stream count and enforce a configurable max concurrent streams per pooled connection.
- Evict pooled connections on:
  - connection driver error
  - GOAWAY / graceful shutdown
  - protocol error
  - idle timeout
  - config key mismatch after reload
- Add backoff for repeated connection failures to avoid reconnect storms.

## pb_http defaults and options

- Add configurable pool settings:
  - `pool_max_idle_per_key`
  - `pool_max_active_streams_per_connection`
  - `pool_idle_timeout_ms`
  - `pool_connect_backoff_ms`
  - `pool_connect_backoff_max_ms`
- Keep current behavior available with `pool_enabled = false` for debugging and compatibility.
- Review h3 defaults after pooling:
  - QUIC idle timeout
  - keep-alive interval
  - stream receive window
  - connection receive window
  - max concurrent bidirectional streams
- Consider using separate defaults for latency-oriented and throughput-oriented deployments.

## pb_http tests

- Add integration tests that verify multiple proxied TCP streams reuse one h2/h3 connection.
- Add tests for pooled connection eviction after remote close.
- Add tests for h3 `SendRequest` lifetime so the connection is not closed while active streams exist.
- Add stress tests for:
  - many short TCP streams
  - long-lived TCP streams
  - mixed short and long streams
  - fallback from h3 to h2
- Add a regression test for the previous H3 early-close bug.

## raddy interop

- Keep validating this deployment shape:
  - client `pb_http` over h2/h3
  - raddy terminates TLS and H3
  - raddy proxies `/pb_http/` to rog over h2c
  - rog server-side `pb_http` only listens on h2c
- Keep ordinary HTTP/3 request checks alongside `pb_http` stream checks.
