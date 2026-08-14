//! P2-14: Agent 运行指标 — 进程级计数器,供网关 `GET /metrics` 以 Prometheus 文本格式导出。
//!
//! 只用 `AtomicU64`,不引入指标框架:实战需要的就是"跑了多少轮/工具失败率/provider 出错率"。

use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! counters {
    ($($name:ident => $help:expr),+ $(,)?) => {
        $(pub static $name: AtomicU64 = AtomicU64::new(0);)+

        /// 导出为 Prometheus 文本格式
        pub fn render() -> String {
            let mut out = String::new();
            $(
                let metric = concat!("rustclaw_", stringify!($name)).to_lowercase();
                out.push_str(&format!("# HELP {} {}\n", metric, $help));
                out.push_str(&format!("# TYPE {} counter\n", metric));
                out.push_str(&format!("{} {}\n", metric, $name.load(Ordering::Relaxed)));
            )+
            out
        }
    };
}

counters! {
    LOOPS_STARTED => "Agent loops started",
    LOOPS_COMPLETED => "Agent loops that produced a final answer",
    LOOPS_CANCELLED => "Agent loops aborted by user cancellation",
    LOOPS_DEADLINE => "Agent loops stopped by the wall-clock budget",
    LOOPS_MAX_ITERATIONS => "Agent loops stopped by the iteration budget",
    ITERATIONS => "Agent loop iterations executed",
    TOOL_CALLS => "Tool calls executed",
    TOOL_FAILURES => "Tool calls that returned an error",
    PROVIDER_ERRORS => "LLM provider calls that failed after retries and failover",
    COMPRESSIONS => "Context compressions performed",
}

/// 计数器 +1
pub fn inc(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_render() {
        inc(&TOOL_CALLS);
        let text = render();
        assert!(text.contains("# TYPE rustclaw_tool_calls counter"));
        assert!(text.contains("rustclaw_provider_errors 0"));
        // 计数确实增长
        let before: u64 = TOOL_CALLS.load(Ordering::Relaxed);
        inc(&TOOL_CALLS);
        assert_eq!(TOOL_CALLS.load(Ordering::Relaxed), before + 1);
    }
}
