//! The [`Session`] methods of keeping offline: what the shells call.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use stickpile_api::{ItemSummary, SetSummary, TargetApp, Variant};

use super::run::Run;
use super::{
    derive, orphaned, reasons, AutoKeepConfig, CoverInfo, Device, OfflineControl, OfflineOptions, OfflineOutcome,
    OfflineStatus, Plan, PlanInput, Reason, RunEnd, Scope, Skipped, Totals,
};
use crate::cache::EvictReport;
use crate::client::SearchParams;
use crate::session::{clip_rel, pick_variant};
use crate::{Result, Session};

/// Kept files and the others, on the device.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfflineUsage {
    pub kept_files: u64,
    pub kept_bytes: u64,
    /// Thumbnails and copied files the size cap (`cache_max_mb`) governs.
    pub other_files: u64,
    pub other_bytes: u64,
    /// `offline.max_mb` in bytes; `None`: no limit.
    pub budget: Option<u64>,
}

/// What keeping a scope would take, shown before the user confirms keeping
/// a pack or the library ([`Session::offline_estimate`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Estimate {
    /// The scope's own files: how many, how large, how much of it is on the
    /// device already.
    pub totals: Totals,
    /// Items it would keep (skipped ones left out).
    pub items: u64,
    /// What the options leave out, with the reason.
    pub skipped: Vec<Skipped>,
    /// Bytes every kept file would take with the scope kept too (files
    /// other keeps want already count once).
    pub kept_bytes_after: u64,
    /// `offline.max_mb` in bytes; `None`: no limit.
    pub budget: Option<u64>,
}

impl Estimate {
    /// Whether it fits in the budget.
    pub fn fits(&self) -> bool {
        self.budget.is_none_or(|b| self.kept_bytes_after <= b)
    }
}

/// Liked items listed for `offline.auto.liked`, at most this many.
const LIKED_MAX: usize = 2000;
const LIKED_PAGE: usize = 200;

impl Session {
    /// Whether this client's grids play animations (the Android app):
    /// kept items then also keep the rendition played
    /// ([`super::played_rendition`]).
    pub fn with_grid_playback(mut self, plays: bool) -> Session {
        self.plays_motion = plays;
        self
    }

    /// The offline options in use: `[offline]` of the config, or what
    /// [`Session::set_offline_options`] set.
    pub fn offline_options(&self) -> OfflineOptions {
        self.offline_opts.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Use these offline options (the Android app keeps them in its own
    /// settings; a desktop shell after its settings changed). Takes effect
    /// at the next [`Session::offline_refresh`] or run.
    pub fn set_offline_options(&self, opts: OfflineOptions) {
        *self.offline_opts.lock().unwrap_or_else(|e| e.into_inner()) = opts;
    }

    /// This device for [`super::needs`]: `offline.targets`, or when empty
    /// the default target and the targets of the copy history.
    pub fn offline_device(&self) -> Result<Device> {
        let opts = self.offline_options();
        let targets = if opts.targets.is_empty() {
            let mut t = vec![self.config().default_target];
            for h in self.cache().history_targets()? {
                if !t.contains(&h) {
                    t.push(h);
                }
            }
            t
        } else {
            let mut t = Vec::new();
            for x in opts.targets {
                if !t.contains(&x) {
                    t.push(x);
                }
            }
            t
        };
        Ok(Device {
            targets,
            plays_motion: self.plays_motion,
        })
    }

    /// Keep `scope` on this device (idempotent) and flag what of it is on
    /// the device already; [`Session::offline_run`] fetches the rest.
    pub fn offline_keep(&self, scope: &Scope) -> Result<()> {
        self.offline_check_owner()?;
        self.cache().offline_keep(scope)?;
        if let Some(me) = self.cache().me()? {
            self.cache().meta_set("offline_owner", &me.id)?;
        }
        self.offline_refresh().map(drop)
    }

    /// Stop keeping `scope`. Files no other reason holds go back to ordinary
    /// least-recently-used life: they are not deleted at once.
    pub fn offline_release(&self, scope: &Scope) -> Result<()> {
        self.cache().offline_release(scope)?;
        self.offline_refresh().map(drop)
    }

    /// What the user keeps (automatic rules are options, not listed here),
    /// oldest choice first.
    pub fn offline_keeps(&self) -> Result<Vec<Scope>> {
        Ok(self.cache().offline_keeps()?.into_iter().map(|(s, _)| s).collect())
    }

    /// Why `item` is kept on this device (empty: it is not).
    pub fn offline_reasons(&self, item: &ItemSummary) -> Result<Vec<Reason>> {
        let opts = self.offline_options();
        let device = self.offline_device()?;
        let parts = self.plan_parts(&opts)?;
        let input = parts.input(std::slice::from_ref(item), &opts, &device, &|_, _| false);
        Ok(reasons(&input).remove(&item.id).unwrap_or_default())
    }

    /// The plan as things are: what is kept and what that means in files,
    /// derived from the cache. Changes nothing.
    pub fn offline_plan(&self) -> Result<Plan> {
        let opts = self.offline_options();
        self.plan_with(&opts, None)
    }

    /// [`Session::offline_plan`], and bring the cache in line with it: the
    /// wanted files on the device are flagged as kept (again, after a client
    /// that does not know the flag), files no reason holds any more go back
    /// to the LRU, and the kept files of items a sync removed are deleted.
    /// Runs after every sync ([`Session::sync`]), keep and release.
    pub fn offline_refresh(&self) -> Result<Plan> {
        self.offline_check_owner()?;
        let plan = self.offline_plan()?;
        let wanted: HashSet<String> = plan.files.iter().map(|f| f.file.rel.clone()).collect();
        let index = self.cache().file_index()?;
        let kept = index.iter().filter(|(_, (_, k))| *k).map(|(p, _)| p);
        let items = self.cache().items()?;
        let delete: HashSet<String> = orphaned(kept, &items)
            .into_iter()
            .filter(|p| !wanted.contains(p))
            .collect();
        let report = self.cache().offline_mark(&wanted, &delete)?;
        if report != Default::default() {
            tracing::debug!(
                flagged = report.flagged,
                released = report.released,
                deleted = report.deleted,
                "offline files brought in line with the plan"
            );
        }
        Ok(plan)
    }

    /// What keeping `scope` would take with the current options: its files,
    /// how much of them is on the device, what the options leave out, and
    /// the kept bytes with it.
    pub fn offline_estimate(&self, scope: &Scope) -> Result<Estimate> {
        let opts = self.offline_options();
        let mut alone = opts.clone();
        alone.auto = AutoKeepConfig {
            thumbnails: false,
            used: false,
            liked: false,
            own_packs: false,
            ..opts.auto.clone()
        };
        let own = self.plan_with(&alone, Some(std::slice::from_ref(scope)))?;
        let current = self.plan_with(&opts, None)?;
        let have: HashSet<&str> = current.files.iter().map(|f| f.file.rel.as_str()).collect();
        let extra: u64 = own
            .files
            .iter()
            .filter(|f| !have.contains(f.file.rel.as_str()))
            .map(|f| f.file.size)
            .sum();
        Ok(Estimate {
            totals: own.totals,
            items: own.scopes.first().map_or(0, |s| s.items),
            skipped: own.skipped,
            kept_bytes_after: current.totals.bytes + extra,
            budget: current.budget,
        })
    }

    /// Download what the plan wants and the device lacks, until done,
    /// cancelled, the server is unreachable, the budget is reached or the
    /// disk is full; while paused it waits. Progress goes to `control`
    /// ([`OfflineControl::subscribe`]). The shells run it after each
    /// successful sync (the CLI with `stickpile offline sync`); with the
    /// server unreachable it ends at once with
    /// [`RunEnd::WaitingForNetwork`]. Dropping the future stops it cleanly.
    pub async fn offline_run(&self, control: &OfflineControl) -> Result<OfflineOutcome> {
        control.begin();
        let opts = self.offline_options();
        if let Err(e) = self.offline_fetch_server_state(&opts).await {
            if e.is_offline() {
                let plan = self.offline_refresh()?;
                let status = waiting_status(&plan, e.to_string());
                control.publish(status.clone());
                return Ok(OfflineOutcome {
                    end: RunEnd::WaitingForNetwork,
                    status,
                });
            }
            tracing::warn!("offline: listing liked items or pack covers: {e}");
        }
        let epoch = self.cache().offline_epoch()?;
        let plan = self.offline_refresh()?;
        let run = Run::new(self, control, plan, epoch);
        Ok(run.go().await)
    }

    /// Kept files and the others on the device.
    pub fn offline_usage(&self) -> Result<OfflineUsage> {
        let mut usage = self.cache().offline_usage()?;
        let opts = self.offline_options();
        usage.budget = (opts.max_mb > 0).then(|| opts.max_mb.saturating_mul(super::MIB));
        Ok(usage)
    }

    /// The confirmed "Remove kept files": delete every kept file and forget
    /// what to keep. The automatic rules stay as the options say (turn them
    /// off so that they do not keep files again).
    pub fn offline_remove_all(&self) -> Result<EvictReport> {
        self.cache().offline_reset()
    }

    /// "Clear cache": delete the cached files that are not kept; the report
    /// says how many kept files stay ([`EvictReport::kept_files`]).
    pub fn clear_cache(&self) -> Result<EvictReport> {
        self.cache().clear_files()
    }

    /// For each item, whether a copy to `target` (with the sticker flag
    /// `sticker`) works with the server unreachable: the file
    /// [`Session::prepare_copy`] would use is on the device. For the tile
    /// marks; nothing is downloaded or touched.
    pub fn available_offline(&self, items: &[ItemSummary], target: TargetApp, sticker: bool) -> Vec<bool> {
        items
            .iter()
            .map(|item| match pick_variant(item, target, sticker) {
                Ok((info, _, _)) => self
                    .cache()
                    .file_present(&clip_rel(item, &info), Some(info.size_bytes))
                    .unwrap_or(false),
                Err(_) => false,
            })
            .collect()
    }

    /// Whether a given rendition of `item` is on the device (what
    /// [`Session::fetch_variant`] would give offline). Nothing is touched.
    pub fn variant_on_device(&self, item: &ItemSummary, variant: Variant) -> bool {
        item.variants.iter().find(|v| v.variant == variant).is_some_and(|info| {
            self.cache()
                .file_present(&clip_rel(item, info), Some(info.size_bytes))
                .unwrap_or(false)
        })
    }

    // -- internals --------------------------------------------------------------

    /// The kept data belongs to the account that kept it. When the cached
    /// account is another one, or none while something is kept (a client
    /// that does not know offline data signed out, or changed account), it
    /// is reset: kept files deleted, keeps forgotten.
    fn offline_check_owner(&self) -> Result<()> {
        let cache = self.cache();
        let me = cache.me()?.map(|u| u.id);
        let owner = cache.meta_get("offline_owner")?;
        match (owner.as_deref(), me.as_deref()) {
            (Some(o), Some(m)) if o == m => {}
            (None, None) => {}
            (None, Some(m)) => cache.meta_set("offline_owner", m)?,
            (Some(_), m) => {
                tracing::info!("offline data of another account (or a signed-out one) found; removing it");
                cache.offline_reset()?;
                if let Some(m) = m {
                    cache.meta_set("offline_owner", m)?;
                }
            }
        }
        Ok(())
    }

    /// Ask the server what the plan needs beyond the cache: liked items
    /// (`auto.liked`) and the covers of kept packs. Stored in the cache's
    /// `meta` (`offline_liked`, `offline_covers`) for the plans made
    /// offline.
    async fn offline_fetch_server_state(&self, opts: &OfflineOptions) -> Result<()> {
        if opts.auto.liked {
            let mut liked: Vec<String> = Vec::new();
            while liked.len() < LIKED_MAX {
                let params = SearchParams {
                    q: "is:liked".into(),
                    limit: Some(LIKED_PAGE),
                    offset: Some(liked.len()),
                    target: None,
                };
                let page = self.client().search(&params).await?;
                let n = page.hits.len();
                liked.extend(page.hits.into_iter().map(|h| h.item.id));
                if n < LIKED_PAGE {
                    break;
                }
            }
            let json = serde_json::to_string(&liked).map_err(|e| crate::CoreError::Other(e.to_string()))?;
            self.cache().meta_set("offline_liked", &json)?;
        }
        let keeps_sets = self
            .cache()
            .offline_keeps()?
            .iter()
            .any(|(s, _)| matches!(s, Scope::Set(_)));
        if keeps_sets || opts.auto.own_packs {
            let (mine, all) = tokio::join!(self.client().sets(Some("mine")), self.client().sets(Some("all")));
            let covers: BTreeMap<String, CoverInfo> = mine?
                .into_iter()
                .chain(all?)
                .map(|s: SetSummary| {
                    (
                        s.id,
                        CoverInfo {
                            url: s.cover_url,
                            item: s.cover_item_id,
                        },
                    )
                })
                .collect();
            let json = serde_json::to_string(&covers).map_err(|e| crate::CoreError::Other(e.to_string()))?;
            self.cache().meta_set("offline_covers", &json)?;
        }
        Ok(())
    }

    /// The liked item ids last listed, if any.
    fn offline_liked(&self) -> Result<HashSet<String>> {
        Ok(self
            .cache()
            .meta_get("offline_liked")?
            .and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok())
            .unwrap_or_default()
            .into_iter()
            .collect())
    }

    /// Note a like or an unlike for `auto.liked` (kept in line with the
    /// server without waiting for the next run).
    pub(crate) fn offline_note_like(&self, item_id: &str, liked: bool) -> Result<()> {
        let Some(json) = self.cache().meta_get("offline_liked")? else {
            return Ok(());
        };
        let mut ids: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
        ids.retain(|i| i != item_id);
        if liked {
            ids.push(item_id.to_string());
        }
        let json = serde_json::to_string(&ids).map_err(|e| crate::CoreError::Other(e.to_string()))?;
        self.cache().meta_set("offline_liked", &json)
    }

    fn plan_parts(&self, opts: &OfflineOptions) -> Result<PlanParts> {
        let cache = self.cache();
        let used = if opts.auto.used {
            cache
                .recent(opts.auto.used_count as usize)?
                .into_iter()
                .map(|i| i.id)
                .collect()
        } else {
            Vec::new()
        };
        Ok(PlanParts {
            keeps: cache.offline_keeps()?.into_iter().map(|(s, _)| s).collect(),
            me: cache.me()?.map(|u| u.username),
            used,
            liked: if opts.auto.liked {
                self.offline_liked()?
            } else {
                HashSet::new()
            },
            counts: cache.copy_counts()?,
            covers: cache
                .meta_get("offline_covers")?
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default(),
        })
    }

    /// The plan for `opts`, with `keeps` instead of the stored ones.
    fn plan_with(&self, opts: &OfflineOptions, keeps: Option<&[Scope]>) -> Result<Plan> {
        let device = self.offline_device()?;
        let mut parts = self.plan_parts(opts)?;
        if let Some(k) = keeps {
            parts.keeps = k.to_vec();
        }
        let items = self.cache().items()?;
        let index = self.cache().file_index()?;
        let dir = self.cache().dir().to_path_buf();
        let present = |rel: &str, size: u64| {
            index.contains_key(rel)
                && std::fs::metadata(dir.join(rel))
                    .ok()
                    .filter(|m| m.is_file())
                    .is_some_and(|m| size == 0 || m.len() == size)
        };
        let input = parts.input(&items, opts, &device, &present);
        let mut plan = derive(&input);
        let wanted: HashSet<&str> = plan.files.iter().map(|f| f.file.rel.as_str()).collect();
        plan.failures = self
            .cache()
            .offline_failures()?
            .into_iter()
            .filter(|f| wanted.contains(f.path.as_str()))
            .collect();
        Ok(plan)
    }
}

/// What a plan is derived from besides the items and the files.
struct PlanParts {
    keeps: Vec<Scope>,
    me: Option<String>,
    used: Vec<String>,
    liked: HashSet<String>,
    counts: HashMap<String, u64>,
    covers: BTreeMap<String, CoverInfo>,
}

impl PlanParts {
    fn input<'a>(
        &'a self,
        items: &'a [ItemSummary],
        opts: &'a OfflineOptions,
        device: &'a Device,
        present: &'a dyn Fn(&str, u64) -> bool,
    ) -> PlanInput<'a> {
        PlanInput {
            items,
            keeps: &self.keeps,
            opts,
            device,
            me: self.me.as_deref(),
            used: &self.used,
            liked: &self.liked,
            counts: &self.counts,
            covers: &self.covers,
            present,
        }
    }
}

/// The status of a run that could not start: the server is unreachable.
fn waiting_status(plan: &Plan, message: String) -> OfflineStatus {
    OfflineStatus {
        state: super::OfflineState::WaitingForNetwork,
        files_done: plan.totals.present_files,
        files_total: plan.totals.files,
        bytes_done: plan.totals.present_bytes,
        bytes_total: plan.totals.bytes,
        skipped: plan.skipped.len() as u64,
        failed: plan.failures.len() as u64,
        scopes: plan
            .scopes
            .iter()
            .map(|s| super::ScopeProgress {
                scope: s.scope.clone(),
                name: s.name.clone(),
                files_done: s.totals.present_files,
                files_total: s.totals.files,
                bytes_done: s.totals.present_bytes,
                bytes_total: s.totals.bytes,
            })
            .collect(),
        message: Some(message),
    }
}
