//! Volatile single-message scheduler. A known receipt survives reconnects, never restarts.
use crate::{
    BATCH, FAILURE_LIMIT,
    config::Config,
    envelope::EnvelopeParser,
    error::{Error, Kind, Result},
    imap::Imap,
    smtp::Smtp,
};
use std::{collections::VecDeque, time::Duration};
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

#[derive(Default)]
pub struct Streak {
    failures: u8,
}
impl Streak {
    pub fn success(&mut self, stage: &'static str) {
        if self.failures != 0 {
            tracing::info!(stage, failures = self.failures, event = "streak_reset");
        }
        self.failures = 0;
    }
    pub fn failure(&mut self, error: Error, stage: &'static str) -> Result<Duration> {
        if error.kind == Kind::Fatal {
            return Err(error);
        }
        self.failures += 1;
        tracing::warn!(
            stage,
            attempt = self.failures,
            reason = error.reason,
            event = "recoverable_failure"
        );
        if self.failures >= FAILURE_LIMIT {
            return Err(Error::fatal("fifth consecutive recoverable failure"));
        }
        // Bounded jitter without extra dependency or durable random state.
        let jitter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
            % 1001;
        Ok(Duration::from_secs((5u64 << (self.failures - 1)).min(300))
            + Duration::from_millis(u64::from(jitter)))
    }
    pub fn failures(&self) -> u8 {
        self.failures
    }
}
#[derive(Clone, Copy)]
enum Stage {
    Fetch,
    Submission,
}
struct Job {
    uid: u32,
    accepted: bool,
    fetch: Streak,
    movement: Streak,
}
impl Job {
    fn new(uid: u32) -> Self {
        Self {
            uid,
            accepted: false,
            fetch: Streak::default(),
            movement: Streak::default(),
        }
    }
}

/// Retrieval and DATA writes overlap through exactly two bounded chunks.
async fn submit(
    imap: &mut Imap,
    smtp: &mut Smtp,
    config: &Config,
    job: &mut Job,
) -> std::result::Result<(), (Stage, Error)> {
    let size = imap.size(job.uid).await.map_err(|e| (Stage::Fetch, e))?;
    if size > config.max_bytes {
        return Err((
            Stage::Fetch,
            Error::fatal("source exceeds message size limit"),
        ));
    }
    let first = imap
        .chunk(job.uid, 0)
        .await
        .map_err(|e| (Stage::Fetch, e))?;
    if first.len() as u64 > config.max_bytes || first.len() as u64 > size {
        return Err((
            Stage::Fetch,
            Error::fatal("source size mismatch or exceeded limit"),
        ));
    }
    let mut envelope = EnvelopeParser::default();
    envelope.feed(&first).map_err(|e| (Stage::Fetch, e))?;
    let sender = envelope.sender().map_err(|e| (Stage::Fetch, e))?;
    smtp.validate_headers(&first)
        .map_err(|e| (Stage::Submission, e))?;
    smtp.start(&sender, &config.recipient, size)
        .await
        .map_err(|e| (Stage::Submission, e))?;
    let offset = first.len() as u64;
    smtp.write(&first)
        .await
        .map_err(|e| (Stage::Submission, e))?;
    drop(first);
    {
        let (sender, mut receiver) = mpsc::channel::<Vec<u8>>(2);
        let uid = job.uid;
        let producer = async move {
            let mut offset = offset;
            loop {
                let chunk = imap.chunk(uid, offset).await?;
                if chunk.is_empty() {
                    if offset != size {
                        return Err(Error::fatal("truncated source or inaccurate size"));
                    }
                    return Ok(());
                }
                offset = offset
                    .checked_add(chunk.len() as u64)
                    .ok_or_else(|| Error::fatal("message byte count overflow"))?;
                if offset > config.max_bytes || offset > size {
                    return Err(Error::fatal("source size mismatch or exceeded limit"));
                }
                if sender.send(chunk).await.is_err() {
                    return Err(Error::temporary("stream consumer cancelled"));
                }
            }
        };
        let smtp_writer = &mut *smtp;
        let consumer = async move {
            while let Some(chunk) = receiver.recv().await {
                smtp_writer.write(&chunk).await?;
            }
            Ok::<_, Error>(())
        };
        tokio::pin!(producer, consumer);
        tokio::select! {
            biased;
            read = &mut producer => {
                read.map_err(|e| (Stage::Fetch, e))?;
                job.fetch.success("fetch");
                consumer.await.map_err(|e| (Stage::Submission, e))?;
            }
            write = &mut consumer => {
                write.map_err(|e| (Stage::Submission, e))?;
                producer.await.map_err(|e| (Stage::Fetch, e))?;
                job.fetch.success("fetch");
            }
        }
    }
    smtp.finish().await.map_err(|e| (Stage::Submission, e))
}

pub async fn run(config: Config, mut shutdown: watch::Receiver<bool>) -> Result<()> {
    let mut imap: Option<Imap> = None;
    let mut generation = None;
    let mut imap_setup = Streak::default();
    let mut smtp_setup = Streak::default();
    let mut submission = Streak::default();
    let mut discovery = Streak::default();
    let mut pending = VecDeque::new();
    let mut active: Option<Job> = None;
    let mut cursor = 1u32;
    let mut retry_at = Instant::now();
    let mut scan_again = true;
    loop {
        if *shutdown.borrow() && (active.is_none() || retry_at > Instant::now()) {
            return Ok(());
        }
        if retry_at > Instant::now() {
            tokio::select! {
                _ = tokio::time::sleep_until(retry_at) => {}
                _ = shutdown.changed() => { if *shutdown.borrow() { return Ok(()); } }
            }
        }
        if imap.is_none() {
            match Imap::connect(&config).await {
                Ok(connection) => {
                    imap_setup.success("imap_setup");
                    if generation.is_some_and(|value| value != connection.generation) {
                        tracing::warn!(event = "generation_invalidated");
                        active = None;
                        pending.clear();
                    }
                    generation = Some(connection.generation);
                    cursor = 1;
                    scan_again = true;
                    tracing::info!(
                        uidvalidity = connection.generation,
                        discovery = if connection.idle { "idle" } else { "poll" },
                        event = "imap_ready"
                    );
                    imap = Some(connection);
                }
                Err(error) => {
                    retry_at = Instant::now() + imap_setup.failure(error, "imap_setup")?;
                    continue;
                }
            }
        }
        let connection = imap.as_mut().expect("connected above");
        if let Some(job) = active.as_mut() {
            let span = tracing::info_span!(
                "message",
                uid = job.uid,
                uidvalidity = connection.generation
            );
            let _guard = span.enter();
            // Reconcile every retry/reconnect; absent unaccepted work is never silently released.
            match connection.present(job.uid).await {
                Ok(false) if job.accepted => {
                    tracing::warn!(
                        event = "accepted_source_absent",
                        reason = "move placement uncertain"
                    );
                    active = None;
                    continue;
                }
                Ok(false) => return Err(Error::fatal("source disappeared before acceptance")),
                Ok(true) => {}
                Err(error) => {
                    let streak = if job.accepted {
                        &mut job.movement
                    } else {
                        &mut job.fetch
                    };
                    retry_at = Instant::now()
                        + streak.failure(error, if job.accepted { "move" } else { "fetch" })?;
                    imap = None;
                    continue;
                }
            }
            if job.accepted {
                match connection.move_uid(job.uid, &config.destination).await {
                    Ok(()) => {
                        job.movement.success("move");
                        tracing::info!(event = "moved");
                        active = None;
                    }
                    Err(error) => {
                        retry_at = Instant::now() + job.movement.failure(error, "move")?;
                        imap = None;
                    }
                }
                continue;
            }
            let mut smtp = match Smtp::connect(&config).await {
                Ok(smtp) => {
                    smtp_setup.success("smtp_setup");
                    smtp
                }
                Err(error) => {
                    retry_at = Instant::now() + smtp_setup.failure(error, "smtp_setup")?;
                    continue;
                }
            };
            match submit(connection, &mut smtp, &config, job).await {
                Ok(()) => {
                    submission.success("submission");
                    job.accepted = true;
                    tracing::info!(event = "smtp_accepted");
                    // No new discovery or submission before the exact UID move.
                }
                Err((stage, error)) => {
                    retry_at = Instant::now()
                        + match stage {
                            Stage::Fetch => job.fetch.failure(error, "fetch")?,
                            Stage::Submission => submission.failure(error, "submission")?,
                        };
                    imap = None;
                }
            }
            continue;
        }
        if let Some(uid) = pending.pop_front() {
            active = Some(Job::new(uid));
            continue;
        }
        if scan_again && cursor < connection.next_uid {
            let end = cursor
                .saturating_add(BATCH - 1)
                .min(connection.next_uid - 1);
            match connection.search(cursor, end).await {
                Ok(uids) => {
                    discovery.success("discovery");
                    tracing::info!(
                        uidvalidity = connection.generation,
                        count = uids.len(),
                        event = "discovered"
                    );
                    pending.extend(uids);
                    cursor = end + 1;
                }
                Err(error) => {
                    retry_at = Instant::now() + discovery.failure(error, "discovery")?;
                    imap = None;
                }
            }
            continue;
        }
        scan_again = false;
        let duration = Duration::from_secs(if connection.idle { 300 } else { 60 });
        let wait = tokio::select! {
            result = connection.wait(duration) => result,
            _ = shutdown.changed() => { return Ok(()); }
        };
        match wait {
            Ok(()) => {
                // Refresh UIDNEXT/UIDVALIDITY, including missed arrivals and generation changes.
                let old_generation = connection.generation;
                match connection.select(&config.source).await {
                    Ok(()) => {
                        discovery.success("discovery");
                        generation = Some(connection.generation);
                        if connection.generation != old_generation {
                            tracing::warn!(event = "generation_invalidated");
                        }
                        cursor = 1;
                        scan_again = true;
                    }
                    Err(error) => {
                        retry_at = Instant::now() + discovery.failure(error, "discovery")?;
                        imap = None;
                    }
                }
            }
            Err(error) => {
                retry_at = Instant::now() + discovery.failure(error, "discovery")?;
                imap = None;
            }
        }
    }
}
