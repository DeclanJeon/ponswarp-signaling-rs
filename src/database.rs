//! Optional Postgres persistence for Cloud Drop monetization state.

use crate::config::Config;
use crate::handlers::cloud_share::{CloudFileManifest, CloudShareManifest};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub struct CloudDatabase {
    pool: PgPool,
}

pub struct CloudSharePolicyRecord {
    pub plan_snapshot: Value,
    pub owner_user_id: Option<Uuid>,
    pub drop_pass_id: Option<Uuid>,
    pub password_hash: Option<String>,
    pub download_limit: Option<u32>,
}

impl CloudDatabase {
    pub async fn from_config(config: &Config) -> Result<Option<Self>> {
        let database = &config.database;
        if database.url.trim().is_empty() {
            if config.cloud.billing_enabled {
                bail!("PONSWARP_BILLING_ENABLED=true requires DATABASE_URL or POSTGRES_URL");
            }
            tracing::info!("Cloud Drop database disabled: DATABASE_URL is not configured");
            return Ok(None);
        }

        let pool = PgPoolOptions::new()
            .max_connections(database.max_connections)
            .connect(&database.url)
            .await
            .context("failed to connect to Postgres")?;

        if database.run_migrations {
            sqlx::migrate!("./migrations")
                .run(&pool)
                .await
                .context("failed to run Cloud Drop migrations")?;
        }

        tracing::info!("Cloud Drop database enabled");
        Ok(Some(Self { pool }))
    }

    pub async fn insert_cloud_share(
        &self,
        manifest: &CloudShareManifest,
        policy: CloudSharePolicyRecord,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO cloud_shares (
                id, owner_user_id, drop_pass_id, plan_snapshot, root_name,
                total_size, total_files, completed, password_hash,
                download_limit, created_at, expires_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (id) DO UPDATE SET
                owner_user_id = EXCLUDED.owner_user_id,
                drop_pass_id = EXCLUDED.drop_pass_id,
                plan_snapshot = EXCLUDED.plan_snapshot,
                root_name = EXCLUDED.root_name,
                total_size = EXCLUDED.total_size,
                total_files = EXCLUDED.total_files,
                completed = EXCLUDED.completed,
                password_hash = EXCLUDED.password_hash,
                download_limit = EXCLUDED.download_limit,
                created_at = EXCLUDED.created_at,
                expires_at = EXCLUDED.expires_at,
                deleted_at = NULL
            "#,
        )
        .bind(&manifest.share_id)
        .bind(policy.owner_user_id)
        .bind(policy.drop_pass_id)
        .bind(policy.plan_snapshot)
        .bind(&manifest.root_name)
        .bind(to_i64(manifest.total_size))
        .bind(to_i32(manifest.total_files))
        .bind(manifest.completed)
        .bind(policy.password_hash)
        .bind(policy.download_limit.map(to_i32_from_u32))
        .bind(to_i64(manifest.created_at))
        .bind(to_i64(manifest.expires_at))
        .execute(&mut *tx)
        .await?;

        for file in &manifest.files {
            sqlx::query(
                r#"
                INSERT INTO cloud_share_files (
                    id, share_id, name, path, size, content_type,
                    last_modified, object_key
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (id) DO UPDATE SET
                    share_id = EXCLUDED.share_id,
                    name = EXCLUDED.name,
                    path = EXCLUDED.path,
                    size = EXCLUDED.size,
                    content_type = EXCLUDED.content_type,
                    last_modified = EXCLUDED.last_modified,
                    object_key = EXCLUDED.object_key
                "#,
            )
            .bind(&file.id)
            .bind(&manifest.share_id)
            .bind(&file.name)
            .bind(&file.path)
            .bind(to_i64(file.size))
            .bind(&file.content_type)
            .bind(file.last_modified.map(to_i64))
            .bind(&file.object_key)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn mark_cloud_share_completed(
        &self,
        share_id: &str,
        total_size: u64,
        created_at: u64,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        let update = sqlx::query("UPDATE cloud_shares SET completed = true WHERE id = $1")
            .bind(share_id)
            .execute(&mut *tx)
            .await?;

        if update.rows_affected() > 0 {
            sqlx::query(
                r#"
                INSERT INTO cloud_usage_events (
                    id, user_id, share_id, event_type, bytes, created_at
                )
                SELECT $1, owner_user_id, id, 'cloud_share_completed', $3, $4
                FROM cloud_shares
                WHERE id = $2
                "#,
            )
            .bind(uuid::Uuid::new_v4())
            .bind(share_id)
            .bind(to_i64(total_size))
            .bind(to_i64(created_at))
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn mark_cloud_share_deleted(
        &self,
        share_id: &str,
        deleted_at: u64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE cloud_shares SET deleted_at = $2 WHERE id = $1")
            .bind(share_id)
            .bind(to_i64(deleted_at))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn read_cloud_share(
        &self,
        share_id: &str,
    ) -> Result<Option<CloudShareManifest>, sqlx::Error> {
        let Some(share) = sqlx::query(
            r#"
            SELECT id, root_name, total_size, total_files, created_at, expires_at, completed
            FROM cloud_shares
            WHERE id = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(share_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let file_rows = sqlx::query(
            r#"
            SELECT id, name, path, size, content_type, last_modified, object_key
            FROM cloud_share_files
            WHERE share_id = $1
            ORDER BY path, id
            "#,
        )
        .bind(share_id)
        .fetch_all(&self.pool)
        .await?;

        let files = file_rows
            .into_iter()
            .map(|row| CloudFileManifest {
                id: row.get("id"),
                name: row.get("name"),
                path: row.get("path"),
                size: from_i64(row.get("size")),
                content_type: row.get("content_type"),
                last_modified: row.get::<Option<i64>, _>("last_modified").map(from_i64),
                object_key: row.get("object_key"),
            })
            .collect::<Vec<_>>();

        Ok(Some(CloudShareManifest {
            share_id: share.get("id"),
            root_name: share.get("root_name"),
            total_size: from_i64(share.get("total_size")),
            total_files: share
                .get::<i32, _>("total_files")
                .try_into()
                .unwrap_or_default(),
            created_at: from_i64(share.get("created_at")),
            expires_at: from_i64(share.get("expires_at")),
            completed: share.get("completed"),
            files,
        }))
    }
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn to_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn to_i32_from_u32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn from_i64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}
