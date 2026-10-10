# Email Ingestor: automatable test plan

Status: tests to implement later, not executed application tests. IDs link to [requirements](requirements.md); use the [architecture](architecture.md) and [stack](tech-stack.md) for the contract. The plan covers one acceptance-then-move path and the fixed operational defaults.

## 1. Test harness

Use scripted IMAP/SMTP peers with an independent record of accepted raw messages and timestamps. Control responses, capabilities, UIDVALIDITY, source/target copies, notifications, literal lengths/ranges, chunk splits, receiver backpressure, connection drops, and cancellation. Generate large messages incrementally; track received byte count/digest, DATA completion, buffer high-water marks, and peak RSS. Use paused/controlled timers and seeded jitter for deterministic assertions. Process/container tests observe exit status and sanitized logs; restart with no retained filesystem state. Destination contents may be inspected by tests, but the running ingestor must not query Stalwart mailboxes or rely on target content searches for recovery.

Each crash/fault fixture must prove: unaccepted available source messages remain, no move precedes final acceptance, every retained source fixture is eventually accepted after recovery/restart, and distinct arrivals are never collapsed. Assert minimum one SMTP acceptance per fixture rather than exactly one. Count commands to demonstrate no sixth failed attempt or forbidden purge.

## 2. Configuration, TLS, and folders

| ID | Level | Setup / action | Expected result | Requirements |
| --- | --- | --- | --- | --- |
| TC-01 | Unit | Parse every documented option/default and reject missing/empty required fields, invalid port/size/log level, unknown prefixed keys, and removed mode/TLS/timing settings. | Complete reduced environment contract; unrelated environment ignored; no config-file or CLI override; invalid input rejected before networking. | FR-01, NFR-13 |
| TC-02 | Unit/process | Test each direct credential and `_FILE` form; conflicts, UTF-8, one final newline, other whitespace, unreadable/missing secrets or CA files. | Exactly one credential form required; only final file newline removed; errors before networking; no credential exposure. | FR-01, NFR-05 |
| TC-03 | TLS integration | Valid public/private chains, hostname mismatch, expiry, untrusted CA, and plaintext/STARTTLS-only peer; vary listener ports. | Only validated implicit TLS succeeds on both protocols; invalid trust fatal; no insecure fallback or credential leak. | FR-02, FR-09, FR-14, NFR-04, NFR-05 |
| TC-04 | Integration | Missing MOVE, wrong password, missing/nonselectable/read-only target, and source equal to target including equivalent source names. | Fatal exit before source processing for known unusable prerequisites; no creation, guessing, or alternate-folder fallback. | FR-02, FR-14, NFR-12 |
| TC-05 | Integration | Configure an existing non-English/nested Trash path, then an ordinary archive replacement; special-use listings are absent/ambiguous. | Exact configured folder used in both cases; one processing path, no keyword writes, special-use discovery, or provider cleanup commands. | FR-02, FR-10, FR-11 |

## 3. Discovery, identity, and message preservation

| ID | Level | Setup / action | Expected result | Requirements |
| --- | --- | --- | --- | --- |
| TC-06 | Integration | Startup/reconnect with read/unread/deleted/custom-flag source messages and messages only in destination. | All source identities considered; destination never scanned for ingestion; no flags cause skips. | FR-03, FR-06 |
| TC-07 | Integration | Two UIDs with identical bytes/Message-ID, missing Message-ID, and changed sequence numbers during processing. | Each arrival submitted independently using UID operations; no content-based suppression or wrong-message move. | FR-03, FR-04, NFR-01 |
| TC-08 | Crash/integration | Change UIDVALIDITY after fetch or acceptance, reuse an old UID for new mail, then reconnect. | Stale work/acceptance discarded; fresh scan; new identity submitted before movement; duplicates allowed. | FR-04, FR-06, FR-12, NFR-02 |
| TC-09 | Integration | Fetch read and unread fixtures using PEEK; compare flags before/after and command trace. | Complete message retrieved without Seen/custom/deletion flag writes. | FR-05, NFR-06 |
| TC-10 | Integration | Empty/truncated literals, misleading advertised size, 250 MiB exact limit and limit+1, hostile oversized literal, invalid partial offsets/lengths. | Only complete size-compliant sources finalize DATA; in-stream errors close SMTP without terminator/acceptance/MOVE; actual-byte and parser limits prevent runaway allocation. | FR-05, FR-07, FR-14, NFR-07 |
| TC-11 | Integration | MIME/attachments, DKIM headers, Received/Date, leading dots, unusual headers, 8-bit/internationalized data, missing final CRLF; split CRLF/dots/headers at every relevant chunk boundary. | Independent peer sees original decoded bytes for supported forms; framing is chunk-independent; unsupported forms abort without DATA completion, with source retained and no rebuilding/normalization. | FR-07, FR-14 |
| TC-12 | Unit/integration | Valid, missing, malformed, duplicate, null, and injected Return-Path; differing From/To/Cc/Bcc. | Exactly one configured recipient; valid single Return-Path or null sender; display headers never route; injection impossible. | FR-08, FR-09 |
| TC-13 | Process | Another client removes a tracked source message before acceptance; separately remove it after acceptance without a MOVE. | Before acceptance, observed missing-source error exits 1; after acceptance, disappearance is reported without claiming destination placement. | FR-04, FR-12, FR-14, NFR-02 |

## 4. SMTP acceptance, movement, and crash recovery

| ID | Level | Setup / action | Expected result | Requirements |
| --- | --- | --- | --- | --- |
| TC-14 | Integration | Positive greeting/MAIL/RCPT/DATA-intermediate replies followed by temporary or permanent final rejection. | Only final positive post-data response authorizes movement; temporary failures retry, permanent rejection exits with source retained. | FR-09, FR-10, FR-13, FR-14, NFR-01 |
| TC-15 | Crash/process | Kill before fetching, during fetch, before send, and during DATA; restart without filesystem state. | Source recovered and eventually accepted; no pre-acceptance move; partial/uncertain submission can duplicate. | FR-05, FR-06, FR-13, FR-18, NFR-01, NFR-02 |
| TC-16 | Integration | SMTP accepts, then drops the final reply or delays beyond the final-response timeout. | Submission treated as uncertain, source retained, safely retried; independent peer records one or more accepted copies. | FR-09, FR-13, NFR-01 |
| TC-17 | Crash/process | Kill immediately after observed final acceptance and before UID MOVE; restart. | Source refetched/resubmitted before movement; occasional duplicate proves conservative recovery without markers/receipts. | FR-06, FR-10, FR-12, FR-18, NFR-01, NFR-02 |
| TC-18 | Integration | Temporary MOVE failure after acceptance while process remains alive, then success. | Required move retries; no repeated SMTP while acceptance/identity valid; no keyword checkpoint or mode switch. | FR-10, FR-12, FR-13 |
| TC-19 | Integration | Lose MOVE reply with source still present, then with source absent; reconnect within same UIDVALIDITY. | Present source retries movement using remembered acceptance; absent source logs uncertain completion; no guessed target UID or content search. | FR-04, FR-11, FR-12 |
| TC-20 | Crash/process | Kill during MOVE; provider models source-only, target-only, or both copies; restart. | Remaining source submitted before moving; target-only copy not ingested; both-copy case permits duplicates, never client purge. | FR-06, FR-11, FR-12, NFR-01, NFR-02 |
| TC-21 | Integration | Native MOVE returns tagged failure after partial effects; unrelated source mail has Deleted flag; move returns new target UID. | Reconcile exact source identity; touch no unrelated mail, reuse no source UID in target; never COPY/STORE/EXPUNGE/CLOSE/delete mailbox. | FR-04, FR-11, FR-12 |
| TC-22 | Integration | Destination quota/temporary server failure, permanent permission denial, and later provider expiry of successfully moved mail. | Temporary failure retries within threshold, permanent denial fatal, no alternate target/creation; expiry causes no ingestor action. | FR-10, FR-11, FR-13, FR-14 |
| TC-23 | Process/integration | Return a processed message to source, rotate credentials, restart with a different configured recipient. | Source arrivals submitted again; rotation needs no migration/state file; remaining source uses current recipient. | FR-06, FR-08, FR-19, NFR-02, NFR-05 |

## 5. IDLE and bounded scheduling

| ID | Level | Setup / action | Expected result | Requirements |
| --- | --- | --- | --- | --- |
| TC-24 | Integration | IDLE advertised, startup backlog, then new-mail notification with at most 10s retrieval/submission. | Initial scan and automatic IDLE; acceptance within nominal 15s of notification without polling delay. | FR-03, FR-16, NFR-08 |
| TC-25 | Integration | IDLE absent or rejected on usable connection; arrival just after a poll. | Fixed 60s polling fallback; unsupported optional IDLE not a failure; nominal acceptance within 90s with at most 10s processing. | FR-16, FR-17, NFR-08 |
| TC-26 | Integration | Suppress notification; later emit bursts during fetch/move; peer rejects commands before DONE completion. | Fixed 300s reconciliation recovers mail; DONE completion precedes commands, notifications coalesce, no overlapping identity work. | FR-04, FR-16, NFR-08, NFR-11 |
| TC-27 | Timer/integration | Quiet IDLE past ordinary command timeout, renewal deadline, due retries/moves, and shutdown. | Healthy wait not failed; renewal within fixed ceiling; deadlines wake session without new arrivals; shutdown does not expunge. | FR-11, FR-16, FR-18, NFR-11 |
| TC-28 | Integration | IDLE connection drop, new arrivals during reconnect, and changed capabilities/UIDVALIDITY. | Appropriate failure recorded, backoff/reconnect/immediate scan, capabilities renegotiated; stale identities not mutated. | FR-04, FR-13, FR-16 |
| TC-29 | Capacity | 10,000-message source backlog, 250 MiB messages, 1,000 simulated daily arrivals, queue overflow, and notification bursts. | Fixed queue/batch/single-stream bounds; excess source work rediscovered, retries/moves not starved; aggregate message buffers at most 8 MiB, total RSS within 512 MiB target, throughput reported. | FR-03, FR-06, NFR-07, NFR-09, NFR-11 |

## 6. Failure thresholds, security, and deployment

| ID | Level | Setup / action | Expected result | Requirements |
| --- | --- | --- | --- | --- |
| TC-30 | Process | Five consecutive recoverable setup failures for each endpoint, independently. | Backoff for 1–4, exit 1 on 5, no sixth connection; endpoint streaks independent and not double-counted as submissions. | FR-13, FR-15, NFR-10 |
| TC-31 | Process | Five temporary/uncertain submissions with successful connections/scans between; insert final acceptance in a separate reset scenario. | Connections/scans never reset submission streak; fifth exits 1; final acceptance resets only submission; source never moved without acceptance. | FR-09, FR-13, FR-15, NFR-01, NFR-10 |
| TC-32 | Process | Same UID/stage fetch or move fails five times with other jobs succeeding; include operation disconnect then failed reconnect. | Matching job alone resets/completes its streak; fifth exits; operation and later setup failures classified separately. | FR-05, FR-10, FR-15, NFR-10 |
| TC-33 | Process | Successful authenticated setup between connection failures; normal idle waits, empty scans, unsupported IDLE, and coalesced events. | Matching endpoint streak resets; unrelated streaks persist; non-attempt events do not increment failures. | FR-15, FR-16, FR-17 |
| TC-34 | Process/container | Unrecoverable auth/trust/config/content/permanent SMTP failures and threshold failure followed by successful cleanup; restart after correction. | Immediate/threshold exit remains 1 with terminal reason; no cleanup-to-success conversion; restart resets counters and resumes source delivery. | FR-14, FR-15, FR-17, FR-18, NFR-01, NFR-10 |
| TC-35 | Process | SIGTERM/SIGINT during IDLE, fetch, SMTP, and MOVE; stalled operation exceeds 30s shutdown budget. | New work stops, bounded safe completion/cancellation, normal exit 0; uncertain connections disposed and available source recoverable. | FR-06, FR-18, NFR-02, NFR-11 |
| TC-36 | Security | Sentinel secrets, body/subject/addresses/header values at every log level; peer errors echo secrets/message data. | Sanitized structured stage/UID/streak/terminal logs with no sensitive values or raw transcripts. | FR-17, NFR-05, NFR-06 |
| TC-37 | Container | Execute README deployment with disposable peers, read-only root, non-root, optional secret/CA mounts, no state volume or published port. | Documented environment works; no listener/healthcheck/socket requirement; fatal exit visible with restart disabled. | FR-01, FR-17, NFR-03, NFR-10, NFR-13 |
| TC-38 | Container/docs | Replace container stop-before-start, rotate credentials, change target path; inspect deployment example and release status text. | One active instance; source remains recovery state, existing target not reingested; no persistent migration or unsupported claim of implemented artifacts. | FR-06, FR-19, NFR-02, NFR-13 |

## 7. Interoperability and workflow automation

| ID | Level | Setup / action | Expected result | Requirements |
| --- | --- | --- | --- | --- |
| TC-39 | Interoperability | Disposable real IMAP and pinned Stalwart with Sieve fileinto/discard; record versions and commands. | Authenticated original-byte SMTP pipeline works; acceptance alone authorizes move even on discard; runtime makes no final-mailbox query. | FR-02, FR-07, FR-09, FR-10, NFR-12 |
| TC-40 | Provider conformance | Dedicated selected-provider test account: TLS/auth, native MOVE, explicit target path/permissions, UID stability, PEEK, IDLE or polling. | Record one validated provider/version/capability profile; contract holds without keyword/special-use dependency; unsupported prerequisites fail safely. | FR-02, FR-04, FR-05, FR-11, FR-16, NFR-12 |
| TC-41 | Soak/model | 24h arrivals/outages/crashes, source/target move windows, missed IDLE, polling, and supervisor restarts after faults resolve. | Every available source fixture eventually accepted; no pre-acceptance move or purge; duplicates permitted; bounded resources and independent peer evidence. | NFR-01, NFR-02, NFR-07, NFR-14 |
| TC-42 | Workflow | Branch push and ordinary/fork PR; inject format/lint/test/integration/docs/image-smoke failures. | Required checks fail appropriately; branch protection blocks failing merges when configured; no registry publication or production credentials. | FR-20, NFR-14, NFR-15 |
| TC-43 | Workflow | Successful merge/main validation; inspect publication job image ID, smoke result, OCI revision, tags, digest, and job steps. | One job builds/smokes/pushes same merged-SHA image directly; no artifact handoff/rebuild; correct GHCR namespace and traceable digest. | FR-21, NFR-15 |
| TC-44 | Workflow | Failed main checks, failed build/smoke, registry push failure, and delayed successful run after main advances. | Checks/build/smoke failure prevents push; push failure visible; obsolete run cannot replace latest; no host deployment. | FR-21, NFR-15 |
| TC-45 | Workflow/docs | Compare env table/parser/default constants/examples/test references; inspect action triggers, permissions, secret handling, and commit pins. | Reduced interface consistent, all requirements mapped; package-write/login only in main publication job; docs accurately describe deferred workflow implementation. | FR-01, FR-20, FR-21, NFR-05, NFR-12, NFR-13, NFR-15 |

## 8. Large-message streaming

| ID | Level | Setup / action | Expected result | Requirements |
| --- | --- | --- | --- | --- |
| TC-46 | Integration/capacity | Stream generated 1 MiB, 50 MiB, and 250 MiB fixtures through IMAP to disposable Stalwart with compatible size limits; compare byte counts/digests and memory high-water marks. | Complete original bytes accepted and exact UID moved; no full-message allocation or spool; aggregate payload buffers at most 8 MiB and memory independent of input size. | FR-05, FR-06, FR-07, FR-09, FR-10, NFR-07, NFR-12 |
| TC-47 | Fault/integration | Throttle/pause SMTP receiver while IMAP is fast; then interrupt the source mid-literal/range, stall a DATA write, or cancel near completion. | Bounded channel stops further source reads; actual stalls hit appropriate deadlines; failures never send DATA terminator for incomplete input; source unmoved and retry starts from byte zero; induced abort does not double-count the originating failure. | FR-05, FR-07, FR-13, FR-15, FR-18, NFR-01, NFR-07 |
| TC-48 | Timer/integration | Progressing large transfer takes longer than 600s overall; separately stall IMAP, DATA-start reply, DATA write, and final acceptance reply. | Progressing stream succeeds without total-message timeout; stage/no-progress deadlines fire on actual stalls and final-reply timeout remains uncertain/retryable; intentional read backpressure does not become IMAP failure. | FR-07, FR-09, FR-13, FR-15, NFR-07, NFR-11 |
| TC-49 | Integration | Return-Path/header metadata spans chunks, exceeds parser storage, or contains duplicates; peer sends an unexpectedly huge literal or another UID/range. | Bounded parser produces the fixed valid/null envelope when supported; excessive unsupported metadata/protocol violations abort visibly; no large allocation, wrong-UID bytes, or premature acceptance/move. | FR-04, FR-05, FR-08, FR-14, NFR-07 |

## 9. Execution plan

Run policy/configuration, deterministic protocol/fault, process, and disposable real-server checks on branches/PRs and merged main. Build/smoke branch images without pushing; after successful main checks, build/smoke/push in one publication job. Provider conformance uses a dedicated account outside untrusted PR CI; the 24h soak is scheduled/manual for release candidates.

Release gates are correct final acceptance, at-least-once recovery, exact native movement, no ingestor purge, visible fatal/fifth-failure termination, and the selected provider's conformance. These are future application tests; checks on this document set do not establish runtime correctness.
