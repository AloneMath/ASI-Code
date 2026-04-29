use super::types::{ExecutionBatch, ToolCallRequest};

pub fn partition_tool_calls(calls: Vec<ToolCallRequest>) -> Vec<ExecutionBatch> {
    let mut batches: Vec<ExecutionBatch> = Vec::new();

    for call in calls {
        let concurrency_safe = call.is_concurrency_safe();

        if concurrency_safe {
            if let Some(last) = batches.last_mut() {
                if last.concurrency_safe {
                    last.calls.push(call);
                    continue;
                }
            }
        }

        batches.push(ExecutionBatch {
            concurrency_safe,
            calls: vec![call],
        });
    }

    batches
}
