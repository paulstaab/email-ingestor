# Implementation and validation status

Updated 10 October 2026. The [requirements](../design/requirements.md) and [test plan](../design/test-cases.md) remain the release contract; this file distinguishes implemented code from release validation.

## Implemented artifacts

The executable validates the environment before networking, loads public or mounted TLS roots, authenticates both protocols over implicit TLS, validates the configured selectable/writable folders, and requires native MOVE. It uses UID commands and bounded numeric UID windows, never unread/content filters or hard deletion. One volatile active identity and at most 100 pending UIDs keep discovery memory bounded; tracked pending UIDs survive same-generation reconnects.

Partial PEEK literals are limited to 1 MiB. The first chunk supplies bounded Return-Path parsing. Subsequent retrieval overlaps SMTP writing through a two-chunk channel. Actual byte counts, UID/offset checks, tagged FETCH completion, and an empty end-of-message probe all gate the SMTP terminator. There is no whole-message buffer or spool. Supported SMTP data must use CRLF, have a final CRLF, contain no NUL, and fit the 998-byte line limit; 8-bit bodies and internationalized headers require appropriate advertised capabilities. Unsupported content fails visibly.

Only final positive SMTP acceptance enables an exact native UID MOVE. Same-process accepted work retries movement without resubmission, while restart has no receipt history. Generation changes on reconnect discard old work/acceptance. Optional IDLE is serialized with DONE/completion, with 60s polling fallback and 300s reconciliation. Independent setup, submission, fetch/move, and discovery streaks enforce the fifth-failure threshold. Signals stop new work and impose a 30s active-work completion budget. Logs contain static sanitized reasons, stage, UID, UIDVALIDITY, and outcomes.

Rust 1.99.0, edition 2024, implementation dependency versions, Cargo.lock, container bases, and GitHub action commits are pinned. The Dockerfile runs as UID 65532 without ports or persistent state. Branch/PR workflows check code and disposable TLS/container peers with read-only permissions. Main validation gates one job that builds/smokes/pushes the same image, with only the `:beta` tag, digest, OCI metadata, and a current-main check before publication. `:latest` is reserved for after testing; SHA image tags are not published. Workflow definitions do not establish that a package has been published or deployed.

## Automated coverage

| Harness | Coverage in the acceptance plan |
| --- | --- |
| `tests/policy.rs` | Configuration defaults/invalid options/conflicts (TC-01/02 subset); Return-Path and injection (TC-12); arbitrary framing boundaries and unsupported representations (TC-11); independent fifth-failure/reset policy (TC-30–34 unit subset) |
| `tests/protocol.rs` | Exact UID partial PEEK, suffix UID, incorrect identity/offset/oversized literal, truncation and completion rejection (TC-09/10/49 subset); bounded discovery and native MOVE (TC-05/21 subset); serialized IDLE DONE (TC-26 subset); capped wire parser |
| `scripts/protocol_smoke.py` | Real executable with disposable private-CA TLS peers: normal delivery, identical-content UIDs, oversize, wrong UID, inaccurate size, missing tagged completion of the EOF probe with DATA abort and retry from byte zero, permanent rejection, hostname mismatch, lost SMTP acceptance, retrying MOVE, lost MOVE reply with absent source, generation invalidation, restart after accepted-but-unmoved delivery, sanitized logs, and controlled shutdown (TC-03/07/08/10/12/14/16–20/34–37 subsets) |
| `scripts/protocol_smoke.py --thresholds` | Real fixed backoff and exactly five attempts, independent setup/submission/fetch/move exits; accepted-but-unmoved source retained (TC-30–34 subsets) |
| `scripts/protocol_smoke.py --large` | Incremental exact 250 MiB fixture, independent accepted-byte count/SHA-256, native move, and sampled process peak RSS (TC-46 subset; scripted receiver rather than Stalwart) |
| `scripts/check_docs.py` | README/parser environment names, dependency/toolchain pins, immutable bases/actions, and workflow safety basics (TC-45 subset) |

Locally executed checks pass: format, strict Clippy, 12 Rust tests, documentation checks, and the disposable TLS process suite, including all five failure-stream thresholds. Exact 250 MiB transfers matched independent SHA-256 digests; sampled release-process peak RSS was approximately 7–9 MiB across runs. This measures the scripted workload, not real-provider/Stalwart capacity or all parser allocations.

## Outstanding release gates

Run the image smoke on a Docker host and execute the workflows on GitHub; configure branch protection and package visibility. No Docker daemon is available in the current workspace, so local container execution is unverified.

Add disposable real IMAP/Stalwart interoperability (including Sieve discard), select and validate one upstream provider, and record their exact versions, capability profile, size limits, and folder paths (TC-39/40). The adapter deliberately supports a narrow protocol response grammar; unsupported literal/suffix forms fail safely rather than falling back to whole-message retrieval. Numeric UID windows may scan slowly on a mailbox with a very large sparse UID history; measure that provider before deployment.

Complete the remaining fault/process matrix: additional crash windows and source/target-copy outcomes, further generation/reused-UID variants, externally disappearing queued messages, controlled stalls/backpressure and timer boundaries, notification bursts/missed IDLE, authentication/folder variants, credential file edge cases, the 10,000-message workload, and workflow failure injection. Measure aggregate payload allocations in addition to RSS, and compare 1/50/250 MiB workloads. Run the 24h release-candidate soak with independent acceptance records and supervisor restarts (TC-41).

The implementation and existing checks are a development baseline, not a claim that all 49 acceptance cases pass or that the service is ready for production.
