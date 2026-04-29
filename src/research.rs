pub fn run_research(topic: &str, rounds: usize) -> String {
    let rounds = rounds.clamp(1, 50);
    let mut lines = vec![
        "# ASI Code AutoResearch".to_string(),
        format!("Topic: {}", topic),
        String::new(),
    ];
    for i in 1..=rounds {
        lines.push(format!("## Round {}", i));
        lines.push(format!("Objective: Improve {}", topic));
        lines.push(format!(
            "Proposal: Apply one bounded change and evaluate in round {}",
            i
        ));
        lines.push("Validation: Keep change only when metrics improve".to_string());
        lines.push(String::new());
    }
    lines.join("\n")
}
