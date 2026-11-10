//! Turning rows into API DTOs in bulk: one joined query for items + media +
//! set + owner, one for variants, one for the viewer's likes.

use std::collections::{HashMap, HashSet};

use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use stickpile_api::profiles::{choose_variant, ItemShape};
use stickpile_api::{Incompatibility, Item, ItemSummary, MediaFormat, MediaInfo, MediaKind, TargetApp, VariantInfo};
use stickpile_media::hash::to_hex;

use crate::db::{i64_to_u64, json_list, parse_enum, source, visibility, VariantRow};
use crate::error::AppResult;

/// Item joined with its media, set and owner.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ItemFull {
    pub id: String,
    pub set_id: String,
    pub position: i64,
    pub filename: String,
    pub name: String,
    pub name_source: String,
    pub keywords: String,
    pub keywords_source: String,
    pub emojis: String,
    pub emojis_source: String,
    pub vibe: Option<String>,
    pub vibe_source: String,
    pub tags: String,
    pub source_url: Option<String>,
    pub extra_toml: Option<String>,
    pub likes: i64,
    pub copies: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub media_id: String,
    pub sha256: String,
    pub original_sha256: String,
    pub kind: String,
    pub format: String,
    pub width: i64,
    pub height: i64,
    pub duration_ms: Option<i64>,
    pub frame_count: Option<i64>,
    pub fps: Option<f64>,
    pub has_alpha: bool,
    pub size_bytes: i64,
    pub phash: i64,
    pub dhash: i64,
    pub quality: Option<i64>,
    pub set_slug: String,
    pub set_name: String,
    pub owner_id: String,
    pub visibility: String,
    pub owner_username: String,
}

pub const ITEM_FULL_SELECT: &str = "SELECT i.id, i.set_id, i.position, i.filename, i.name, i.name_source, i.keywords, \
     i.keywords_source, i.emojis, i.emojis_source, i.vibe, i.vibe_source, i.tags, i.source_url, i.extra_toml, i.likes, \
     i.copies, i.created_at, i.updated_at, i.media_id, m.sha256, m.original_sha256, m.kind, m.format, m.width, m.height, \
     m.duration_ms, m.frame_count, m.fps, m.has_alpha, m.size_bytes, m.phash, m.dhash, m.quality, s.slug AS set_slug, \
     s.name AS set_name, s.owner_id, s.visibility, u.username AS owner_username \
     FROM items i JOIN media m ON m.id = i.media_id JOIN sets s ON s.id = i.set_id JOIN users u ON u.id = s.owner_id";

impl ItemFull {
    pub fn kind(&self) -> MediaKind {
        parse_enum(&self.kind).unwrap_or(MediaKind::Still)
    }
    pub fn format(&self) -> MediaFormat {
        parse_enum(&self.format).unwrap_or(MediaFormat::Png)
    }
    pub fn visibility(&self) -> stickpile_api::Visibility {
        visibility(&self.visibility)
    }
    pub fn keywords(&self) -> Vec<String> {
        json_list(&self.keywords)
    }
    pub fn emojis(&self) -> Vec<String> {
        json_list(&self.emojis)
    }
    pub fn tags(&self) -> Vec<String> {
        json_list(&self.tags)
    }
    pub fn phash_u64(&self) -> u64 {
        i64_to_u64(self.phash)
    }
}

const CHUNK: usize = 500;

/// Items by id (any order; missing ids are skipped).
pub async fn fetch_by_ids(db: &SqlitePool, ids: &[String]) -> AppResult<Vec<ItemFull>> {
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(CHUNK) {
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(ITEM_FULL_SELECT);
        qb.push(" WHERE i.id IN (");
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(id);
        }
        qb.push(")");
        out.extend(qb.build_query_as::<ItemFull>().fetch_all(db).await?);
    }
    Ok(out)
}

/// Items of a set in order.
pub async fn fetch_by_set(db: &SqlitePool, set_id: &str) -> AppResult<Vec<ItemFull>> {
    Ok(sqlx::query_as::<_, ItemFull>(&format!(
        "{ITEM_FULL_SELECT} WHERE i.set_id = ? ORDER BY i.position, i.id"
    ))
    .bind(set_id)
    .fetch_all(db)
    .await?)
}

pub async fn fetch_one(db: &SqlitePool, id: &str) -> AppResult<Option<ItemFull>> {
    Ok(
        sqlx::query_as::<_, ItemFull>(&format!("{ITEM_FULL_SELECT} WHERE i.id = ?"))
            .bind(id)
            .fetch_optional(db)
            .await?,
    )
}

/// Variant rows for media ids, in [`stickpile_api::Variant::ALL`] order.
pub async fn variant_rows(db: &SqlitePool, media_ids: &[String]) -> AppResult<HashMap<String, Vec<VariantRow>>> {
    let mut map: HashMap<String, Vec<VariantRow>> = HashMap::new();
    let unique: Vec<&String> = media_ids.iter().collect::<HashSet<_>>().into_iter().collect();
    for chunk in unique.chunks(CHUNK) {
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
            "SELECT media_id, variant, mime, width, height, size_bytes, sha256, path FROM media_variants WHERE media_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(")");
        for row in qb.build_query_as::<VariantRow>().fetch_all(db).await? {
            map.entry(row.media_id.clone()).or_default().push(row);
        }
    }
    for rows in map.values_mut() {
        rows.sort_by_key(|r| r.variant());
    }
    Ok(map)
}

pub async fn variants(db: &SqlitePool, media_ids: &[String]) -> AppResult<HashMap<String, Vec<VariantInfo>>> {
    Ok(variant_rows(db, media_ids)
        .await?
        .into_iter()
        .map(|(k, rows)| (k, rows.iter().filter_map(VariantRow::to_api).collect()))
        .collect())
}

/// Which of `item_ids` the viewer liked.
pub async fn liked_by(db: &SqlitePool, viewer: &str, item_ids: &[String]) -> AppResult<HashSet<String>> {
    let mut out = HashSet::new();
    for chunk in item_ids.chunks(CHUNK) {
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("SELECT item_id FROM ratings WHERE user_id = ");
        qb.push_bind(viewer);
        qb.push(" AND item_id IN (");
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(id);
        }
        qb.push(")");
        let rows: Vec<(String,)> = qb.build_query_as().fetch_all(db).await?;
        out.extend(rows.into_iter().map(|r| r.0));
    }
    Ok(out)
}

pub fn media_info(f: &ItemFull, variants: Vec<VariantInfo>) -> MediaInfo {
    MediaInfo {
        id: f.media_id.clone(),
        sha256: f.sha256.clone(),
        kind: f.kind(),
        format: f.format(),
        width: f.width as u32,
        height: f.height as u32,
        duration_ms: f.duration_ms.map(|v| v as u64),
        frame_count: f.frame_count.map(|v| v as u32),
        fps: f.fps.map(|v| v as f32),
        has_alpha: f.has_alpha,
        size_bytes: f.size_bytes as u64,
        phash: to_hex(i64_to_u64(f.phash)),
        dhash: to_hex(i64_to_u64(f.dhash)),
        quality: f.quality.map(|q| q.clamp(0, 100) as u8),
        variants,
    }
}

pub fn to_item(f: &ItemFull, variants: Vec<VariantInfo>, liked_by_me: bool) -> Item {
    Item {
        id: f.id.clone(),
        set_id: f.set_id.clone(),
        position: f.position as u32,
        filename: f.filename.clone(),
        name: f.name.clone(),
        name_source: source(&f.name_source),
        keywords: f.keywords(),
        keywords_source: source(&f.keywords_source),
        emojis: f.emojis(),
        emojis_source: source(&f.emojis_source),
        vibe: f.vibe.clone(),
        vibe_source: source(&f.vibe_source),
        tags: f.tags(),
        media: media_info(f, variants),
        likes: f.likes.max(0) as u64,
        liked_by_me,
        copies: f.copies.max(0) as u64,
        created_at: f.created_at,
        updated_at: f.updated_at,
    }
}

pub fn to_summary(f: &ItemFull, variants: Vec<VariantInfo>) -> ItemSummary {
    ItemSummary {
        id: f.id.clone(),
        set_id: f.set_id.clone(),
        set_slug: f.set_slug.clone(),
        set_name: f.set_name.clone(),
        owner_username: f.owner_username.clone(),
        name: f.name.clone(),
        keywords: f.keywords(),
        emojis: f.emojis(),
        vibe: f.vibe.clone(),
        tags: f.tags(),
        kind: f.kind(),
        format: f.format(),
        width: f.width as u32,
        height: f.height as u32,
        quality: f.quality.map(|q| q.clamp(0, 100) as u8),
        likes: f.likes.max(0) as u64,
        copies: f.copies.max(0) as u64,
        media_id: f.media_id.clone(),
        sha256: f.sha256.clone(),
        phash: to_hex(i64_to_u64(f.phash)),
        variants,
        updated_at: f.updated_at,
    }
}

/// Full items for `rows` (keeps the order).
pub async fn items(db: &SqlitePool, viewer: &str, rows: &[ItemFull]) -> AppResult<Vec<Item>> {
    let media_ids: Vec<String> = rows.iter().map(|r| r.media_id.clone()).collect();
    let item_ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let vars = variants(db, &media_ids).await?;
    let liked = liked_by(db, viewer, &item_ids).await?;
    Ok(rows
        .iter()
        .map(|r| {
            to_item(
                r,
                vars.get(&r.media_id).cloned().unwrap_or_default(),
                liked.contains(&r.id),
            )
        })
        .collect())
}

/// Summaries for `rows` (keeps the order).
pub async fn summaries(db: &SqlitePool, rows: &[ItemFull]) -> AppResult<Vec<ItemSummary>> {
    let media_ids: Vec<String> = rows.iter().map(|r| r.media_id.clone()).collect();
    let vars = variants(db, &media_ids).await?;
    Ok(rows
        .iter()
        .map(|r| to_summary(r, vars.get(&r.media_id).cloned().unwrap_or_default()))
        .collect())
}

/// Summaries by id, as a map.
pub async fn summaries_by_id(db: &SqlitePool, ids: &[String]) -> AppResult<HashMap<String, ItemSummary>> {
    let rows = fetch_by_ids(db, ids).await?;
    Ok(summaries(db, &rows)
        .await?
        .into_iter()
        .map(|s| (s.id.clone(), s))
        .collect())
}

pub fn shape<'a>(f: &ItemFull, variants: &'a [VariantInfo]) -> ItemShape<'a> {
    ItemShape {
        kind: f.kind(),
        format: f.format(),
        duration_ms: f.duration_ms.map(|v| v as u64),
        variants,
    }
}

/// Why `f` cannot be sent to `target` with its current variants, if it cannot.
pub fn incompatibility(f: &ItemFull, variants: &[VariantInfo], target: TargetApp) -> Option<String> {
    choose_variant(target, &shape(f, variants), false).err().map(|e| e.0)
}

/// Every target app that cannot accept the item, with reasons.
pub fn incompatibilities(f: &ItemFull, variants: &[VariantInfo]) -> Vec<Incompatibility> {
    TargetApp::ALL
        .into_iter()
        .filter_map(|t| incompatibility(f, variants, t).map(|reason| Incompatibility { target: t, reason }))
        .collect()
}
