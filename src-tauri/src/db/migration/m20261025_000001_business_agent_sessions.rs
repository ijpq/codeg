use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::Statement;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "CREATE TABLE IF NOT EXISTS business_conversation_state (
              conversation_id INTEGER PRIMARY KEY REFERENCES conversation(id) ON DELETE CASCADE,
              enabled INTEGER NOT NULL DEFAULT 1, revision INTEGER NOT NULL DEFAULT 0,
              message_count INTEGER NOT NULL DEFAULT 0,
              original_agent TEXT NOT NULL, original_external_id TEXT, original_model TEXT,
              pending_request_id TEXT, task_state TEXT NOT NULL DEFAULT '{}',
              updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)",
            "CREATE TABLE IF NOT EXISTS conversation_agent_session (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              conversation_id INTEGER NOT NULL REFERENCES conversation(id) ON DELETE CASCADE,
              agent_type TEXT NOT NULL, external_id TEXT NOT NULL,
              mode_id TEXT, config_values TEXT NOT NULL DEFAULT '{}',
              seen_sequence INTEGER NOT NULL DEFAULT 0,
              last_bridge_sequence INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
              UNIQUE(conversation_id,agent_type), UNIQUE(agent_type,external_id))",
            "CREATE TABLE IF NOT EXISTS business_agent_session_alias (
              agent_type TEXT NOT NULL, external_id TEXT NOT NULL,
              session_id INTEGER NOT NULL REFERENCES conversation_agent_session(id) ON DELETE CASCADE,
              PRIMARY KEY(agent_type,external_id))",
            "CREATE TABLE IF NOT EXISTS business_agent_switch (
              request_id TEXT PRIMARY KEY, conversation_id INTEGER NOT NULL REFERENCES conversation(id) ON DELETE CASCADE,
              request_json TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'queued', error TEXT,
              created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
              updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)",
            "CREATE TABLE IF NOT EXISTS business_message (
              sequence INTEGER PRIMARY KEY AUTOINCREMENT,
              conversation_id INTEGER NOT NULL REFERENCES conversation(id) ON DELETE CASCADE,
              session_id INTEGER NOT NULL REFERENCES conversation_agent_session(id) ON DELETE CASCADE,
              native_turn_id TEXT NOT NULL, role TEXT NOT NULL, turn_json TEXT NOT NULL,
              UNIQUE(conversation_id,session_id,native_turn_id))",
            "CREATE INDEX IF NOT EXISTS idx_business_message_page ON business_message(conversation_id,sequence)",
            "CREATE INDEX IF NOT EXISTS idx_business_message_round ON business_message(conversation_id,role,sequence)",
            "CREATE TABLE IF NOT EXISTS business_prompt_receipt (
              conversation_id INTEGER NOT NULL REFERENCES conversation(id) ON DELETE CASCADE, message_id TEXT NOT NULL,
              session_id INTEGER NOT NULL REFERENCES conversation_agent_session(id) ON DELETE CASCADE,
              body_digest TEXT NOT NULL, original_blocks TEXT NOT NULL,
              context_sequence INTEGER NOT NULL, status TEXT NOT NULL,
              created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
              PRIMARY KEY(conversation_id,message_id))",
            "CREATE TABLE IF NOT EXISTS business_shared_memory (
              scope TEXT NOT NULL, scope_id INTEGER NOT NULL, name TEXT NOT NULL,
              content TEXT NOT NULL, source TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 1,
              updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
              PRIMARY KEY(scope,scope_id,name))",
        ] {
            manager.get_connection().execute(Statement::from_string(
                manager.get_database_backend(), sql.to_owned(),
            )).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Rollback restores the original legacy projection. Keep the additive
        // tables as an inactive archive: downgrading must not destroy replies
        // or native session identities written after the upgrade.
        for sql in [
            "UPDATE conversation SET
              agent_type=(SELECT original_agent FROM business_conversation_state b WHERE b.conversation_id=conversation.id),
              external_id=(SELECT original_external_id FROM business_conversation_state b WHERE b.conversation_id=conversation.id),
              model=(SELECT original_model FROM business_conversation_state b WHERE b.conversation_id=conversation.id)
              WHERE id IN (SELECT conversation_id FROM business_conversation_state WHERE enabled=1)",
            "UPDATE business_conversation_state SET enabled=0, pending_request_id=NULL",
            "UPDATE business_agent_switch SET status='suspended' WHERE status IN ('queued','preparing')",
        ] {
            manager.get_connection().execute(Statement::from_string(
                manager.get_database_backend(), sql.to_owned(),
            )).await?;
        }
        Ok(())
    }
}
