use std::{collections::HashMap, str::FromStr};

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

use crate::{
    config::Config,
    domain::{
        AdminRouteRow, ChannelRow, ChannelRuntimeStats, ModelRouteRow, RequestLogRow,
        RequestLogWrite,
    },
    error::AppError,
    protocol::Protocol,
};

#[derive(Debug, Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

#[derive(Debug, Clone)]
pub struct DeleteChannelOutcome {
    pub channel: ChannelRow,
    pub route_model: String,
}

#[derive(Debug, Clone)]
pub struct DeleteRouteOutcome {
    pub route: ModelRouteRow,
    pub deleted_channel_count: i64,
}

impl SqliteStore {
    pub async fn connect(config: &Config) -> Result<Self, AppError> {
        let options = SqliteConnectOptions::from_str(&config.database_url)
            .map_err(|error| AppError::Config(format!("invalid sqlite url: {error}")))?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }

    pub async fn find_route(&self, model: &str) -> Result<ModelRouteRow, AppError> {
        let route = sqlx::query_as::<_, ModelRouteRow>(
            r#"
            select id, model_pattern, enabled, routing_strategy, cooldown_seconds
            from model_routes
            where enabled = 1 and model_pattern = ?
            limit 1
            "#,
        )
        .bind(model)
        .fetch_optional(&self.pool)
        .await?;

        route.ok_or_else(|| AppError::NoRoute(format!("no route configured for model: {model}")))
    }

    pub async fn get_route(&self, route_id: i64) -> Result<ModelRouteRow, AppError> {
        let route = sqlx::query_as::<_, ModelRouteRow>(
            r#"
            select id, model_pattern, enabled, routing_strategy, cooldown_seconds
            from model_routes
            where id = ?
            limit 1
            "#,
        )
        .bind(route_id)
        .fetch_optional(&self.pool)
        .await?;

        route.ok_or_else(|| AppError::NotFound(format!("route not found: {route_id}")))
    }

    pub async fn create_or_get_route(
        &self,
        route_model: &str,
        cooldown_seconds: i64,
    ) -> Result<(ModelRouteRow, bool), AppError> {
        let route_model = normalize_route_model(route_model)?;
        let cooldown_seconds = normalize_cooldown_seconds(cooldown_seconds)?;

        if let Some(existing) = sqlx::query_as::<_, ModelRouteRow>(
            r#"
            select id, model_pattern, enabled, routing_strategy, cooldown_seconds
            from model_routes
            where model_pattern = ?
            limit 1
            "#,
        )
        .bind(&route_model)
        .fetch_optional(&self.pool)
        .await?
        {
            return Ok((existing, false));
        }

        let route_id = sqlx::query(
            r#"
            insert into model_routes (model_pattern, enabled, routing_strategy, cooldown_seconds)
            values (?, 1, 'priority', ?)
            "#,
        )
        .bind(&route_model)
        .bind(cooldown_seconds)
        .execute(&self.pool)
        .await?
        .last_insert_rowid();

        Ok((self.get_route(route_id).await?, true))
    }

    pub async fn upsert_route(
        &self,
        route_model: &str,
        cooldown_seconds: i64,
    ) -> Result<(ModelRouteRow, bool), AppError> {
        let route_model = normalize_route_model(route_model)?;
        let cooldown_seconds = normalize_cooldown_seconds(cooldown_seconds)?;

        if let Some(existing) = sqlx::query_as::<_, ModelRouteRow>(
            r#"
            select id, model_pattern, enabled, routing_strategy, cooldown_seconds
            from model_routes
            where model_pattern = ?
            limit 1
            "#,
        )
        .bind(&route_model)
        .fetch_optional(&self.pool)
        .await?
        {
            sqlx::query(
                r#"
                update model_routes
                set cooldown_seconds = ?,
                    updated_at = current_timestamp
                where id = ?
                "#,
            )
            .bind(cooldown_seconds)
            .bind(existing.id)
            .execute(&self.pool)
            .await?;

            return Ok((self.get_route(existing.id).await?, false));
        }

        self.create_or_get_route(&route_model, cooldown_seconds)
            .await
    }

    pub async fn list_routes(&self, now_ts: i64) -> Result<Vec<AdminRouteRow>, AppError> {
        let rows = sqlx::query_as::<_, AdminRouteRow>(
            r#"
            select
              mr.id,
              mr.model_pattern,
              mr.enabled,
              mr.routing_strategy,
              mr.cooldown_seconds,
              count(c.id) as channel_count,
              coalesce(sum(case when c.enabled = 1 then 1 else 0 end), 0) as enabled_channel_count,
              coalesce(sum(case
                when c.enabled = 1
                  and a.status = 'active'
                  and s.status = 'active'
                  and c.manual_blocked = 0
                  and (c.cooldown_until is null or c.cooldown_until <= ?)
                then 1 else 0 end), 0) as ready_channel_count,
              coalesce(sum(case
                when c.cooldown_until is not null and c.cooldown_until > ?
                then 1 else 0 end), 0) as cooling_channel_count
              ,
              coalesce(sum(case
                when c.manual_blocked = 1
                then 1 else 0 end), 0) as manual_blocked_channel_count
            from model_routes mr
            left join channels c on c.route_id = mr.id
            left join accounts a on a.id = c.account_id
            left join sites s on s.id = a.site_id
            group by mr.id
            order by mr.id asc
            "#,
        )
        .bind(now_ts)
        .bind(now_ts)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    pub async fn list_export_routes(&self) -> Result<Vec<ModelRouteRow>, AppError> {
        let rows = sqlx::query_as::<_, ModelRouteRow>(
            r#"
            select id, model_pattern, enabled, routing_strategy, cooldown_seconds
            from model_routes
            order by id asc
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    pub async fn load_channels(&self, route_id: i64) -> Result<Vec<ChannelRow>, AppError> {
        let rows = sqlx::query_as::<_, ChannelRow>(CHANNEL_SELECT_BY_ROUTE_SQL)
            .bind(route_id)
            .fetch_all(&self.pool)
            .await?;

        Ok(rows)
    }

    pub async fn create_channel_for_route(
        &self,
        route: &ModelRouteRow,
        base_url: &str,
        api_key: &str,
        upstream_model: Option<&str>,
        protocol: &str,
        priority: i64,
    ) -> Result<ChannelRow, AppError> {
        let normalized_base_url = normalize_base_url(base_url)?;
        let api_key = normalize_api_key(api_key)?;
        let upstream_model = normalize_upstream_model(upstream_model, &route.model_pattern)?;
        let protocol = normalize_protocol(protocol)?;
        let priority = normalize_priority(priority)?;

        let mut tx = self.pool.begin().await?;
        let account_id = ensure_account(&mut tx, &normalized_base_url, api_key).await?;
        let channel_id = insert_channel(
            &mut tx,
            route.id,
            account_id,
            &upstream_model,
            &protocol,
            true,
            priority,
        )
        .await?;

        tx.commit().await?;

        self.load_channel(channel_id).await
    }

    pub async fn update_channel(
        &self,
        channel_id: i64,
        base_url: &str,
        api_key: &str,
        upstream_model: &str,
        protocol: &str,
        priority: i64,
    ) -> Result<ChannelRow, AppError> {
        let existing = self.load_channel(channel_id).await?;
        let normalized_base_url = normalize_base_url(base_url)?;
        let api_key = normalize_api_key(api_key)?;
        let upstream_model = normalize_upstream_model(Some(upstream_model), "")?;
        let protocol = normalize_protocol(protocol)?;
        let priority = normalize_priority(priority)?;

        let mut tx = self.pool.begin().await?;
        let next_account_id = ensure_account(&mut tx, &normalized_base_url, api_key).await?;

        sqlx::query(
            r#"
            update channels
            set account_id = ?,
                upstream_model = ?,
                protocol = ?,
                priority = ?,
                updated_at = current_timestamp
            where id = ?
            "#,
        )
        .bind(next_account_id)
        .bind(upstream_model)
        .bind(protocol)
        .bind(priority)
        .bind(channel_id)
        .execute(&mut *tx)
        .await?;

        cleanup_account_if_unused(&mut tx, existing.account_id).await?;
        tx.commit().await?;

        self.load_channel(channel_id).await
    }

    pub async fn sync_channel_for_route(
        &self,
        route: &ModelRouteRow,
        base_url: &str,
        api_key: &str,
        upstream_model: &str,
        protocol: &str,
        priority: i64,
        enabled: bool,
    ) -> Result<(ChannelRow, bool), AppError> {
        let normalized_base_url = normalize_base_url(base_url)?;
        let api_key = normalize_api_key(api_key)?;
        let upstream_model = normalize_upstream_model(Some(upstream_model), &route.model_pattern)?;
        let protocol = normalize_protocol(protocol)?;
        let priority = normalize_priority(priority)?;

        let mut tx = self.pool.begin().await?;
        let account_id = ensure_account(&mut tx, &normalized_base_url, api_key).await?;

        if let Some(existing_channel_id) = sqlx::query_scalar::<_, i64>(
            r#"
            select id
            from channels
            where route_id = ?
              and account_id = ?
              and upstream_model = ?
            limit 1
            "#,
        )
        .bind(route.id)
        .bind(account_id)
        .bind(&upstream_model)
        .fetch_optional(&mut *tx)
        .await?
        {
            sqlx::query(
                r#"
                update channels
                set enabled = ?,
                    protocol = ?,
                    priority = ?,
                    updated_at = current_timestamp
                where id = ?
                "#,
            )
            .bind(if enabled { 1_i64 } else { 0_i64 })
            .bind(&protocol)
            .bind(priority)
            .bind(existing_channel_id)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            return Ok((self.load_channel(existing_channel_id).await?, false));
        }

        let channel_id = insert_channel(
            &mut tx,
            route.id,
            account_id,
            &upstream_model,
            &protocol,
            enabled,
            priority,
        )
        .await?;

        tx.commit().await?;
        Ok((self.load_channel(channel_id).await?, true))
    }

    pub async fn delete_channel(&self, channel_id: i64) -> Result<DeleteChannelOutcome, AppError> {
        let channel = self.load_channel(channel_id).await?;
        let route = self.get_route(channel.route_id).await?;
        let mut tx = self.pool.begin().await?;

        let affected = sqlx::query("delete from channels where id = ?")
            .bind(channel_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!(
                "channel not found: {channel_id}"
            )));
        }

        cleanup_account_if_unused(&mut tx, channel.account_id).await?;

        tx.commit().await?;

        Ok(DeleteChannelOutcome {
            channel,
            route_model: route.model_pattern,
        })
    }

    pub async fn delete_route(&self, route_id: i64) -> Result<DeleteRouteOutcome, AppError> {
        let route = self.get_route(route_id).await?;
        let channel_count =
            sqlx::query_scalar::<_, i64>("select count(*) from channels where route_id = ?")
                .bind(route_id)
                .fetch_one(&self.pool)
                .await?;

        if channel_count > 0 {
            return Err(AppError::BadRequest(format!(
                "route `{}` is not empty; delete its channels first",
                route.model_pattern
            )));
        }

        let affected = sqlx::query("delete from model_routes where id = ?")
            .bind(route_id)
            .execute(&self.pool)
            .await?
            .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!("route not found: {route_id}")));
        }

        Ok(DeleteRouteOutcome {
            route,
            deleted_channel_count: channel_count,
        })
    }

    pub async fn onboard_route_channel(
        &self,
        route_model: &str,
        base_url: &str,
        api_key: &str,
        upstream_model: Option<&str>,
        protocol: &str,
        priority: i64,
        cooldown_seconds: i64,
    ) -> Result<(ModelRouteRow, ChannelRow, bool), AppError> {
        let cooldown_seconds = normalize_cooldown_seconds(cooldown_seconds)?;
        let (route, route_created) = self
            .create_or_get_route(route_model, cooldown_seconds)
            .await?;
        let channel = self
            .create_channel_for_route(
                &route,
                base_url,
                api_key,
                upstream_model,
                protocol,
                priority,
            )
            .await?;

        Ok((route, channel, route_created))
    }

    pub async fn load_channel(&self, channel_id: i64) -> Result<ChannelRow, AppError> {
        let row = sqlx::query_as::<_, ChannelRow>(CHANNEL_SELECT_BY_ID_SQL)
            .bind(channel_id)
            .fetch_optional(&self.pool)
            .await?;

        row.ok_or_else(|| AppError::NotFound(format!("channel not found: {channel_id}")))
    }

    pub async fn set_channel_enabled(
        &self,
        channel_id: i64,
        enabled: bool,
    ) -> Result<ChannelRow, AppError> {
        let affected = sqlx::query(
            r#"
            update channels
            set enabled = ?,
                updated_at = current_timestamp
            where id = ?
            "#,
        )
        .bind(if enabled { 1_i64 } else { 0_i64 })
        .bind(channel_id)
        .execute(&self.pool)
        .await?
        .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!(
                "channel not found: {channel_id}"
            )));
        }

        self.load_channel(channel_id).await
    }

    pub async fn reset_channel_cooldown(&self, channel_id: i64) -> Result<ChannelRow, AppError> {
        let affected = sqlx::query(
            r#"
            update channels
            set cooldown_until = null,
                manual_blocked = 0,
                consecutive_fail_count = 0,
                updated_at = current_timestamp
            where id = ?
            "#,
        )
        .bind(channel_id)
        .execute(&self.pool)
        .await?
        .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!(
                "channel not found: {channel_id}"
            )));
        }

        self.load_channel(channel_id).await
    }

    pub async fn record_request(&self, log: &RequestLogWrite) -> Result<(), AppError> {
        sqlx::query(
            r#"
            insert into request_logs (
              request_id,
              downstream_path,
              upstream_path,
              model_requested,
              channel_id,
              http_status,
              latency_ms,
              error_message,
              input_tokens,
              output_tokens,
              total_tokens
            ) values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&log.request_id)
        .bind(&log.downstream_path)
        .bind(&log.upstream_path)
        .bind(&log.model_requested)
        .bind(log.channel_id)
        .bind(log.http_status)
        .bind(log.latency_ms)
        .bind(&log.error_message)
        .bind(log.input_tokens)
        .bind(log.output_tokens)
        .bind(log.total_tokens)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn list_channel_runtime_stats(
        &self,
        route_id: i64,
    ) -> Result<HashMap<i64, ChannelRuntimeStats>, AppError> {
        let rows = sqlx::query_as::<_, ChannelRuntimeStats>(
            r#"
            select
              c.id as channel_id,
              c.avg_latency_ms as avg_latency_ms,
              count(rl.id) as requests_24h,
              coalesce(sum(case
                when rl.http_status is not null
                  and rl.http_status < 400
                  and rl.error_message is null
                then 1 else 0 end), 0) as success_requests_24h,
              coalesce(sum(coalesce(rl.input_tokens, 0)), 0) as input_tokens_24h,
              coalesce(sum(coalesce(rl.output_tokens, 0)), 0) as output_tokens_24h,
              coalesce(sum(coalesce(rl.total_tokens, 0)), 0) as total_tokens_24h
            from channels c
            left join request_logs rl
              on rl.channel_id = c.id
             and rl.created_at >= datetime('now', '-1 day')
            where c.route_id = ?
            group by c.id, c.avg_latency_ms
            order by c.id asc
            "#,
        )
        .bind(route_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|row| (row.channel_id, row)).collect())
    }

    pub async fn load_channel_runtime_stats(
        &self,
        channel_id: i64,
    ) -> Result<ChannelRuntimeStats, AppError> {
        let row = sqlx::query_as::<_, ChannelRuntimeStats>(
            r#"
            select
              c.id as channel_id,
              c.avg_latency_ms as avg_latency_ms,
              count(rl.id) as requests_24h,
              coalesce(sum(case
                when rl.http_status is not null
                  and rl.http_status < 400
                  and rl.error_message is null
                then 1 else 0 end), 0) as success_requests_24h,
              coalesce(sum(coalesce(rl.input_tokens, 0)), 0) as input_tokens_24h,
              coalesce(sum(coalesce(rl.output_tokens, 0)), 0) as output_tokens_24h,
              coalesce(sum(coalesce(rl.total_tokens, 0)), 0) as total_tokens_24h
            from channels c
            left join request_logs rl
              on rl.channel_id = c.id
             and rl.created_at >= datetime('now', '-1 day')
            where c.id = ?
            group by c.id, c.avg_latency_ms
            "#,
        )
        .bind(channel_id)
        .fetch_optional(&self.pool)
        .await?;

        row.ok_or_else(|| AppError::NotFound(format!("channel not found: {channel_id}")))
    }

    pub async fn list_route_request_logs(
        &self,
        route_id: i64,
        limit: i64,
    ) -> Result<Vec<RequestLogRow>, AppError> {
        let limit = limit.clamp(1, 100);

        let rows = sqlx::query_as::<_, RequestLogRow>(
            r#"
            select
              rl.id,
              rl.request_id,
              rl.downstream_path,
              rl.upstream_path,
              rl.model_requested,
              rl.channel_id,
              rl.http_status,
              rl.latency_ms,
              rl.error_message,
              rl.input_tokens,
              rl.output_tokens,
              rl.total_tokens,
              rl.created_at,
              c.label as channel_label,
              s.name as site_name,
              c.upstream_model
            from request_logs rl
            join channels c on c.id = rl.channel_id
            join accounts a on a.id = c.account_id
            join sites s on s.id = a.site_id
            where c.route_id = ?
            order by rl.id desc
            limit ?
            "#,
        )
        .bind(route_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    pub async fn mark_channel_success(
        &self,
        channel_id: i64,
        http_status: u16,
        latency_ms: Option<i64>,
    ) -> Result<(), AppError> {
        sqlx::query(
            r#"
            update channels
            set cooldown_until = null,
                manual_blocked = 0,
                consecutive_fail_count = 0,
                last_status = ?,
                last_error = null,
                avg_latency_ms = case
                    when ? is null then avg_latency_ms
                    when avg_latency_ms is null then ?
                    else cast(round(avg_latency_ms * 0.7 + ? * 0.3) as integer)
                end,
                updated_at = current_timestamp
            where id = ?
            "#,
        )
        .bind(i64::from(http_status))
        .bind(latency_ms)
        .bind(latency_ms)
        .bind(latency_ms)
        .bind(channel_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_channel_failure(
        &self,
        channel_id: i64,
        http_status: Option<u16>,
        error_message: &str,
        cooldown_until: Option<i64>,
        manual_blocked: bool,
    ) -> Result<(), AppError> {
        sqlx::query(
            r#"
            update channels
            set cooldown_until = ?,
                manual_blocked = ?,
                consecutive_fail_count = consecutive_fail_count + 1,
                last_status = ?,
                last_error = ?,
                updated_at = current_timestamp
            where id = ?
            "#,
        )
        .bind(cooldown_until)
        .bind(if manual_blocked { 1_i64 } else { 0_i64 })
        .bind(http_status.map(i64::from))
        .bind(error_message)
        .bind(channel_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

const CHANNEL_SELECT_BY_ROUTE_SQL: &str = r#"
    select
      c.id as channel_id,
      c.route_id,
      c.account_id,
      a.label as account_label,
      a.api_key as account_api_key,
      a.status as account_status,
      s.name as site_name,
      s.base_url as site_base_url,
      s.status as site_status,
      c.label as channel_label,
      c.upstream_model,
              c.protocol,
              c.enabled,
              c.priority,
              c.avg_latency_ms,
              c.cooldown_until,
              c.manual_blocked,
              c.consecutive_fail_count,
      c.last_status,
      c.last_error
    from channels c
    join accounts a on a.id = c.account_id
    join sites s on s.id = a.site_id
    where c.route_id = ?
    order by c.priority asc, c.id asc
"#;

const CHANNEL_SELECT_BY_ID_SQL: &str = r#"
    select
      c.id as channel_id,
      c.route_id,
      c.account_id,
      a.label as account_label,
      a.api_key as account_api_key,
      a.status as account_status,
      s.name as site_name,
      s.base_url as site_base_url,
      s.status as site_status,
      c.label as channel_label,
      c.upstream_model,
      c.protocol,
      c.enabled,
      c.priority,
      c.avg_latency_ms,
      c.cooldown_until,
      c.manual_blocked,
      c.consecutive_fail_count,
      c.last_status,
      c.last_error
    from channels c
    join accounts a on a.id = c.account_id
    join sites s on s.id = a.site_id
    where c.id = ?
"#;

pub(crate) fn normalize_base_url(base_url: &str) -> Result<String, AppError> {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(
            "field `base_url` is required".to_string(),
        ));
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return Err(AppError::BadRequest(
            "field `base_url` must start with http:// or https://".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

fn normalize_api_key(api_key: &str) -> Result<&str, AppError> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(
            "field `api_key` is required".to_string(),
        ));
    }
    Ok(trimmed)
}

fn normalize_cooldown_seconds(value: i64) -> Result<i64, AppError> {
    if value < 0 {
        return Err(AppError::BadRequest(
            "field `cooldown_seconds` must be >= 0".to_string(),
        ));
    }
    Ok(value)
}

fn normalize_upstream_model(
    upstream_model: Option<&str>,
    fallback_model: &str,
) -> Result<String, AppError> {
    let value = upstream_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_model)
        .trim();

    if value.is_empty() {
        return Err(AppError::BadRequest(
            "field `upstream_model` is required".to_string(),
        ));
    }

    Ok(value.to_string())
}

fn normalize_priority(priority: i64) -> Result<i64, AppError> {
    if priority < 0 {
        return Err(AppError::BadRequest(
            "field `priority` must be >= 0".to_string(),
        ));
    }
    Ok(priority)
}

fn normalize_protocol(protocol: &str) -> Result<String, AppError> {
    Ok(Protocol::parse(protocol)?.as_str().to_string())
}

fn normalize_route_model(route_model: &str) -> Result<String, AppError> {
    let trimmed = route_model.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(
            "field `route_model` is required".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

fn derive_site_name(base_url: &str) -> String {
    let without_scheme = base_url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = without_scheme
        .split('/')
        .next()
        .unwrap_or("site")
        .replace(':', "-");
    if host.is_empty() {
        "site".to_string()
    } else {
        host
    }
}

async fn next_channel_label(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    route_id: i64,
) -> Result<String, AppError> {
    let count = sqlx::query_scalar::<_, i64>("select count(*) from channels where route_id = ?")
        .bind(route_id)
        .fetch_one(&mut **tx)
        .await?;
    Ok(format!("ch-{}", count + 1))
}

async fn ensure_account(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    normalized_base_url: &str,
    api_key: &str,
) -> Result<i64, AppError> {
    let site_name = derive_site_name(normalized_base_url);
    let account_label = format!("{}-key", site_name);

    let site_id = if let Some(existing_site_id) =
        sqlx::query_scalar::<_, i64>("select id from sites where base_url = ? limit 1")
            .bind(normalized_base_url)
            .fetch_optional(&mut **tx)
            .await?
    {
        existing_site_id
    } else {
        sqlx::query("insert into sites (name, base_url, status) values (?, ?, 'active')")
            .bind(&site_name)
            .bind(normalized_base_url)
            .execute(&mut **tx)
            .await?
            .last_insert_rowid()
    };

    let account_id = if let Some(existing_account_id) = sqlx::query_scalar::<_, i64>(
        "select id from accounts where site_id = ? and api_key = ? limit 1",
    )
    .bind(site_id)
    .bind(api_key)
    .fetch_optional(&mut **tx)
    .await?
    {
        existing_account_id
    } else {
        sqlx::query(
            "insert into accounts (site_id, label, api_key, status) values (?, ?, ?, 'active')",
        )
        .bind(site_id)
        .bind(&account_label)
        .bind(api_key)
        .execute(&mut **tx)
        .await?
        .last_insert_rowid()
    };

    Ok(account_id)
}

async fn cleanup_account_if_unused(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    account_id: i64,
) -> Result<(), AppError> {
    let still_used =
        sqlx::query_scalar::<_, i64>("select count(*) from channels where account_id = ?")
            .bind(account_id)
            .fetch_one(&mut **tx)
            .await?;
    if still_used > 0 {
        return Ok(());
    }

    let site_id = sqlx::query_scalar::<_, i64>("select site_id from accounts where id = ? limit 1")
        .bind(account_id)
        .fetch_optional(&mut **tx)
        .await?;

    sqlx::query("delete from accounts where id = ?")
        .bind(account_id)
        .execute(&mut **tx)
        .await?;

    if let Some(site_id) = site_id {
        let site_still_used = sqlx::query_scalar::<_, i64>(
            r#"
            select count(*)
            from accounts
            where site_id = ?
            "#,
        )
        .bind(site_id)
        .fetch_one(&mut **tx)
        .await?;

        if site_still_used == 0 {
            sqlx::query("delete from sites where id = ?")
                .bind(site_id)
                .execute(&mut **tx)
                .await?;
        }
    }

    Ok(())
}

async fn insert_channel(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    route_id: i64,
    account_id: i64,
    upstream_model: &str,
    protocol: &str,
    enabled: bool,
    priority: i64,
) -> Result<i64, AppError> {
    let channel_label = next_channel_label(tx, route_id).await?;
    let channel_id = sqlx::query(
        r#"
        insert into channels (
          route_id,
          account_id,
          label,
          upstream_model,
          protocol,
          enabled,
          priority
        ) values (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(route_id)
    .bind(account_id)
    .bind(&channel_label)
    .bind(upstream_model)
    .bind(protocol)
    .bind(if enabled { 1_i64 } else { 0_i64 })
    .bind(priority)
    .execute(&mut **tx)
    .await?
    .last_insert_rowid();

    Ok(channel_id)
}
