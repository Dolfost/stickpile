//! Sets larger than one pack (docs/telegram.md, "Sets larger than one
//! pack").
//!
//! A Telegram pack holds at most [`MAX_PACK_STICKERS`] stickers, so a larger
//! set is published as several packs, its parts. [`plan_parts`] decides,
//! before any call, which item goes to which part; [`export_parts`] runs the
//! plan part by part with the single-pack export, then deletes the parts
//! left empty. [`part_pack_name`] and [`part_title`] name the parts.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;

use serde::Serialize;

use crate::api::{BotApi, TelegramError};
use crate::sync::{
    clamp_title, sync_pack, valid_pack_name, ExportItem, Pack, PackState, PackThumbnail, MAX_PACK_STICKERS,
    MAX_TITLE_CHARS,
};

/// What the previous export left on each part, by part number (from 1): the
/// states stored from [`PartUpdate`]s. A part known to exist without a
/// mapping (its first export failed before any sticker was confirmed) is
/// listed with an empty state, so that it is filled or deleted, never
/// forgotten on Telegram.
pub type PartStates = BTreeMap<u32, PackState>;

/// Which item goes to which part, decided before any call.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PartsPlan {
    /// The parts that hold stickers, by number.
    pub parts: Vec<PlannedPart>,
    /// Parts of the previous export left without a sticker: their packs are
    /// deleted after the other parts are updated.
    pub delete: Vec<u32>,
    /// Items left out because another item of the set has the same media
    /// (`content_key`), as (item key, key of the item exported instead), in
    /// set order.
    pub duplicates: Vec<(String, String)>,
    /// Published stickers that change part. Only a repack moves any.
    pub moved: usize,
}

/// One part of a plan.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedPart {
    pub number: u32,
    /// Its items, as indices into the planned items, in set order.
    pub items: Vec<usize>,
}

impl PartsPlan {
    /// The numbers of every part the export touches: the planned ones, then
    /// the ones to delete.
    pub fn numbers(&self) -> impl Iterator<Item = u32> + '_ {
        self.parts.iter().map(|p| p.number).chain(self.delete.iter().copied())
    }
}

fn invalid(message: impl Into<String>) -> TelegramError {
    TelegramError::Invalid(message.into())
}

/// Split `items` (the set's exportable items, in set order) into parts of at
/// most `capacity` stickers.
///
/// * Items with the same `content_key` are exported once, for the whole set:
///   the one already published with that media, else one already published,
///   else the first in set order. The others are listed in `duplicates`.
/// * An item already published (its key is in the mapping of a part of
///   `previous`; the lowest part if in several) stays in that part.
/// * New items fill the first part with room, in set order: the parts of
///   `previous` by number, then new parts, numbered from the lowest number
///   not in use.
/// * The order inside a part is the set's order. A removed item frees its
///   place; a part of `previous` left without items is listed in `delete`.
/// * `repack`: every item is assigned again in set order (part 1 gets the
///   first `capacity` items, and so on), and `moved` counts the published
///   stickers that change part.
///
/// Fails, before any call, without items, with duplicate item keys, with a
/// `capacity` of 0, with a part numbered 0, or when a part of `previous`
/// already holds more than `capacity` of the items (only a repack can fix
/// that).
pub fn plan_parts(
    items: &[ExportItem],
    previous: &PartStates,
    capacity: usize,
    repack: bool,
) -> Result<PartsPlan, TelegramError> {
    plan_parts_skipping(items, previous, &BTreeSet::new(), capacity, repack)
}

/// [`plan_parts`] that never gives a new part one of the numbers in
/// `retired`, nor uses one in a repack: the numbers of parts whose packs were
/// deleted, as Telegram does not say whether a deleted pack's name can be
/// created again. A number in `previous` is a part that exists and is
/// planned as such, retired or not.
pub fn plan_parts_skipping(
    items: &[ExportItem],
    previous: &PartStates,
    retired: &BTreeSet<u32>,
    capacity: usize,
    repack: bool,
) -> Result<PartsPlan, TelegramError> {
    // The numbers a part may have, in order: existing parts and numbers
    // never retired.
    let usable = |n: &u32| previous.contains_key(n) || !retired.contains(n);
    if capacity == 0 {
        return Err(invalid("a part must hold at least one sticker"));
    }
    if previous.contains_key(&0) {
        return Err(invalid("parts are numbered from 1"));
    }
    let mut keys = HashSet::new();
    if let Some(it) = items.iter().find(|it| !keys.insert(it.key.as_str())) {
        return Err(invalid(format!("duplicate item key {:?}", it.key)));
    }
    if items.is_empty() {
        return Err(invalid("nothing to export: the set has no items"));
    }

    // Where each item is published: the lowest part whose mapping has it,
    // with the media it was published with.
    let mut published: HashMap<&str, (u32, &str)> = HashMap::new();
    for (&number, state) in previous {
        for (key, (content, _)) in &state.mapping {
            published.entry(key.as_str()).or_insert((number, content.as_str()));
        }
    }

    // One item per media.
    let rank = |it: &ExportItem| match published.get(it.key.as_str()) {
        Some((_, content)) if *content == it.content_key => 0,
        Some(_) => 1,
        None => 2,
    };
    let mut chosen: HashMap<&str, usize> = HashMap::new();
    for (i, it) in items.iter().enumerate() {
        match chosen.get(it.content_key.as_str()) {
            Some(&j) if rank(&items[j]) <= rank(it) => {}
            _ => {
                chosen.insert(it.content_key.as_str(), i);
            }
        }
    }
    let winner = |it: &ExportItem| chosen[it.content_key.as_str()];
    let exported: Vec<usize> = (0..items.len()).filter(|&i| winner(&items[i]) == i).collect();
    let duplicates = items
        .iter()
        .enumerate()
        .filter(|&(i, it)| winner(it) != i)
        .map(|(_, it)| (it.key.clone(), items[winner(it)].key.clone()))
        .collect();

    let mut parts: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    let mut moved = 0;
    if repack {
        let mut numbers = (1..=u32::MAX).filter(usable);
        let mut number = 0;
        for (n, &i) in exported.iter().enumerate() {
            if n % capacity == 0 {
                number = numbers.next().ok_or_else(|| invalid("too many parts"))?;
            }
            if published
                .get(items[i].key.as_str())
                .is_some_and(|(was, _)| *was != number)
            {
                moved += 1;
            }
            parts.entry(number).or_default().push(i);
        }
    } else {
        let mut new = Vec::new();
        for &i in &exported {
            match published.get(items[i].key.as_str()) {
                Some(&(number, _)) => parts.entry(number).or_default().push(i),
                None => new.push(i),
            }
        }
        if let Some((number, held)) = parts.iter().find(|(_, held)| held.len() > capacity) {
            return Err(invalid(format!(
                "part {number} holds {} stickers, more than {capacity}: repack the parts",
                held.len()
            )));
        }
        let mut candidates = previous
            .keys()
            .copied()
            .chain((1..=u32::MAX).filter(|n| !previous.contains_key(n) && usable(n)));
        let mut current = candidates.next().ok_or_else(|| invalid("too many parts"))?;
        for i in new {
            while parts.get(&current).map_or(0, Vec::len) >= capacity {
                current = candidates.next().ok_or_else(|| invalid("too many parts"))?;
            }
            parts.entry(current).or_default().push(i);
        }
        for held in parts.values_mut() {
            held.sort_unstable();
        }
    }
    Ok(PartsPlan {
        delete: previous.keys().copied().filter(|n| !parts.contains_key(n)).collect(),
        parts: parts
            .into_iter()
            .map(|(number, items)| PlannedPart { number, items })
            .collect(),
        duplicates,
        moved,
    })
}

/// The pack name of part `part` of a set whose part 1 is `part1_name`
/// (`<name>_by_<bot>`: the name a single pack has, which may be one the user
/// chose). Part 1 keeps it; part k is `<name>_<k>_by_<bot>`, with the
/// suffix written as in `part1_name`. `<name>` is shortened when the whole
/// would pass 64 characters, without leaving a `_` at its end (so no `__`),
/// and further in the one case where that would give part 1's name. The
/// result is always a valid pack name ([`valid_pack_name`]).
///
/// Fails when `part1_name` is not a valid pack name for `bot_username`, for
/// part 0, and when the bot username leaves no room for `<name>`.
pub fn part_pack_name(part1_name: &str, part: u32, bot_username: &str) -> Result<String, TelegramError> {
    let bot = bot_username.trim_start_matches('@');
    if !valid_pack_name(part1_name, bot) {
        return Err(invalid(format!("{part1_name:?} is not a valid pack name for @{bot}")));
    }
    match part {
        0 => return Err(invalid("parts are numbered from 1")),
        1 => return Ok(part1_name.to_string()),
        _ => {}
    }
    // Valid names are ASCII, so byte offsets are character offsets.
    let (base, suffix) = part1_name.split_at(part1_name.len() - "_by_".len() - bot.len());
    let tag = format!("_{part}");
    let room = 64usize.saturating_sub(suffix.len() + tag.len());
    let mut base = &base[..base.len().min(room)];
    loop {
        base = base.trim_end_matches('_');
        if base.is_empty() {
            return Err(invalid(format!("no room for the name of part {part} of {part1_name}")));
        }
        let name = format!("{base}{tag}{suffix}");
        if !name.eq_ignore_ascii_case(part1_name) && valid_pack_name(&name, bot) {
            return Ok(name);
        }
        base = &base[..base.len() - 1];
    }
}

/// The title of part `part` of a set named `set_name`: part 1 has the set's
/// name, part k `<set name> <k>`, both at most 64 characters. A long name is
/// cut on characters, before ` <k>` so the number stays. An empty name gives
/// an empty title, for which the export uses the pack name.
pub fn part_title(set_name: &str, part: u32) -> String {
    let name = set_name.trim();
    if part <= 1 {
        return name.chars().take(MAX_TITLE_CHARS).collect();
    }
    let tag = format!(" {part}");
    let cut: String = name.chars().take(MAX_TITLE_CHARS - tag.len()).collect();
    let cut = cut.trim_end();
    if cut.is_empty() {
        return String::new();
    }
    format!("{cut}{tag}")
}

/// `https://t.me/addstickers/<name>`.
fn pack_url(name: &str) -> String {
    format!("https://t.me/addstickers/{name}")
}

/// One export of a set as parts: what [`export_parts`] needs besides the
/// plan.
#[derive(Debug, Clone, Copy)]
pub struct PartsExport<'a> {
    /// The Telegram user who owns the packs.
    pub owner_user_id: i64,
    pub bot_username: &'a str,
    /// Part 1's pack name, as a single pack's: the other names derive from it
    /// ([`part_pack_name`]).
    pub part1_name: &'a str,
    /// The set's name (or the title asked for): part 1's title, and the base
    /// of the others' ([`part_title`]).
    pub title: &'a str,
    /// The items the plan was made from.
    pub items: &'a [ExportItem],
    /// The states the plan was made from.
    pub previous: &'a PartStates,
    /// The set's cover, every part's thumbnail.
    pub thumbnail: Option<&'a PackThumbnail>,
}

/// What to store about a part, handed over as soon as the export is done
/// with it.
#[derive(Debug, Clone, PartialEq)]
pub struct PartUpdate {
    pub number: u32,
    pub name: String,
    /// The part's state, to store in place of the previous one and pass back
    /// in [`PartStates`]; after a failure, its state as far as the export got.
    /// `None`: the part's pack was deleted (or was gone already), forget the
    /// part.
    pub state: Option<PackState>,
}

/// What an export did to one part.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PartReport {
    pub number: u32,
    pub name: String,
    /// `https://t.me/addstickers/<name>`.
    pub url: String,
    pub title: String,
    /// Stickers planned for the part.
    pub stickers: usize,
    pub created_pack: bool,
    pub added: u32,
    pub removed: u32,
    pub updated: u32,
    pub reordered: bool,
    /// The pack has the set's cover as its thumbnail.
    pub thumbnail: bool,
    pub thumbnail_changed: bool,
    /// The part had no sticker left: its pack was deleted (or was gone
    /// already).
    pub deleted: bool,
    /// Why the export stopped at this part.
    pub error: Option<String>,
}

/// What an export of parts did.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PartsReport {
    /// In the order the export handled them: the planned parts by number,
    /// then the deleted ones. After a failure the part that failed is last,
    /// with its `error`, and the parts after it are missing.
    pub parts: Vec<PartReport>,
    /// Published stickers moved to another part (by a repack).
    pub moved: usize,
}

/// The sums over the parts of a [`PartsReport`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PartsTotals {
    pub stickers: usize,
    pub created_packs: u32,
    pub deleted_packs: u32,
    pub added: u32,
    pub removed: u32,
    pub updated: u32,
    pub reordered: bool,
    pub thumbnail_changed: bool,
}

impl PartsReport {
    /// The report of part `number`; part 1's fields are those of a single
    /// pack's export.
    pub fn part(&self, number: u32) -> Option<&PartReport> {
        self.parts.iter().find(|p| p.number == number)
    }

    pub fn totals(&self) -> PartsTotals {
        let mut t = PartsTotals::default();
        for p in &self.parts {
            t.stickers += p.stickers;
            t.created_packs += u32::from(p.created_pack);
            t.deleted_packs += u32::from(p.deleted);
            t.added += p.added;
            t.removed += p.removed;
            t.updated += p.updated;
            t.reordered |= p.reordered;
            t.thumbnail_changed |= p.thumbnail_changed;
        }
        t
    }
}

/// Why an export of parts stopped.
#[derive(Debug)]
pub enum PartsFailure<E> {
    /// A Bot API call failed, or the job was invalid (before any call).
    Telegram(TelegramError),
    /// Storing a part's state failed.
    Persist(E),
}

/// An export of parts that stopped: why, and what it did until then.
#[derive(Debug)]
pub struct PartsError<E> {
    pub failure: PartsFailure<E>,
    pub report: PartsReport,
}

impl<E: std::fmt::Display> std::fmt::Display for PartsError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(p) = self.report.parts.last().filter(|p| p.error.is_some()) {
            write!(f, "part {} ({}): ", p.number, p.name)?;
        }
        match &self.failure {
            PartsFailure::Telegram(e) => write!(f, "{e}"),
            PartsFailure::Persist(e) => write!(f, "storing the part's state: {e}"),
        }
    }
}

impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for PartsError<E> {}

/// Export a set as the parts of `plan` (from [`plan_parts`] with
/// `job.items` and `job.previous`).
///
/// Every name is made and checked first ([`part_pack_name`]): an invalid one
/// fails before any call. Then each planned part, by number, is exported
/// like a single pack ([`crate::export_pack`]) with its name, its title
/// ([`part_title`]), its previous state and the cover as thumbnail, and
/// `persist` gets its new state as soon as it is done. Only after every part
/// is done are the parts in `plan.delete` deleted (`deleteStickerSet`; a
/// pack that is gone already counts as deleted), each handed to `persist`
/// with no state.
///
/// The export stops at the first failure: the failed part's state as far as
/// it got goes to `persist`, nothing is deleted, and the error carries the
/// report so far. A `persist` failure stops it too: its error wins, as the
/// stored state is what the next export starts from.
pub async fn export_parts<F, Fut, E>(
    api: &BotApi,
    job: &PartsExport<'_>,
    plan: &PartsPlan,
    mut persist: F,
) -> Result<PartsReport, PartsError<E>>
where
    F: FnMut(PartUpdate) -> Fut,
    Fut: Future<Output = Result<(), E>>,
{
    let mut report = PartsReport {
        parts: Vec::new(),
        moved: plan.moved,
    };
    let stop = |failure, report| Err(PartsError { failure, report });
    let mut names = BTreeMap::new();
    for number in plan.numbers() {
        match part_pack_name(job.part1_name, number, job.bot_username) {
            Ok(name) => names.insert(number, name),
            Err(e) => return stop(PartsFailure::Telegram(e), report),
        };
    }
    let planned: Vec<usize> = plan.parts.iter().flat_map(|p| p.items.iter().copied()).collect();
    let distinct: HashSet<usize> = planned.iter().copied().collect();
    if distinct.len() != planned.len() || planned.iter().any(|&i| i >= job.items.len()) {
        let e = invalid("the plan was not made from these items");
        return stop(PartsFailure::Telegram(e), report);
    }
    if let Some(p) = plan.parts.iter().find(|p| p.items.len() > MAX_PACK_STICKERS) {
        let e = invalid(format!(
            "part {} has {} stickers; a Telegram pack holds at most {MAX_PACK_STICKERS}",
            p.number,
            p.items.len()
        ));
        return stop(PartsFailure::Telegram(e), report);
    }

    for part in &plan.parts {
        let name = &names[&part.number];
        let title = part_title(job.title, part.number);
        let items: Vec<&ExportItem> = part.items.iter().map(|&i| &job.items[i]).collect();
        let previous = job.previous.get(&part.number).cloned().unwrap_or_default();
        let pack = Pack {
            api,
            owner_user_id: job.owner_user_id,
            name,
            thumbnail: job.thumbnail,
        };
        let mut line = PartReport {
            number: part.number,
            name: name.clone(),
            url: pack_url(name),
            title: clamp_title(&title, name),
            stickers: items.len(),
            ..PartReport::default()
        };
        let (state, failed) = match sync_pack(&pack, &title, &items, &previous).await {
            Ok(r) => {
                line.created_pack = r.created_pack;
                line.added = r.added;
                line.removed = r.removed;
                line.updated = r.updated;
                line.reordered = r.reordered;
                line.thumbnail = r.thumbnail.is_some();
                line.thumbnail_changed = r.thumbnail_changed;
                (r.state(), None)
            }
            Err(e) => {
                line.error = Some(e.error.to_string());
                (e.partial, Some(e.error))
            }
        };
        report.parts.push(line);
        let update = PartUpdate {
            number: part.number,
            name: name.clone(),
            state: Some(state),
        };
        if let Err(e) = persist(update).await {
            return stop(PartsFailure::Persist(e), report);
        }
        if let Some(e) = failed {
            return stop(PartsFailure::Telegram(e), report);
        }
    }

    for &number in &plan.delete {
        let name = &names[&number];
        let mut line = PartReport {
            number,
            name: name.clone(),
            url: pack_url(name),
            ..PartReport::default()
        };
        match api.delete_sticker_set(name).await {
            Ok(()) => {}
            Err(e) if e.is_missing_sticker_set() => {}
            Err(e) => {
                line.error = Some(e.to_string());
                report.parts.push(line);
                return stop(PartsFailure::Telegram(e), report);
            }
        }
        line.deleted = true;
        report.parts.push(line);
        let update = PartUpdate {
            number,
            name: name.clone(),
            state: None,
        };
        if let Err(e) = persist(update).await {
            return stop(PartsFailure::Persist(e), report);
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::default_pack_name;
    use crate::StickerFormat;

    const CAP: usize = 120;

    fn item_with(key: &str, content: &str) -> ExportItem {
        ExportItem {
            key: key.into(),
            content_key: content.into(),
            bytes: vec![],
            format: StickerFormat::Static,
            emojis: vec![],
            keywords: vec![],
        }
    }

    fn named(prefix: &str, range: std::ops::Range<usize>) -> Vec<String> {
        range.map(|i| format!("{prefix}{i:03}")).collect()
    }

    fn items(keys: &[String]) -> Vec<ExportItem> {
        keys.iter().map(|k| item_with(k, &format!("c-{k}"))).collect()
    }

    /// A part as an export leaves it: every key published with its media.
    fn state(keys: &[String]) -> PackState {
        PackState {
            mapping: keys
                .iter()
                .map(|k| (k.clone(), (format!("c-{k}"), format!("f-{k}"))))
                .collect(),
            thumbnail: None,
        }
    }

    fn layout(plan: &PartsPlan, items: &[ExportItem]) -> Vec<(u32, Vec<String>)> {
        plan.parts
            .iter()
            .map(|p| (p.number, p.items.iter().map(|&i| items[i].key.clone()).collect()))
            .collect()
    }

    fn sizes(plan: &PartsPlan) -> Vec<(u32, usize)> {
        plan.parts.iter().map(|p| (p.number, p.items.len())).collect()
    }

    fn cat(parts: &[&[String]]) -> Vec<String> {
        parts.iter().flat_map(|p| p.iter().cloned()).collect()
    }

    fn one(key: &str) -> Vec<String> {
        vec![key.to_string()]
    }

    #[test]
    fn plan_no_items() {
        let err = plan_parts(&[], &PartStates::new(), CAP, false).unwrap_err();
        assert!(err.to_string().contains("nothing to export"), "{err}");
        // Never "delete every part": an empty set is an error, not a plan.
        let previous = PartStates::from([(1, state(&named("a", 0..3)))]);
        for repack in [false, true] {
            assert!(plan_parts(&[], &previous, CAP, repack).is_err());
        }
        // Other invalid input, also before any call.
        let single = items(&named("a", 0..1));
        assert!(plan_parts(&single, &PartStates::new(), 0, false).is_err());
        assert!(plan_parts(&single, &PartStates::from([(0, PackState::default())]), CAP, false).is_err());
        let twice = vec![item_with("a", "x"), item_with("a", "y")];
        assert!(plan_parts(&twice, &PartStates::new(), CAP, false).is_err());
    }

    #[test]
    fn plan_one_item() {
        let its = items(&named("a", 0..1));
        let plan = plan_parts(&its, &PartStates::new(), CAP, false).unwrap();
        assert_eq!(layout(&plan, &its), [(1, named("a", 0..1))]);
        assert!(plan.delete.is_empty() && plan.duplicates.is_empty());
        assert_eq!(plan.moved, 0);
        // Published: the same plan.
        let previous = PartStates::from([(1, state(&named("a", 0..1)))]);
        assert_eq!(plan_parts(&its, &previous, CAP, false).unwrap(), plan);
        assert_eq!(plan_parts(&its, &previous, CAP, true).unwrap(), plan);
    }

    #[test]
    fn plan_120_items_one_part() {
        let its = items(&named("a", 0..120));
        for repack in [false, true] {
            let plan = plan_parts(&its, &PartStates::new(), CAP, repack).unwrap();
            assert_eq!(layout(&plan, &its), [(1, named("a", 0..120))]);
        }
    }

    #[test]
    fn plan_121_items_two_parts() {
        let its = items(&named("a", 0..121));
        for repack in [false, true] {
            let plan = plan_parts(&its, &PartStates::new(), CAP, repack).unwrap();
            assert_eq!(
                layout(&plan, &its),
                [(1, named("a", 0..120)), (2, named("a", 120..121))]
            );
        }
    }

    #[test]
    fn plan_240_items() {
        let its = items(&named("a", 0..240));
        for repack in [false, true] {
            let plan = plan_parts(&its, &PartStates::new(), CAP, repack).unwrap();
            assert_eq!(
                layout(&plan, &its),
                [(1, named("a", 0..120)), (2, named("a", 120..240))]
            );
        }
    }

    #[test]
    fn plan_241_items() {
        let its = items(&named("a", 0..241));
        for repack in [false, true] {
            let plan = plan_parts(&its, &PartStates::new(), CAP, repack).unwrap();
            assert_eq!(sizes(&plan), [(1, 120), (2, 120), (3, 1)]);
            assert_eq!(layout(&plan, &its)[2], (3, named("a", 240..241)));
        }
        // Published as such, one more at the end goes to part 3.
        let previous = PartStates::from([
            (1, state(&named("a", 0..120))),
            (2, state(&named("a", 120..240))),
            (3, state(&named("a", 240..241))),
        ]);
        let its = items(&named("a", 0..242));
        let plan = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(sizes(&plan), [(1, 120), (2, 120), (3, 2)]);
    }

    #[test]
    fn plan_additions_at_start_middle_end_fill_first_part_with_room() {
        let a = named("a", 0..120);
        let b = named("b", 0..50);
        let previous = PartStates::from([(1, state(&a)), (2, state(&b))]);
        // Two of part 1 removed, so it has room for two.
        let kept_a: Vec<String> = a.iter().filter(|k| *k != "a010" && *k != "a020").cloned().collect();
        let order = cat(&[
            &one("start"),
            &kept_a,
            &one("mid"),
            &b[..25],
            &one("mid2"),
            &b[25..],
            &one("end1"),
            &one("end2"),
        ]);
        let its = items(&order);
        let plan = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(
            layout(&plan, &its),
            [
                (1, cat(&[&one("start"), &kept_a, &one("mid")])),
                (2, cat(&[&b[..25], &one("mid2"), &b[25..], &one("end1"), &one("end2")])),
            ],
            "the first part with room, in set order; order inside a part is the set's"
        );
        assert!(plan.delete.is_empty());
        assert_eq!(plan.moved, 0);

        // With every part full, a new part is started.
        let b = named("b", 0..120);
        let full = PartStates::from([(1, state(&a)), (2, state(&b))]);
        let its = items(&cat(&[&one("first"), &a, &b]));
        let plan = plan_parts(&its, &full, CAP, false).unwrap();
        assert_eq!(sizes(&plan), [(1, 120), (2, 120), (3, 1)]);
        assert_eq!(layout(&plan, &its)[2], (3, one("first")));
    }

    #[test]
    fn plan_deletion_that_empties_a_part_marks_it_for_deletion() {
        let (a, b, c) = (named("a", 0..120), named("b", 0..5), named("c", 0..10));
        let previous = PartStates::from([(1, state(&a)), (2, state(&b)), (3, state(&c))]);
        // Every item of part 2 removed.
        let its = items(&cat(&[&a, &c]));
        let plan = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(layout(&plan, &its), [(1, a.clone()), (3, c.clone())]);
        assert_eq!(plan.delete, [2]);

        // A new item in the same export fills the emptied part instead.
        let its = items(&cat(&[&a, &c, &one("n")]));
        let plan = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(layout(&plan, &its), [(1, a.clone()), (2, one("n")), (3, c.clone())]);
        assert!(plan.delete.is_empty());

        // Even part 1: the other stickers stay where they are published.
        let previous = PartStates::from([(1, state(&a)), (2, state(&b))]);
        let its = items(&b);
        let plan = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(layout(&plan, &its), [(2, b.clone())]);
        assert_eq!(plan.delete, [1]);
    }

    #[test]
    fn plan_reorder_across_a_boundary_moves_nothing() {
        let (a, b) = (named("a", 0..120), named("b", 0..30));
        let previous = PartStates::from([(1, state(&a)), (2, state(&b))]);
        // b005 moved to the start of the set, a003 to the end.
        let a_rest: Vec<String> = a.iter().filter(|k| *k != "a003").cloned().collect();
        let b_rest: Vec<String> = b.iter().filter(|k| *k != "b005").cloned().collect();
        let its = items(&cat(&[&one("b005"), &a_rest, &b_rest, &one("a003")]));
        let plan = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(
            layout(&plan, &its),
            [(1, cat(&[&a_rest, &one("a003")])), (2, cat(&[&one("b005"), &b_rest]))]
        );
        assert_eq!(plan.moved, 0);
        assert!(plan.delete.is_empty());
    }

    #[test]
    fn plan_repack_assigns_in_set_order_and_counts_moves() {
        let (a, b) = (named("a", 0..120), named("b", 0..30));
        let previous = PartStates::from([(1, state(&a)), (2, state(&b))]);
        let a_rest: Vec<String> = a.iter().filter(|k| *k != "a003").cloned().collect();
        let b_rest: Vec<String> = b.iter().filter(|k| *k != "b005").cloned().collect();
        let its = items(&cat(&[&one("b005"), &a_rest, &b_rest, &one("a003")]));
        let plan = plan_parts(&its, &previous, CAP, true).unwrap();
        assert_eq!(
            layout(&plan, &its),
            [(1, cat(&[&one("b005"), &a_rest])), (2, cat(&[&b_rest, &one("a003")]))]
        );
        assert_eq!(plan.moved, 2, "b005 to part 1, a003 to part 2");

        // Parts that shrank are packed again; the last one goes.
        let (a, b, c) = (named("a", 0..60), named("b", 0..60), named("c", 0..10));
        let previous = PartStates::from([(1, state(&a)), (2, state(&b)), (3, state(&c))]);
        let its = items(&cat(&[&a, &b, &c]));
        let stable = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(sizes(&stable), [(1, 60), (2, 60), (3, 10)]);
        assert_eq!(stable.moved, 0);
        let plan = plan_parts(&its, &previous, CAP, true).unwrap();
        assert_eq!(layout(&plan, &its), [(1, cat(&[&a, &b])), (2, c.clone())]);
        assert_eq!(plan.delete, [3]);
        assert_eq!(plan.moved, 70, "all of part 2 and part 3");
        // A new item is not a move. Here it takes part 1's first place, so
        // b000..b058 move to part 1, b059 stays in part 2, and part 3's ten
        // move to part 2.
        let its = items(&cat(&[&one("new"), &a, &b, &c]));
        let plan = plan_parts(&its, &previous, CAP, true).unwrap();
        assert_eq!(sizes(&plan), [(1, 120), (2, 11)]);
        assert_eq!(plan.moved, 59 + 10);
    }

    #[test]
    fn plan_duplicates_and_skips_decided_once() {
        let (a, b) = (named("a", 0..120), named("b", 0..10));
        let previous = PartStates::from([(1, state(&a)), (2, state(&b))]);
        // A new twin of b003 before it in the set, b005 skipped (no
        // rendition: not among the items), and two new identical items.
        let b_rest: Vec<String> = b.iter().filter(|k| *k != "b005").cloned().collect();
        let mut its = vec![item_with("twin", "c-b003")];
        its.extend(items(&a));
        its.extend(items(&b_rest));
        its.push(item_with("x", "same"));
        its.push(item_with("y", "same"));
        let plan = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(
            plan.duplicates,
            [
                ("twin".to_string(), "b003".to_string()),
                ("y".to_string(), "x".to_string())
            ],
            "the published one wins, else the first in set order"
        );
        assert_eq!(
            layout(&plan, &its),
            [(1, a.clone()), (2, cat(&[&b_rest, &one("x")]))],
            "the skipped item frees its place; each media is exported once"
        );
        let planned: Vec<usize> = plan.parts.iter().flat_map(|p| p.items.clone()).collect();
        let contents: HashSet<&str> = planned.iter().map(|&i| its[i].content_key.as_str()).collect();
        assert_eq!(contents.len(), planned.len());

        // An item changed to the media another one is published with: that
        // one stays, the changed one is the duplicate.
        let mut its = items(&cat(&[&a, &b]));
        its[1].content_key = "c-b001".into();
        let plan = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(plan.duplicates, [("a001".to_string(), "b001".to_string())]);
        assert_eq!(sizes(&plan), [(1, 119), (2, 10)]);
        // An item whose media changed keeps its part.
        let mut its = items(&cat(&[&a, &b]));
        its[125].content_key = "new media".into();
        let plan = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(layout(&plan, &its), [(1, a.clone()), (2, b.clone())]);
    }

    #[test]
    fn plan_lost_part_mapping() {
        let (a, b) = (named("a", 0..120), named("b", 0..10));
        let its = items(&cat(&[&a, &b]));
        // Part 2's pack was deleted by hand: the planner cannot tell, and
        // the items stay in part 2, whose export creates the pack again.
        let previous = PartStates::from([(1, state(&a)), (2, state(&b))]);
        let plan = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(layout(&plan, &its), [(1, a.clone()), (2, b.clone())]);

        // Part 2's mapping is lost (its first export failed before any
        // sticker was confirmed): its items are new again and fill it, not a
        // part 3.
        let previous = PartStates::from([(1, state(&a)), (2, PackState::default())]);
        let plan = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(layout(&plan, &its), [(1, a.clone()), (2, b.clone())]);
        assert!(plan.delete.is_empty());

        // A known part that is not needed any more is deleted, never left
        // behind.
        let previous = PartStates::from([(1, state(&a)), (2, state(&b)), (3, PackState::default())]);
        let plan = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(sizes(&plan), [(1, 120), (2, 10)]);
        assert_eq!(plan.delete, [3]);

        // An item in two parts' mappings (a repack that stopped after part 1
        // took it over from part 2) belongs to the lower part.
        let mut took = a.clone();
        took[119] = "b000".into();
        let previous = PartStates::from([(1, state(&took)), (2, state(&b))]);
        let its = items(&cat(&[&took, &b[1..], &a[119..]]));
        let plan = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(
            layout(&plan, &its),
            [(1, took.clone()), (2, cat(&[&b[1..], &a[119..]]))]
        );
    }

    #[test]
    fn plan_skips_retired_numbers() {
        let (a, b) = (named("a", 0..120), named("b", 0..120));
        // Part 1 was emptied and deleted; part 2 is full.
        let previous = PartStates::from([(2, state(&b))]);
        let retired = BTreeSet::from([1, 3]);
        let its = items(&cat(&[&one("new"), &b]));
        let plan = plan_parts_skipping(&its, &previous, &retired, CAP, false).unwrap();
        assert_eq!(layout(&plan, &its), [(2, b.clone()), (4, one("new"))]);
        assert!(plan.delete.is_empty());
        // Without retired numbers the new sticker would start part 1 again.
        let plain = plan_parts(&its, &previous, CAP, false).unwrap();
        assert_eq!(layout(&plain, &its), [(1, one("new")), (2, b.clone())]);

        // A repack uses the usable numbers in order: 2, 4, 5.
        let its = items(&cat(&[&a, &b, &one("x")]));
        let plan = plan_parts_skipping(&its, &previous, &retired, CAP, true).unwrap();
        assert_eq!(sizes(&plan), [(2, 120), (4, 120), (5, 1)]);
        assert_eq!(plan.moved, 120, "all of part 2 moves to part 4");

        // An existing part keeps its number, retired or not.
        let previous = PartStates::from([(1, state(&a))]);
        let its = items(&cat(&[&a, &one("y")]));
        let plan = plan_parts_skipping(&its, &previous, &BTreeSet::from([1, 2]), CAP, false).unwrap();
        assert_eq!(layout(&plan, &its), [(1, a.clone()), (3, one("y"))]);
        let plan = plan_parts_skipping(&its, &previous, &BTreeSet::from([1]), CAP, true).unwrap();
        assert_eq!(sizes(&plan), [(1, 120), (2, 1)]);
        assert_eq!(plan.moved, 0);
    }

    #[test]
    fn part_names_valid_for_every_part() {
        let bot = "stickpile_bot";
        let part1 = default_pack_name("my-cats", bot).unwrap();
        assert_eq!(part_pack_name(&part1, 1, bot).unwrap(), part1);
        assert_eq!(part_pack_name(&part1, 2, bot).unwrap(), "my_cats_2_by_stickpile_bot");
        assert_eq!(
            part_pack_name(&part1, 3, "@stickpile_bot").unwrap(),
            "my_cats_3_by_stickpile_bot"
        );
        let mut seen = HashSet::new();
        for k in 1..=500 {
            let name = part_pack_name(&part1, k, bot).unwrap();
            assert!(valid_pack_name(&name, bot), "{name}");
            assert!(seen.insert(name.to_ascii_lowercase()), "{name} twice");
        }
        assert!(part_pack_name(&part1, 0, bot).is_err());
    }

    #[test]
    fn part_names_shorten_long_base_to_64() {
        let bot = "stickpile_bot";
        let suffix = "_by_stickpile_bot";
        let part1 = default_pack_name(&"x".repeat(100), bot).unwrap();
        assert_eq!(part1.len(), 64);
        let mut seen = HashSet::new();
        for k in (1..=2000).chain([12345, u32::MAX]) {
            let name = part_pack_name(&part1, k, bot).unwrap();
            assert!(name.len() <= 64 && valid_pack_name(&name, bot), "{name}");
            if k > 1 {
                assert!(name.ends_with(&format!("x_{k}{suffix}")), "{name}");
            }
            assert!(seen.insert(name), "part {k} twice");
        }
        assert_eq!(
            part_pack_name(&part1, 2, bot).unwrap(),
            format!("{}_2{suffix}", "x".repeat(45))
        );

        // The cut lands just after an underscore: it goes too.
        let part1 = format!("{}_bc{suffix}", "a".repeat(44));
        assert_eq!(part1.len(), 64);
        assert_eq!(
            part_pack_name(&part1, 2, bot).unwrap(),
            format!("{}_2{suffix}", "a".repeat(44))
        );

        // The one case where the cut would give part 1's own name.
        let part1 = format!("{}_2{suffix}", "a".repeat(45));
        assert_eq!(part1.len(), 64);
        let part2 = part_pack_name(&part1, 2, bot).unwrap();
        assert_ne!(part2, part1);
        assert_eq!(part2, format!("{}_2{suffix}", "a".repeat(44)));

        // A bot username that leaves no room for a part number.
        let bot = format!("{}_bot", "b".repeat(55));
        let part1 = format!("a_by_{bot}");
        assert_eq!(part1.len(), 64);
        assert!(valid_pack_name(&part1, &bot));
        assert_eq!(part_pack_name(&part1, 1, &bot).unwrap(), part1);
        assert!(part_pack_name(&part1, 2, &bot).is_err());
    }

    #[test]
    fn part_names_from_custom_part1_name() {
        let bot = "StickPile_Bot";
        // The suffix is written as part 1 has it.
        assert_eq!(
            part_pack_name("MyCats_by_stickpile_bot", 2, bot).unwrap(),
            "MyCats_2_by_stickpile_bot"
        );
        assert_eq!(
            part_pack_name("cats_2_by_stickpile_bot", 2, bot).unwrap(),
            "cats_2_2_by_stickpile_bot"
        );
        assert_eq!(
            part_pack_name("made_by_hand_by_StickPile_Bot", 10, bot).unwrap(),
            "made_by_hand_10_by_StickPile_Bot"
        );
        for bad in ["cats_by_other_bot", "cats", "1cats_by_stickpile_bot", ""] {
            assert!(part_pack_name(bad, 2, bot).is_err(), "{bad}");
            assert!(part_pack_name(bad, 1, bot).is_err(), "{bad}");
        }
    }

    #[test]
    fn part_titles_cut_cyrillic_on_characters() {
        assert_eq!(part_title("Котики", 1), "Котики");
        assert_eq!(part_title("  Котики ", 2), "Котики 2");
        let zh = "ж".repeat(70);
        let t1 = part_title(&zh, 1);
        assert_eq!(t1, "ж".repeat(64));
        assert!(t1.len() > 64, "cut on characters, not bytes");
        assert_eq!(part_title(&zh, 2), format!("{} 2", "ж".repeat(62)));
        assert_eq!(part_title(&zh, 12), format!("{} 12", "ж".repeat(61)));
        let long = "Котики та песики ".repeat(5);
        for k in 1..=150 {
            let t = part_title(&long, k);
            assert!(t.chars().count() <= 64, "{t}");
            if k > 1 {
                assert!(t.ends_with(&format!(" {k}")) && !t.contains("  "), "{t}");
            }
        }
        assert_eq!(part_title("", 1), "");
        assert_eq!(part_title(" ", 3), "");
    }
}
