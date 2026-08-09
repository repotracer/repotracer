use crate::types::{ToolCall, ToolResult};
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use std::future::Future;
use std::time::Duration;
use tokio::time::timeout;

pub const DEFAULT_CONCURRENCY: usize = 8;
pub const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(10);

pub trait ToolExecutor {
    fn execute(&self, call: &ToolCall) -> impl Future<Output = ToolResult> + Send;
}

/// Run tool calls with bounded concurrency and per-tool timeout.
/// Results are returned in the same order as `calls`.
pub async fn execute_tools<E: ToolExecutor + Sync>(
    calls: &[ToolCall],
    executor: &E,
    concurrency: usize,
    tool_timeout: Duration,
) -> Vec<ToolResult> {
    if calls.is_empty() {
        return Vec::new();
    }

    let concurrency = concurrency.max(1);
    let mut results: Vec<Option<ToolResult>> = (0..calls.len()).map(|_| None).collect();
    let mut in_flight: FuturesUnordered<BoxFuture<'_, (usize, ToolResult)>> =
        FuturesUnordered::new();
    let mut next = 0usize;

    while next < calls.len() || !in_flight.is_empty() {
        while next < calls.len() && in_flight.len() < concurrency {
            let idx = next;
            next += 1;
            let call = &calls[idx];
            in_flight.push(Box::pin(async move {
                let res = match timeout(tool_timeout, executor.execute(call)).await {
                    Ok(r) => r,
                    Err(_) => ToolResult {
                        tool_call_id: call.id.clone(),
                        name: call.name.clone(),
                        output: format!(
                            "<system-reminder>Tool `{}` timed out after {}s.</system-reminder>",
                            call.name,
                            tool_timeout.as_secs()
                        ),
                        failed: true,
                        duration_ms: tool_timeout.as_millis() as u64,
                    },
                };
                (idx, res)
            }));
        }

        if let Some((idx, res)) = in_flight.next().await {
            results[idx] = Some(res);
        }
    }

    results.into_iter().map(|r| r.expect("filled")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    struct Sleepy {
        peak: Arc<AtomicUsize>,
        current: Arc<AtomicUsize>,
    }

    impl ToolExecutor for Sleepy {
        async fn execute(&self, call: &ToolCall) -> ToolResult {
            let cur = self.current.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(cur, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(500)).await;
            self.current.fetch_sub(1, Ordering::SeqCst);
            ToolResult {
                tool_call_id: call.id.clone(),
                name: call.name.clone(),
                output: "ok".into(),
                failed: false,
                duration_ms: 500,
            }
        }
    }

    #[tokio::test]
    async fn parallel_is_much_faster_than_sequential() {
        let peak = Arc::new(AtomicUsize::new(0));
        let current = Arc::new(AtomicUsize::new(0));
        let exec = Sleepy {
            peak: peak.clone(),
            current,
        };
        let calls: Vec<ToolCall> = (0..4)
            .map(|i| ToolCall {
                id: format!("c{i}"),
                name: "Sleep".into(),
                arguments: "{}".into(),
            })
            .collect();

        let started = Instant::now();
        let results = execute_tools(&calls, &exec, 4, Duration::from_secs(5)).await;
        let elapsed = started.elapsed();

        assert_eq!(results.len(), 4);
        assert!(results.iter().all(|r| !r.failed));
        assert!(
            elapsed.as_millis() < 1200,
            "expected parallel ~500ms, got {}ms (concurrency regressed?)",
            elapsed.as_millis()
        );
        assert!(
            peak.load(Ordering::SeqCst) >= 2,
            "expected concurrent execution, peak={}",
            peak.load(Ordering::SeqCst)
        );
        for (i, r) in results.iter().enumerate() {
            assert_eq!(r.tool_call_id, format!("c{i}"));
        }
    }
}
