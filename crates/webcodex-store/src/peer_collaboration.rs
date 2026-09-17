use crate::Database;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

pub const MAX_PEER_MESSAGE_LIMIT: usize = 8;
pub const MAX_PEER_DISCOVERY_LIMIT: usize = 8;
const MAX_RETAINED_PEER_MESSAGES_PER_PRINCIPAL: usize = 512;
const MAX_RETAINED_PEER_DISCOVERIES_PER_PRINCIPAL: usize = 1024;

#[derive(Debug, Clone)]
pub struct NewPeerMessage {
    pub principal_kind: String,
    pub principal_id: String,
    pub sender_window_key: String,
    pub recipient_window_key: String,
    pub sender_peer_id: String,
    pub recipient_peer_id: String,
    pub kind: String,
    pub priority: String,
    pub message: String,
    pub tags: Vec<String>,
    pub requires_ack: bool,
    pub sender_session_id: Option<String>,
    pub sender_project: Option<String>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerMessageRecord {
    pub message_id: String,
    pub sender_peer_id: String,
    pub recipient_peer_id: String,
    pub kind: String,
    pub priority: String,
    pub message: String,
    pub tags: Vec<String>,
    pub requires_ack: bool,
    pub created_at_ms: i64,
    pub first_projected_at_ms: Option<i64>,
    pub last_projected_at_ms: Option<i64>,
    pub projection_count: usize,
    pub first_ack_observed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct PeerAttentionBatch {
    pub messages: Vec<PeerMessageRecord>,
    pub accepted_ack_ids: Vec<String>,
    pub projection_rollbacks: Vec<PeerProjectionRollback>,
}

#[derive(Debug, Clone)]
pub struct PeerProjectionRollback {
    pub message_id: String,
    pub first_projected_at_ms: Option<i64>,
    pub last_projected_at_ms: Option<i64>,
    pub projection_count: usize,
}

#[derive(Debug, Clone)]
pub struct RecentProjectPeerRecord {
    pub client_window_key: String,
    pub client_window_source: String,
    pub last_meaningful_activity_at_ms: i64,
    pub discovery_rowid: Option<i64>,
}

impl Database {
    pub fn resolve_peer_window_prefix(
        &self,
        principal_kind: &str,
        principal_id: &str,
        window_key_prefix: &str,
    ) -> anyhow::Result<Option<String>> {
        let conn = self.lock_connection(crate::StoreDomain::WindowActivity);
        let mut stmt = conn.prepare(
            "SELECT candidate.client_window_key
             FROM (
                 SELECT d.peer_window_key AS client_window_key,
                        d.first_projected_at_ms AS observed_at_ms
                 FROM window_peer_discoveries d
                 WHERE d.principal_kind = ?1
                   AND d.principal_id = ?2
                 UNION ALL
                 SELECT m.sender_window_key,
                        m.created_at_ms AS observed_at_ms
                 FROM window_peer_messages m
                 WHERE m.principal_kind = ?1
                   AND m.principal_id = ?2
                 UNION ALL
                 SELECT m.recipient_window_key,
                        m.created_at_ms AS observed_at_ms
                 FROM window_peer_messages m
                 WHERE m.principal_kind = ?1
                   AND m.principal_id = ?2
             ) candidate
             WHERE substr(candidate.client_window_key, 1, 32) = ?3
             GROUP BY candidate.client_window_key
             ORDER BY MAX(candidate.observed_at_ms) DESC
             LIMIT 2",
        )?;
        let mut rows = stmt.query(params![principal_kind, principal_id, window_key_prefix])?;
        let mut matches = Vec::new();
        while let Some(row) = rows.next()? {
            matches.push(row.get::<_, String>(0)?);
        }
        Ok((matches.len() == 1).then(|| matches.remove(0)))
    }

    pub fn post_peer_message(&self, input: NewPeerMessage) -> anyhow::Result<PeerMessageRecord> {
        let tags_json = serde_json::to_string(&input.tags)?;
        let mut conn = self.lock_connection(crate::StoreDomain::Communication);
        let tx = conn.transaction()?;
        let message_id = loop {
            let candidate = format!("wc_msg_{}", webcodex_core::compact::random_suffix::<12>());
            let exists = tx
                .query_row(
                    "SELECT 1 FROM window_peer_messages WHERE message_id = ?1",
                    params![candidate],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !exists {
                break candidate;
            }
        };
        tx.execute(
            "INSERT INTO window_peer_messages (
                message_id, principal_kind, principal_id,
                sender_window_key, recipient_window_key,
                sender_peer_id, recipient_peer_id,
                kind, priority, message, tags_json, requires_ack,
                created_at_ms, sender_session_id, sender_project,
                first_projected_at_ms, last_projected_at_ms,
                projection_count, first_ack_observed_at_ms
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, NULL, NULL, 0, NULL
             )",
            params![
                message_id,
                input.principal_kind,
                input.principal_id,
                input.sender_window_key,
                input.recipient_window_key,
                input.sender_peer_id,
                input.recipient_peer_id,
                input.kind,
                input.priority,
                input.message,
                tags_json,
                i64::from(input.requires_ack),
                input.created_at_ms,
                input.sender_session_id,
                input.sender_project,
            ],
        )?;
        tx.execute(
            "DELETE FROM window_peer_messages
             WHERE rowid IN (
                 SELECT rowid
                 FROM window_peer_messages
                 WHERE principal_kind = ?1 AND principal_id = ?2
                 ORDER BY created_at_ms DESC, message_id DESC
                 LIMIT -1 OFFSET ?3
             )",
            params![
                input.principal_kind,
                input.principal_id,
                MAX_RETAINED_PEER_MESSAGES_PER_PRINCIPAL as i64,
            ],
        )?;
        let record = PeerMessageRecord {
            message_id,
            sender_peer_id: input.sender_peer_id,
            recipient_peer_id: input.recipient_peer_id,
            kind: input.kind,
            priority: input.priority,
            message: input.message,
            tags: input.tags,
            requires_ack: input.requires_ack,
            created_at_ms: input.created_at_ms,
            first_projected_at_ms: None,
            last_projected_at_ms: None,
            projection_count: 0,
            first_ack_observed_at_ms: None,
        };
        tx.commit()?;
        Ok(record)
    }

    pub fn take_peer_attention(
        &self,
        principal_kind: &str,
        principal_id: &str,
        recipient_window_key: &str,
        ack_message_ids: &[String],
        now_ms: i64,
        limit: usize,
    ) -> anyhow::Result<PeerAttentionBatch> {
        let mut conn = self.lock_connection(crate::StoreDomain::Communication);
        let tx = conn.transaction()?;
        let mut accepted_ack_ids = Vec::new();
        for message_id in ack_message_ids.iter().take(8) {
            let exists = tx
                .query_row(
                    "SELECT 1
                     FROM window_peer_messages
                     WHERE message_id = ?1
                       AND principal_kind = ?2
                       AND principal_id = ?3
                       AND recipient_window_key = ?4
                       AND requires_ack = 1",
                    params![
                        message_id,
                        principal_kind,
                        principal_id,
                        recipient_window_key
                    ],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !exists {
                continue;
            }
            accepted_ack_ids.push(message_id.clone());
            tx.execute(
                "UPDATE window_peer_messages
                 SET first_ack_observed_at_ms = COALESCE(first_ack_observed_at_ms, ?2)
                 WHERE message_id = ?1",
                params![message_id, now_ms],
            )?;
        }

        let scan_limit = limit
            .clamp(1, MAX_PEER_MESSAGE_LIMIT)
            .saturating_add(accepted_ack_ids.len())
            .min(MAX_PEER_MESSAGE_LIMIT + 8) as i64;
        let mut candidates = {
            let mut stmt = tx.prepare(
                "SELECT message_id, sender_peer_id, recipient_peer_id,
                        kind, priority, message, tags_json, requires_ack,
                        created_at_ms, first_projected_at_ms, last_projected_at_ms,
                        projection_count, first_ack_observed_at_ms
                 FROM window_peer_messages
                 WHERE principal_kind = ?1
                   AND principal_id = ?2
                   AND recipient_window_key = ?3
                   AND (requires_ack = 1 OR first_projected_at_ms IS NULL)
                 ORDER BY (first_projected_at_ms IS NULL) DESC,
                           created_at_ms ASC, message_id ASC
                 LIMIT ?4",
            )?;
            let mut rows = stmt.query(params![
                principal_kind,
                principal_id,
                recipient_window_key,
                scan_limit
            ])?;
            let mut records = Vec::new();
            while let Some(row) = rows.next()? {
                records.push(peer_message_from_row(row)?);
            }
            records
        };
        candidates.retain(|message| {
            !message.requires_ack || !accepted_ack_ids.contains(&message.message_id)
        });
        candidates.truncate(limit.clamp(1, MAX_PEER_MESSAGE_LIMIT));

        let projection_rollbacks = candidates
            .iter()
            .map(|message| PeerProjectionRollback {
                message_id: message.message_id.clone(),
                first_projected_at_ms: message.first_projected_at_ms,
                last_projected_at_ms: message.last_projected_at_ms,
                projection_count: message.projection_count,
            })
            .collect::<Vec<_>>();

        for message in &mut candidates {
            tx.execute(
                "UPDATE window_peer_messages
                 SET first_projected_at_ms = COALESCE(first_projected_at_ms, ?2),
                     last_projected_at_ms = ?2,
                     projection_count = projection_count + 1
                 WHERE message_id = ?1",
                params![message.message_id, now_ms],
            )?;
            message.first_projected_at_ms.get_or_insert(now_ms);
            message.last_projected_at_ms = Some(now_ms);
            message.projection_count = message.projection_count.saturating_add(1);
        }
        tx.commit()?;
        Ok(PeerAttentionBatch {
            messages: candidates,
            accepted_ack_ids,
            projection_rollbacks,
        })
    }

    pub fn take_new_recent_project_peers(
        &self,
        principal_kind: &str,
        principal_id: &str,
        observer_window_key: &str,
        project: &str,
        since_ms: i64,
        now_ms: i64,
        limit: usize,
    ) -> anyhow::Result<Vec<RecentProjectPeerRecord>> {
        let mut conn = self.lock_connection(crate::StoreDomain::WindowActivity);
        let tx = conn.transaction()?;
        let limit = limit.clamp(1, MAX_PEER_DISCOVERY_LIMIT) as i64;
        let peers = {
            let mut stmt = tx.prepare(
                "SELECT e.client_window_key,
                        MAX(e.client_window_source),
                        MAX(e.window_ended_at_ms)
                 FROM action_events e
                 WHERE e.client_window_key IS NOT NULL
                   AND e.client_window_key <> ?1
                   AND e.principal_correlation_kind = ?2
                   AND e.principal_correlation_id = ?3
                   AND e.project = ?4
                   AND e.window_meaningful = 1
                   AND e.window_ended_at_ms >= ?5
                   AND NOT EXISTS (
                       SELECT 1 FROM window_peer_discoveries d
                       WHERE d.principal_kind = ?2
                         AND d.principal_id = ?3
                         AND d.observer_window_key = ?1
                         AND d.peer_window_key = e.client_window_key
                         AND d.project = ?4
                   )
                 GROUP BY e.client_window_key
                 ORDER BY MAX(e.window_ended_at_ms) DESC, e.client_window_key ASC
                 LIMIT ?6",
            )?;
            let mut rows = stmt.query(params![
                observer_window_key,
                principal_kind,
                principal_id,
                project,
                since_ms,
                limit,
            ])?;
            let mut peers = Vec::new();
            while let Some(row) = rows.next()? {
                peers.push(RecentProjectPeerRecord {
                    client_window_key: row.get(0)?,
                    client_window_source: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    last_meaningful_activity_at_ms: row.get(2)?,
                    discovery_rowid: None,
                });
            }
            peers
        };
        let mut inserted_peers = Vec::new();
        for mut peer in peers {
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO window_peer_discoveries (
                    principal_kind, principal_id, observer_window_key,
                    peer_window_key, project, first_projected_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    principal_kind,
                    principal_id,
                    observer_window_key,
                    peer.client_window_key,
                    project,
                    now_ms,
                ],
            )?;
            if inserted == 1 {
                peer.discovery_rowid = Some(tx.last_insert_rowid());
                inserted_peers.push(peer);
            }
        }
        tx.execute(
            "DELETE FROM window_peer_discoveries
             WHERE rowid IN (
                 SELECT rowid
                 FROM window_peer_discoveries
                 WHERE principal_kind = ?1 AND principal_id = ?2
                 ORDER BY first_projected_at_ms DESC, rowid DESC
                 LIMIT -1 OFFSET ?3
             )",
            params![
                principal_kind,
                principal_id,
                MAX_RETAINED_PEER_DISCOVERIES_PER_PRINCIPAL as i64,
            ],
        )?;
        tx.commit()?;
        Ok(inserted_peers)
    }
    pub fn rollback_peer_attention(
        &self,
        principal_kind: &str,
        principal_id: &str,
        recipient_window_key: &str,
        projected_at_ms: i64,
        rollbacks: &[PeerProjectionRollback],
    ) -> anyhow::Result<()> {
        if rollbacks.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock_connection(crate::StoreDomain::Communication);
        let tx = conn.transaction()?;
        for rollback in rollbacks {
            let previous_count = i64::try_from(rollback.projection_count).unwrap_or(i64::MAX);
            let projected_count = previous_count.saturating_add(1);
            tx.execute(
                "UPDATE window_peer_messages
                 SET first_projected_at_ms = ?5,
                     last_projected_at_ms = ?6,
                     projection_count = ?7
                 WHERE message_id = ?1
                   AND principal_kind = ?2
                   AND principal_id = ?3
                   AND recipient_window_key = ?4
                   AND last_projected_at_ms = ?8
                   AND projection_count = ?9",
                params![
                    rollback.message_id,
                    principal_kind,
                    principal_id,
                    recipient_window_key,
                    rollback.first_projected_at_ms,
                    rollback.last_projected_at_ms,
                    previous_count,
                    projected_at_ms,
                    projected_count,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn rollback_peer_discoveries(
        &self,
        principal_kind: &str,
        principal_id: &str,
        observer_window_key: &str,
        project: &str,
        rowids: &[i64],
    ) -> anyhow::Result<()> {
        if rowids.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock_connection(crate::StoreDomain::WindowActivity);
        let tx = conn.transaction()?;
        for rowid in rowids {
            tx.execute(
                "DELETE FROM window_peer_discoveries
                 WHERE rowid = ?1
                   AND principal_kind = ?2
                   AND principal_id = ?3
                   AND observer_window_key = ?4
                   AND project = ?5",
                params![
                    rowid,
                    principal_kind,
                    principal_id,
                    observer_window_key,
                    project
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

fn peer_message_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PeerMessageRecord> {
    let tags_json = row.get::<_, String>(6)?;
    let tags = serde_json::from_str::<Vec<String>>(&tags_json).unwrap_or_default();
    Ok(PeerMessageRecord {
        message_id: row.get(0)?,
        sender_peer_id: row.get(1)?,
        recipient_peer_id: row.get(2)?,
        kind: row.get(3)?,
        priority: row.get(4)?,
        message: row.get(5)?,
        tags,
        requires_ack: row.get::<_, i64>(7)? != 0,
        created_at_ms: row.get(8)?,
        first_projected_at_ms: row.get(9)?,
        last_projected_at_ms: row.get(10)?,
        projection_count: usize::try_from(row.get::<_, i64>(11)?).unwrap_or(usize::MAX),
        first_ack_observed_at_ms: row.get(12)?,
    })
}
