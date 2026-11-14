//! Tabs, modes, Normal-mode keys and the per-tab lists.

use stickpile_api::{SetSource, Visibility};

use super::tests::{cfg, hits, ids, item, items, opened, with_results};
use super::*;
use crate::vim::NormalKey;

fn set(id: &str, name: &str) -> SetSummary {
    SetSummary {
        id: id.into(),
        slug: name.to_lowercase().replace(' ', "-"),
        name: name.into(),
        description: String::new(),
        owner_username: "me".into(),
        visibility: Visibility::Private,
        source: SetSource::Local,
        item_count: 3,
        cover_item_id: None,
        cover_url: None,
        updated_at: 0,
    }
}

fn open_with(cfg: ModelConfig) -> (Model, Vec<Command>) {
    let mut m = Model::new(cfg);
    let cmds = m.handle(Event::Open(OpenInfo::default()));
    (m, cmds)
}

fn open_on(tab: Tab) -> Model {
    open_with(ModelConfig {
        start_tab: Some(tab),
        ..cfg()
    })
    .0
}

fn non_modal() -> ModelConfig {
    ModelConfig { modal: false, ..cfg() }
}

fn key(m: &mut Model, c: char) -> Vec<Command> {
    m.handle(Event::Normal(NormalKey::Char(c)))
}

/// Plain characters in Normal mode; every command they gave.
fn keys(m: &mut Model, s: &str) -> Vec<Command> {
    s.chars().flat_map(|c| key(m, c)).collect()
}

fn nk(m: &mut Model, k: NormalKey) -> Vec<Command> {
    m.handle(Event::Normal(k))
}

fn most_used_gen(cmds: &[Command]) -> Option<u64> {
    cmds.iter().find_map(|c| match c {
        Command::LoadMostUsed { generation, .. } => Some(*generation),
        _ => None,
    })
}

fn favorites_gen(cmds: &[Command]) -> Option<u64> {
    cmds.iter().find_map(|c| match c {
        Command::LoadFavorites { generation, .. } => Some(*generation),
        _ => None,
    })
}

fn packs_gen(cmds: &[Command]) -> Option<u64> {
    cmds.iter().find_map(|c| match c {
        Command::LoadPacks { generation } => Some(*generation),
        _ => None,
    })
}

fn pack_items_gen(cmds: &[Command]) -> Option<(String, u64)> {
    cmds.iter().find_map(|c| match c {
        Command::LoadPackItems { set_id, generation } => Some((set_id.clone(), *generation)),
        _ => None,
    })
}

fn timeout_seq(cmds: &[Command]) -> u64 {
    match cmds {
        [Command::KeyTimeout { after_ms, seq }] => {
            assert_eq!(*after_ms, DEFAULT_KEY_TIMEOUT_MS);
            *seq
        }
        other => panic!("{other:?}"),
    }
}

fn has(cmds: &[Command], want: &Command) -> bool {
    cmds.contains(want)
}

fn loads_recent(cmds: &[Command]) -> bool {
    cmds.iter().any(|c| matches!(c, Command::LoadRecent { .. }))
}

/// Normal mode on Most used with `n` items (4 columns, 2 visible rows).
fn grid(n: usize) -> Model {
    let (mut m, cmds) = open_with(ModelConfig {
        start_tab: Some(Tab::MostUsed),
        ..cfg()
    });
    let generation = most_used_gen(&cmds).unwrap();
    m.handle(Event::MostUsedLoaded {
        generation,
        outcome: Ok(items(n)),
    });
    assert_eq!(m.mode(), Mode::Normal);
    m
}

fn packs_model(cfg: ModelConfig) -> Model {
    let (mut m, cmds) = open_with(ModelConfig {
        start_tab: Some(Tab::Packs),
        ..cfg
    });
    let generation = packs_gen(&cmds).unwrap();
    m.handle(Event::PacksLoaded {
        generation,
        outcome: Ok(vec![set("s1", "Cats"), set("s2", "Dogs"), set("s3", "Cool cats")]),
    });
    m
}

fn pack_names(m: &Model) -> Vec<&str> {
    m.packs().iter().map(|s| s.name.as_str()).collect()
}

fn pack_item(id: &str, name: &str, keywords: &[&str], emojis: &[&str], tags: &[&str]) -> ItemSummary {
    let mut it = item(id);
    it.name = name.into();
    it.keywords = keywords.iter().map(|s| s.to_string()).collect();
    it.emojis = emojis.iter().map(|s| s.to_string()).collect();
    it.tags = tags.iter().map(|s| s.to_string()).collect();
    it
}

/// Packs tab with "Cool cats" (s3) opened and its two items loaded.
fn opened_pack(cfg: ModelConfig) -> Model {
    let mut m = packs_model(cfg);
    m.handle(Event::QueryChanged("cat".into()));
    m.handle(Event::Navigate(Nav::Right));
    let cmds = m.handle(Event::CopySelected { toggle_sticker: false });
    let (set_id, generation) = pack_items_gen(&cmds).unwrap();
    assert_eq!(set_id, "s3");
    m.handle(Event::PackItemsLoaded {
        set_id,
        generation,
        outcome: Ok(vec![
            pack_item("a", "grumpy", &["angry"], &["😾"], &[]),
            pack_item("b", "happy", &["smile"], &[], &["mood"]),
        ]),
    });
    m
}

// -- opening ------------------------------------------------------------------------

#[test]
fn opens_on_the_start_tab_in_its_mode() {
    for tab in Tab::ALL {
        let (m, cmds) = open_with(ModelConfig {
            start_tab: Some(tab),
            ..cfg()
        });
        assert_eq!(m.tab(), tab);
        assert_eq!(m.mode(), tab.default_mode(), "{tab:?}");
        assert_eq!(cmds[..2], [Command::Present, Command::CancelSearch]);
        let load = &cmds[2..];
        match tab {
            Tab::Search | Tab::Recent => assert_eq!(load, [Command::LoadRecent { limit: 5 }]),
            Tab::MostUsed => assert_eq!(
                load,
                [Command::LoadMostUsed {
                    limit: 120,
                    generation: 1
                }]
            ),
            Tab::Favorites => assert!(matches!(load, [Command::LoadFavorites { limit: 120, .. }])),
            Tab::Packs => assert!(matches!(load, [Command::LoadPacks { .. }])),
            Tab::Me | Tab::Settings => assert!(load.is_empty()),
        }
    }
}

#[test]
fn search_and_packs_open_in_insert_the_rest_in_normal() {
    assert_eq!(open_on(Tab::Search).mode(), Mode::Insert);
    assert_eq!(open_on(Tab::Packs).mode(), Mode::Insert);
    for tab in [Tab::Recent, Tab::MostUsed, Tab::Favorites, Tab::Me, Tab::Settings] {
        assert_eq!(open_on(tab).mode(), Mode::Normal);
    }
}

#[test]
fn reopening_starts_over_on_the_start_tab() {
    let mut m = open_on(Tab::Recent);
    m.handle(Event::ShowTab(Tab::Packs));
    m.handle(Event::ToggleHelp);
    m.handle(Event::Hide);
    m.handle(Event::Open(OpenInfo::default()));
    assert_eq!(m.tab(), Tab::Recent);
    assert_eq!(m.mode(), Mode::Normal);
    assert!(!m.help_visible());
}

#[test]
fn footer_shows_mode_and_pending_keys() {
    let mut m = grid(10);
    let f = m.footer();
    assert_eq!(f.mode, Some(Mode::Normal));
    assert_eq!(f.pending, "");
    keys(&mut m, "12g");
    assert_eq!(m.footer().pending, "12g");
    assert_eq!(m.pending(), "12g");
    keys(&mut m, "g");
    assert_eq!(m.footer().pending, "");
    assert_eq!(m.selected(), Some(9), "12gg: item 12, clamped");
    let m = open_on(Tab::Search);
    assert_eq!(m.footer().mode, Some(Mode::Insert));
    let (m, _) = open_with(non_modal());
    assert_eq!(m.footer().mode, None);
    assert_eq!(Mode::Normal.label(), "NORMAL");
}

// -- Insert and Normal on Search ------------------------------------------------------------

#[test]
fn escape_goes_to_normal_then_hides() {
    let mut m = opened(cfg(), None);
    m.handle(Event::QueryChanged("cat".into()));
    assert!(m.handle(Event::Escape).is_empty());
    assert_eq!(m.mode(), Mode::Normal);
    assert!(m.is_visible());
    assert_eq!(m.query(), "cat", "the query stays");
    assert_eq!(nk(&mut m, NormalKey::Escape), [Command::Hide]);
    assert!(!m.is_visible());
}

#[test]
fn i_and_a_enter_insert_keeping_the_query() {
    for c in ['i', 'a'] {
        let mut m = opened(cfg(), None);
        m.handle(Event::QueryChanged("cat".into()));
        m.handle(Event::Escape);
        let g = m.generation();
        assert!(key(&mut m, c).is_empty());
        assert_eq!(m.mode(), Mode::Insert);
        assert_eq!(m.query(), "cat");
        assert_eq!(m.generation(), g, "no new search");
    }
}

#[test]
fn slash_clears_the_query_and_enters_insert() {
    let mut m = with_results(3);
    m.handle(Event::RecentLoaded(items(2)));
    m.handle(Event::Escape);
    let cmds = key(&mut m, '/');
    assert!(has(&cmds, &Command::CancelSearch));
    assert_eq!(m.mode(), Mode::Insert);
    assert_eq!(m.query(), "");
    assert_eq!(ids(&m), ["0", "1"], "back to recent items");
}

#[test]
fn slash_and_i_from_a_grid_tab_go_to_search() {
    let mut m = opened(cfg(), None);
    m.handle(Event::QueryChanged("cat".into()));
    m.handle(Event::ShowTab(Tab::Recent));
    assert_eq!(m.mode(), Mode::Normal);
    let cmds = key(&mut m, 'i');
    assert_eq!((m.tab(), m.mode(), m.query()), (Tab::Search, Mode::Insert, "cat"));
    assert!(!loads_recent(&cmds), "the query is still there");

    m.handle(Event::ShowTab(Tab::Favorites));
    let cmds = key(&mut m, '/');
    assert_eq!((m.tab(), m.mode(), m.query()), (Tab::Search, Mode::Insert, ""));
    assert!(has(&cmds, &Command::CancelSearch));
    assert!(loads_recent(&cmds));
}

#[test]
fn normal_keys_are_ignored_in_insert_and_without_modes() {
    let mut m = with_results(5);
    assert_eq!(m.mode(), Mode::Insert);
    assert!(keys(&mut m, "ljq").is_empty());
    assert_eq!(m.selected(), Some(0));
    assert!(m.is_visible());
    let (mut m, _) = open_with(non_modal());
    m.handle(Event::RecentLoaded(items(3)));
    assert!(keys(&mut m, "lq").is_empty());
    assert_eq!(m.selected(), Some(0));
    assert_eq!(m.mode(), Mode::Insert);
}

#[test]
fn text_field_focus_switches_modes() {
    let mut m = opened(cfg(), None);
    m.handle(Event::Escape);
    // A click into the search entry.
    m.handle(Event::TextFocus(true));
    assert_eq!(m.mode(), Mode::Insert);
    m.handle(Event::TextFocus(false));
    assert_eq!(m.mode(), Mode::Normal);
    m.handle(Event::TextFocus(false));
    assert_eq!(m.mode(), Mode::Normal);

    // No text field on Recent; a hidden popup and modal = false ignore focus.
    let mut m = open_on(Tab::Recent);
    m.handle(Event::TextFocus(true));
    assert_eq!(m.mode(), Mode::Normal);
    let mut m = opened(cfg(), None);
    m.handle(Event::Hide);
    m.handle(Event::TextFocus(false));
    assert_eq!(m.mode(), Mode::Insert);
    let (mut m, _) = open_with(non_modal());
    m.handle(Event::TextFocus(false));
    assert_eq!(m.mode(), Mode::Insert);
}

// -- Normal-mode movement ----------------------------------------------------------------------

#[test]
fn hjkl_and_counts() {
    // 4 columns: rows [0..4) [4..8) [8..10)
    let mut m = grid(10);
    assert_eq!(m.selected(), Some(0));
    keys(&mut m, "l");
    assert_eq!(m.selected(), Some(1));
    keys(&mut m, "3l");
    assert_eq!(m.selected(), Some(4));
    keys(&mut m, "j");
    assert_eq!(m.selected(), Some(8));
    keys(&mut m, "k");
    assert_eq!(m.selected(), Some(4));
    keys(&mut m, "2k");
    assert_eq!(m.selected(), Some(0));
    keys(&mut m, "20j");
    assert_eq!(m.selected(), Some(8));
    keys(&mut m, "h");
    assert_eq!(m.selected(), Some(7), "h wraps to the previous row");
    nk(&mut m, NormalKey::Nav(Nav::Up));
    assert_eq!(m.selected(), Some(3));
    keys(&mut m, "2");
    nk(&mut m, NormalKey::Nav(Nav::Right));
    assert_eq!(m.selected(), Some(5), "counts apply to arrows too");
}

#[test]
fn first_last_rows_and_half_pages() {
    let mut m = grid(10);
    keys(&mut m, "G");
    assert_eq!(m.selected(), Some(9));
    keys(&mut m, "gg");
    assert_eq!(m.selected(), Some(0));
    keys(&mut m, "5G");
    assert_eq!(m.selected(), Some(4));
    keys(&mut m, "$");
    assert_eq!(m.selected(), Some(7));
    keys(&mut m, "0");
    assert_eq!(m.selected(), Some(4));
    keys(&mut m, "3gg");
    assert_eq!(m.selected(), Some(2));
    keys(&mut m, "99G");
    assert_eq!(m.selected(), Some(9));
    keys(&mut m, "$");
    assert_eq!(m.selected(), Some(9), "short last row");
    // Two visible rows: half a page is one row.
    keys(&mut m, "gg");
    nk(&mut m, NormalKey::Ctrl('d'));
    assert_eq!(m.selected(), Some(4));
    nk(&mut m, NormalKey::Ctrl('d'));
    assert_eq!(m.selected(), Some(8));
    nk(&mut m, NormalKey::Ctrl('u'));
    assert_eq!(m.selected(), Some(4));
    m.handle(Event::Layout {
        columns: 2,
        visible_rows: 4,
    });
    nk(&mut m, NormalKey::Ctrl('d'));
    assert_eq!(m.selected(), Some(8), "two rows of two");
}

#[test]
fn movement_on_an_empty_grid() {
    let mut m = grid(0);
    assert!(keys(&mut m, "jlG0$").is_empty());
    assert_eq!(m.selected(), None);
    assert!(keys(&mut m, "y").is_empty());
    assert_eq!(m.status(), &Status::NothingSelected);
}

#[test]
fn unknown_keys_do_nothing() {
    let mut m = grid(5);
    let rev = m.entries_revision();
    assert!(keys(&mut m, "zZwbx!").is_empty());
    assert!(nk(&mut m, NormalKey::Other).is_empty());
    assert!(nk(&mut m, NormalKey::Ctrl('x')).is_empty());
    assert!(nk(&mut m, NormalKey::Space).is_empty());
    assert!(nk(&mut m, NormalKey::Backspace).is_empty());
    assert_eq!(
        (m.tab(), m.mode(), m.selected()),
        (Tab::MostUsed, Mode::Normal, Some(0))
    );
    assert_eq!(m.entries_revision(), rev);
    assert!(m.is_visible());
}

#[test]
fn other_input_cancels_a_pending_sequence() {
    let mut m = grid(10);
    keys(&mut m, "g");
    m.handle(Event::Navigate(Nav::Right));
    assert_eq!(m.pending(), "");
    keys(&mut m, "g");
    assert_eq!(m.selected(), Some(1), "g did not survive into gg");
    nk(&mut m, NormalKey::Escape);
    keys(&mut m, "3");
    assert_eq!(m.pending(), "3");
    m.handle(Event::CycleTarget { backwards: false });
    assert_eq!(m.pending(), "");
    keys(&mut m, "3");
    // Hover and answers leave it alone.
    m.handle(Event::Select(2));
    m.handle(Event::RecentLoaded(items(1)));
    assert_eq!(m.pending(), "3");
    nk(&mut m, NormalKey::Escape);
    assert_eq!(m.pending(), "");
    assert!(m.is_visible(), "the first Escape only cancels");
}

// -- tabs -------------------------------------------------------------------------------------------

#[test]
fn lone_digit_switches_tab_after_the_timeout() {
    let mut m = grid(10);
    let seq = timeout_seq(&key(&mut m, '2'));
    assert_eq!(m.tab(), Tab::MostUsed);
    assert_eq!(m.footer().pending, "2");
    assert!(m.handle(Event::KeyTimeout { seq: seq + 1 }).is_empty());
    assert_eq!(m.tab(), Tab::MostUsed);
    let cmds = m.handle(Event::KeyTimeout { seq });
    assert_eq!(m.tab(), Tab::Recent);
    assert!(loads_recent(&cmds));
    assert_eq!(m.pending(), "");
    assert!(m.handle(Event::KeyTimeout { seq }).is_empty(), "only once");
}

#[test]
fn digit_then_motion_is_a_count_not_a_tab() {
    let mut m = grid(10);
    let seq = timeout_seq(&key(&mut m, '2'));
    key(&mut m, 'l');
    assert_eq!(m.selected(), Some(2));
    assert!(m.handle(Event::KeyTimeout { seq }).is_empty());
    assert_eq!(m.tab(), Tab::MostUsed);
    // Escape cancels the digit instead.
    let seq = timeout_seq(&key(&mut m, '4'));
    nk(&mut m, NormalKey::Escape);
    assert!(m.handle(Event::KeyTimeout { seq }).is_empty());
    assert_eq!(m.tab(), Tab::MostUsed);
    assert!(m.is_visible());
}

#[test]
fn digit_then_another_key_switches_then_runs_it() {
    let mut m = opened(cfg(), None);
    m.handle(Event::RecentLoaded(items(3)));
    m.handle(Event::ShowTab(Tab::MostUsed));
    let cmds = keys(&mut m, "2y");
    assert_eq!(m.tab(), Tab::Recent);
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::Copy { item, .. } if item.id == "0")),
        "{cmds:?}"
    );
}

#[test]
fn digits_without_a_grid_switch_at_once() {
    let mut m = open_on(Tab::Me);
    let cmds = key(&mut m, '4');
    assert_eq!(m.tab(), Tab::Packs);
    assert_eq!(m.mode(), Mode::Insert);
    assert!(packs_gen(&cmds).is_some());
    let mut m = open_on(Tab::Settings);
    assert!(keys(&mut m, "89").is_empty());
    assert_eq!(m.tab(), Tab::Settings);
    key(&mut m, '2');
    assert_eq!(m.tab(), Tab::Recent);
}

#[test]
fn gt_and_gt_backwards_wrap() {
    let mut m = open_on(Tab::Recent);
    keys(&mut m, "gt");
    assert_eq!(m.tab(), Tab::MostUsed);
    keys(&mut m, "gT");
    assert_eq!(m.tab(), Tab::Recent);
    keys(&mut m, "2gT");
    assert_eq!(m.tab(), Tab::Settings);
    keys(&mut m, "gt");
    assert_eq!(m.tab(), Tab::Search);
    assert_eq!(m.mode(), Mode::Insert, "Search starts typing");
    assert!(keys(&mut m, "gt").is_empty(), "typed into the query, not Normal keys");
    assert_eq!(m.tab(), Tab::Search);

    let mut m = open_on(Tab::Recent);
    keys(&mut m, "5gt");
    assert_eq!(m.tab(), Tab::Favorites);
    keys(&mut m, "9gt");
    assert_eq!(m.tab(), Tab::Favorites, "no tab 9");
    keys(&mut m, "7gT");
    assert_eq!(m.tab(), Tab::Favorites, "seven back is a full circle");
}

#[test]
fn tab_and_shift_tab_cycle_in_both_modes() {
    let mut m = opened(cfg(), None);
    assert_eq!(m.mode(), Mode::Insert);
    m.handle(Event::CycleTab { backwards: false });
    assert_eq!((m.tab(), m.mode()), (Tab::Recent, Mode::Normal));
    m.handle(Event::CycleTab { backwards: false });
    m.handle(Event::CycleTab { backwards: false });
    assert_eq!((m.tab(), m.mode()), (Tab::Packs, Mode::Insert));
    m.handle(Event::CycleTab { backwards: true });
    assert_eq!(m.tab(), Tab::MostUsed);
    for _ in 0..3 {
        m.handle(Event::CycleTab { backwards: true });
    }
    assert_eq!(m.tab(), Tab::Settings, "wraps backwards");
    m.handle(Event::CycleTab { backwards: false });
    assert_eq!(m.tab(), Tab::Search, "wraps forwards");
}

#[test]
fn show_tab_by_number_and_same_tab() {
    let mut m = opened(cfg(), None);
    assert!(m.handle(Event::ShowTab(Tab::Search)).is_empty());
    assert_eq!(m.mode(), Mode::Insert);
    let cmds = m.handle(Event::ShowTab(Tab::Favorites));
    assert!(favorites_gen(&cmds).is_some());
    assert_eq!(m.title(), "Favorites");
    assert_eq!(m.view(), View::Items);
    m.handle(Event::ShowTab(Tab::Settings));
    assert_eq!(m.view(), View::Page);
    assert!(m.entries().is_empty());
}

#[test]
fn selection_is_remembered_per_tab_within_an_opening() {
    let mut m = opened(cfg(), None);
    m.handle(Event::RecentLoaded(items(5)));
    m.handle(Event::Select(2));
    m.handle(Event::ShowTab(Tab::Recent));
    assert_eq!(m.selected(), Some(0));
    m.handle(Event::Navigate(Nav::Right));
    m.handle(Event::ShowTab(Tab::Search));
    assert_eq!(m.selected(), Some(2));
    m.handle(Event::ShowTab(Tab::Recent));
    assert_eq!(m.selected(), Some(1));
    // A refresh keeps the remembered item, wherever it moved.
    m.handle(Event::RecentLoaded(
        ["4", "3", "2", "1", "0"].into_iter().map(item).collect(),
    ));
    assert_eq!(m.selected_entry().unwrap().item.id, "1");
    // A new opening starts at the top.
    m.handle(Event::Hide);
    m.handle(Event::Open(OpenInfo::default()));
    assert_eq!(m.selected(), Some(0));
}

#[test]
fn refresh_without_user_movement_selects_the_first_item() {
    let mut m = opened(cfg(), None);
    m.handle(Event::RecentLoaded(items(3)));
    m.handle(Event::RecentLoaded(["2", "0", "1"].into_iter().map(item).collect()));
    assert_eq!(m.selected(), Some(0));
    assert_eq!(m.selected_entry().unwrap().item.id, "2");
}

#[test]
fn search_results_wait_for_the_search_tab() {
    let mut m = opened(cfg(), None);
    m.handle(Event::RecentLoaded(items(2)));
    m.handle(Event::QueryChanged("cat".into()));
    let g = m.generation();
    m.handle(Event::ShowTab(Tab::Recent));
    assert_ne!(m.status(), &Status::Searching, "Recent has nothing to search");
    let cmds = m.handle(Event::Results {
        generation: g,
        outcome: Ok(Results {
            hits: hits(3),
            offline: true,
        }),
    });
    assert!(cmds.is_empty());
    assert_eq!(ids(&m), ["0", "1"]);
    assert!(!m.is_offline(), "offline is about search results");
    let cmds = m.handle(Event::ShowTab(Tab::Search));
    assert_eq!(ids(&m), ["h0", "h1", "h2"]);
    assert_eq!(m.status(), &Status::Offline);
    assert!(!loads_recent(&cmds));
    assert!(matches!(&cmds[..], [Command::LoadThumbnails { items }] if items.len() == 3));
}

#[test]
fn coming_back_to_a_running_search_says_searching() {
    let mut m = opened(cfg(), None);
    m.handle(Event::QueryChanged("cat".into()));
    m.handle(Event::ShowTab(Tab::Recent));
    // Target changes re-search right away even from another tab.
    let cmds = m.handle(Event::SetTarget(TargetApp::Discord));
    assert!(matches!(&cmds[..], [Command::Search { debounce_ms: 0, .. }]));
    assert_ne!(m.status(), &Status::Searching);
    m.handle(Event::ShowTab(Tab::Search));
    assert_eq!(m.status(), &Status::Searching);
}

#[test]
fn query_routes_to_the_tab_text_field() {
    let mut m = opened(cfg(), None);
    m.handle(Event::QueryChanged("cat".into()));
    m.handle(Event::ShowTab(Tab::Recent));
    assert_eq!(m.query(), "");
    let g = m.generation();
    assert!(m.handle(Event::QueryChanged("x".into())).is_empty(), "no field here");
    assert_eq!(m.generation(), g);
    m.handle(Event::ShowTab(Tab::Packs));
    m.handle(Event::QueryChanged("dog".into()));
    assert_eq!(m.query(), "dog");
    assert_eq!(m.search_query(), "cat");
    m.handle(Event::ShowTab(Tab::Search));
    assert_eq!(m.query(), "cat");
}

// -- other Normal keys ----------------------------------------------------------------------------

#[test]
fn yank_copies_like_enter() {
    let mut m = grid(3);
    m.handle(Event::SetTarget(TargetApp::Telegram));
    keys(&mut m, "l");
    let cmds = keys(&mut m, "y");
    assert!(
        matches!(&cmds[..], [Command::Copy { item, sticker: false, target: TargetApp::Telegram }] if item.id == "1")
    );
    let cmds = m.handle(Event::CopyFinished {
        item_id: "1".into(),
        warning: None,
    });
    assert_eq!(cmds, [Command::Hide]);

    for (k, sticker) in [
        (NormalKey::Char('Y'), true),
        (NormalKey::Enter { shift: false }, false),
        (NormalKey::Enter { shift: true }, true),
    ] {
        let mut m = grid(3);
        m.handle(Event::SetTarget(TargetApp::Telegram));
        let cmds = nk(&mut m, k);
        assert!(
            matches!(&cmds[..], [Command::Copy { sticker: s, .. }] if *s == sticker),
            "{k:?}: {cmds:?}"
        );
    }
}

#[test]
fn target_and_sticker_keys() {
    let mut m = grid(2);
    keys(&mut m, "t");
    assert_eq!(m.target(), TargetApp::Telegram);
    assert_eq!(m.target_source(), &TargetSource::Chosen);
    keys(&mut m, "TT");
    assert_eq!(m.target(), TargetApp::Slack);
    assert!(!m.sticker());
    keys(&mut m, "s");
    assert!(m.sticker());
}

#[test]
fn gx_opens_the_selected_item_in_the_browser() {
    let mut m = grid(3);
    keys(&mut m, "l");
    assert_eq!(keys(&mut m, "gx"), [Command::OpenInBrowser { item_id: "1".into() }]);
    let mut m = grid(0);
    assert!(keys(&mut m, "gx").is_empty());
    assert_eq!(m.status(), &Status::NothingSelected);
}

#[test]
fn help_toggles_and_closes_first() {
    let mut m = grid(5);
    keys(&mut m, "?");
    assert!(m.help_visible());
    keys(&mut m, "l");
    assert_eq!(m.selected(), Some(1), "keys still work under the help");
    assert!(nk(&mut m, NormalKey::Escape).is_empty());
    assert!(!m.help_visible());
    assert!(m.is_visible());
    keys(&mut m, "?");
    assert!(keys(&mut m, "q").is_empty());
    assert!(!m.help_visible());
    keys(&mut m, "??");
    assert!(!m.help_visible());
    m.handle(Event::ToggleHelp);
    assert!(m.help_visible());
    assert_eq!(keys(&mut m, "qq"), [Command::Hide]);
}

#[test]
fn q_hides() {
    let mut m = grid(1);
    assert_eq!(keys(&mut m, "q"), [Command::Hide]);
    assert!(!m.is_visible());
    assert!(keys(&mut m, "j").is_empty(), "hidden: nothing happens");
}

#[test]
fn key_timeout_after_hiding_is_ignored() {
    let mut m = grid(3);
    let seq = timeout_seq(&key(&mut m, '3'));
    m.handle(Event::Hide);
    assert!(m.handle(Event::KeyTimeout { seq }).is_empty());
    assert_eq!(m.tab(), Tab::MostUsed);
}

// -- Most used, Favorites ---------------------------------------------------------------------------

#[test]
fn most_used_loads_on_every_show() {
    let (mut m, cmds) = open_with(ModelConfig {
        start_tab: Some(Tab::MostUsed),
        ..cfg()
    });
    let g1 = most_used_gen(&cmds).unwrap();
    assert_eq!(m.status(), &Status::Loading);
    assert_eq!(m.footer().level, Level::Busy);
    assert_eq!(m.footer().status, "Loading...");
    assert!(m
        .handle(Event::MostUsedLoaded {
            generation: g1 + 1,
            outcome: Ok(items(3)),
        })
        .is_empty());
    assert!(m.entries().is_empty(), "unknown generation dropped");
    let cmds = m.handle(Event::MostUsedLoaded {
        generation: g1,
        outcome: Ok(items(3)),
    });
    assert!(matches!(&cmds[..], [Command::LoadThumbnails { items }] if items.len() == 3));
    assert_eq!(ids(&m), ["0", "1", "2"]);
    assert_eq!(m.selected(), Some(0));
    assert_eq!(m.status(), &Status::Idle);
    assert!(m
        .handle(Event::MostUsedLoaded {
            generation: g1,
            outcome: Ok(items(1)),
        })
        .is_empty());
    assert_eq!(m.entries().len(), 3, "a second answer to the same request is dropped");

    // Shown again: the last list right away, and a fresh request.
    m.handle(Event::ShowTab(Tab::Recent));
    let cmds = m.handle(Event::ShowTab(Tab::MostUsed));
    let g2 = most_used_gen(&cmds).unwrap();
    assert!(g2 > g1);
    assert_eq!(ids(&m), ["0", "1", "2"]);
    assert_eq!(m.status(), &Status::Idle);
    m.handle(Event::MostUsedLoaded {
        generation: g2,
        outcome: Err("server unreachable".into()),
    });
    assert_eq!(m.footer().status, "Could not load most used items: server unreachable");
    assert_eq!(m.footer().level, Level::Error);
    assert_eq!(m.entries().len(), 3, "kept on error");
}

#[test]
fn list_requests_in_flight_are_not_repeated_within_an_opening() {
    let (mut m, cmds) = open_with(ModelConfig {
        start_tab: Some(Tab::MostUsed),
        ..cfg()
    });
    let g1 = most_used_gen(&cmds).unwrap();
    m.handle(Event::ShowTab(Tab::Recent));
    let cmds = m.handle(Event::ShowTab(Tab::MostUsed));
    assert_eq!(most_used_gen(&cmds), None, "still waiting for {g1}");
    // A new opening asks again; the old answer is then stale.
    m.handle(Event::Hide);
    let cmds = m.handle(Event::Open(OpenInfo::default()));
    let g2 = most_used_gen(&cmds).unwrap();
    assert_ne!(g1, g2);
    m.handle(Event::MostUsedLoaded {
        generation: g1,
        outcome: Ok(items(2)),
    });
    assert!(m.entries().is_empty());
    m.handle(Event::MostUsedLoaded {
        generation: g2,
        outcome: Ok(vec![]),
    });
    assert_eq!(m.status(), &Status::NoMostUsed);
    assert_eq!(m.footer().status, "Nothing used yet");
}

#[test]
fn list_answers_for_hidden_tabs_are_kept_for_later() {
    let (mut m, cmds) = open_with(ModelConfig {
        start_tab: Some(Tab::Favorites),
        ..cfg()
    });
    let g = favorites_gen(&cmds).unwrap();
    m.handle(Event::ShowTab(Tab::Search));
    m.handle(Event::RecentLoaded(items(1)));
    let rev = m.entries_revision();
    assert!(m
        .handle(Event::FavoritesLoaded {
            generation: g,
            outcome: Ok(items(4)),
        })
        .is_empty());
    assert_eq!(m.entries_revision(), rev);
    assert_eq!(ids(&m), ["0"]);
    m.handle(Event::ShowTab(Tab::Favorites));
    assert_eq!(m.entries().len(), 4);
}

#[test]
fn list_answers_keep_a_message_the_user_caused() {
    let mut m = grid(2);
    m.handle(Event::ToggleFavorite);
    m.handle(Event::FavoriteToggled {
        item_id: "0".into(),
        outcome: Ok(true),
    });
    m.handle(Event::RecentLoaded(items(3)));
    m.handle(Event::ShowTab(Tab::Recent));
    assert_eq!(m.status(), &Status::Idle, "a tab switch starts fresh");
    m.handle(Event::ToggleFavorite);
    m.handle(Event::FavoriteToggled {
        item_id: "0".into(),
        outcome: Ok(true),
    });
    m.handle(Event::RecentLoaded(items(3)));
    assert_eq!(m.footer().status, "Added to favorites");
}

#[test]
fn favorite_toggle_round_trip() {
    let (mut m, cmds) = open_with(ModelConfig {
        start_tab: Some(Tab::Favorites),
        ..cfg()
    });
    let generation = favorites_gen(&cmds).unwrap();
    assert_eq!(m.status(), &Status::Loading);
    m.handle(Event::FavoritesLoaded {
        generation,
        outcome: Ok(items(3)),
    });
    keys(&mut m, "l");
    assert_eq!(keys(&mut m, "f"), [Command::ToggleFavorite { item_id: "1".into() }]);
    assert!(m
        .handle(Event::FavoriteToggled {
            item_id: "1".into(),
            outcome: Ok(false),
        })
        .is_empty());
    assert_eq!(ids(&m), ["0", "2"], "unliked: gone from Favorites");
    assert_eq!(m.selected_entry().unwrap().item.id, "2", "the next one is selected");
    assert_eq!(m.footer().status, "Removed from favorites");
    assert_eq!(m.footer().level, Level::Info);

    keys(&mut m, "l");
    m.handle(Event::FavoriteToggled {
        item_id: "2".into(),
        outcome: Ok(false),
    });
    assert_eq!(ids(&m), ["0"]);
    assert_eq!(m.selected(), Some(0), "removing the last one selects the new last");

    m.handle(Event::FavoriteToggled {
        item_id: "9".into(),
        outcome: Ok(true),
    });
    assert_eq!(m.footer().status, "Added to favorites");
    m.handle(Event::FavoriteToggled {
        item_id: "0".into(),
        outcome: Err("offline".into()),
    });
    assert_eq!(m.footer().status, "Favorite failed: offline");
    assert_eq!(m.footer().level, Level::Error);
    assert_eq!(ids(&m), ["0"]);

    m.handle(Event::FavoriteToggled {
        item_id: "0".into(),
        outcome: Ok(false),
    });
    assert!(m.entries().is_empty());
    assert_eq!(m.selected(), None);
    assert!(keys(&mut m, "f").is_empty());
    assert_eq!(m.status(), &Status::NothingSelected);
    assert_eq!(Status::NoFavorites.text(), "No favorites yet");
}

#[test]
fn unliking_elsewhere_drops_the_item_from_favorites() {
    let (mut m, cmds) = open_with(ModelConfig {
        start_tab: Some(Tab::Favorites),
        ..cfg()
    });
    m.handle(Event::FavoritesLoaded {
        generation: favorites_gen(&cmds).unwrap(),
        outcome: Ok(items(3)),
    });
    m.handle(Event::RecentLoaded(items(3)));
    m.handle(Event::ShowTab(Tab::Search));
    // Ctrl+D in Insert mode.
    assert_eq!(
        m.handle(Event::ToggleFavorite),
        [Command::ToggleFavorite { item_id: "0".into() }]
    );
    m.handle(Event::FavoriteToggled {
        item_id: "0".into(),
        outcome: Ok(false),
    });
    assert_eq!(ids(&m), ["0", "1", "2"], "Search is not Favorites");
    m.handle(Event::ShowTab(Tab::Favorites));
    assert_eq!(ids(&m), ["1", "2"]);
}

// -- Packs ------------------------------------------------------------------------------------------------

#[test]
fn pack_list_loads_filters_and_shows_covers() {
    let (mut m, cmds) = open_with(ModelConfig {
        start_tab: Some(Tab::Packs),
        ..cfg()
    });
    assert_eq!(m.view(), View::Packs);
    assert_eq!(m.status(), &Status::Loading);
    let generation = packs_gen(&cmds).unwrap();
    let cmds = m.handle(Event::PacksLoaded {
        generation,
        outcome: Ok(vec![set("s1", "Cats"), set("s2", "Dogs"), set("s3", "Cool cats")]),
    });
    assert!(matches!(&cmds[..], [Command::LoadPackCovers { packs }] if packs.len() == 3));
    assert_eq!(pack_names(&m), ["Cats", "Dogs", "Cool cats"]);
    assert_eq!(m.selected(), Some(0));
    assert_eq!(m.selected_pack().unwrap().id, "s1");
    assert_eq!(m.selected_entry(), None);
    assert!(m.entries().is_empty());
    assert_eq!(m.title(), "Packs");
    assert_eq!(m.status(), &Status::Idle);

    let cmds = m.handle(Event::QueryChanged("CAT".into()));
    assert_eq!(pack_names(&m), ["Cats", "Cool cats"]);
    assert!(matches!(&cmds[..], [Command::LoadPackCovers { packs }] if packs.len() == 2));
    assert_eq!(m.query(), "CAT");
    assert_eq!(m.search_query(), "");
    m.handle(Event::QueryChanged("cool-c".into()));
    assert_eq!(pack_names(&m), ["Cool cats"], "the slug matches too");
    assert!(m.handle(Event::QueryChanged("zzz".into())).is_empty());
    assert!(m.packs().is_empty());
    assert_eq!(m.selected(), None);
    assert_eq!(m.status(), &Status::NoResults { query: "zzz".into() });
    m.handle(Event::QueryChanged(String::new()));
    assert_eq!(m.packs().len(), 3);
}

#[test]
fn no_packs_and_errors() {
    let (mut m, cmds) = open_with(ModelConfig {
        start_tab: Some(Tab::Packs),
        ..cfg()
    });
    m.handle(Event::PacksLoaded {
        generation: packs_gen(&cmds).unwrap(),
        outcome: Ok(vec![]),
    });
    assert_eq!(m.footer().status, "No packs yet");
    m.handle(Event::ShowTab(Tab::Recent));
    let cmds = m.handle(Event::ShowTab(Tab::Packs));
    m.handle(Event::PacksLoaded {
        generation: packs_gen(&cmds).unwrap(),
        outcome: Err("401".into()),
    });
    assert_eq!(m.footer().status, "Could not load packs: 401");
}

#[test]
fn opening_a_pack_and_filtering_its_items() {
    let mut m = packs_model(cfg());
    m.handle(Event::QueryChanged("cat".into()));
    m.handle(Event::Navigate(Nav::Right));
    let cmds = m.handle(Event::CopySelected { toggle_sticker: false });
    let (set_id, generation) = pack_items_gen(&cmds).unwrap();
    assert_eq!(set_id, "s3");
    assert_eq!(m.view(), View::Items);
    assert_eq!(m.title(), "Packs / Cool cats");
    assert_eq!(m.open_pack().unwrap().id, "s3");
    assert_eq!(m.query(), "", "the item filter starts empty");
    assert_eq!(m.mode(), Mode::Insert, "Enter from the filter keeps typing");
    assert_eq!(m.status(), &Status::Loading);

    let answer = |set_id: &str, generation| Event::PackItemsLoaded {
        set_id: set_id.into(),
        generation,
        outcome: Ok(vec![
            pack_item("a", "grumpy", &["angry"], &["😾"], &[]),
            pack_item("b", "happy", &["smile"], &[], &["mood"]),
        ]),
    };
    assert!(m.handle(answer("s1", generation)).is_empty(), "another pack");
    assert!(m.handle(answer("s3", generation + 1)).is_empty(), "another request");
    assert!(m.entries().is_empty());
    let cmds = m.handle(answer("s3", generation));
    assert!(matches!(&cmds[..], [Command::LoadThumbnails { items }] if items.len() == 2));
    assert_eq!(ids(&m), ["a", "b"]);
    assert_eq!(m.status(), &Status::Idle);

    for (filter, want) in [
        ("smi", vec!["b"]),
        ("😾", vec!["a"]),
        ("GRUMPY", vec!["a"]),
        ("mood happy", vec!["b"]),
        ("", vec!["a", "b"]),
    ] {
        m.handle(Event::QueryChanged(filter.into()));
        assert_eq!(ids(&m), want, "{filter}");
    }
    m.handle(Event::QueryChanged("nope".into()));
    assert!(m.entries().is_empty());
    assert_eq!(m.status(), &Status::NoResults { query: "nope".into() });
    m.handle(Event::QueryChanged("happy".into()));

    // Copying from a pack is the usual copy.
    let cmds = m.handle(Event::CopySelected { toggle_sticker: false });
    assert!(matches!(&cmds[..], [Command::Copy { item, .. }] if item.id == "b"));
}

#[test]
fn empty_pack_status() {
    let mut m = packs_model(cfg());
    let cmds = m.handle(Event::CopyAt {
        index: 1,
        toggle_sticker: false,
    });
    let (set_id, generation) = pack_items_gen(&cmds).unwrap();
    assert_eq!(set_id, "s2");
    m.handle(Event::PackItemsLoaded {
        set_id,
        generation,
        outcome: Ok(vec![]),
    });
    assert_eq!(m.footer().status, "This pack is empty");
}

#[test]
fn back_from_a_pack_restores_the_list() {
    for back in [NormalKey::Char('-'), NormalKey::Backspace, NormalKey::Escape] {
        let mut m = opened_pack(cfg());
        m.handle(Event::Escape);
        assert_eq!(m.mode(), Mode::Normal);
        let cmds = nk(&mut m, back);
        assert_eq!(m.view(), View::Packs, "{back:?}");
        assert!(m.is_visible());
        assert_eq!(m.title(), "Packs");
        assert_eq!(m.query(), "cat", "the list filter is back");
        assert_eq!(pack_names(&m), ["Cats", "Cool cats"]);
        assert_eq!(m.selected_pack().unwrap().id, "s3", "on the pack we came from");
        assert!(matches!(&cmds[..], [Command::LoadPackCovers { .. }]));
        assert_eq!(m.mode(), Mode::Normal);
        // On the list itself `-` and Backspace do nothing; Escape hides.
        assert!(keys(&mut m, "-").is_empty());
        assert!(nk(&mut m, NormalKey::Backspace).is_empty());
        assert_eq!(nk(&mut m, NormalKey::Escape), [Command::Hide]);
    }
}

#[test]
fn pack_list_in_normal_mode() {
    let mut m = packs_model(cfg());
    m.handle(Event::Escape);
    keys(&mut m, "l");
    assert_eq!(m.selected_pack().unwrap().id, "s2", "l moves right");
    keys(&mut m, "G");
    assert_eq!(m.selected_pack().unwrap().id, "s3");
    assert!(keys(&mut m, "yf").is_empty(), "no item here");
    let cmds = keys(&mut m, "o");
    assert_eq!(pack_items_gen(&cmds).unwrap().0, "s3");
    assert_eq!(m.mode(), Mode::Normal, "o keeps Normal");
    m.handle(Event::PackBack);
    assert_eq!(m.view(), View::Packs);
    let cmds = nk(&mut m, NormalKey::Enter { shift: false });
    assert_eq!(pack_items_gen(&cmds).unwrap().0, "s3");
    // Pointer: hover selects, a click opens.
    m.handle(Event::PackBack);
    m.handle(Event::Select(0));
    assert_eq!(m.selected_pack().unwrap().id, "s1");
    assert!(m.handle(Event::Select(7)).is_empty());
    assert_eq!(m.selected(), Some(0));
    let cmds = m.handle(Event::CopyAt {
        index: 1,
        toggle_sticker: false,
    });
    assert_eq!(pack_items_gen(&cmds).unwrap().0, "s2");
}

#[test]
fn slash_inside_a_pack_clears_its_filter() {
    let mut m = opened_pack(cfg());
    m.handle(Event::QueryChanged("happy".into()));
    m.handle(Event::Escape);
    keys(&mut m, "/");
    assert_eq!(m.mode(), Mode::Insert);
    assert_eq!(m.query(), "");
    assert_eq!(ids(&m), ["a", "b"]);
    assert_eq!(m.title(), "Packs / Cool cats");
}

#[test]
fn opened_pack_survives_tab_switches_and_refreshes() {
    let mut m = opened_pack(cfg());
    m.handle(Event::Navigate(Nav::Right));
    m.handle(Event::ShowTab(Tab::Recent));
    assert!(m.entries().is_empty());
    let cmds = m.handle(Event::ShowTab(Tab::Packs));
    assert_eq!(m.title(), "Packs / Cool cats");
    assert!(packs_gen(&cmds).is_some());
    let (set_id, generation) = pack_items_gen(&cmds).unwrap();
    assert_eq!(set_id, "s3");
    assert_eq!(m.selected_entry().unwrap().item.id, "b", "selection remembered");
    m.handle(Event::PackItemsLoaded {
        set_id,
        generation,
        outcome: Ok(vec![item("c"), item("b")]),
    });
    assert_eq!(m.selected_entry().unwrap().item.id, "b", "and kept by the refresh");
    // A new opening starts on the list, filter cleared.
    m.handle(Event::Hide);
    m.handle(Event::Open(OpenInfo::default()));
    assert_eq!(m.view(), View::Packs);
    assert_eq!(m.query(), "");
    assert_eq!(m.packs().len(), 3);
}

#[test]
fn pack_answers_after_leaving_the_pack_are_dropped() {
    let mut m = packs_model(cfg());
    let cmds = m.handle(Event::CopySelected { toggle_sticker: false });
    let (set_id, generation) = pack_items_gen(&cmds).unwrap();
    m.handle(Event::PackBack);
    assert!(m
        .handle(Event::PackItemsLoaded {
            set_id,
            generation,
            outcome: Ok(items(2)),
        })
        .is_empty());
    assert!(m.entries().is_empty());
    assert_eq!(m.view(), View::Packs);
    // Back does nothing outside a pack or the Packs tab.
    assert!(m.handle(Event::PackBack).is_empty());
    m.handle(Event::ShowTab(Tab::Recent));
    assert!(m.handle(Event::PackBack).is_empty());
}

// -- Me, Settings ---------------------------------------------------------------------------------------

#[test]
fn page_keys_are_forwarded_in_normal_mode() {
    let mut m = open_on(Tab::Me);
    assert_eq!(m.view(), View::Page);
    let page = |k| vec![Command::Page(k)];
    assert_eq!(keys(&mut m, "j"), page(PageKey::Down));
    assert_eq!(keys(&mut m, "k"), page(PageKey::Up));
    assert_eq!(keys(&mut m, "h"), page(PageKey::Left));
    assert_eq!(keys(&mut m, "l"), page(PageKey::Right));
    assert_eq!(nk(&mut m, NormalKey::Nav(Nav::Down)), page(PageKey::Down));
    assert_eq!(nk(&mut m, NormalKey::Space), page(PageKey::Activate));
    assert_eq!(nk(&mut m, NormalKey::Enter { shift: false }), page(PageKey::Activate));
    assert_eq!(keys(&mut m, "i"), page(PageKey::Edit));
    assert_eq!(m.mode(), Mode::Normal, "Insert only once the shell focused a field");
    assert!(keys(&mut m, "ayfGgg0$").is_empty());
    assert_eq!(
        keys(&mut m, "/"),
        [Command::LoadRecent { limit: 5 }],
        "/ leaves for Search"
    );
    assert_eq!((m.tab(), m.mode()), (Tab::Search, Mode::Insert));

    let mut m = open_on(Tab::Settings);
    assert!(keys(&mut m, "ayfG0$").is_empty());
    assert!(nk(&mut m, NormalKey::Nav(Nav::Home)).is_empty());
    assert!(m.handle(Event::Navigate(Nav::Down)).is_empty());
    assert_eq!(m.status(), &Status::Idle);
}

#[test]
fn page_fields_take_insert_mode_and_escape_leaves_it() {
    let mut m = open_on(Tab::Settings);
    keys(&mut m, "i");
    m.handle(Event::TextFocus(true));
    assert_eq!(m.mode(), Mode::Insert);
    assert!(keys(&mut m, "jq").is_empty(), "typed into the field");
    assert!(m.handle(Event::Escape).is_empty());
    assert_eq!(m.mode(), Mode::Normal);
    // The shell then takes the focus from the field.
    m.handle(Event::TextFocus(false));
    assert_eq!(m.mode(), Mode::Normal);
    m.handle(Event::TextFocus(true));
    m.handle(Event::TextFocus(false));
    assert_eq!(m.mode(), Mode::Normal, "a click elsewhere");
}

#[test]
fn global_keys_work_on_pages() {
    let mut m = open_on(Tab::Me);
    keys(&mut m, "?");
    assert!(m.help_visible());
    nk(&mut m, NormalKey::Escape);
    assert!(!m.help_visible());
    keys(&mut m, "gt");
    assert_eq!(m.tab(), Tab::Settings);
    keys(&mut m, "t");
    assert_eq!(m.target(), TargetApp::Telegram);
    assert_eq!(keys(&mut m, "q"), [Command::Hide]);
    let mut m = open_on(Tab::Me);
    assert_eq!(nk(&mut m, NormalKey::Escape), [Command::Hide]);
}

// -- modal = false --------------------------------------------------------------------------------------

#[test]
fn without_modes_the_field_keeps_the_keys() {
    let (mut m, _) = open_with(non_modal());
    assert_eq!(m.mode(), Mode::Insert);
    let cmds = m.handle(Event::CycleTab { backwards: false });
    assert_eq!((m.tab(), m.mode()), (Tab::Recent, Mode::Insert));
    assert!(loads_recent(&cmds));
    let cmds = m.handle(Event::ShowTab(Tab::Favorites));
    m.handle(Event::FavoritesLoaded {
        generation: favorites_gen(&cmds).unwrap(),
        outcome: Ok(items(2)),
    });
    m.handle(Event::Navigate(Nav::Right));
    assert_eq!(
        m.handle(Event::ToggleFavorite),
        [Command::ToggleFavorite { item_id: "1".into() }]
    );
    m.handle(Event::ShowTab(Tab::Me));
    assert_eq!(m.mode(), Mode::Insert);
    assert_eq!(m.handle(Event::Escape), [Command::Hide]);
}

#[test]
fn without_modes_escape_leaves_a_pack_then_hides() {
    let mut m = opened_pack(non_modal());
    assert_eq!(m.view(), View::Items);
    let cmds = m.handle(Event::Escape);
    assert!(matches!(&cmds[..], [Command::LoadPackCovers { .. }]));
    assert_eq!(m.view(), View::Packs);
    assert!(m.is_visible());
    assert_eq!(m.handle(Event::Escape), [Command::Hide]);
}

#[test]
fn without_modes_a_start_tab_still_applies() {
    let (m, cmds) = open_with(ModelConfig {
        start_tab: Some(Tab::Favorites),
        ..non_modal()
    });
    assert_eq!((m.tab(), m.mode()), (Tab::Favorites, Mode::Insert));
    assert!(favorites_gen(&cmds).is_some());
}

// -- live settings, account changes ---------------------------------------------------------------------

#[test]
fn recent_tab_says_nothing_used_without_a_search_hint() {
    let mut m = open_on(Tab::Recent);
    m.handle(Event::RecentLoaded(vec![]));
    assert_eq!(m.status(), &Status::NothingUsed);
    assert_eq!(m.footer().status, "Nothing used yet");
    m.handle(Event::ShowTab(Tab::Search));
    assert_eq!(m.status(), &Status::NoRecent, "Search still invites typing");
}

#[test]
fn configure_applies_new_settings_at_once() {
    let mut m = grid(10);
    let mut new = ModelConfig {
        columns: 5,
        modal: false,
        prefer_stickers: true,
        default_target: TargetApp::Telegram,
        hide_on_copy: false,
        start_tab: Some(Tab::Favorites),
        ..cfg()
    };
    assert!(m.handle(Event::Configure(new.clone())).is_empty());
    assert_eq!(m.config(), &new);
    assert_eq!(m.mode(), Mode::Insert, "no modes: the keys go to the fields");
    assert_eq!(m.columns(), 5);
    m.handle(Event::Navigate(Nav::Down));
    assert_eq!(m.selected(), Some(5));
    assert_eq!(m.target(), TargetApp::Telegram, "the default target, nothing detected");
    assert!(m.sticker());
    assert_eq!(m.tab(), Tab::MostUsed, "the start tab waits for the next opening");

    // Modes back on: the tab's own mode; a target picked by hand stays.
    m.handle(Event::SetTarget(TargetApp::Discord));
    new.modal = true;
    new.default_target = TargetApp::Slack;
    m.handle(Event::Configure(new.clone()));
    assert_eq!(m.mode(), Mode::Normal);
    assert_eq!(m.target(), TargetApp::Discord);
    // An unchanged preference keeps a sticker toggle.
    m.handle(Event::ToggleSticker);
    m.handle(Event::Configure(new.clone()));
    assert!(!m.sticker());

    m.handle(Event::Hide);
    m.handle(Event::Open(OpenInfo::default()));
    assert_eq!((m.tab(), m.mode()), (Tab::Favorites, Mode::Normal));
    assert_eq!(m.target(), TargetApp::Slack);
}

#[test]
fn reload_forgets_the_old_accounts_lists() {
    let (mut m, cmds) = open_with(ModelConfig {
        start_tab: Some(Tab::Favorites),
        ..cfg()
    });
    let old = favorites_gen(&cmds).unwrap();
    m.handle(Event::FavoritesLoaded {
        generation: old,
        outcome: Ok(items(3)),
    });
    let cmds = m.handle(Event::ShowTab(Tab::Packs));
    m.handle(Event::PacksLoaded {
        generation: packs_gen(&cmds).unwrap(),
        outcome: Ok(vec![set("s1", "Cats")]),
    });
    m.handle(Event::CopySelected { toggle_sticker: false });
    assert!(m.open_pack().is_some());
    m.handle(Event::ShowTab(Tab::Favorites));

    let cmds = m.handle(Event::Reload);
    let new = favorites_gen(&cmds).expect("the shown tab is fetched again");
    assert!(new > old);
    assert!(m.entries().is_empty(), "nothing of the old account is shown");
    assert_eq!(m.status(), &Status::Loading);
    m.handle(Event::FavoritesLoaded {
        generation: new,
        outcome: Ok(items(1)),
    });
    assert_eq!(ids(&m), ["0"]);

    let cmds = m.handle(Event::ShowTab(Tab::Packs));
    assert!(packs_gen(&cmds).is_some());
    assert_eq!(m.open_pack(), None, "the opened pack is closed");
    assert!(m.packs().is_empty());
}

#[test]
fn reload_searches_the_query_again() {
    let mut m = with_results(3);
    let before = m.generation();
    let cmds = m.handle(Event::Reload);
    assert!(has(&cmds, &Command::CancelSearch));
    let (query, _, generation, debounce) = super::tests::search_of(&cmds).unwrap();
    assert_eq!((query.as_str(), debounce), ("cat", 0));
    assert!(generation > before);
    assert_eq!(m.status(), &Status::Searching);
    // An answer for the old account is dropped.
    m.handle(Event::Results {
        generation: before,
        outcome: Ok(Results {
            hits: hits(2),
            offline: false,
        }),
    });
    assert_eq!(m.status(), &Status::Searching);

    let mut hidden = with_results(2);
    hidden.handle(Event::Hide);
    assert_eq!(hidden.handle(Event::Reload), [Command::CancelSearch]);
}
