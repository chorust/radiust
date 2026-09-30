//! Bounded, shuffled discovery fan-out shared by native and Python entry points.

use futures_util::stream::{self, StreamExt};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Debug, PartialEq)]
pub enum DiscoveryOutcome<R, E> {
    Completed(Result<R, E>),
    /// The operation started and was interrupted by the shared cancellation token.
    Cancelled,
    /// The item remained queued when cancellation was observed.
    NotStarted,
    /// The shared discovery deadline expired while this operation was active.
    Timeout,
}

/// Run targets in a deterministic shuffled order with a strict worker bound.
/// Results are returned in the caller's original order so reports remain stable.
pub async fn run_shuffled<T, R, E, F, Fut>(
    targets: Vec<T>,
    workers: usize,
    seed: u64,
    cancellation: CancellationToken,
    operation: F,
) -> Vec<(T, DiscoveryOutcome<R, E>)>
where
    T: Clone + Hash + Send + Sync,
    R: Send,
    E: Send,
    F: Fn(T) -> Fut + Sync,
    Fut: Future<Output = Result<R, E>> + Send,
{
    run_shuffled_with_deadline(targets, workers, seed, cancellation, None, operation).await
}

pub async fn run_shuffled_with_deadline<T, R, E, F, Fut>(
    targets: Vec<T>,
    workers: usize,
    seed: u64,
    cancellation: CancellationToken,
    time_budget: Option<Duration>,
    operation: F,
) -> Vec<(T, DiscoveryOutcome<R, E>)>
where
    T: Clone + Hash + Send + Sync,
    R: Send,
    E: Send,
    F: Fn(T) -> Fut + Sync,
    Fut: Future<Output = Result<R, E>> + Send,
{
    if targets.is_empty() {
        return Vec::new();
    }
    let mut shuffled: Vec<(usize, T, u64)> = targets
        .into_iter()
        .enumerate()
        .map(|(index, target)| {
            let mut hasher = DefaultHasher::new();
            seed.hash(&mut hasher);
            target.hash(&mut hasher);
            (index, target, hasher.finish())
        })
        .collect();
    shuffled.sort_by_key(|(_, _, order)| *order);
    let ordered = shuffled.into_iter().map(|(index, target, _)| (index, target)).collect();
    run_ordered_with_deadline(ordered, workers, cancellation, time_budget, operation).await
}

/// Interleave source groups fairly, randomizing both source start order and
/// targets within each source before applying the same bounded worker pool.
pub async fn run_source_fair_with_deadline<T, K, S, R, E, F, Fut>(
    targets: Vec<T>,
    workers: usize,
    seed: u64,
    cancellation: CancellationToken,
    time_budget: Option<Duration>,
    source_key: S,
    operation: F,
) -> Vec<(T, DiscoveryOutcome<R, E>)>
where
    T: Clone + Hash + Send + Sync,
    K: Clone + Hash + Ord,
    S: Fn(&T) -> K + Sync,
    R: Send,
    E: Send,
    F: Fn(T) -> Fut + Sync,
    Fut: Future<Output = Result<R, E>> + Send,
{
    if targets.is_empty() {
        return Vec::new();
    }
    let mut groups: BTreeMap<K, Vec<(usize, T, u64)>> = BTreeMap::new();
    for (index, target) in targets.into_iter().enumerate() {
        let key = source_key(&target);
        let order = hash_order(seed ^ 0x9e37_79b9_7f4a_7c15, &target);
        groups.entry(key).or_default().push((index, target, order));
    }
    for group in groups.values_mut() {
        group.sort_by_key(|(_, _, order)| *order);
    }
    let mut source_order = groups.keys().cloned().collect::<Vec<_>>();
    source_order.sort_by_key(|key| hash_order(seed, key));
    let mut queues = groups
        .into_iter()
        .map(|(key, group)| {
            (
                key,
                group
                    .into_iter()
                    .map(|(index, target, _)| (index, target))
                    .collect::<VecDeque<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut ordered = Vec::new();
    loop {
        let mut found = false;
        for key in &source_order {
            if let Some(item) = queues.get_mut(key).and_then(VecDeque::pop_front) {
                ordered.push(item);
                found = true;
            }
        }
        if !found {
            break;
        }
    }
    run_ordered_with_deadline(ordered, workers, cancellation, time_budget, operation).await
}

fn hash_order<T: Hash>(seed: u64, value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    value.hash(&mut hasher);
    hasher.finish()
}

async fn run_ordered_with_deadline<T, R, E, F, Fut>(
    ordered: Vec<(usize, T)>,
    workers: usize,
    cancellation: CancellationToken,
    time_budget: Option<Duration>,
    operation: F,
) -> Vec<(T, DiscoveryOutcome<R, E>)>
where
    T: Clone + Send + Sync,
    R: Send,
    E: Send,
    F: Fn(T) -> Fut + Sync,
    Fut: Future<Output = Result<R, E>> + Send,
{
    let deadline = time_budget.map(|budget| tokio::time::Instant::now() + budget);

    let completed = stream::iter(ordered)
        .map(|(index, target)| {
            let cancellation = cancellation.clone();
            let operation = &operation;
            async move {
                let outcome = if cancellation.is_cancelled() {
                    DiscoveryOutcome::NotStarted
                } else if deadline.is_some_and(|instant| tokio::time::Instant::now() >= instant) {
                    DiscoveryOutcome::NotStarted
                } else {
                    tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => DiscoveryOutcome::Cancelled,
                        _ = async {
                            if let Some(deadline) = deadline {
                                tokio::time::sleep_until(deadline).await;
                            } else {
                                std::future::pending::<()>().await;
                            }
                        } => DiscoveryOutcome::Timeout,
                        result = operation(target.clone()) => DiscoveryOutcome::Completed(result),
                    }
                };
                (index, target, outcome)
            }
        })
        .buffer_unordered(workers.max(1))
        .collect::<Vec<_>>()
        .await;

    let mut completed = completed;
    completed.sort_by_key(|(index, _, _)| *index);
    completed.into_iter().map(|(_, target, outcome)| (target, outcome)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn shuffles_work_while_enforcing_the_worker_limit_and_stable_result_order() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let output = run_shuffled((0_u64..20).collect(), 3, 71, CancellationToken::new(), {
            let active = active.clone();
            let maximum = maximum.clone();
            move |target| {
                let active = active.clone();
                let maximum = maximum.clone();
                async move {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(3)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok::<_, ()>(target)
                }
            }
        })
        .await;

        assert_eq!(maximum.load(Ordering::SeqCst), 3);
        assert_eq!(
            output.iter().map(|(target, _)| *target).collect::<Vec<_>>(),
            (0..20).collect::<Vec<_>>()
        );
        assert!(output.iter().all(|(target, outcome)| {
            matches!(outcome, DiscoveryOutcome::Completed(Ok(value)) if value == target)
        }));
    }

    #[tokio::test]
    async fn source_fair_order_rotates_sources_and_shuffles_within_each_source() {
        let started_order = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let inputs = vec![
            "au:one".to_owned(),
            "au:two".to_owned(),
            "vn:one".to_owned(),
            "ca:one".to_owned(),
        ];
        let output = run_source_fair_with_deadline(
            inputs.clone(),
            1,
            101,
            CancellationToken::new(),
            None,
            |target: &String| target.split(':').next().unwrap().to_owned(),
            {
                let started_order = started_order.clone();
                move |target| {
                    let started_order = started_order.clone();
                    async move {
                        started_order.lock().push(target.clone());
                        Ok::<_, ()>(target)
                    }
                }
            },
        )
        .await;

        let observed = started_order.lock().clone();
        let first_cycle = observed[..3]
            .iter()
            .map(|target| target.split(':').next().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(first_cycle.len(), 3);
        assert_eq!(output.iter().map(|(target, _)| target.clone()).collect::<Vec<_>>(), inputs);
        assert_eq!(output.len(), 4);
    }

    #[tokio::test]
    async fn pre_cancelled_queue_is_reported_as_not_started() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let output =
            run_shuffled(
                vec![1, 2, 3],
                2,
                0,
                cancellation,
                |target| async move { Ok::<_, ()>(target) },
            )
            .await;
        assert!(output.iter().all(|(_, outcome)| *outcome == DiscoveryOutcome::NotStarted));
    }

    #[tokio::test(start_paused = true)]
    async fn shared_deadline_times_out_active_work_and_does_not_start_queued_work() {
        let started = Arc::new(tokio::sync::Notify::new());
        let task = tokio::spawn({
            let started = started.clone();
            async move {
                run_shuffled_with_deadline(
                    vec![1, 2, 3],
                    1,
                    13,
                    CancellationToken::new(),
                    Some(Duration::from_millis(5)),
                    move |target| {
                        let started = started.clone();
                        async move {
                            started.notify_one();
                            std::future::pending::<()>().await;
                            Ok::<_, ()>(target)
                        }
                    },
                )
                .await
            }
        });
        started.notified().await;
        tokio::time::advance(Duration::from_millis(5)).await;
        let output = task.await.unwrap();
        assert_eq!(
            output.iter().filter(|(_, outcome)| *outcome == DiscoveryOutcome::Timeout).count(),
            1
        );
        assert_eq!(
            output.iter().filter(|(_, outcome)| *outcome == DiscoveryOutcome::NotStarted).count(),
            2
        );
    }

    #[tokio::test]
    async fn cancellation_keeps_the_running_and_not_started_states_distinct() {
        let cancellation = CancellationToken::new();
        let started = Arc::new(tokio::sync::Notify::new());
        let task = tokio::spawn({
            let cancellation = cancellation.clone();
            let started = started.clone();
            async move {
                run_shuffled(vec![1, 2, 3], 1, 29, cancellation, move |target| {
                    let started = started.clone();
                    async move {
                        started.notify_one();
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        Ok::<_, ()>(target)
                    }
                })
                .await
            }
        });
        started.notified().await;
        cancellation.cancel();
        let output = task.await.unwrap();
        assert_eq!(
            output.iter().filter(|(_, outcome)| *outcome == DiscoveryOutcome::Cancelled).count(),
            1
        );
        assert_eq!(
            output.iter().filter(|(_, outcome)| *outcome == DiscoveryOutcome::NotStarted).count(),
            2
        );
    }
}
