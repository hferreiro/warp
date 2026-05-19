use super::*;

// Pre-order traversal correctness for the descendant walker is exercised in
// `app/src/ai/blocklist/orchestration_topology_tests.rs`. These tests stay
// focused on the pill bar's own dispatch behavior.

#[test]
fn navigation_action_for_child_pill_reveals_existing_child_pane() {
    let conversation_id = AIConversationId::new();

    assert!(matches!(
        navigation_action_for_pill(PillKind::Child, conversation_id),
        TerminalAction::RevealChildAgent {
            conversation_id: actual_id,
        } if actual_id == conversation_id
    ));
}

#[test]
fn navigation_action_for_orchestrator_pill_switches_in_place() {
    let conversation_id = AIConversationId::new();

    assert!(matches!(
        navigation_action_for_pill(PillKind::Orchestrator, conversation_id),
        TerminalAction::SwitchAgentViewToConversation {
            conversation_id: actual_id,
        } if actual_id == conversation_id
    ));
}

#[test]
fn pill_status_sort_key_orders_attention_then_in_progress_then_done() {
    // Pinned + unpinned pill sections are both sorted by this key.
    // Lower values render closer to the start of their section, so
    // attention-needing pills bubble up and finished pills sink down.
    // Cancelled and Success deliberately share the same key so the
    // trailing "done" section is ordered by recency rather than by
    // completion type — see `pill_done_recency_key` and its tests.
    let blocked = ConversationStatus::Blocked {
        blocked_action: String::new(),
    };
    let error = ConversationStatus::Error;
    let in_progress = ConversationStatus::InProgress;
    let cancelled = ConversationStatus::Cancelled;
    let success = ConversationStatus::Success;

    let blocked_key = pill_status_sort_key(Some(&blocked));
    let error_key = pill_status_sort_key(Some(&error));
    let in_progress_key = pill_status_sort_key(Some(&in_progress));
    let cancelled_key = pill_status_sort_key(Some(&cancelled));
    let success_key = pill_status_sort_key(Some(&success));

    assert!(blocked_key < error_key);
    assert!(error_key < in_progress_key);
    assert!(in_progress_key < cancelled_key);
    assert_eq!(
        cancelled_key, success_key,
        "Cancelled and Success share the trailing done bucket so recency \
         (not completion type) decides their relative order"
    );
}

#[test]
fn pill_status_sort_key_treats_none_as_in_progress() {
    // Orchestrator pills carry `status = None` but are never routed
    // through the sort (they render first unconditionally). Picking the
    // same key as `InProgress` is a safety default — if a future caller
    // ever passes a `None` status through the sort path, it lands in the
    // middle bucket rather than corrupting the ordering at either end.
    assert_eq!(
        pill_status_sort_key(None),
        pill_status_sort_key(Some(&ConversationStatus::InProgress)),
    );
}

#[test]
fn pill_done_recency_key_puts_most_recent_first_and_unknown_last() {
    // Within the done bucket the sort key is
    // `pill_done_recency_key(last_modified_ms)` sorted ascending; smaller
    // values render leftmost. Larger timestamps (more recent) must
    // therefore produce smaller keys, and `None` (unknown finish time)
    // must produce a key greater than any populated finish time so it
    // lands at the trailing edge of the done section.
    let older = pill_done_recency_key(Some(1_000));
    let newer = pill_done_recency_key(Some(2_000));
    let unknown = pill_done_recency_key(None);
    assert!(newer < older, "newer finish_time must sort first");
    assert!(older < unknown, "unknown finish_time must sort last");
}

#[test]
fn sort_pills_bubbles_attention_in_progress_keeps_spawn_done_uses_recency() {
    // End-to-end check of the render-loop sort tuple. Each entry mimics
    // a `(status_key, secondary_key, spawn_index)` triple as built in
    // `View::render`. The secondary key is `pill_done_recency_key` for
    // done pills and `0` for everything else, so spawn order is the
    // tiebreaker in the non-done buckets.
    const DONE_STATUS_KEY: u8 = 3;
    let blocked = ConversationStatus::Blocked {
        blocked_action: String::new(),
    };
    let secondary_for = |status: &ConversationStatus, last_modified_ms: Option<i64>| -> i64 {
        if pill_status_sort_key(Some(status)) == DONE_STATUS_KEY {
            pill_done_recency_key(last_modified_ms)
        } else {
            0
        }
    };
    // Spawn order is the input index. Statuses + finish times:
    //   0: Success, finished at t=100  (oldest done)
    //   1: InProgress
    //   2: Blocked
    //   3: Cancelled, finished at t=300 (newest done)
    //   4: InProgress
    //   5: Error
    //   6: Success, finished at t=200  (middle done)
    let entries_input: Vec<(ConversationStatus, Option<i64>)> = vec![
        (ConversationStatus::Success, Some(100)),
        (ConversationStatus::InProgress, None),
        (blocked.clone(), None),
        (ConversationStatus::Cancelled, Some(300)),
        (ConversationStatus::InProgress, None),
        (ConversationStatus::Error, None),
        (ConversationStatus::Success, Some(200)),
    ];
    let mut sortable: Vec<(u8, i64, usize)> = entries_input
        .iter()
        .enumerate()
        .map(|(spawn_index, (status, last_modified_ms))| {
            (
                pill_status_sort_key(Some(status)),
                secondary_for(status, *last_modified_ms),
                spawn_index,
            )
        })
        .collect();
    sortable.sort_by_key(|(k, s, idx)| (*k, *s, *idx));
    let spawn_order: Vec<usize> = sortable.iter().map(|(_, _, idx)| *idx).collect();
    // Expected order:
    //   - Blocked (spawn 2)
    //   - Error (spawn 5)
    //   - InProgress in spawn order (1, then 4)
    //   - Done bucket sorted by recency: newest=Cancelled@300 (spawn 3),
    //     then Success@200 (spawn 6), then Success@100 (spawn 0).
    assert_eq!(spawn_order, vec![2, 5, 1, 4, 3, 6, 0]);
}

#[test]
fn sort_pills_is_stable_within_in_progress_bucket() {
    // Two `InProgress` pills should always come out in spawn order, even
    // when the input is in reverse order. This guards against future
    // changes that might collect pills out of spawn order — the explicit
    // `spawn_index` tiebreaker keeps the result deterministic regardless.
    let in_progress_key = pill_status_sort_key(Some(&ConversationStatus::InProgress));
    let mut entries: Vec<(u8, i64, usize)> = vec![(in_progress_key, 0, 7), (in_progress_key, 0, 3)];
    entries.sort_by_key(|(key, secondary, spawn_index)| (*key, *secondary, *spawn_index));
    let spawn_order: Vec<usize> = entries.iter().map(|(_, _, idx)| *idx).collect();
    assert_eq!(spawn_order, vec![3, 7]);
}

#[test]
fn sort_pills_done_bucket_orders_by_recency_regardless_of_completion_type() {
    // An old Cancelled pill should sink behind a freshly-finished Success
    // pill: within the done bucket only recency matters, not whether the
    // completion was successful or cancelled.
    const DONE_STATUS_KEY: u8 = 3;
    let cancelled_old: i64 = pill_done_recency_key(Some(100));
    let success_new: i64 = pill_done_recency_key(Some(500));
    let mut entries: Vec<(u8, i64, usize)> = vec![
        (DONE_STATUS_KEY, cancelled_old, 0),
        (DONE_STATUS_KEY, success_new, 1),
    ];
    entries.sort_by_key(|(key, secondary, spawn_index)| (*key, *secondary, *spawn_index));
    let spawn_order: Vec<usize> = entries.iter().map(|(_, _, idx)| *idx).collect();
    // Spawn index 1 (the newer Success) leads, spawn index 0 (the older
    // Cancelled) trails.
    assert_eq!(spawn_order, vec![1, 0]);
}
