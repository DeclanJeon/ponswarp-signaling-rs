//! Optional Postgres persistence for Cloud Drop monetization state.

use crate::config::Config;
use crate::handlers::cloud_share::{CloudFileManifest, CloudShareManifest};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Row, Transaction};
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

#[derive(Debug, Clone)]
pub struct EntitlementRecord {
    pub owner_user_id: Option<Uuid>,
    pub drop_pass_id: Option<Uuid>,
    pub sku: String,
    pub label: String,
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
    pub retention_seconds: u64,
    pub download_limit: Option<u32>,
    pub monthly_quota_bytes: Option<u64>,
    pub concurrent_storage_bytes: Option<u64>,
}

pub struct PayPalOrderEntitlementInput<'a> {
    pub paypal_order_id: &'a str,
    pub paypal_capture_id: &'a str,
    pub email: Option<&'a str>,
    pub sku: &'a str,
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
    pub retention_seconds: u64,
    pub created_at: u64,
}

pub struct PayPalSubscriptionEntitlementInput<'a> {
    pub paypal_subscription_id: &'a str,
    pub paypal_plan_id: Option<&'a str>,
    pub email: Option<&'a str>,
    pub created_at: u64,
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

    pub async fn try_record_paypal_event(
        &self,
        event_id: &str,
        event_type: &str,
        created_at: u64,
        processed_at: u64,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            INSERT INTO paypal_events (id, event_type, created_at, processed_at)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(event_id)
        .bind(event_type)
        .bind(to_i64(created_at))
        .bind(to_i64(processed_at))
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn upsert_paypal_order_entitlement(
        &self,
        input: PayPalOrderEntitlementInput<'_>,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let user_id = user_id_for_email(&mut tx, input.email, input.created_at).await?;

        sqlx::query(
            r#"
            INSERT INTO drop_passes (
                id, user_id, email, paypal_order_id, paypal_capture_id,
                sku, status, max_total_bytes, max_file_bytes,
                retention_seconds, remaining_uses, created_at, expires_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, 'active', $7, $8, $9, 1, $10, $11)
            ON CONFLICT (paypal_order_id) DO UPDATE SET
                user_id = EXCLUDED.user_id,
                email = EXCLUDED.email,
                paypal_capture_id = EXCLUDED.paypal_capture_id,
                sku = EXCLUDED.sku,
                status = CASE WHEN drop_passes.status = 'consumed' THEN drop_passes.status ELSE 'active' END,
                max_total_bytes = EXCLUDED.max_total_bytes,
                max_file_bytes = EXCLUDED.max_file_bytes,
                retention_seconds = EXCLUDED.retention_seconds,
                expires_at = EXCLUDED.expires_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(input.email.and_then(normalize_email))
        .bind(input.paypal_order_id)
        .bind(input.paypal_capture_id)
        .bind(input.sku)
        .bind(to_i64(input.max_total_bytes))
        .bind(to_i64(input.max_file_bytes))
        .bind(to_i64(input.retention_seconds))
        .bind(to_i64(input.created_at))
        .bind(to_i64(input.created_at.saturating_add(30 * 24 * 60 * 60)))
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn upsert_paypal_subscription_entitlement(
        &self,
        input: PayPalSubscriptionEntitlementInput<'_>,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let fallback_email;
        let email = match input.email.and_then(normalize_email) {
            Some(email) => email,
            None => {
                fallback_email = paypal_local_email(input.paypal_subscription_id);
                &fallback_email
            }
        };
        let user_id = user_id_for_email(&mut tx, Some(email), input.created_at)
            .await?
            .expect("subscription email fallback always creates a user");

        sqlx::query(
            r#"
            INSERT INTO subscriptions (
                id, user_id, paypal_subscription_id, paypal_plan_id,
                status, current_period_start, current_period_end, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, 'active', NULL, NULL, $5, $5)
            ON CONFLICT (paypal_subscription_id) DO UPDATE SET
                user_id = EXCLUDED.user_id,
                paypal_plan_id = EXCLUDED.paypal_plan_id,
                status = 'active',
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(input.paypal_subscription_id)
        .bind(input.paypal_plan_id)
        .bind(to_i64(input.created_at))
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn set_paypal_subscription_status(
        &self,
        paypal_subscription_id: &str,
        status: &str,
        updated_at: u64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE subscriptions
            SET status = $2, updated_at = $3
            WHERE paypal_subscription_id = $1
            "#,
        )
        .bind(paypal_subscription_id)
        .bind(status)
        .bind(to_i64(updated_at))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn resolve_entitlement(
        &self,
        token: &str,
        now: u64,
    ) -> Result<Option<EntitlementRecord>, sqlx::Error> {
        if let Some(row) = sqlx::query(
            r#"
            SELECT id, user_id, sku, max_total_bytes, max_file_bytes,
                   retention_seconds, expires_at
            FROM drop_passes
            WHERE (paypal_order_id = $1 OR stripe_checkout_session_id = $1)
              AND status = 'active'
              AND remaining_uses > 0
              AND (expires_at IS NULL OR expires_at > $2)
            "#,
        )
        .bind(token)
        .bind(to_i64(now))
        .fetch_optional(&self.pool)
        .await?
        {
            let sku: String = row.get("sku");
            return Ok(Some(EntitlementRecord {
                owner_user_id: row.get("user_id"),
                drop_pass_id: Some(row.get("id")),
                label: paid_label(&sku),
                sku,
                max_total_bytes: from_i64(row.get("max_total_bytes")),
                max_file_bytes: from_i64(row.get("max_file_bytes")),
                retention_seconds: from_i64(row.get("retention_seconds")),
                download_limit: Some(30),
                monthly_quota_bytes: None,
                concurrent_storage_bytes: None,
            }));
        }

        if let Some(row) = sqlx::query(
            r#"
            SELECT s.user_id, s.paypal_subscription_id
            FROM subscriptions s
            WHERE (s.paypal_subscription_id = $1 OR s.stripe_checkout_session_id = $1)
              AND s.status IN ('active', 'trialing')
            "#,
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?
        {
            const TB: u64 = 1024 * 1024 * 1024 * 1024;
            return Ok(Some(EntitlementRecord {
                owner_user_id: Some(row.get("user_id")),
                drop_pass_id: None,
                sku: "pro_monthly_krw_9900".to_string(),
                label: "PonsWarp Pro".to_string(),
                max_total_bytes: TB,
                max_file_bytes: TB,
                retention_seconds: 7 * 24 * 60 * 60,
                download_limit: Some(30),
                monthly_quota_bytes: Some(2 * TB),
                concurrent_storage_bytes: Some(TB),
            }));
        }

        Ok(None)
    }

    pub async fn consume_drop_pass(&self, drop_pass_id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE drop_passes
            SET status = 'consumed', remaining_uses = 0
            WHERE id = $1 AND status = 'active' AND remaining_uses > 0
            "#,
        )
        .bind(drop_pass_id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn try_increment_download_count(&self, share_id: &str) -> Result<bool, sqlx::Error> {
        let Some(row) = sqlx::query(
            r#"
            SELECT download_limit, download_count
            FROM cloud_shares
            WHERE id = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(share_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(true);
        };

        let download_limit = row.get::<Option<i32>, _>("download_limit");
        let download_count = row.get::<i32, _>("download_count");
        if let Some(limit) = download_limit {
            if download_count >= limit {
                return Ok(false);
            }
        }

        sqlx::query("UPDATE cloud_shares SET download_count = download_count + 1 WHERE id = $1")
            .bind(share_id)
            .execute(&self.pool)
            .await?;
        Ok(true)
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

fn normalize_email(email: &str) -> Option<&str> {
    let email = email.trim();
    if email.is_empty() {
        None
    } else {
        Some(email)
    }
}

async fn user_id_for_email(
    tx: &mut Transaction<'_, Postgres>,
    email: Option<&str>,
    created_at: u64,
) -> Result<Option<Uuid>, sqlx::Error> {
    let Some(email) = email.and_then(normalize_email) else {
        return Ok(None);
    };
    let id = Uuid::new_v4();
    let row = sqlx::query(
        r#"
        INSERT INTO users (id, email, created_at, plan)
        VALUES ($1, $2, $3, 'free')
        ON CONFLICT (email) DO UPDATE SET email = EXCLUDED.email
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(email)
    .bind(to_i64(created_at))
    .fetch_one(&mut **tx)
    .await?;

    Ok(Some(row.get::<Uuid, _>("id")))
}

fn paypal_local_email(subscription_id: &str) -> String {
    let safe_id = subscription_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    format!("paypal+{}@ponswarp.local", safe_id)
}

fn paid_label(sku: &str) -> String {
    match sku {
        "drop_100gb_3d" => "100GB Drop Pass",
        "drop_500gb_7d" => "500GB Drop Pass",
        "drop_1tb_7d" => "1TB Drop Pass",
        _ => "Cloud Drop Pass",
    }
    .to_string()
}
