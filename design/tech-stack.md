# Email Ingestor: technology stack

Status: Rust is confirmed; libraries are proposed choices to validate during implementation. This scope uses IMAP-only durable state, one processing path, and fixed operational defaults. See [architecture](architecture.md), [requirements](requirements.md), and [test plan](test-cases.md).

## 1. Proposed stack

| Area | Choice | Purpose |
| --- | --- | --- |
| Language / packaging | Rust stable, Cargo, one binary | Container entrypoint; no CLI configuration layer |
| Runtime | Tokio | Async networking, bounded scheduling, monotonic deadlines, signals, test timers |
| IMAP | `async-imap` candidate, behind a bounded byte-source adapter | UID/IDLE/MOVE plus bounded partial PEEK or incremental literal reads; prove parser allocation limits |
| TLS | `tokio-rustls` / `rustls` | Implicit TLS with validated public/private trust roots |
| SMTP | Streaming-capable async SMTP adapter; library selection gated | Separate envelope, incremental DATA framing, backpressure, explicit abort/final acceptance |
| Configuration | `std::env` and typed validation | Small environment-only option surface and secret/CA file references |
| Errors | `thiserror` | Explicit temporary, uncertain, and fatal outcomes |
| Logging | `tracing` / `tracing-subscriber` JSON | Sanitized scoped events and failure streaks |
| Volatile state | Bounded collections/chunk channel | In-process streaming and work/acceptance/retry metadata; no full-message buffer or persistence |
| Header parsing | Bounded incremental parser behind the envelope policy | Validate Return-Path without collecting a full header/message; excessive unsupported metadata fails visibly |

The proposed IMAP library exposes `uid_mv`, IDLE, and a stream of Fetch results; that alone does not guarantee incremental body bytes. Prove bounded partial-fetch literals, parser caps, ordering, and cancellation against the selected version. If it cannot enforce the budget, adapt or replace the body reader. [IMAP session API](https://docs.rs/async-imap/latest/async_imap/struct.Session.html)

`lettre::AsyncTransport::send_raw` takes a complete `&[u8]`, so it does not meet this streaming design as-is. Do not restore a whole-message buffer to accommodate that API. Select or narrowly adapt an SMTP transport that accepts an asynchronous byte source and exposes DATA completion/abort; a Tokio/rustls protocol adapter is a candidate, subject to the gates below. No streaming SMTP library is selected yet. [Current lettre transport API](https://docs.rs/lettre/latest/lettre/trait.AsyncTransport.html)

No database/ORM, spool, message broker, persistent lock volume, HTTP server, metrics exporter, JMAP client, strategy plugin, or CLI parser is needed. Do not use Message-ID or content hashes to suppress distinct source arrivals.

## 2. Implementation acceptance gates

Before building the full supervisor, prove these adapter behaviors:

1. Implicit TLS and password/app-password authentication against the selected provider and private Stalwart, including mounted private CA trust. No insecure or STARTTLS fallback. IMAPS 993 and submissions 465 are the normal defaults. [Implicit TLS guidance](https://www.rfc-editor.org/rfc/rfc8314.html#section-3)
2. Stream complete PEEK retrieval without changing flags, using incremental literals or validated bounded partial ranges. Demonstrate a 250 MiB fixture without whole-message allocations, enforce actual-byte limits, and verify source completion before finalizing DATA. Oversize/truncation mid-stream must close SMTP without its terminator.
3. Automatic IDLE detection, serialized DONE/completion, missed-notification reconciliation, deadline wakeups, reconnect, and polling fallback.
4. Exact native UID MOVE to the explicit existing folder, including partial effects, lost replies, changed UIDVALIDITY, new target UIDs, and unrelated deletion flags. Never emulate with COPY/STORE/EXPUNGE. [MOVE specification](https://www.rfc-editor.org/rfc/rfc6851.html)
5. Streaming SMTP with arbitrary chunk boundaries, dots/CRLF, multipart/attachments, signed headers, 8-bit/internationalized data, and malformed/missing terminators. Prove bounded buffers and end-to-end backpressure. Use server capabilities where needed; reject representations that cannot be submitted faithfully rather than silently changing them.
6. Final acceptance distinct from preliminary replies; lost response conservatively retries. Cancellation disposes of the session so a hidden transaction cannot race a replacement attempt.
7. Stalwart permits the fixed Return-Path-or-null sender and configured recipient, and applies its normal rules. A Sieve discard must still count as SMTP handoff; the ingestor never queries final mailboxes.

One upstream provider must pass conformance before the first release; its identity is still to be selected. Standards-based adapters leave room for later providers, but v1 does not promise a provider matrix. If a library cannot meet a gate, adapt or replace it without weakening recovery/message preservation.

## 3. Fixed operational defaults

These values are internal constants, not environment options. Change them in a future reviewed software release if measured workloads require it; no runtime tuning interface is planned.

| Behavior | Fixed value / rule |
| --- | --- |
| Active SMTP submissions | 1 streaming transaction |
| Maximum input chunk | 1 MiB |
| Streaming channel | At most two chunks |
| Envelope-parser storage | At most 64 KiB; do not collect entire headers |
| Aggregate message-buffer budget | At most 8 MiB across protocol parsers, chunks, and framing; no hidden full-message copies |
| Discovery batch | At most 100 UID/metadata entries |
| Queued work | At most 1,024 scoped UID records; rediscover deferred source mail |
| IDLE | Automatic capability detection; polling on absence/usable rejection |
| Poll interval | 60 seconds |
| Reconciliation interval | 300 seconds while IDLE is active |
| IDLE renewal ceiling | 1,500 seconds; reconciliation may end IDLE earlier |
| IMAP operation timeout | 60 seconds without protocol progress during an active read; excludes normal IDLE wait and deliberate downstream backpressure |
| SMTP transaction timers | 300 seconds for command/DATA-start replies, 180 seconds without progress on a DATA write, 600 seconds for final post-DATA reply; no total-message deadline |
| Recoverable retry backoff | Exponential from 5 seconds, capped at 300 seconds, with bounded jitter |
| Failure threshold | Fifth consecutive failure in the affected stream exits; no sixth attempt |
| Shutdown budget | 30 seconds |
| Envelope sender | One valid Return-Path, otherwise null reverse path |

See [failure streak semantics](architecture.md#7-failure-and-shutdown-policy). Separate endpoint setup streaks, an SMTP submission streak, and per-UID/stage required fetch/move streaks prevent unrelated success from masking failure. Normal idle waits and unsupported optional IDLE do not count as attempts. Immediate fatal errors bypass retries. All counters reset on process restart.

Use a multi-stage container build with a pinned stable Rust toolchain and minimal runtime containing trust roots. Run as non-root with read-only root and optional read-only secret/CA mounts; expose no ports or HEALTHCHECK. The binary propagates fatal/threshold exit `1`, normal shutdown `0`. There is no persistent data directory or Docker socket dependency.

IMAP retrieval and SMTP writing overlap during DATA; a slow but progressing 250 MiB transfer must not hit a timeout merely because the complete transaction exceeds 600 seconds. Use stage/progress deadlines, and suspend only upstream read deadlines deliberately blocked by downstream backpressure. The DATA-write timer applies to an active blocked write, not waiting for the next source chunk. Final-response timeout retains its separate uncertainty handling. [SMTP timeout guidance](https://www.rfc-editor.org/rfc/rfc5321.html#section-4.5.3.2)

The [README configuration table](../README.md#configuration) defines every exposed setting and credential alternative. Unknown `INGESTOR_` settings are rejected; unrelated environment keys are ignored. The destination is a required exact folder path, not a mode or automatic special-use lookup. Ports may be changed for implicit-TLS listeners; changing them does not enable STARTTLS. Configuration takes effect by stop-before-start replacement.

## 4. Tests and tooling

Use Rust unit/integration tests, scripted IMAP/SMTP peers, controllable monotonic clocks, seeded jitter, and child-process crashes at fetch/submission/move boundaries. Independently record SMTP acceptance to distinguish a lost response from a lost message. Assert source state, exact commands, process exit, failure counters, buffer high-water marks, and peak RSS. Generate large fixtures incrementally and verify received bytes with a running digest rather than keeping 250 MiB test copies in memory. [Tokio timer testing](https://tokio.rs/tokio/topics/testing)

Use a disposable real IMAP server and pinned Stalwart container in CI for interoperability, with server-side message-size limits configured for at least 250 MiB. Run selected-provider conformance separately with dedicated test-account credentials. Production credentials never enter branch/PR/main CI. Synthetic Sieve fileinto/discard validates acceptance semantics, not runtime mailbox verification. A scheduled/manual 24h release-candidate soak exercises arrivals, outages, restart, and resource bounds; it need not block each PR for 24 hours.

During implementation commit `Cargo.lock`, `rust-toolchain.toml`, container pins, and action commit pins. Required checks include `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`, protocol/container integration, and design/configuration consistency. Dependency review can run alongside these checks. This repository currently contains no Rust application or implemented test suite.

## 5. GitHub Actions and branch workflow

Workflows are documented for later implementation only; no YAML files are added by this scoping task. Repository: [paulstaab/email-ingestor](https://github.com/paulstaab/email-ingestor). Container: `ghcr.io/paulstaab/email-ingestor`.

| Planned workflow / job | Trigger / dependency | Behavior |
| --- | --- | --- |
| `ci.yml`: checks | Branch pushes, PRs into main; reusable invocation for main | Format/lint/locked tests, protocol and real-server integration, design/config checks; read-only permissions |
| `ci.yml`: branch image check | Branch/PR checks succeed | Build and smoke-test image using disposable peers; no registry login or publication |
| `release.yml`: main checks | Push to protected main following merge | Invoke the same checks for the checked-out merged SHA; no publication on failure |
| `release.yml`: publish | Main checks succeed | One main-only job builds, smoke-tests, tags, and pushes the same locally built image directly to GHCR |
| Soak / provider conformance | Schedule or manual release-candidate invocation | Reliability/resource soak and selected-provider test account; separate from ordinary untrusted CI |

Use short-lived `feature/<topic>`/`fix/<topic>` branches and reviewed PRs. Configure main protection to require CI and prohibit direct pushes. Use regular `pull_request` events on isolated hosted runners; never expose release/production credentials to PR code. Branch/PR jobs have `contents: read`; grant `packages: write` only to the main publication job. [Workflow syntax and permissions](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax)

In that publication job, check out the exact merged SHA, build a single-platform image, load it into the runner, and smoke-test it against disposable endpoints. Only then log in and push that same local image with `GITHUB_TOKEN`. No image export/upload/download or transfer between jobs is needed; do not rebuild between smoke and push. Add OCI source/revision metadata, `sha-<full-commit-sha>`, and record the pushed digest. Choose one deployment platform for v1; multi-platform manifests are deferred. [GHCR publication](https://docs.github.com/en/actions/tutorials/publish-packages/publish-docker-images)

Serialize publication using a main-release concurrency group. Before tagging/pushing `latest`, ensure the SHA is still current main; an obsolete successful run may publish its commit tag but must not replace `latest`. Failed main checks or image smoke publish nothing; failed push is visible in Actions. Publication does not redeploy the host.

Add the application, Dockerfile, smoke fixtures, and pinned workflows during implementation before enabling releases. Configure branch protection and package visibility/access separately. Until the first actual release, README commands remain planned deployment instructions rather than evidence that an image exists.
