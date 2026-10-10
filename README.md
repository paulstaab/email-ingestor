# Email Ingestor

A small Rust service that forwards original messages from a public IMAP inbox to a private mail server such as Stalwart using authenticated SMTP. It aims for at-least-once delivery: uncertain attempts are retried, and occasional duplicates are acceptable.

After final SMTP acceptance, the ingestor immediately moves the message to a configured upstream folder, normally the provider's Trash. Source-folder membership is the only durable processing state. No database or persistent volume is needed. Messages up to 250 MiB are streamed through bounded buffers without loading the complete message into memory. IMAP IDLE provides fast discovery when available, with polling and periodic reconciliation. Stalwart owns subsequent Sieve/spam processing; final mailbox verification is outside this project.

**Status:** architecture and scope only. The Rust application, container image, and GitHub Actions workflows will be implemented later. The deployment and configuration below describe the planned interface.

## Deployment

Run one instance per upstream account on the private server. V1 targets one provider validated before deployment; that provider must support native IMAP MOVE and password/app-password authentication. Both connections use implicit TLS, normally IMAPS port 993 and SMTP port 465. Configure a private authenticated Stalwart listener that accepts the configured recipient and forwarded envelope senders, with its message-size limit at least as large as the ingestor limit.

1. Find the provider's actual Trash folder path, or choose another existing writable folder. The destination must differ from the source. The ingestor does not discover or create folders; the provider's policy controls destination retention.
2. Create a private environment file outside the repository, for example `/etc/email-ingestor.env`:

   ```dotenv
   INGESTOR_IMAP_HOST=imap.provider.example
   INGESTOR_IMAP_USERNAME=upstream-account
   INGESTOR_IMAP_PASSWORD=replace-with-provider-app-password
   INGESTOR_SMTP_HOST=stalwart.internal.example
   INGESTOR_SMTP_USERNAME=ingestor-account
   INGESTOR_SMTP_PASSWORD=replace-with-smtp-password
   INGESTOR_SMTP_RECIPIENT=local-user@example.internal
   INGESTOR_DESTINATION_FOLDER=Trash
   ```

   `Trash` is an example: use the exact folder path from your provider. For a private certificate authority, also set `INGESTOR_SMTP_CA_FILE` and mount its PEM file read-only.

3. Once the first image is published, run it on an existing private Docker network that reaches Stalwart (`mail` here):

   ```sh
   chmod 600 /etc/email-ingestor.env
   docker pull ghcr.io/paulstaab/email-ingestor:latest
   docker run -d --name email-ingestor --restart no \
     --network mail --read-only --cap-drop ALL --memory 512m \
     --env-file /etc/email-ingestor.env \
     ghcr.io/paulstaab/email-ingestor:latest
   ```

The container runs as non-root and exposes no ports. Optional secret/CA file variables require read-only mounts at their container paths. Use an image digest or `sha-<full-commit-sha>` tag for a specific release; private GHCR packages require login. To update configuration or the image, stop/remove the old container before starting its replacement.

## Configuration

All application options use environment variables. There is no application configuration file or CLI override; Docker's `--env-file` injects environment values. Existing source mail is processed on startup regardless of read status or custom flags.

| Variable | Default / meaning |
| --- | --- |
| `INGESTOR_IMAP_HOST` | Required provider hostname |
| `INGESTOR_IMAP_PORT` | `993`; implicit TLS only |
| `INGESTOR_IMAP_CA_FILE` | Optional mounted CA PEM path; otherwise public trust roots |
| `INGESTOR_IMAP_MAILBOX` | `INBOX`; source folder |
| `INGESTOR_IMAP_USERNAME` | Required upstream username, or its `_FILE` alternative |
| `INGESTOR_IMAP_PASSWORD` | Required password/app password, or its `_FILE` alternative |
| `INGESTOR_SMTP_HOST` | Required private SMTP hostname |
| `INGESTOR_SMTP_PORT` | `465`; implicit TLS only |
| `INGESTOR_SMTP_CA_FILE` | Optional mounted CA PEM path; otherwise public trust roots |
| `INGESTOR_SMTP_USERNAME` | Required SMTP username, or its `_FILE` alternative |
| `INGESTOR_SMTP_PASSWORD` | Required SMTP password, or its `_FILE` alternative |
| `INGESTOR_SMTP_RECIPIENT` | Required single local envelope recipient |
| `INGESTOR_DESTINATION_FOLDER` | Required exact upstream destination path, normally actual Trash; must already exist |
| `INGESTOR_MAX_MESSAGE_BYTES` | `262144000` (250 MiB); limit counts raw RFC822 bytes including encoded attachments; larger messages stay upstream and cause failure exit |
| `INGESTOR_LOG_LEVEL` | `info`; allowed: `error`, `warn`, `info`, `debug`, `trace` |

Credential alternatives are `INGESTOR_IMAP_USERNAME_FILE`, `INGESTOR_IMAP_PASSWORD_FILE`, `INGESTOR_SMTP_USERNAME_FILE`, and `INGESTOR_SMTP_PASSWORD_FILE`. Set exactly one direct or file form per credential. Files contain UTF-8; remove one final newline without trimming other characters. Unknown `INGESTOR_` variables, empty required values, invalid numbers/log levels, and conflicting credential forms fail validation before connecting. Ports are 1–65535; message size is a positive integer. Secrets never belong in source control or logs.

Streaming buffers, IDLE negotiation, timing, retry delays, queue bounds, and single-message concurrency are fixed implementation defaults, described in the [technology stack](docs/tech-stack.md#3-fixed-operational-defaults).

## Failure behavior

Unrecoverable errors exit `1` immediately. Recoverable errors back off and exit `1` on the fifth consecutive failed attempt in the affected stream; there is no sixth attempt. Connection streaks are separate for IMAP and SMTP. Only authenticated connection setup resets its connection streak; only final SMTP acceptance resets the SMTP submission streak. Required IMAP fetch/move jobs have their own per-message/stage streaks. Unrelated successes cannot hide repeated failures. Normal shutdown exits `0`.

Invalid configuration/credentials/TLS trust, unavailable required folders or MOVE capability, permanent SMTP rejection, oversize/unsupported content, and observed disappearance of unaccepted source mail are fatal. The ingestor never moves unaccepted mail or hard-deletes anything. If a move fails after known acceptance, it retries the move in the running process; after a restart, source mail can be delivered again.

Monitor exit status and structured logs. The example leaves failures stopped and visible; an operator-managed restart/backoff policy can resume processing after recovery. Counters reset on restart. Eventual delivery depends on the source remaining available, the destination accepting mail, and restarting after failure. Returning a processed message to the source triggers another delivery.

## Development and releases

Use short-lived branches and reviewed PRs into protected `main`. Planned CI checks Rust formatting, Clippy, locked tests, protocol/container integration, and documentation/configuration consistency. Branch/PR image builds are smoke-tested without publishing.

After a successful merge and main validation, one main-only job builds, smoke-tests, and publishes that same image directly to [GitHub Container Registry](https://github.com/paulstaab/email-ingestor/pkgs/container/email-ingestor). Releases have a commit tag, digest, and `latest`; host deployment remains operator-managed. Workflow files are deferred to implementation; see the [workflow design](docs/tech-stack.md#5-github-actions-and-branch-workflow).

See [architecture](docs/architecture.md), [requirements](docs/requirements.md), [planned tests](docs/test-cases.md), and [technology stack](docs/tech-stack.md).
