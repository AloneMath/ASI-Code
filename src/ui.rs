use crate::cost;
use crate::meta::APP_NAME;

pub struct Ui {
    color: bool,
    theme: String,
}

impl Ui {
    pub fn new(theme: &str) -> Self {
        let color = std::env::var("NO_COLOR").is_err();
        Self {
            color,
            theme: theme.to_string(),
        }
    }

    pub fn set_theme(&mut self, theme: &str) {
        self.theme = theme.to_string();
    }

    pub fn welcome_card(&self, cwd: &str) -> String {
        self.rounded_box(
            &[
                format!("✶ Welcome to {}!", APP_NAME),
                "/help for help, /status for your current setup".to_string(),
                format!("cwd: {}", cwd),
            ],
            self.warn_code(),
        )
    }

    pub fn tips_section(&self) -> String {
        [
            self.dim("Tips for getting started:"),
            "1. Ask ASI Code to scaffold a feature or inspect a repository".to_string(),
            "2. Use precise prompts for editing, commands, and debugging".to_string(),
            "3. Auto mode is already on; use /auto off only if you want manual tool control"
                .to_string(),
        ]
        .join("\n")
    }

    pub fn input_prompt(&self) -> String {
        format!("{} ", self.accent("ASI Code>"))
    }

    pub fn assistant_label(&self) -> String {
        self.accent("ASI Code")
    }

    pub fn assistant(&self, text: &str) -> String {
        format!("{}\n• {}", self.assistant_label(), text)
    }

    pub fn tool_panel(&self, name: &str, status: &str, body: &str) -> String {
        let head = self.warn(&format!("TOOL {} ({})", name, status));
        let border = self.dim("──────────────────────────────");
        format!("{}\n{}\n{}\n{}", head, border, body, border)
    }

    pub fn spinning_line(&self, elapsed_secs: u64, tokens: usize) -> String {
        format!(
            "{} {}",
            self.warn("✶ Spinning…"),
            self.dim(&format!(
                "({}s · × {} tokens · esc to interrupt)",
                elapsed_secs, tokens
            ))
        )
    }

    pub fn done_line(&self, elapsed_secs: u64, tokens: usize) -> String {
        format!(
            "{} {}",
            self.ok("✓ Completed"),
            self.dim(&format!("({}s · × {} tokens)", elapsed_secs, tokens))
        )
    }

    pub fn thinking_block(&self, text: &str) -> String {
        self.rounded_box(
            &["Thinking".to_string(), text.to_string()],
            self.accent_code(),
        )
    }

    pub fn cost_line(&self, turn_cost: f64, total_cost: f64) -> String {
        self.dim(&format!(
            "cost(turn/total)={}/{}",
            cost::format_usd(turn_cost),
            cost::format_usd(total_cost)
        ))
    }

    pub fn error(&self, text: &str) -> String {
        format!("{} {}", self.err("ERROR"), text)
    }

    pub fn info(&self, text: &str) -> String {
        format!("{} {}", self.dim("INFO"), text)
    }

    pub fn status_line(
        &self,
        provider: &str,
        model: &str,
        permission_mode: &str,
        turns: usize,
        in_tokens: usize,
        out_tokens: usize,
        total_cost: f64,
    ) -> String {
        self.dim(&format!(
            "provider={} model={} mode={} turns={} tokens(in/out)={}/{} total_cost={}",
            provider,
            model,
            permission_mode,
            turns,
            in_tokens,
            out_tokens,
            cost::format_usd(total_cost)
        ))
    }

    pub fn theme_menu(&self, current: &str) -> String {
        let items = [
            ("dark", "Dark mode"),
            ("light", "Light mode"),
            ("dark-colorblind", "Dark mode (colorblind-friendly)"),
            ("light-colorblind", "Light mode (colorblind-friendly)"),
            ("dark-ansi", "Dark mode (ANSI colors only)"),
            ("light-ansi", "Light mode (ANSI colors only)"),
        ];

        let mut lines = vec![
            "Let's get started.".to_string(),
            "".to_string(),
            "Choose the text style that looks best with your terminal".to_string(),
            "To change this later, run /theme".to_string(),
            "".to_string(),
        ];

        for (idx, (key, label)) in items.iter().enumerate() {
            let selected = if *key == current { " ✓" } else { "" };
            let prefix = if *key == current {
                self.accent("› ")
            } else {
                "  ".to_string()
            };
            lines.push(format!("{}{}. {}{}", prefix, idx + 1, label, selected));
        }

        lines.push("".to_string());
        lines.push(self.theme_preview());
        lines.push(format!(
            "Syntax theme: {} (ctrl+t to disable)",
            "Monokai Extended"
        ));

        lines.join("\n")
    }

    pub fn theme_preview(&self) -> String {
        [
            self.dim("------------------------------------------------------------"),
            "1  function greet() {".to_string(),
            self.diff_removed("2  - console.log(\"Hello, World!\");"),
            self.diff_added("2  + console.log(\"Hello, Claude!\");"),
            "3  }".to_string(),
            self.dim("------------------------------------------------------------"),
        ]
        .join("\n")
    }

    fn rounded_box(&self, lines: &[String], color_code: &str) -> String {
        let width = lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0)
            + 2;
        let top = self.paint(&format!("╭{}╮", "─".repeat(width + 2)), color_code);
        let bottom = self.paint(&format!("╰{}╯", "─".repeat(width + 2)), color_code);

        let mut out = vec![top];
        for line in lines {
            let pad = width.saturating_sub(line.chars().count());
            out.push(format!(
                "{} {}{} {}",
                self.paint("│", color_code),
                line,
                " ".repeat(pad),
                self.paint("│", color_code)
            ));
        }
        out.push(bottom);
        out.join("\n")
    }

    fn accent(&self, s: &str) -> String {
        self.paint(s, self.accent_code())
    }

    fn warn(&self, s: &str) -> String {
        self.paint(s, self.warn_code())
    }

    fn ok(&self, s: &str) -> String {
        self.paint(s, self.ok_code())
    }

    fn err(&self, s: &str) -> String {
        self.paint(s, self.err_code())
    }

    fn diff_removed(&self, s: &str) -> String {
        self.paint(s, self.diff_removed_code())
    }

    fn diff_added(&self, s: &str) -> String {
        self.paint(s, self.diff_added_code())
    }

    fn dim(&self, s: &str) -> String {
        self.paint(s, self.dim_code())
    }

    fn accent_code(&self) -> &str {
        match self.theme.as_str() {
            "light" => "34",
            "dark-colorblind" => "94",
            "light-colorblind" => "94",
            "dark-ansi" => "36",
            "light-ansi" => "34",
            _ => "36",
        }
    }

    fn warn_code(&self) -> &str {
        match self.theme.as_str() {
            "light" => "35",
            "dark-colorblind" => "93",
            "light-colorblind" => "95",
            "dark-ansi" => "33",
            "light-ansi" => "33",
            _ => "33",
        }
    }

    fn ok_code(&self) -> &str {
        match self.theme.as_str() {
            "dark-colorblind" | "light-colorblind" => "96",
            _ => "32",
        }
    }

    fn err_code(&self) -> &str {
        match self.theme.as_str() {
            "dark-colorblind" | "light-colorblind" => "91",
            _ => "31",
        }
    }

    fn dim_code(&self) -> &str {
        match self.theme.as_str() {
            "light" | "light-colorblind" | "light-ansi" => "30",
            _ => "90",
        }
    }

    fn diff_removed_code(&self) -> &str {
        match self.theme.as_str() {
            "light" | "light-colorblind" => "31",
            _ => "97;41",
        }
    }

    fn diff_added_code(&self) -> &str {
        match self.theme.as_str() {
            "light" | "light-colorblind" => "32",
            _ => "30;42",
        }
    }

    fn paint(&self, s: &str, code: &str) -> String {
        if self.color {
            format!("\x1b[{}m{}\x1b[0m", code, s)
        } else {
            s.to_string()
        }
    }
}
