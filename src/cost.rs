pub fn pricing_per_1k(model: &str) -> (f64, f64) {
    let lower = model.to_ascii_lowercase();
    if lower.contains("opus") {
        (0.015, 0.075)
    } else if lower.contains("sonnet") {
        (0.003, 0.015)
    } else if lower.contains("haiku") {
        (0.0008, 0.004)
    } else if lower.contains("deepseek") {
        (0.001, 0.002)
    } else if lower.contains("gpt-4.1") {
        (0.005, 0.015)
    } else {
        (0.001, 0.003)
    }
}

pub fn estimate_cost_usd(model: &str, input_tokens: usize, output_tokens: usize) -> f64 {
    let (in_per_1k, out_per_1k) = pricing_per_1k(model);
    (input_tokens as f64 / 1000.0) * in_per_1k + (output_tokens as f64 / 1000.0) * out_per_1k
}

pub fn format_usd(v: f64) -> String {
    format!("${:.6}", v)
}
