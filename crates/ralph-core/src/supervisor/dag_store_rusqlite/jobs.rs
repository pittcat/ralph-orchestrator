//! Durable launch journal. A reservation is written before OS spawn; replay
//! returns `false`, never permission to launch a second process. Recovery must
//! resolve an unfinished reservation before doing any further work on its unit.

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::{DagStoreError, DagStoreResult, RusqliteDagSchedulerStore, plan_io_err};

/// Runtime-owned identity, bound to a plan-qualified unit key. Token values
/// are opaque runtime nonces, never credentials or agent output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobIdentity {
    pub plan_key: String,
    pub unit_id: String,
    pub job_id: String,
    pub hat: String,
    pub stage: String,
    pub attempt: u32,
    pub token: String,
}

impl JobIdentity {
    pub fn unit_key(&self) -> String {
        format!("forge:{}:{}", self.plan_key, self.unit_id)
    }

    fn validate(&self) -> DagStoreResult<()> {
        for value in [
            &self.plan_key,
            &self.unit_id,
            &self.job_id,
            &self.hat,
            &self.token,
        ] {
            if value.is_empty()
                || value.len() > 256
                || !value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
            {
                return Err(conflict("invalid bounded job identity"));
            }
        }
        if !matches!(self.stage.as_str(), "execute" | "review" | "verify" | "fix")
            || self.attempt > 3
        {
            return Err(conflict("invalid job stage or correction attempt"));
        }
        Ok(())
    }
}

/// Only bounded facts are retained. Results stay in their durable channel
/// until EventLoop acceptance and downstream acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalJob {
    pub identity: JobIdentity,
    pub pid: Option<u32>,
    pub terminal: Option<String>,
    pub result_digest: Option<String>,
}

fn conflict(reason: &str) -> DagStoreError {
    DagStoreError::IoError(format!("DAG job journal: {reason}"))
}

fn read_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<JournalJob> {
    let plan_key: String = row.get("plan_key")?;
    let unit_key: String = row.get("unit_key")?;
    let prefix = format!("forge:{plan_key}:");
    let unit_id = unit_key
        .strip_prefix(&prefix)
        .ok_or(rusqlite::Error::InvalidQuery)?
        .to_string();
    Ok(JournalJob {
        identity: JobIdentity {
            plan_key,
            unit_id,
            job_id: row.get("job_id")?,
            hat: row.get("hat")?,
            stage: row.get("stage")?,
            attempt: row.get("attempt")?,
            token: row.get("token")?,
        },
        pid: row.get("pid")?,
        terminal: row.get("terminal_state")?,
        result_digest: row.get("result_digest")?,
    })
}

impl RusqliteDagSchedulerStore {
    /// Atomically reserve the current job and its unit identity. Exactly one
    /// competing caller receives `true`. `false` means a matching reservation
    /// exists, including when the process died before its PID was recorded.
    ///
    /// When `claims_and_caps` is `Some`, the lease writes share this
    /// transaction's atomicity with the `dag_jobs` / `dag_units` INSERTs:
    /// an oversubscribed claim rolls back both lease rows AND any
    /// half-applied journal state, so a failed launch never leaks a
    /// resource permit nor a stale reservation row (R2/S2). `None`
    /// preserves the legacy zero-capacity path used by every existing
    /// caller / test until spawn-side capacity plumbing lands.
    pub fn reserve_job(
        &self,
        identity: &JobIdentity,
        now_ms: i64,
        claims_and_caps: Option<(&[(String, u32)], &[(String, u32)])>,
    ) -> DagStoreResult<bool> {
        identity.validate()?;
        let mut conn = self
            .inner
            .conn
            .lock()
            .map_err(|_| conflict("connection poisoned"))?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_io_err)?;
        let unit_key = identity.unit_key();
        let plan = tx
            .query_row(
                "SELECT status, unit_ids FROM dag_plans WHERE plan_key=?1",
                [&identity.plan_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(plan_io_err)?
            .ok_or_else(|| DagStoreError::UnknownPlan(identity.plan_key.clone()))?;
        let ids: Vec<String> =
            serde_json::from_str(&plan.1).map_err(|_| conflict("invalid registered unit list"))?;
        if plan.0 != "active" || !ids.contains(&identity.unit_id) {
            return Err(conflict("job requires an active plan and registered unit"));
        }
        let existing = tx
            .query_row(
                "SELECT * FROM dag_jobs WHERE unit_key=?1 AND stage=?2 AND attempt=?3",
                params![unit_key, identity.stage, identity.attempt],
                read_job,
            )
            .optional()
            .map_err(plan_io_err)?;
        if let Some(existing) = existing {
            if existing.identity != *identity {
                return Err(conflict("reservation identity conflict"));
            }
            return Ok(false);
        }
        let reused: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM dag_jobs WHERE job_id=?1 OR token=?2)",
                params![identity.job_id, identity.token],
                |row| row.get(0),
            )
            .map_err(plan_io_err)?;
        if reused {
            return Err(conflict("job id and token must be fresh for each launch"));
        }
        let previous = tx.query_row("SELECT j.* FROM dag_units u JOIN dag_jobs j ON j.unit_key=u.unit_key AND j.job_id=u.job_id AND j.token=u.current_token AND j.stage=u.stage AND j.attempt=u.attempt AND j.hat=u.hat WHERE u.unit_key=?1", [&unit_key], read_job).optional().map_err(plan_io_err)?;
        if previous.is_none() {
            let broken: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM dag_units WHERE unit_key=?1 AND (job_id IS NOT NULL OR current_token IS NOT NULL))",
                [&unit_key], |row| row.get(0),
            ).map_err(plan_io_err)?;
            if broken {
                return Err(conflict("current unit job reference is inconsistent"));
            }
        }
        let valid = match previous {
            None => identity.stage == "execute" && identity.attempt == 0,
            Some(previous) => {
                let p = previous.identity;
                match (
                    p.stage.as_str(),
                    previous.terminal.as_deref(),
                    identity.stage.as_str(),
                ) {
                    ("execute" | "fix", Some("accepted"), "review")
                    | ("review", Some("accepted"), "verify") => identity.attempt == p.attempt,
                    ("review" | "verify", Some("rejected" | "failed"), "fix") => {
                        identity.attempt == p.attempt + 1
                    }
                    _ => false,
                }
            }
        };
        if !valid {
            return Err(conflict(
                "previous job is unresolved or transition is invalid",
            ));
        }
        // U2 (R2/S2): resource capacity oversubscription
        // prevention. The lease writes share this transaction's
        // commit boundary with the dag_jobs / dag_units inserts
        // below — a failed claim rolls back the whole
        // reservation, so the durable journal never holds a
        // reservation row whose unit is missing its declared
        // resource permits (or vice versa). `None` skips the
        // claim entirely, preserving the legacy path.
        if let Some((claims, capacities)) = claims_and_caps {
            Self::claim_resources_in_tx(&tx, identity, claims, capacities, now_ms)?;
        }
        tx.execute("INSERT INTO dag_jobs (job_id,plan_key,unit_key,hat,stage,attempt,token,created_at_ms) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![identity.job_id,identity.plan_key,unit_key,identity.hat,identity.stage,identity.attempt,identity.token,now_ms]).map_err(plan_io_err)?;
        tx.execute("INSERT INTO dag_units (unit_key,plan_key,state,stage,hat,job_id,attempt,current_token,created_at_ms,updated_at_ms) VALUES (?1,?2,'launch_reserved',?3,?4,?5,?6,?7,?8,?8) ON CONFLICT(unit_key) DO UPDATE SET state='launch_reserved',stage=excluded.stage,hat=excluded.hat,job_id=excluded.job_id,attempt=excluded.attempt,current_token=excluded.current_token,updated_at_ms=excluded.updated_at_ms", params![unit_key,identity.plan_key,identity.stage,identity.hat,identity.job_id,identity.attempt,identity.token,now_ms]).map_err(plan_io_err)?;
        tx.commit().map_err(plan_io_err)?;
        Ok(true)
    }

    /// Persist the observed PID only for the current reserved identity.
    /// A repeated write for the same PID is harmless; changing it is refused.
    pub fn record_job_pid(
        &self,
        identity: &JobIdentity,
        pid: u32,
        now_ms: i64,
    ) -> DagStoreResult<()> {
        identity.validate()?;
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(conflict("invalid process id"));
        }
        let conn = self
            .inner
            .conn
            .lock()
            .map_err(|_| conflict("connection poisoned"))?;
        let n = conn.execute("UPDATE dag_jobs SET pid=?1,launched_at_ms=COALESCE(launched_at_ms,?2) WHERE unit_key=?3 AND job_id=?4 AND stage=?5 AND attempt=?6 AND hat=?7 AND token=?8 AND terminal_state IS NULL AND (pid IS NULL OR pid=?1) AND EXISTS(SELECT 1 FROM dag_units u WHERE u.unit_key=dag_jobs.unit_key AND u.job_id=dag_jobs.job_id AND u.current_token=dag_jobs.token)", params![pid,now_ms,identity.unit_key(),identity.job_id,identity.stage,identity.attempt,identity.hat,identity.token]).map_err(plan_io_err)?;
        if n != 1 {
            return Err(conflict("PID write did not match current reservation"));
        }
        Ok(())
    }

    /// Called only after the real EventLoop accepted the result. The full
    /// identity is compared against the current unit; stale results cannot
    /// change either the job row or the unit row.
    pub fn accept_job_terminal(
        &self,
        identity: &JobIdentity,
        terminal: &str,
        digest: &str,
        now_ms: i64,
    ) -> DagStoreResult<()> {
        identity.validate()?;
        if !matches!(terminal, "accepted" | "rejected" | "failed" | "blocked")
            || digest.len() != 64
            || !digest.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(conflict("invalid bounded terminal fact"));
        }
        let mut conn = self
            .inner
            .conn
            .lock()
            .map_err(|_| conflict("connection poisoned"))?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_io_err)?;
        let n = tx.execute("UPDATE dag_jobs SET terminal_state=?1,result_digest=?2,finished_at_ms=COALESCE(finished_at_ms,?3) WHERE unit_key=?4 AND job_id=?5 AND stage=?6 AND attempt=?7 AND hat=?8 AND token=?9 AND (terminal_state IS NULL OR (terminal_state=?1 AND result_digest=?2)) AND EXISTS(SELECT 1 FROM dag_units u WHERE u.unit_key=dag_jobs.unit_key AND u.job_id=dag_jobs.job_id AND u.current_token=dag_jobs.token)", params![terminal,digest,now_ms,identity.unit_key(),identity.job_id,identity.stage,identity.attempt,identity.hat,identity.token]).map_err(plan_io_err)?;
        if n != 1 {
            return Err(conflict("terminal did not match current reservation"));
        }
        tx.execute(
            "UPDATE dag_units SET state='job_terminal',updated_at_ms=?1 WHERE unit_key=?2",
            params![now_ms, identity.unit_key()],
        )
        .map_err(plan_io_err)?;
        tx.commit().map_err(plan_io_err)
    }

    /// Recovery reads unresolved launches including NULL PID reservations.
    /// A NULL PID is an ambiguous launch, not proof that no child exists.
    pub fn unresolved_jobs(&self, plan_key: &str) -> DagStoreResult<Vec<JournalJob>> {
        let conn = self
            .inner
            .conn
            .lock()
            .map_err(|_| conflict("connection poisoned"))?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM dag_jobs WHERE plan_key=?1 AND terminal_state IS NULL ORDER BY id",
            )
            .map_err(plan_io_err)?;
        stmt.query_map([plan_key], read_job)
            .map_err(plan_io_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(plan_io_err)
    }

    /// E3 recovery read: every journaled job of a plan, resolved or
    /// not, in launch order. The pipeline hydration pass replays this
    /// history to rebuild each unit's stage/attempt without guessing.
    pub fn list_jobs(&self, plan_key: &str) -> DagStoreResult<Vec<JournalJob>> {
        let conn = self
            .inner
            .conn
            .lock()
            .map_err(|_| conflict("connection poisoned"))?;
        let mut stmt = conn
            .prepare("SELECT * FROM dag_jobs WHERE plan_key=?1 ORDER BY id")
            .map_err(plan_io_err)?;
        stmt.query_map([plan_key], read_job)
            .map_err(plan_io_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(plan_io_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::dag_store::{CanonicalPlanRecord, DagSchedulerStore};

    fn fixture() -> (tempfile::TempDir, RusqliteDagSchedulerStore, JobIdentity) {
        let dir = tempfile::tempdir().unwrap();
        let store = RusqliteDagSchedulerStore::open(dir.path().join("dag.db")).unwrap();
        store
            .register_plan(&CanonicalPlanRecord {
                plan_key: "plan".into(),
                artifact_digest: "digest".into(),
                target_branch: "main".into(),
                unit_ids: vec!["U1".into()],
                created_at_ms: 0,
            })
            .unwrap();
        store.activate_plan("plan", "main").unwrap();
        let id = JobIdentity {
            plan_key: "plan".into(),
            unit_id: "U1".into(),
            job_id: "job1".into(),
            hat: "executor".into(),
            stage: "execute".into(),
            attempt: 0,
            token: "nonce1".into(),
        };
        (dir, store, id)
    }

    #[test]
    fn launch_intent_survives_reopen_and_never_grants_relaunch() {
        let (dir, store, id) = fixture();
        assert!(store.reserve_job(&id, 1, None).unwrap());
        drop(store);
        let store = RusqliteDagSchedulerStore::open(dir.path().join("dag.db")).unwrap();
        assert!(!store.reserve_job(&id, 2, None).unwrap());
        let rows = store.unresolved_jobs("plan").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].identity, id);
        assert_eq!(rows[0].pid, None);
        store.record_job_pid(&id, 1234, 3).unwrap();
        store.record_job_pid(&id, 1234, 4).unwrap();
        assert!(store.record_job_pid(&id, 5678, 5).is_err());
        drop(store);
        let store = RusqliteDagSchedulerStore::open(dir.path().join("dag.db")).unwrap();
        assert_eq!(store.unresolved_jobs("plan").unwrap()[0].pid, Some(1234));
    }

    #[test]
    fn accepted_identity_fences_each_field_and_correction_attempt() {
        let (_dir, store, id) = fixture();
        store.reserve_job(&id, 1, None).unwrap();
        let digest = "a".repeat(64);
        for field in 0..7 {
            let mut wrong = id.clone();
            match field {
                0 => wrong.plan_key = "other".into(),
                1 => wrong.unit_id = "U2".into(),
                2 => wrong.job_id = "other".into(),
                3 => wrong.hat = "reviewer".into(),
                4 => wrong.stage = "review".into(),
                5 => wrong.attempt = 1,
                _ => wrong.token = "other".into(),
            }
            assert!(
                store
                    .accept_job_terminal(&wrong, "accepted", &digest, 2)
                    .is_err()
            );
        }
        let mut review = id.clone();
        review.job_id = "job2".into();
        review.token = "nonce2".into();
        review.hat = "reviewer".into();
        review.stage = "review".into();
        assert!(store.reserve_job(&review, 2, None).is_err());
        store
            .accept_job_terminal(&id, "accepted", &digest, 3)
            .unwrap();
        store
            .accept_job_terminal(&id, "accepted", &digest, 4)
            .unwrap();
        assert!(
            store
                .accept_job_terminal(&id, "rejected", &digest, 5)
                .is_err()
        );
        assert!(store.reserve_job(&review, 6, None).unwrap());
        assert!(
            store
                .accept_job_terminal(&id, "accepted", &digest, 7)
                .is_err()
        );
        store
            .accept_job_terminal(&review, "rejected", &digest, 8)
            .unwrap();
        let mut fix = review.clone();
        fix.stage = "fix".into();
        fix.hat = "fixer".into();
        fix.job_id = "job3".into();
        fix.token = "nonce3".into();
        assert!(store.reserve_job(&fix, 9, None).is_err());
        fix.attempt = 1;
        assert!(store.reserve_job(&fix, 10, None).unwrap());
        assert_eq!(store.unresolved_jobs("plan").unwrap()[0].identity, fix);
    }

    #[test]
    fn competing_connections_only_one_launch_reservation_wins() {
        let (dir, store, id) = fixture();
        let other = RusqliteDagSchedulerStore::open(dir.path().join("dag.db")).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles = [store, other]
            .into_iter()
            .map(|store| {
                let barrier = barrier.clone();
                let id = id.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.reserve_job(&id, 1, None).unwrap()
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            handles
                .into_iter()
                .filter_map(|h| h.join().unwrap().then_some(()))
                .count(),
            1
        );
    }

    #[test]
    fn terminal_reopen_keeps_digest_and_requires_fresh_successor_identity() {
        let (dir, store, id) = fixture();
        store.reserve_job(&id, 1, None).unwrap();
        let digest = "a".repeat(64);
        store
            .accept_job_terminal(&id, "accepted", &digest, 2)
            .unwrap();
        drop(store);
        let store = RusqliteDagSchedulerStore::open(dir.path().join("dag.db")).unwrap();
        assert!(store.unresolved_jobs("plan").unwrap().is_empty());
        assert!(!store.reserve_job(&id, 3, None).unwrap());
        store
            .accept_job_terminal(&id, "accepted", &digest, 4)
            .unwrap();
        assert!(
            store
                .accept_job_terminal(&id, "accepted", &"b".repeat(64), 5)
                .is_err()
        );
        let mut next = id.clone();
        next.stage = "review".into();
        next.hat = "reviewer".into();
        assert!(store.reserve_job(&next, 6, None).is_err());
        next.job_id = "job2".into();
        assert!(store.reserve_job(&next, 7, None).is_err());
        next.token = "nonce2".into();
        assert!(store.reserve_job(&next, 8, None).unwrap());
        assert_eq!(store.unresolved_jobs("plan").unwrap().len(), 1);
    }
}
