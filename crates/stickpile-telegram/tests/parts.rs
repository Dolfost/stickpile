//! Sets exported as several packs (parts), against the in-process mock Bot
//! API.

mod mock_bot;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mock_bot::{start, unique_id, Shared, BOT, OWNER, TOKEN};
use stickpile_telegram::{
    export_pack, export_parts, part_pack_name, plan_parts, BotApi, ExportItem, PackState, PackThumbnail, PartStates,
    PartUpdate, PartsError, PartsExport, PartsFailure, PartsReport, StickerFormat, TelegramError, MAX_PACK_STICKERS,
};

const PART1: &str = "cats_by_stickpile_test_bot";

fn api(base: &str) -> BotApi {
    BotApi::new(base, TOKEN).with_max_retry_wait(Duration::from_millis(20))
}

fn star(i: usize) -> ExportItem {
    ExportItem {
        key: format!("k{i:03}"),
        content_key: format!("C{i}"),
        bytes: format!("bytes of C{i}").into_bytes(),
        format: StickerFormat::Static,
        emojis: vec!["⭐".into()],
        keywords: vec![],
    }
}

fn stars(range: impl Iterator<Item = usize>) -> Vec<ExportItem> {
    range.map(star).collect()
}

fn fuid(it: &ExportItem) -> String {
    unique_id(&it.bytes)
}

fn name(part: u32) -> String {
    part_pack_name(PART1, part, BOT).unwrap()
}

/// A 100x100 PNG header, as the mock checks it.
fn thumb_png(seed: u8) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend(100u32.to_be_bytes());
    png.extend(100u32.to_be_bytes());
    png.extend([8, 6, 0, 0, 0, seed]);
    png
}

/// Keeps the part states as the server does: what `persist` hands over.
#[derive(Clone, Default)]
struct Store(Arc<Mutex<(PartStates, Vec<PartUpdate>)>>);

impl Store {
    fn states(&self) -> PartStates {
        self.0.lock().unwrap().0.clone()
    }

    fn updates(&self) -> Vec<PartUpdate> {
        self.0.lock().unwrap().1.clone()
    }

    fn apply(&self, u: PartUpdate) {
        let mut inner = self.0.lock().unwrap();
        match &u.state {
            Some(state) => inner.0.insert(u.number, state.clone()),
            None => inner.0.remove(&u.number),
        };
        inner.1.push(u);
    }
}

/// One export job: plan from the stored states, then export the parts.
async fn run(
    client: &BotApi,
    store: &Store,
    items: &[ExportItem],
    thumbnail: Option<&PackThumbnail>,
    repack: bool,
) -> Result<PartsReport, PartsError<String>> {
    let previous = store.states();
    let plan = plan_parts(items, &previous, MAX_PACK_STICKERS, repack).unwrap();
    let job = PartsExport {
        owner_user_id: OWNER,
        bot_username: BOT,
        part1_name: PART1,
        title: "Cats",
        items,
        previous: &previous,
        thumbnail,
    };
    let s = store.clone();
    export_parts(client, &job, &plan, move |u| {
        let s = s.clone();
        async move {
            s.apply(u);
            Ok::<(), String>(())
        }
    })
    .await
}

/// Every item's sticker is in exactly one pack, the one its part stored, in
/// set order, and the packs hold nothing else.
fn assert_each_once(state: &Shared, store: &Store, items: &[ExportItem]) {
    let st = state.lock().unwrap();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for pack in st.packs.values() {
        for s in &pack.stickers {
            *seen.entry(s.fuid.clone()).or_default() += 1;
        }
    }
    for it in items {
        assert_eq!(
            seen.remove(&fuid(it)),
            Some(1),
            "{} must be in exactly one pack",
            it.key
        );
    }
    assert!(seen.is_empty(), "stickers of no item: {seen:?}");
    let states = store.states();
    assert_eq!(st.packs.len(), states.len(), "one pack per stored part");
    let position: HashMap<&str, usize> = items.iter().enumerate().map(|(i, it)| (it.key.as_str(), i)).collect();
    for (number, part) in &states {
        let pack = st.pack(&name(*number));
        let mut keys: Vec<&String> = part.mapping.keys().collect();
        keys.sort_by_key(|k| position[k.as_str()]);
        let want: Vec<String> = keys.iter().map(|k| fuid(&items[position[k.as_str()]])).collect();
        let got: Vec<String> = pack.stickers.iter().map(|s| s.fuid.clone()).collect();
        assert_eq!(got, want, "part {number} holds its items in set order");
        for (key, (content, f)) in &part.mapping {
            let it = &items[position[key.as_str()]];
            assert_eq!((content, f), (&it.content_key, &fuid(it)), "{key}");
        }
    }
}

fn reset_calls(state: &Shared) {
    state.lock().unwrap().calls.clear();
}

fn count(state: &Shared, method: &str) -> usize {
    state.lock().unwrap().count(method)
}

#[tokio::test]
async fn export_parts_three_parts_from_scratch() {
    let (base, state) = start().await;
    let client = api(&base);
    let store = Store::default();
    let items = stars(0..250);
    let cover = PackThumbnail {
        key: "cover:sha".into(),
        png: thumb_png(1),
    };
    let report = run(&client, &store, &items, Some(&cover), false).await.unwrap();

    assert_eq!(report.parts.len(), 3);
    for (p, (number, stickers)) in report.parts.iter().zip([(1, 120), (2, 120), (3, 10)]) {
        assert_eq!(p.number, number);
        assert_eq!(p.name, name(number));
        assert_eq!(p.url, format!("https://t.me/addstickers/{}", name(number)));
        assert_eq!(p.stickers, stickers);
        assert!(p.created_pack && p.thumbnail && p.thumbnail_changed && !p.deleted);
        assert_eq!((p.added, p.removed, p.error.as_deref()), (stickers as u32, 0, None));
    }
    assert_eq!(report.parts[0].name, PART1, "part 1 keeps the single pack's name");
    assert_eq!(name(2), "cats_2_by_stickpile_test_bot");
    assert_eq!(
        report.parts.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
        ["Cats", "Cats 2", "Cats 3"]
    );
    let totals = report.totals();
    assert_eq!((totals.created_packs, totals.added, totals.stickers), (3, 250, 250));

    let updates = store.updates();
    assert_eq!(updates.iter().map(|u| u.number).collect::<Vec<_>>(), [1, 2, 3]);
    assert!(updates
        .iter()
        .all(|u| u.state.as_ref().unwrap().thumbnail.as_deref() == Some("cover:sha")));
    {
        let st = state.lock().unwrap();
        for number in 1..=3 {
            let pack = st.pack(&name(number));
            assert_eq!(pack.owner, OWNER);
            assert_eq!(pack.thumbnail, Some(thumb_png(1)), "the cover on every part");
        }
        assert_eq!(st.pack(&name(2)).title, "Cats 2");
    }
    assert_each_once(&state, &store, &items);

    // Again, unchanged: nothing is uploaded, deleted or moved.
    reset_calls(&state);
    let again = run(&client, &store, &items, Some(&cover), false).await.unwrap();
    let totals = again.totals();
    assert_eq!(
        (totals.created_packs, totals.added, totals.removed, totals.updated),
        (0, 0, 0, 250)
    );
    assert!(!totals.reordered && !totals.thumbnail_changed);
    for method in [
        "createNewStickerSet",
        "addStickerToSet",
        "deleteStickerFromSet",
        "setStickerPositionInSet",
        "setStickerSetThumbnail",
        "deleteStickerSet",
    ] {
        assert_eq!(count(&state, method), 0, "{method}");
    }
}

#[tokio::test]
async fn export_parts_fail_between_parts_then_continue() {
    for applied in [false, true] {
        let (base, state) = start().await;
        let client = api(&base);
        let store = Store::default();
        let items = stars(0..250);
        // Part 1 is created; creating part 2 fails (with `applied`, the
        // pack is created and only the answer is lost).
        {
            let mut st = state.lock().unwrap();
            st.fail_after.insert("createNewStickerSet".into(), 1);
            st.fail_after_applying = applied;
        }
        let err = run(&client, &store, &items, None, false).await.unwrap_err();
        assert!(
            matches!(
                err.failure,
                PartsFailure::Telegram(TelegramError::Api { code: 500, .. })
            ),
            "{err}"
        );
        assert!(
            err.to_string().starts_with("part 2 (cats_2_by_stickpile_test_bot): "),
            "{err}"
        );
        assert_eq!(err.report.parts.len(), 2);
        assert!(err.report.parts[0].error.is_none() && err.report.parts[0].created_pack);
        assert!(err.report.parts[1].error.is_some());
        let states = store.states();
        assert_eq!(states[&1].mapping.len(), 120, "part 1 stored when it was done");
        assert_eq!(
            states[&2].mapping.len(),
            if applied { 50 } else { 0 },
            "part 2 stored as far as it got"
        );
        assert!(!states.contains_key(&3));
        assert_eq!(state.lock().unwrap().packs.contains_key(&name(2)), applied);
        assert!(!state.lock().unwrap().packs.contains_key(&name(3)));

        // The next export goes on: no second pack 2, every sticker once.
        state.lock().unwrap().fail_after.clear();
        reset_calls(&state);
        let report = run(&client, &store, &items, None, false).await.unwrap();
        assert_eq!(
            report
                .parts
                .iter()
                .map(|p| (p.number, p.created_pack))
                .collect::<Vec<_>>(),
            [(1, false), (2, !applied), (3, true)]
        );
        assert_eq!(report.parts[1].removed, 0, "nothing part 2 got is uploaded again");
        assert_eq!(count(&state, "createNewStickerSet"), if applied { 1 } else { 2 });
        assert_eq!(count(&state, "addStickerToSet"), 70);
        assert_eq!(count(&state, "deleteStickerFromSet"), 0);
        assert_eq!(state.lock().unwrap().packs.len(), 3);
        assert_each_once(&state, &store, &items);
    }
}

#[tokio::test]
async fn export_parts_fail_in_the_middle_of_a_part_then_continue() {
    // `read_back`: whether reading the pack back after the failure works.
    for (applied, read_back) in [(false, true), (true, true), (true, false)] {
        let (base, state) = start().await;
        let client = api(&base);
        let store = Store::default();
        run(&client, &store, &stars(0..120), None, false).await.unwrap();

        // The set grows to 300: part 2 is created with 50 and stops at its
        // 31st added sticker.
        let items = stars(0..300);
        {
            let mut st = state.lock().unwrap();
            st.fail_after.insert("addStickerToSet".into(), 30);
            st.fail_after_applying = applied;
            if !read_back {
                // Part 1's read and part 2's first read work, the read back
                // after the failure does not.
                st.fail_after.insert("getStickerSet".into(), 2);
            }
        }
        reset_calls(&state);
        let err = run(&client, &store, &items, None, false).await.unwrap_err();
        assert_eq!(err.report.parts.len(), 2);
        assert_eq!(err.report.parts[1].number, 2);
        let in_pack = if applied { 81 } else { 80 };
        assert_eq!(state.lock().unwrap().pack(&name(2)).stickers.len(), in_pack);
        let stored = store.states()[&2].mapping.len();
        assert_eq!(stored, if read_back { in_pack } else { 0 });
        assert_eq!(
            count(&state, "deleteStickerSet"),
            0,
            "nothing is deleted after a failure"
        );

        state.lock().unwrap().fail_after.clear();
        reset_calls(&state);
        let report = run(&client, &store, &items, None, false).await.unwrap();
        let part2 = report.part(2).unwrap();
        assert!(!part2.created_pack, "no second pack 2");
        if read_back {
            assert_eq!((part2.added, part2.removed), (120 - in_pack as u32, 0));
        } else {
            // Its stickers are unknown: replaced, never doubled.
            assert_eq!((part2.added, part2.removed), (120, in_pack as u32));
        }
        assert_eq!(count(&state, "createNewStickerSet"), 1, "part 3 only");
        assert_eq!(state.lock().unwrap().packs.len(), 3);
        assert_each_once(&state, &store, &items);
    }
}

#[tokio::test]
async fn export_parts_deletes_empty_parts_after_the_others() {
    let (base, state) = start().await;
    let client = api(&base);
    let store = Store::default();
    run(&client, &store, &stars(0..260), None, false).await.unwrap();
    assert_eq!(state.lock().unwrap().packs.len(), 3);

    // Every item of part 2 removed, and one of part 3.
    let items = stars((0..120).chain(240..260).filter(|&i| i != 250));
    reset_calls(&state);
    let report = run(&client, &store, &items, None, false).await.unwrap();
    assert_eq!(
        report.parts.iter().map(|p| (p.number, p.deleted)).collect::<Vec<_>>(),
        [(1, false), (3, false), (2, true)]
    );
    assert_eq!(report.part(3).unwrap().removed, 1);
    assert_eq!(report.totals().deleted_packs, 1);
    {
        let st = state.lock().unwrap();
        assert!(!st.packs.contains_key(&name(2)));
        assert_eq!(st.calls.last().map(String::as_str), Some("deleteStickerSet"));
        let deleted_at = st.calls.iter().position(|c| c == "deleteStickerSet").unwrap();
        let part3_done = st.calls.iter().rposition(|c| c == "deleteStickerFromSet").unwrap();
        assert!(part3_done < deleted_at, "after the other parts are updated");
        assert_eq!(st.count("deleteStickerSet"), 1);
    }
    let last = store.updates().pop().unwrap();
    assert_eq!(
        (last.number, last.name.as_str(), last.state),
        (2, name(2).as_str(), None)
    );
    assert!(!store.states().contains_key(&2), "the part is forgotten");
    assert_each_once(&state, &store, &items);

    // A part that is gone from Telegram already counts as deleted.
    let mut states = store.states();
    states.insert(4, PackState::default());
    *store.0.lock().unwrap() = (states, Vec::new());
    let report = run(&client, &store, &items, None, false).await.unwrap();
    assert!(report.part(4).unwrap().deleted);
    assert!(!store.states().contains_key(&4));
    assert_each_once(&state, &store, &items);
}

#[tokio::test]
async fn export_single_part_binding_unchanged() {
    let (base, state) = start().await;
    let client = api(&base);
    let items = stars(0..3);
    let cover = PackThumbnail {
        key: "cover:sha".into(),
        png: thumb_png(3),
    };
    // A set bound to a single pack, as before parts.
    let single = export_pack(
        &client,
        OWNER,
        PART1,
        "Cats",
        &items,
        &PackState::default(),
        Some(&cover),
    )
    .await
    .unwrap();
    let store = Store::default();
    store.apply(PartUpdate {
        number: 1,
        name: PART1.into(),
        state: Some(single.state()),
    });

    reset_calls(&state);
    let report = run(&client, &store, &items, Some(&cover), false).await.unwrap();
    assert_eq!(report.parts.len(), 1);
    let p = report.part(1).unwrap();
    assert_eq!((p.name.as_str(), p.title.as_str(), p.stickers), (PART1, "Cats", 3));
    assert_eq!(
        (p.created_pack, p.added, p.removed, p.updated, p.reordered),
        (false, 0, 0, 3, false)
    );
    assert!(p.thumbnail && !p.thumbnail_changed);
    assert_eq!(store.states()[&1], single.state(), "the binding is unchanged");
    {
        let st = state.lock().unwrap();
        assert_eq!(st.packs.len(), 1);
        for method in [
            "createNewStickerSet",
            "addStickerToSet",
            "deleteStickerFromSet",
            "setStickerPositionInSet",
            "setStickerSetThumbnail",
            "setStickerSetTitle",
            "deleteStickerSet",
        ] {
            assert_eq!(st.count(method), 0, "{method}");
        }
    }
    assert_each_once(&state, &store, &items);
}

#[tokio::test]
async fn export_parts_recreates_a_part_deleted_by_hand() {
    let (base, state) = start().await;
    let client = api(&base);
    let store = Store::default();
    let items = stars(0..130);
    run(&client, &store, &items, None, false).await.unwrap();
    state.lock().unwrap().packs.remove(&name(2));

    let report = run(&client, &store, &items, None, false).await.unwrap();
    assert!(!report.part(1).unwrap().created_pack);
    let part2 = report.part(2).unwrap();
    assert!(part2.created_pack);
    assert_eq!(part2.added, 10);
    assert_each_once(&state, &store, &items);
}

#[tokio::test]
async fn export_parts_repack_moves_each_sticker_once() {
    let (base, state) = start().await;
    let client = api(&base);
    let store = Store::default();
    run(&client, &store, &stars(0..250), None, false).await.unwrap();
    // Half of parts 1 and 2 removed: kept where they are, 60/60/10.
    let items = stars((60..120).chain(180..250));
    let stable = run(&client, &store, &items, None, false).await.unwrap();
    assert_eq!(
        stable.parts.iter().map(|p| p.stickers).collect::<Vec<_>>(),
        [60, 60, 10]
    );
    assert_eq!(stable.moved, 0);

    let plan = plan_parts(&items, &store.states(), MAX_PACK_STICKERS, true).unwrap();
    assert_eq!(plan.moved, 70, "the number a confirmation shows");
    let report = run(&client, &store, &items, None, true).await.unwrap();
    assert_eq!(report.moved, 70);
    assert_eq!(
        report
            .parts
            .iter()
            .map(|p| (p.number, p.stickers, p.deleted))
            .collect::<Vec<_>>(),
        [(1, 120, false), (2, 10, false), (3, 0, true)]
    );
    assert!(!state.lock().unwrap().packs.contains_key(&name(3)));
    assert_each_once(&state, &store, &items);
}

#[tokio::test]
async fn export_parts_repack_that_stops_is_continued() {
    for then_repack in [false, true] {
        let (base, state) = start().await;
        let client = api(&base);
        let store = Store::default();
        run(&client, &store, &stars(0..250), None, false).await.unwrap();
        let items = stars((60..120).chain(180..250));
        run(&client, &store, &items, None, false).await.unwrap();

        // The repack takes part 2's stickers into part 1, then stops while
        // part 2 deletes them.
        state
            .lock()
            .unwrap()
            .fail_after
            .insert("deleteStickerFromSet".into(), 10);
        let err = run(&client, &store, &items, None, true).await.unwrap_err();
        assert_eq!(err.report.parts.iter().map(|p| p.number).collect::<Vec<_>>(), [1, 2]);
        assert!(state.lock().unwrap().packs.contains_key(&name(3)), "nothing deleted");
        state.lock().unwrap().fail_after.clear();

        // A normal export after it, or the repack again (a retried job),
        // leaves every sticker in exactly one pack.
        let report = run(&client, &store, &items, None, then_repack).await.unwrap();
        let parts: Vec<(u32, usize, bool)> = report.parts.iter().map(|p| (p.number, p.stickers, p.deleted)).collect();
        if then_repack {
            assert_eq!(parts, [(1, 120, false), (2, 10, false), (3, 0, true)]);
        } else {
            assert_eq!(parts, [(1, 120, false), (3, 10, false), (2, 0, true)]);
        }
        assert_each_once(&state, &store, &items);
    }
}

#[tokio::test]
async fn export_parts_stops_when_a_part_cannot_be_stored() {
    let (base, state) = start().await;
    let client = api(&base);
    let items = stars(0..130);
    let previous = PartStates::new();
    let plan = plan_parts(&items, &previous, MAX_PACK_STICKERS, false).unwrap();
    let job = PartsExport {
        owner_user_id: OWNER,
        bot_username: BOT,
        part1_name: PART1,
        title: "Cats",
        items: &items,
        previous: &previous,
        thumbnail: None,
    };
    let err = export_parts(&client, &job, &plan, |_| async {
        Err::<(), _>("disk full".to_string())
    })
    .await
    .unwrap_err();
    assert!(
        matches!(&err.failure, PartsFailure::Persist(e) if e == "disk full"),
        "{err}"
    );
    assert_eq!(err.report.parts.len(), 1, "stopped after part 1");
    assert!(!state.lock().unwrap().packs.contains_key(&name(2)));

    // An invalid part 1 name fails before any call.
    reset_calls(&state);
    let job = PartsExport {
        part1_name: "cats_by_other_bot",
        ..job
    };
    let err = export_parts(&client, &job, &plan, |_| async { Ok::<(), String>(()) })
        .await
        .unwrap_err();
    assert!(
        matches!(err.failure, PartsFailure::Telegram(TelegramError::Invalid(_))),
        "{err}"
    );
    assert!(state.lock().unwrap().calls.is_empty());

    // So does a plan with parts larger than a Telegram pack.
    let big = plan_parts(&items, &previous, 200, false).unwrap();
    let job = PartsExport {
        part1_name: PART1,
        ..job
    };
    let err = export_parts(&client, &job, &big, |_| async { Ok::<(), String>(()) })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("at most 120"), "{err}");
    assert!(state.lock().unwrap().calls.is_empty());
}

/// The packs holding the sticker made from `it`'s file.
fn packs_with(state: &Shared, it: &ExportItem) -> Vec<String> {
    let st = state.lock().unwrap();
    st.packs
        .iter()
        .filter(|(_, p)| p.stickers.iter().any(|s| s.fuid == fuid(it)))
        .map(|(n, _)| n.clone())
        .collect()
}

/// Review finding: when the exported item of two items with the same media
/// is removed, the item left is planned as a new sticker and goes to the
/// first part with room: the very same sticker leaves part 2 for part 1
/// without a repack (rule 5), and whoever added only part 2 loses it.
#[tokio::test]
async fn review_a_duplicate_left_alone_keeps_its_stickers_pack() {
    let (base, state) = start().await;
    let client = api(&base);
    let store = Store::default();
    run(&client, &store, &stars(0..130), None, false).await.unwrap();
    // 20 stickers of part 1 go: 100 and 10, part 1 has room.
    let mut items = stars(20..130);
    run(&client, &store, &items, None, false).await.unwrap();
    // A second item with the media of k125 (a copy moved in from another
    // set, or a file replaced by the same one): exported once, in part 2.
    let twin = ExportItem {
        key: "twin".into(),
        ..star(125)
    };
    items.push(twin.clone());
    let report = run(&client, &store, &items, None, false).await.unwrap();
    assert_eq!(report.totals().added, 0);
    assert_eq!(packs_with(&state, &twin), [name(2)]);

    // The duplicates check is resolved by deleting k125: its twin stays.
    items.retain(|it| it.key != "k125");
    run(&client, &store, &items, None, false).await.unwrap();
    assert_each_once(&state, &store, &items);
    assert_eq!(
        packs_with(&state, &twin),
        [name(2)],
        "the sticker stays in the pack it was published to"
    );
}
