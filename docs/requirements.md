# Email Ingestor: requirements

Status: agreed v1 scope; core implementation and automated tests exist. Outstanding validation is tracked in [implementation status](implementation.md). Requirements supersede the original draft where later decisions changed it. See [architecture](architecture.md), [stack](tech-stack.md), and [test plan](test-cases.md).

## 1. Functional requirements

| ID | Requirement |
| --- | --- |
| FR-01 | Read all exposed options from environment variables using the complete README contract. Reject unknown `INGESTOR_` names, empty required values, invalid values, credential conflicts, and unreadable secret/CA files before networking. No configuration file or CLI override. |
| FR-02 | Connect to one upstream account using authenticated, certificate/hostname-validated implicit TLS and password/app-password authentication. Require native UID MOVE. Validate explicit pre-existing selectable source/destination folders, reject read-only or same-folder configurations, and never auto-create/discover a destination. |
| FR-03 | Scan the entire source on startup/reconnect and reconcile periodically in bounded batches. Process read and unread messages regardless of custom or deletion flags; never ingest the destination or skip mail based on Message-ID/content equality. |
| FR-04 | Scope source identity to account/mailbox/UIDVALIDITY/UID. Use UID commands, prevent concurrent work for the same identity, invalidate stale work/acceptance on generation changes, and report observed disappearance before acceptance as fatal. |
| FR-05 | Retrieve complete original bytes with PEEK without altering flags. Stream bounded literals or validated partial ranges; enforce the actual raw-byte limit and detect truncation/offset/identity/completion errors. Never finalize SMTP DATA from an incomplete or oversized source. |
| FR-06 | Keep work, fixed-size streaming chunks, outcomes, and counters only in bounded process memory. Never collect a complete message in memory. Reconstruct pending work solely from source membership on restart; no database, durable local receipt/spool, completion keyword, or saved cursor. |
| FR-07 | Stream original RFC822 bytes faithfully through SMTP without rebuilding MIME or adding ingestor headers. Apply backpressure and preserve framing across chunks. Finish DATA only after complete validated retrieval within the size limit; abort by closing without the terminator on input failure. Retry from byte zero after uncertain transport failure. Never silently normalize, omit, or skip content. |
| FR-08 | Use one configured SMTP recipient and the fixed Return-Path-or-null sender policy. Do not route by display headers, use From as sender fallback, or permit envelope command injection. |
| FR-09 | Connect to private Stalwart using authenticated, certificate/hostname-validated implicit TLS. Confirm success only from the final positive response after message data; no final mailbox verification or administrative API. |
| FR-10 | Immediately after known final acceptance, move that exact source UID to the configured existing destination, normally actual Trash. This is the single processing path; no retention modes, keywords, or delay. Retry the move without resubmission while same-process acceptance remains valid. |
| FR-11 | Use native UID MOVE only. Never issue deletion flags, EXPUNGE, UID EXPUNGE, implicitly expunging CLOSE, mailbox deletion, or destination cleanup. Provider policy owns eventual expiry. |
| FR-12 | Reconcile the source after uncertain/failed movement. If the same identity remains and acceptance is remembered, retry movement; after restart, redeliver before moving. If absent, report uncertainty without guessing target identity or suppressing other arrivals. |
| FR-13 | Retry temporary/uncertain failures with fixed bounded backoff and jitter. Treat lost SMTP acceptance responses conservatively as retryable; unconfirmed source messages must not be moved. |
| FR-14 | Exit `1` immediately for unrecoverable configuration/authentication/TLS/protocol/data errors, permanent SMTP rejection, or observed missing unaccepted mail. Retain available unaccepted source mail; a failing message may stop the service. |
| FR-15 | Exit `1` on the fifth consecutive recoverable failure with no sixth attempt. Maintain independent IMAP/SMTP setup streaks, a submission streak, and per-UID/stage required fetch/move streaks; matching success resets only the matching streak. Do not double-count setup failures. |
| FR-16 | Automatically use IMAP IDLE when supported; otherwise poll. Serialize DONE/completion before commands, renew IDLE, coalesce notifications, wake for retry/reconciliation deadlines, and scan after reconnect. Missing notifications must not skip messages. |
| FR-17 | Emit sanitized structured logs and propagate terminal failure to container exit. Include discovery method, scoped UID/stage/attempt, acceptance/move events, streak/reset information, and terminal reason. No HTTP/metrics endpoints or listening ports. |
| FR-18 | On controlled shutdown, stop new work, use the fixed completion budget, and safely cancel/dispose of uncertain operations. Exit `0` normally; a fatal result stays `1`. Restart rescans with fresh counters and may duplicate accepted-but-unmoved mail. |
| FR-19 | Support one active instance per upstream account with stop-before-start replacement. Returning processed messages to the source triggers submission; changing recipient applies to all source mail remaining at restart. |
| FR-20 | Plan branch-push and PR CI with formatting, Clippy, locked tests, protocol/container checks, docs/config checks, and image build/smoke. Branch/PR jobs are read-only and never publish or receive production credentials. |
| FR-21 | Plan automatic GHCR publication after merge/main checks succeed. A single main-only job builds, smoke-tests, and publishes that same merged-SHA image directly, with only current-main `:beta`, digest, and OCI metadata; `:latest` is deferred until after testing and SHA image tags are not used; no artifact transfer or automatic host deployment. |

## 2. Non-functional requirements

| ID | Requirement |
| --- | --- |
| NFR-01 | At-least-once handoff under recoverable faults: every retained source fixture is eventually accepted after peers recover and the process is restarted as required. Duplicates are acceptable; silent skips and pre-acceptance moves are not. |
| NFR-02 | Crash recovery depends only on the provider and source membership, with no database or persistent volume. Do not claim exactly-once delivery, a historical delivery ledger, or recovery of upstream copies deleted externally. |
| NFR-03 | Run non-root with read-only root, outbound-only networking, and optional read-only secret/CA mounts. Require no Docker socket, host state directory, inbound port, or Stalwart administrative access. |
| NFR-04 | Validate certificates/hostnames on both TLS connections using public or explicitly mounted trust roots. Do not offer plaintext, STARTTLS, or insecure certificate-bypass options in v1. |
| NFR-05 | Support direct environment credentials and `_FILE` alternatives with exactly one form per credential. Never include secrets in code, image layers, logs, diagnostic errors, or CI output. |
| NFR-06 | Preserve privacy: logs omit message bodies, subjects, address/header values, credentials, and raw protocol transcripts at every log level. |
| NFR-07 | Support raw messages through 262,144,000 bytes (250 MiB) by default with one active stream and memory independent of message size/backlog. Bound chunks to 1 MiB, channel to two chunks, envelope-parser storage to 64 KiB, and aggregate message buffers to 8 MiB including library/parser/framing copies. Target total RSS within a 512 MiB container; measure and reject hidden whole-message allocations. Oversized content fails visibly with source retained. |
| NFR-08 | Nominal latency target with no backlog/retries and fetch/submission taking at most 10s: acceptance within 15s of an IDLE notification or 90s of arrival under polling. A missed notification is recovered within the 300s reconciliation interval plus processing; outage/backlog latency is not bounded by these targets. |
| NFR-09 | Initial validation workload: up to 1,000 messages/day and a 10,000-message source backlog, subject to configured maximum size. Bound memory independent of backlog and measure throughput; guarantee no skipping rather than fixed backlog drain time. |
| NFR-10 | Fatal/threshold failures are visible through a stopped container, nonzero exit, and terminal log. Operators own alerting/restart; success in unrelated work must not mask repeated failures. |
| NFR-11 | Deadlines/retries must progress during quiet IDLE and notification bursts. Bounded discovery and queued work must avoid starvation of retry/move/shutdown tasks. |
| NFR-12 | Validate one selected provider and deployed Stalwart version before release. Keep protocol adapters isolated and pin implementation dependencies/toolchain/images; broad provider/auth/TLS variants are deferred. |
| NFR-13 | Keep README deployment/options, architecture, requirement IDs, and planned tests consistent. Clearly distinguish implemented artifacts from unexecuted release gates. |
| NFR-14 | Automate deterministic protocol/fault/process tests and a 24h release-candidate soak with independent SMTP acceptance records. Required CI runs without live production mail credentials. |
| NFR-15 | Releases trace to the checked merged SHA and the exact locally built/smoke-tested image. Package-write permission exists only in the main publication job; failed checks/smoke block push and obsolete runs cannot replace `:beta`. |

## 3. Acceptance and operational assumptions

V1 is ready when scripted fault tests demonstrate no pre-acceptance move, visible fatal/fifth-failure exit, retry after uncertain SMTP, safe movement, and restart recovery without local state; a disposable real-server test and the selected provider's conformance suite also pass. The 24h soak uses supervisor restarts after injected failures to prove eventual progress.

The guarantee assumes a conforming provider retains pending source mail, valid credentials/folders, one active ingestor, eventual SMTP acceptance, and restart after exit. Destination retention is provider policy. Final mailbox presence is not an acceptance condition. Provider identity/version, Stalwart version, compatible server-side size limits, and measured streaming workload/resource fit must be recorded before deployment. The size limit applies to complete raw RFC822 bytes, including encoded attachments, rather than decoded attachment size.
