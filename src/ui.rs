use crate::cost;
use crate::meta::{APP_NAME, APP_VERSION};

const CUBIST_COBALT: &str = "38;2;53;92;143";
const CUBIST_OCHRE: &str = "38;2;199;127;61";
const CUBIST_SAGE: &str = "38;2;122;143;92";
const CUBIST_TERRA: &str = "38;2;176;90;60";
const CUBIST_SLATE: &str = "38;2;107;114;128";
const CUBIST_DIFF_REMOVED: &str = "38;2;240;230;210;48;2;176;90;60";
const CUBIST_DIFF_ADDED: &str = "38;2;34;34;34;48;2;122;143;92";
const CUBIST_SPINNER_FRAMES: [&str; 6] = ["\u{25C6}", "\u{25C7}", "\u{25B2}", "\u{25B3}", "\u{25B0}", "\u{25B1}"];

// ── Aurora theme ────────────────────────────────────────────────────────
// A Claude Code / Codex-inspired look: soft truecolor gradient accents,
// star-glyph spinner, `●` tool headers with `⎿` result gutters, rounded
// cards, and quiet dim metadata. Designed to feel calm and hand-crafted
// rather than terminal-loud.
const AURORA_CORAL: &str = "38;2;255;138;101";
const AURORA_VIOLET: &str = "38;2;167;139;250";
const AURORA_TEAL: &str = "38;2;94;234;212";
const AURORA_ROSE: &str = "38;2;251;113;133";
const AURORA_MIST: &str = "38;2;138;143;158";
const AURORA_GOLD: &str = "38;2;250;204;21";
const AURORA_DIFF_REMOVED: &str = "38;2;255;205;210;48;2;103;44;52";
const AURORA_DIFF_ADDED: &str = "38;2;209;250;229;48;2;24;84;62";
const AURORA_GRAD_FROM: (u8, u8, u8) = (255, 138, 101);
const AURORA_GRAD_TO: (u8, u8, u8) = (167, 139, 250);
const AURORA_SPINNER_FRAMES: [&str; 8] =
    ["\u{273B}", "\u{273C}", "\u{273D}", "\u{2733}", "\u{2731}", "\u{2733}", "\u{273D}", "\u{273C}"];

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

    #[cfg(test)]
    pub(crate) fn new_with(color: bool, theme: &str) -> Self {
        Self {
            color,
            theme: theme.to_string(),
        }
    }

    pub fn set_theme(&mut self, theme: &str) {
        self.theme = theme.to_string();
    }

    pub fn welcome_card(&self, cwd: &str) -> String {
        if self.theme == "cubist" {
            return self.welcome_mosaic(cwd);
        }
        if self.theme == "aurora" {
            return self.welcome_aurora(cwd);
        }
        self.rounded_box(
            &[
                format!("\u{2736} Welcome to {}!", APP_NAME),
                "/help for help, /status for your current setup".to_string(),
                format!("cwd: {}", cwd),
            ],
            self.warn_code(),
        )
    }

    fn welcome_aurora(&self, cwd: &str) -> String {
        // Claude Code-style greeting: a starburst glyph, gradient wordmark,
        // then quiet metadata lines. No heavy box — whitespace is the frame.
        let star = self.paint("\u{273B}", AURORA_CORAL);
        let word = self.gradient_text(APP_NAME);
        let version = self.dim(&format!("v{}", APP_VERSION));
        let rule = self.paint(&"\u{2500}".repeat(44), AURORA_MIST);
        let hint = format!(
            "{}  {}  {}",
            self.dim("/help"),
            self.dim("/status"),
            self.dim("/theme"),
        );
        [
            format!("{} {} {}", star, word, version),
            self.dim(&format!("  {}", cwd)),
            format!("  {}", hint),
            format!("  {}", rule),
        ]
        .join("\n")
    }

    /// Paint each character of `text` with a color interpolated between the
    /// aurora gradient endpoints. Falls back to plain text when color is off.
    fn gradient_text(&self, text: &str) -> String {
        if !self.color {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len().max(1);
        let (fr, fg, fb) = AURORA_GRAD_FROM;
        let (tr, tg, tb) = AURORA_GRAD_TO;
        let mut out = String::new();
        for (i, c) in chars.iter().enumerate() {
            let t = i as f64 / (n.saturating_sub(1).max(1)) as f64;
            let r = (fr as f64 + (tr as f64 - fr as f64) * t).round() as u8;
            let g = (fg as f64 + (tg as f64 - fg as f64) * t).round() as u8;
            let b = (fb as f64 + (tb as f64 - fb as f64) * t).round() as u8;
            out.push_str(&format!("\x1b[1;38;2;{};{};{}m{}", r, g, b, c));
        }
        out.push_str("\x1b[0m");
        out
    }

    fn welcome_mosaic(&self, cwd: &str) -> String {
        let left = self.mosaic_panel_slanted(
            &[
                deconstructed_banner(APP_NAME),
                format!("v{}", APP_VERSION),
                "\u{25B0}\u{25B1} Picasso terminal \u{25B1}\u{25B0}".to_string(),
            ],
            self.warn_code(),
            false,
        );
        let right = self.mosaic_panel_slanted(
            &[
                "\u{25C6} /help".to_string(),
                "\u{25C6} /status".to_string(),
                "\u{25C6} /theme".to_string(),
            ],
            self.accent_code(),
            true,
        );
        let combined = side_by_side(&left, &right, "  ");
        let cwd_line = self.dim(&format!("\u{25C7} cwd: {}", cwd));
        format!("{}\n{}", combined, cwd_line)
    }

    fn mosaic_panel(&self, lines: &[String], color: &str) -> Vec<String> {
        self.mosaic_panel_corners(
            lines,
            color,
            ("\u{259B}", "\u{259C}", "\u{2599}", "\u{259F}"),
        )
    }

    fn mosaic_panel_slanted(&self, lines: &[String], color: &str, lean_left: bool) -> Vec<String> {
        let corners = if lean_left {
            ("\u{259B}", "\u{2572}", "\u{2572}", "\u{259F}")
        } else {
            ("\u{2571}", "\u{259C}", "\u{2599}", "\u{2571}")
        };
        self.mosaic_panel_corners(lines, color, corners)
    }

    fn mosaic_panel_corners(
        &self,
        lines: &[String],
        color: &str,
        corners: (&str, &str, &str, &str),
    ) -> Vec<String> {
        let (tl, tr, bl, br) = corners;
        let width = lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        let top = self.paint(&format!("{}{}{}", tl, "\u{2580}".repeat(width + 2), tr), color);
        let bot = self.paint(&format!("{}{}{}", bl, "\u{2584}".repeat(width + 2), br), color);
        let mut out = vec![top];
        for line in lines {
            let pad = width.saturating_sub(line.chars().count());
            out.push(format!(
                "{} {}{} {}",
                self.paint("\u{258C}", color),
                line,
                " ".repeat(pad),
                self.paint("\u{2590}", color),
            ));
        }
        out.push(bot);
        out
    }

    pub fn tips_section(&self) -> String {
        if self.theme == "cubist" {
            return [
                self.dim("\u{25C7} Tips for getting started:"),
                format!(
                    "{} Ask ASI Code to scaffold a feature or inspect a repository",
                    self.warn("\u{25B2}")
                ),
                format!(
                    "{} Use precise prompts for editing, commands, and debugging",
                    self.warn("\u{25B2}")
                ),
                format!(
                    "{} Auto mode is already on; use /auto off only if you want manual tool control",
                    self.warn("\u{25B2}")
                ),
            ]
            .join("\n");
        }
        if self.theme == "aurora" {
            return [
                self.dim("  Try:"),
                format!(
                    "  {} {}",
                    self.paint("\u{00B7}", AURORA_CORAL),
                    self.dim("\"scaffold a feature\" or \"inspect this repository\"")
                ),
                format!(
                    "  {} {}",
                    self.paint("\u{00B7}", AURORA_VIOLET),
                    self.dim("precise prompts for edits, commands, and debugging")
                ),
                format!(
                    "  {} {}",
                    self.paint("\u{00B7}", AURORA_TEAL),
                    self.dim("auto mode is on \u{2014} /auto off for manual tool control")
                ),
            ]
            .join("\n");
        }
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
        if self.theme == "cubist" {
            return format!("{} ", self.accent("\u{25B0}\u{25B0}\u{25B1}\u{25B1} ASI Code \u{25B8}"));
        }
        if self.theme == "aurora" {
            return format!("{} ", self.paint("\u{276F}", AURORA_CORAL));
        }
        format!("{} ", self.accent("ASI Code>"))
    }

    pub fn assistant_label(&self) -> String {
        if self.theme == "cubist" {
            self.accent("\u{25C6} ASI Code")
        } else if self.theme == "aurora" {
            self.paint("\u{273B}", AURORA_CORAL)
        } else {
            self.accent("ASI Code")
        }
    }

    pub fn assistant(&self, text: &str) -> String {
        if self.theme == "aurora" {
            // Claude Code style: a single star bullet, body indented under it.
            let mut out = String::new();
            for (i, line) in text.lines().enumerate() {
                if i == 0 {
                    out.push_str(&format!("{} {}", self.assistant_label(), line));
                } else {
                    out.push_str(&format!("\n  {}", line));
                }
            }
            if text.is_empty() {
                out.push_str(&self.assistant_label());
            }
            return out;
        }
        let bullet = if self.theme == "cubist" { "\u{25C7}" } else { "\u{2022}" };
        format!("{}\n{} {}", self.assistant_label(), bullet, text)
    }

    pub fn tool_panel(&self, name: &str, status: &str, body: &str) -> String {
        if self.theme == "cubist" {
            let head = self.warn(&format!("\u{25E2}\u{25E3} TOOL {} ({})", name, status));
            let border = self.paint(
                "\u{25B0}\u{25B1}\u{25B0}\u{25B1}\u{25B0}\u{25B1}\u{25B0}\u{25B1}\u{25B0}\u{25B1}\u{25B0}\u{25B1}\u{25B0}\u{25B1}\u{25B0}\u{25B1}\u{25B0}\u{25B1}\u{25B0}\u{25B1}\u{25B0}\u{25B1}\u{25B0}\u{25B1}\u{25B0}\u{25B1}\u{25B0}\u{25B1}\u{25B0}\u{25B1}",
                self.dim_code(),
            );
            let styled_body = self.style_tool_body(name, body);
            return format!("{}\n{}\n{}\n{}", head, border, styled_body, border);
        }
        if self.theme == "aurora" {
            return self.tool_panel_aurora(name, status, body);
        }
        let head = self.warn(&format!("TOOL {} ({})", name, status));
        let border = self.dim("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}");
        format!("{}\n{}\n{}\n{}", head, border, body, border)
    }

    /// Claude Code-style tool rendering: a colored `●` header naming the
    /// tool, then the result body in a `⎿`-anchored dim gutter. Long bodies
    /// are shown in full but the gutter keeps them visually secondary.
    fn tool_panel_aurora(&self, name: &str, status: &str, body: &str) -> String {
        let dot_color = if status == "ok" || status == "result" {
            AURORA_TEAL
        } else {
            AURORA_ROSE
        };
        let head = format!(
            "{} {}{}",
            self.paint("\u{25CF}", dot_color),
            self.paint(name, "1"),
            if status == "ok" || status == "result" {
                String::new()
            } else {
                format!(" {}", self.paint(&format!("({})", status), AURORA_ROSE))
            }
        );
        let styled_body = self.style_tool_body(name, body);
        let mut out = String::new();
        out.push_str(&head);
        for (i, line) in styled_body.lines().enumerate() {
            let gutter = if i == 0 { "\u{23BF} " } else { "   " };
            out.push('\n');
            out.push_str(&format!("  {}{}", self.paint(gutter, AURORA_MIST), line));
        }
        out
    }

    pub fn read_file_hidden_notice(&self, header: &str) -> String {
        if self.theme == "cubist" {
            return format!(
                "{}\n{}",
                format!("{} {}", self.warn("\u{25E2}"), header),
                self.dim("\u{25C7} content hidden \u{25C7} use /toolcall read_file <path> <start_line> <max_lines> for exact ranges"),
            );
        }
        if self.theme == "aurora" {
            return format!(
                "{}\n  {}",
                format!("{} {}", self.paint("\u{25CF}", AURORA_TEAL), self.dim(header)),
                self.dim("\u{23BF} content folded \u{00B7} /toolcall read_file <path> <start> <lines> for exact ranges"),
            );
        }
        format!(
            "{}\n(read content hidden in UI to reduce token-heavy output; use /toolcall read_file <path> <start_line> <max_lines> for exact ranges)",
            header
        )
    }

    pub fn toolcall_intent_panel(&self, raw: &str) -> String {
        let cleaned = raw
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>();
        let (head_line, rest_lines): (&str, &[&str]) = match cleaned.split_first() {
            Some((h, r)) => (*h, r),
            None => ("tool call", &[]),
        };
        if self.theme == "aurora" {
            let head = format!(
                "{} {}",
                self.paint("\u{25CF}", AURORA_GOLD),
                self.paint(head_line, "1"),
            );
            let mut out = String::new();
            out.push_str(&head);
            for (i, line) in rest_lines.iter().enumerate() {
                if line.starts_with("</toolcall") {
                    continue;
                }
                let gutter = if i == 0 { "\u{23BF} " } else { "   " };
                out.push('\n');
                out.push_str(&format!("  {}{}", self.paint(gutter, AURORA_MIST), self.dim(line)));
            }
            return out;
        }
        let head = self.warn(&format!("\u{25E2}\u{25E3} INTENT \u{25B8} {}", head_line));
        let mut out = String::new();
        out.push_str(&head);
        for line in rest_lines {
            if line.starts_with("</toolcall") {
                continue;
            }
            out.push('\n');
            out.push_str(&self.dim(&format!("\u{25C7} {}", line)));
        }
        out
    }

    fn style_tool_body(&self, name: &str, body: &str) -> String {
        let restyle_diff = matches!(name, "edit_file" | "write_file");
        body
            .lines()
            .map(|line| self.style_tool_body_line(line, restyle_diff))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn style_tool_body_line(&self, line: &str, restyle_diff: bool) -> String {
        if line.starts_with('[') && line.ends_with(']') && line.len() > 2 {
            return self.warn(line);
        }
        if !restyle_diff {
            return line.to_string();
        }
        if line.starts_with("@@") {
            return self.paint(line, self.accent_code());
        }
        if line.starts_with("+ ") || line == "+" {
            return self.diff_added(line);
        }
        if line.starts_with("- ") || line == "-" {
            return self.diff_removed(line);
        }
        line.to_string()
    }

    pub fn spinning_line(&self, elapsed_secs: u64, tokens: usize) -> String {
        if self.theme == "cubist" {
            let frame = CUBIST_SPINNER_FRAMES[((elapsed_secs.wrapping_mul(5)) % 6) as usize];
            return format!(
                "{} {} {}",
                self.paint(frame, self.accent_code()),
                self.warn("Composing"),
                self.dim(&format!(
                    "({}s . {} tokens . esc to interrupt)",
                    elapsed_secs, tokens
                ))
            );
        }
        if self.theme == "aurora" {
            let frame = AURORA_SPINNER_FRAMES
                [((elapsed_secs.wrapping_mul(5)) % AURORA_SPINNER_FRAMES.len() as u64) as usize];
            let verbs = ["Thinking", "Weaving", "Tracing", "Shaping", "Polishing"];
            let verb = verbs[((elapsed_secs / 4) % verbs.len() as u64) as usize];
            return format!(
                "{} {}{} {}",
                self.paint(frame, AURORA_CORAL),
                self.paint(verb, AURORA_VIOLET),
                self.paint("\u{2026}", AURORA_VIOLET),
                self.dim(&format!(
                    "({}s \u{00B7} {} tokens \u{00B7} esc to interrupt)",
                    elapsed_secs, tokens
                ))
            );
        }
        format!(
            "{} {}",
            self.warn("\u{2736} Spinning\u{2026}"),
            self.dim(&format!(
                "({}s \u{00B7} \u{00D7} {} tokens \u{00B7} esc to interrupt)",
                elapsed_secs, tokens
            ))
        )
    }

    pub fn done_line(&self, elapsed_secs: u64, tokens: usize) -> String {
        if self.theme == "cubist" {
            return format!(
                "{} {}",
                self.ok("\u{25E2}\u{25E3} Completed"),
                self.dim(&format!("({}s . {} tokens)", elapsed_secs, tokens))
            );
        }
        if self.theme == "aurora" {
            return format!(
                "{} {}",
                self.paint("\u{273B} Done", AURORA_TEAL),
                self.dim(&format!("({}s \u{00B7} {} tokens)", elapsed_secs, tokens))
            );
        }
        format!(
            "{} {}",
            self.ok("\u{2713} Completed"),
            self.dim(&format!("({}s \u{00B7} \u{00D7} {} tokens)", elapsed_secs, tokens))
        )
    }

    pub fn thinking_block(&self, text: &str) -> String {
        if self.theme == "cubist" {
            return self
                .mosaic_panel(
                    &["\u{25C7} Thinking".to_string(), text.to_string()],
                    self.accent_code(),
                )
                .join("\n");
        }
        if self.theme == "aurora" {
            // Italic dim thinking, indented — quiet, like Claude Code.
            let mut out = String::new();
            out.push_str(&self.paint("\u{2234} thinking", AURORA_VIOLET));
            for line in text.lines() {
                out.push('\n');
                out.push_str(&format!("  {}", self.paint(line, &format!("3;{}", AURORA_MIST))));
            }
            return out;
        }
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
        if self.theme == "cubist" {
            return format!("{} {}", self.err("\u{25E2} ERROR"), text);
        }
        if self.theme == "aurora" {
            return format!("{} {}", self.paint("\u{2716} error", AURORA_ROSE), text);
        }
        format!("{} {}", self.err("ERROR"), text)
    }

    pub fn info(&self, text: &str) -> String {
        if self.theme == "cubist" {
            return format!("{} {}", self.dim("\u{25C7} INFO"), text);
        }
        if self.theme == "aurora" {
            return self.dim(&format!("\u{00B7} {}", text));
        }
        format!("{} {}", self.dim("INFO"), text)
    }

    pub fn startup_card(
        &self,
        project: &str,
        setup_status: &str,
        auto_agent: &str,
        sandbox: &str,
        profile: &str,
        speed: &str,
    ) -> String {
        if self.theme == "cubist" {
            let rows = [
                ("project", project),
                ("setup", setup_status),
                ("auto", auto_agent),
                ("sandbox", sandbox),
                ("profile", profile),
                ("speed", speed),
            ];
            let key_width = rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
            let lines: Vec<String> = rows
                .iter()
                .map(|(k, v)| {
                    let pad = key_width - k.chars().count();
                    format!("\u{25C6} {}{}  {}", k, " ".repeat(pad), v)
                })
                .collect();
            return self.mosaic_panel(&lines, self.accent_code()).join("\n");
        }
        if self.theme == "aurora" {
            // A single quiet metadata block with dotted leaders, no box.
            let rows = [
                ("project", project),
                ("setup", setup_status),
                ("auto", auto_agent),
                ("sandbox", sandbox),
                ("profile", profile),
                ("speed", speed),
            ];
            let key_width = rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
            return rows
                .iter()
                .map(|(k, v)| {
                    let pad = key_width - k.chars().count();
                    format!(
                        "  {}{}  {}",
                        self.paint(k, AURORA_MIST),
                        self.paint(&format!(" {}", "\u{00B7}".repeat(pad + 2)), AURORA_MIST),
                        v
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
        }
        [
            self.info(&format!("project={}", project)),
            self.info(&format!("setup: {}", setup_status)),
            self.info(&format!("auto-agent: {}", auto_agent)),
            self.info(&format!("sandbox: {}", sandbox)),
            self.info(&format!("profile: {}", profile)),
            self.info(&format!("speed: {}", speed)),
        ]
        .join("\n")
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

    pub fn progress_bar(&self, current: usize, total: usize) -> Option<String> {
        if self.theme != "cubist" && self.theme != "aurora" {
            return None;
        }
        let width: usize = 10;
        let total_safe = total.max(1);
        let filled = ((current.min(total) as f64 / total_safe as f64) * width as f64).round() as usize;
        let filled = filled.min(width);
        if self.theme == "aurora" {
            let mut bar = String::with_capacity(width * 3);
            for i in 0..width {
                bar.push_str(if i < filled { "\u{2501}" } else { "\u{2508}" });
            }
            return Some(format!(
                "  {} {} {}",
                self.paint(&bar, AURORA_VIOLET),
                self.dim(&format!("{}/{}", current, total)),
                self.paint("\u{273B}", AURORA_CORAL),
            ));
        }
        let mut bar = String::with_capacity(width * 3);
        for i in 0..width {
            bar.push_str(if i < filled { "\u{25B0}" } else { "\u{25B1}" });
        }
        Some(format!(
            "{} {} {}/{}",
            self.dim("\u{25C7} progress"),
            self.paint(&bar, self.warn_code()),
            current,
            total
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
            ("cubist", "Cubist (Picasso-inspired truecolor)"),
            ("aurora", "Aurora (Claude Code-inspired truecolor)"),
        ];

        let mut lines = vec![
            "Let's get started.".to_string(),
            "".to_string(),
            "Choose the text style that looks best with your terminal".to_string(),
            "To change this later, run /theme".to_string(),
            "".to_string(),
        ];

        for (idx, (key, label)) in items.iter().enumerate() {
            let selected = if *key == current { " \u{2713}" } else { "" };
            let prefix = if *key == current {
                self.accent("\u{203A} ")
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
            self.diff_added("2  + console.log(\"Hello, ASI Code!\");"),
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
        let top = self.paint(&format!("\u{256D}{}\u{256E}", "\u{2500}".repeat(width + 2)), color_code);
        let bottom = self.paint(&format!("\u{2570}{}\u{256F}", "\u{2500}".repeat(width + 2)), color_code);

        let mut out = vec![top];
        for line in lines {
            let pad = width.saturating_sub(line.chars().count());
            out.push(format!(
                "{} {}{} {}",
                self.paint("\u{2502}", color_code),
                line,
                " ".repeat(pad),
                self.paint("\u{2502}", color_code)
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
            "cubist" => CUBIST_COBALT,
            "aurora" => AURORA_VIOLET,
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
            "cubist" => CUBIST_OCHRE,
            "aurora" => AURORA_GOLD,
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
            "cubist" => CUBIST_SAGE,
            "aurora" => AURORA_TEAL,
            "dark-colorblind" | "light-colorblind" => "96",
            _ => "32",
        }
    }

    fn err_code(&self) -> &str {
        match self.theme.as_str() {
            "cubist" => CUBIST_TERRA,
            "aurora" => AURORA_ROSE,
            "dark-colorblind" | "light-colorblind" => "91",
            _ => "31",
        }
    }

    fn dim_code(&self) -> &str {
        match self.theme.as_str() {
            "cubist" => CUBIST_SLATE,
            "aurora" => AURORA_MIST,
            "light" | "light-colorblind" | "light-ansi" => "30",
            _ => "90",
        }
    }

    fn diff_removed_code(&self) -> &str {
        match self.theme.as_str() {
            "cubist" => CUBIST_DIFF_REMOVED,
            "aurora" => AURORA_DIFF_REMOVED,
            "light" | "light-colorblind" => "31",
            _ => "97;41",
        }
    }

    fn diff_added_code(&self) -> &str {
        match self.theme.as_str() {
            "cubist" => CUBIST_DIFF_ADDED,
            "aurora" => AURORA_DIFF_ADDED,
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

fn side_by_side(left: &[String], right: &[String], sep: &str) -> String {
    let height = left.len().max(right.len());
    let left_blank = left
        .first()
        .map(|x| " ".repeat(visible_width(x)))
        .unwrap_or_default();
    let right_blank = right
        .first()
        .map(|x| " ".repeat(visible_width(x)))
        .unwrap_or_default();
    let mut out = Vec::with_capacity(height);
    for i in 0..height {
        let l = left.get(i).cloned().unwrap_or_else(|| left_blank.clone());
        let r = right.get(i).cloned().unwrap_or_else(|| right_blank.clone());
        out.push(format!("{}{}{}", l, sep, r));
    }
    out.join("\n")
}

fn deconstructed_banner(text: &str) -> String {
    const MARKERS: [&str; 6] = [
        "\u{25C6}", // ◆
        "\u{25C7}", // ◇
        "\u{25E2}", // ◢
        "\u{25B2}", // ▲
        "\u{25BD}", // ▽
        "\u{25E3}", // ◣
    ];
    text.split_whitespace()
        .map(|word| {
            let mut out = String::new();
            let mut idx = 0usize;
            for c in word.chars() {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(MARKERS[idx % MARKERS.len()]);
                out.push(' ');
                out.push(c);
                idx += 1;
            }
            if !out.is_empty() {
                out.push(' ');
                out.push_str(MARKERS[idx % MARKERS.len()]);
            }
            out
        })
        .collect::<Vec<_>>()
        .join("  ")
}

pub struct CubistStreamFilter {
    enabled: bool,
    theme: String,
    buf: String,
    in_toolcall: bool,
    toolcall_acc: String,
}

impl CubistStreamFilter {
    pub fn new(theme: &str) -> Self {
        Self {
            enabled: theme == "cubist" || theme == "aurora",
            theme: theme.to_string(),
            buf: String::new(),
            in_toolcall: false,
            toolcall_acc: String::new(),
        }
    }

    pub fn write(&mut self, delta: &str, out: &mut String) {
        if !self.enabled {
            out.push_str(delta);
            return;
        }
        self.buf.push_str(delta);
        loop {
            if self.in_toolcall {
                if let Some(idx) = self.buf.find("</toolcall>") {
                    let end = idx + "</toolcall>".len();
                    self.toolcall_acc.push_str(&self.buf[..end]);
                    let tail = self.buf[end..].to_string();
                    self.buf = tail;
                    let ui = Ui::new(&self.theme);
                    let panel = ui.toolcall_intent_panel(&self.toolcall_acc);
                    out.push('\n');
                    out.push_str(&panel);
                    out.push('\n');
                    self.toolcall_acc.clear();
                    self.in_toolcall = false;
                    continue;
                }
                self.toolcall_acc.push_str(&self.buf);
                self.buf.clear();
                return;
            }
            if let Some(start) = self.buf.find("<toolcall>") {
                let before = self.buf[..start].to_string();
                out.push_str(&before);
                let tail = self.buf[start + "<toolcall>".len()..].to_string();
                self.buf = tail;
                self.in_toolcall = true;
                continue;
            }
            let safe = safe_print_prefix_bytes(&self.buf, "<toolcall>");
            if safe == 0 {
                return;
            }
            let printable = self.buf[..safe].to_string();
            let tail = self.buf[safe..].to_string();
            out.push_str(&printable);
            self.buf = tail;
            return;
        }
    }

    pub fn flush(&mut self, out: &mut String) {
        if !self.enabled {
            return;
        }
        if self.in_toolcall {
            let ui = Ui::new(&self.theme);
            self.toolcall_acc.push_str(&self.buf);
            self.buf.clear();
            if !self.toolcall_acc.trim().is_empty() {
                let panel = ui.toolcall_intent_panel(&self.toolcall_acc);
                out.push('\n');
                out.push_str(&panel);
                out.push('\n');
            }
            self.toolcall_acc.clear();
            self.in_toolcall = false;
            return;
        }
        if !self.buf.is_empty() {
            out.push_str(&self.buf);
            self.buf.clear();
        }
    }
}

fn safe_print_prefix_bytes(s: &str, tag: &str) -> usize {
    let max = tag.len().min(s.len());
    for n in (1..=max).rev() {
        let split = s.len() - n;
        if !s.is_char_boundary(split) {
            continue;
        }
        let suffix = &s[split..];
        if tag.starts_with(suffix) {
            return split;
        }
    }
    s.len()
}

fn visible_width(s: &str) -> usize {
    let mut count = 0usize;
    let mut in_esc = false;
    for c in s.chars() {
        if in_esc {
            if c == 'm' {
                in_esc = false;
            }
        } else if c == '\x1b' {
            in_esc = true;
        } else {
            count += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn aurora_welcome_uses_gradient_and_star() {
        let ui = Ui::new_with(true, "aurora");
        let w = ui.welcome_card("D:\\test");
        assert!(w.contains('\u{273B}'), "missing star glyph: {:?}", w);
        assert!(w.contains("\x1b[1;38;2;"), "missing gradient escapes: {:?}", w);
        assert!(w.contains("D:\\test"), "missing cwd: {:?}", w);
    }

    #[test]
    fn aurora_prompt_is_minimal_chevron() {
        let ui = Ui::new_with(false, "aurora");
        let p = ui.input_prompt();
        assert!(p.contains('\u{276F}'), "missing chevron: {:?}", p);
        assert!(!p.contains("ASI Code>"), "should not use legacy prompt: {:?}", p);
    }

    #[test]
    fn aurora_tool_panel_uses_dot_header_and_gutter() {
        let ui = Ui::new_with(false, "aurora");
        let p = ui.tool_panel("bash", "ok", "exit=0\nhello");
        assert!(p.contains('\u{25CF}'), "missing dot header: {:?}", p);
        assert!(p.contains('\u{23BF}'), "missing result gutter: {:?}", p);
        assert!(p.contains("bash"));
        assert!(!p.contains("TOOL bash"), "should not use legacy header: {:?}", p);
    }

    #[test]
    fn aurora_tool_panel_error_status_is_visible() {
        let ui = Ui::new_with(false, "aurora");
        let p = ui.tool_panel("bash", "error", "boom");
        assert!(p.contains("(error)"), "error status must be shown: {:?}", p);
    }

    #[test]
    fn aurora_spinner_cycles_star_frames() {
        let ui = Ui::new_with(false, "aurora");
        let mut distinct = HashSet::new();
        for sec in 0..8u64 {
            let line = ui.spinning_line(sec, 0);
            for f in &AURORA_SPINNER_FRAMES {
                if line.starts_with(*f) {
                    distinct.insert(*f);
                }
            }
        }
        assert!(distinct.len() >= 3, "expected >=3 frames, got {:?}", distinct);
    }

    #[test]
    fn aurora_startup_card_uses_dotted_leaders() {
        let ui = Ui::new_with(false, "aurora");
        let card = ui.startup_card("D:\\demo", "ok", "on", "off", "standard", "deep");
        assert!(card.contains("project"));
        assert!(card.contains('\u{00B7}'), "missing dotted leader: {:?}", card);
        assert!(!card.contains("INFO"), "should not use legacy INFO lines: {:?}", card);
    }

    #[test]
    fn aurora_progress_bar_renders() {
        let ui = Ui::new_with(false, "aurora");
        let bar = ui.progress_bar(3, 10).expect("aurora should render bar");
        assert!(bar.contains('\u{2501}'), "missing filled cell: {:?}", bar);
        assert!(bar.contains('\u{2508}'), "missing empty cell: {:?}", bar);
        assert!(bar.contains("3/10"));
    }

    #[test]
    fn aurora_stream_filter_renders_intent_panel() {
        let mut sf = CubistStreamFilter::new("aurora");
        let mut out = String::new();
        sf.write("hi <toolcall>\nread_file foo.rs\n</toolcall> bye", &mut out);
        sf.flush(&mut out);
        assert!(out.contains('\u{25CF}'), "missing intent dot: {:?}", out);
        assert!(out.contains("read_file"));
        assert!(!out.contains("<toolcall>"));
    }

    #[test]
    fn aurora_theme_menu_lists_aurora_as_eighth() {
        let ui = Ui::new_with(false, "dark");
        let m = ui.theme_menu("dark");
        assert!(m.contains("8. Aurora"), "expected 8th item Aurora: {:?}", m);
    }

    #[test]
    fn cubist_welcome_contains_geometric_corners() {
        let ui = Ui::new_with(false, "cubist");
        let w = ui.welcome_card("D:\\test");
        assert!(w.contains('\u{259B}'), "missing top-left block: {:?}", w);
        assert!(w.contains('\u{2599}'), "missing bottom-left block: {:?}", w);
        // After deconstructed banner, markers are scattered; check ◢ presence
        assert!(w.contains('\u{25E2}'), "missing geometric marker ◢: {:?}", w);
    }

    #[test]
    fn cubist_tips_uses_triangle_markers() {
        let ui = Ui::new_with(false, "cubist");
        let t = ui.tips_section();
        assert!(t.contains('\u{25B2}'), "missing triangle marker: {:?}", t);
        assert!(!t.contains("1."), "should not contain numbered list");
    }

    #[test]
    fn cubist_prompt_uses_block_fragment() {
        let ui = Ui::new_with(false, "cubist");
        let p = ui.input_prompt();
        assert!(
            p.contains("\u{25B0}\u{25B0}\u{25B1}\u{25B1}"),
            "missing block fragment: {:?}",
            p
        );
    }

    #[test]
    fn cubist_spinner_frames_cycle() {
        let ui = Ui::new_with(false, "cubist");
        let mut frames = HashSet::new();
        for sec in 0..7u64 {
            let line = ui.spinning_line(sec, 0);
            for c in &CUBIST_SPINNER_FRAMES {
                if line.starts_with(*c) {
                    frames.insert(*c);
                    break;
                }
            }
        }
        assert!(
            frames.len() >= 3,
            "expected at least 3 distinct frames, got {} ({:?})",
            frames.len(),
            frames
        );
    }

    #[test]
    fn cubist_palette_uses_truecolor_escape() {
        let ui = Ui::new_with(true, "cubist");
        let prompt = ui.input_prompt();
        assert!(
            prompt.contains("\x1b[38;2;"),
            "expected truecolor escape in: {:?}",
            prompt
        );
    }

    #[test]
    fn default_theme_welcome_unchanged() {
        let ui = Ui::new_with(false, "dark");
        let w = ui.welcome_card("D:\\test");
        assert!(w.contains('\u{256D}'), "dark theme should keep rounded corners");
        assert!(w.contains('\u{256E}'));
        assert!(!w.contains('\u{259B}'), "dark theme should not use cubist blocks");
    }

    #[test]
    fn cubist_startup_card_uses_mosaic_panel() {
        let ui = Ui::new_with(false, "cubist");
        let card = ui.startup_card("D:\\demo", "ok", "on", "off", "default", "balanced");
        assert!(card.contains('\u{259B}'));
        assert!(card.contains("\u{25C6} project"));
        assert!(card.contains("D:\\demo"));
    }

    #[test]
    fn default_theme_startup_card_uses_info_lines() {
        let ui = Ui::new_with(false, "dark");
        let card = ui.startup_card("D:\\demo", "ok", "on", "off", "default", "balanced");
        assert!(card.contains("INFO project=D:\\demo"));
        assert!(card.contains("INFO setup: ok"));
    }

    #[test]
    fn theme_menu_lists_cubist_as_seventh() {
        let ui = Ui::new_with(false, "dark");
        let m = ui.theme_menu("dark");
        assert!(m.contains("7. Cubist"), "expected 7th item Cubist: {:?}", m);
    }

    #[test]
    fn cubist_welcome_has_diagonal_cuts() {
        let ui = Ui::new_with(false, "cubist");
        let w = ui.welcome_card("D:\\test");
        assert!(w.contains('\u{2571}'), "missing forward slash diagonal: {:?}", w);
        assert!(w.contains('\u{2572}'), "missing back slash diagonal: {:?}", w);
    }

    #[test]
    fn deconstructed_banner_interleaves_markers_between_letters() {
        let s = deconstructed_banner("ASI");
        // Each letter should be preceded by a marker + space, and the word ends with a closing marker
        assert!(s.contains(" A "));
        assert!(s.contains(" S "));
        assert!(s.contains(" I "));
        // Should contain at least 3 distinct geometric markers
        let markers = ['\u{25C6}', '\u{25C7}', '\u{25E2}', '\u{25B2}', '\u{25BD}', '\u{25E3}'];
        let hits: usize = markers.iter().filter(|m| s.contains(**m)).count();
        assert!(hits >= 3, "expected >=3 marker variety, got {}: {:?}", hits, s);
    }

    #[test]
    fn deconstructed_banner_double_spaces_between_words() {
        let s = deconstructed_banner("ASI Code");
        // Two words joined with double-space separator
        assert!(s.contains("  "), "expected double space between words: {:?}", s);
    }

    #[test]
    fn cubist_welcome_uses_deconstructed_banner() {
        let ui = Ui::new_with(false, "cubist");
        let w = ui.welcome_card("D:\\test");
        // Deconstructed "ASI Code" should have letters separated by markers, not as a contiguous "ASI Code" string
        assert!(!w.contains("ASI Code"), "banner should be deconstructed: {:?}", w);
        assert!(w.contains(" A "));
        assert!(w.contains(" I "));
    }

    #[test]
    fn cubist_progress_bar_renders_filled_and_empty_cells() {
        let ui = Ui::new_with(false, "cubist");
        let bar = ui.progress_bar(3, 10).expect("cubist should produce bar");
        assert!(bar.contains('\u{25B0}'), "missing filled cell ▰: {:?}", bar);
        assert!(bar.contains('\u{25B1}'), "missing empty cell ▱: {:?}", bar);
        assert!(bar.contains("3/10"));
    }

    #[test]
    fn cubist_progress_bar_full_at_completion() {
        let ui = Ui::new_with(false, "cubist");
        let bar = ui.progress_bar(5, 5).unwrap();
        assert!(bar.contains("5/5"));
        // Fully filled means no ▱ remaining
        assert!(!bar.contains('\u{25B1}'), "full bar should have no empty cells: {:?}", bar);
    }

    #[test]
    fn non_cubist_progress_bar_returns_none() {
        let ui = Ui::new_with(false, "dark");
        assert!(ui.progress_bar(1, 5).is_none());
    }

    #[test]
    fn cubist_progress_bar_handles_zero_total_without_panic() {
        let ui = Ui::new_with(false, "cubist");
        let bar = ui.progress_bar(0, 0).unwrap();
        assert!(bar.contains("0/0"));
    }

    #[test]
    fn cubist_done_line_uses_geometric_label() {
        let ui = Ui::new_with(false, "cubist");
        let d = ui.done_line(2, 100);
        assert!(d.contains("\u{25E2}\u{25E3} Completed"));
        assert!(!d.contains("\u{2713}"));
    }

    #[test]
    fn cubist_hidden_notice_uses_marker_and_dim_hint() {
        let ui = Ui::new_with(false, "cubist");
        let s = ui.read_file_hidden_notice("[read_file:foo.rs lines 1-10 of 100]");
        assert!(s.contains("[read_file:foo.rs lines 1-10 of 100]"));
        assert!(s.contains('\u{25E2}'), "missing ◢ marker: {:?}", s);
        assert!(s.contains("content hidden"), "missing hint text: {:?}", s);
        assert!(!s.contains("(read content hidden in UI"), "should drop legacy parenthetical: {:?}", s);
    }

    #[test]
    fn default_hidden_notice_keeps_legacy_text() {
        let ui = Ui::new_with(false, "dark");
        let s = ui.read_file_hidden_notice("[read_file:foo.rs lines 1-10 of 100]");
        assert!(s.contains("(read content hidden in UI to reduce token-heavy output"));
    }

    #[test]
    fn cubist_tool_panel_edit_file_colors_plus_and_minus_lines() {
        let ui = Ui::new_with(true, "cubist");
        let body = "Edited foo.rs\n\n[unified diff]\n@@ -1,2 +1,2 @@\n- old line\n+ new line\n";
        let p = ui.tool_panel("edit_file", "ok", body);
        // Plus and minus lines should be wrapped in ANSI sequences (truecolor diff codes)
        assert!(p.contains("\x1b[38;2;"), "expected truecolor escape: {:?}", p);
        // [unified diff] header should be styled
        assert!(p.contains("[unified diff]"));
    }

    #[test]
    fn cubist_tool_panel_other_tools_dont_recolor_diff() {
        let ui = Ui::new_with(true, "cubist");
        let body = "+ this line should not be colored as diff\n- this either";
        let p = ui.tool_panel("bash", "ok", body);
        // bash tool: + / - should pass through plain
        assert!(p.contains("+ this line should not"));
        assert!(p.contains("- this either"));
    }

    #[test]
    fn cubist_toolcall_intent_panel_summarizes_first_line() {
        let ui = Ui::new_with(false, "cubist");
        let raw = "read_file path=D:\\code\\foo.rs\n</toolcall>";
        let p = ui.toolcall_intent_panel(raw);
        assert!(p.contains("INTENT"), "missing INTENT label: {:?}", p);
        assert!(p.contains("read_file"), "missing tool name in panel: {:?}", p);
        assert!(p.contains('\u{25E2}'), "missing ◢ marker: {:?}", p);
    }

    #[test]
    fn cubist_stream_filter_replaces_toolcall_block() {
        let mut sf = CubistStreamFilter::new("cubist");
        let mut out = String::new();
        sf.write("hello <toolcall>\nread_file path=foo.rs\n</toolcall> world", &mut out);
        sf.flush(&mut out);
        assert!(out.starts_with("hello "), "prefix should pass through: {:?}", out);
        assert!(out.contains("INTENT"), "should contain cubist panel: {:?}", out);
        assert!(out.contains("read_file"), "should contain tool name: {:?}", out);
        assert!(out.contains(" world"), "suffix should pass through: {:?}", out);
        assert!(!out.contains("<toolcall>"), "raw open tag should be gone: {:?}", out);
        assert!(!out.contains("</toolcall>"), "raw close tag should be gone: {:?}", out);
    }

    #[test]
    fn cubist_stream_filter_handles_split_open_tag() {
        let mut sf = CubistStreamFilter::new("cubist");
        let mut out = String::new();
        // Open tag split across writes
        sf.write("abc <too", &mut out);
        sf.write("lcall>\nbash echo hi\n</toolcall>z", &mut out);
        sf.flush(&mut out);
        assert!(out.starts_with("abc "), "prefix should pass: {:?}", out);
        assert!(out.contains("bash echo hi"));
        assert!(out.contains("INTENT"));
        assert!(out.ends_with('z'), "trailing should pass: {:?}", out);
    }

    #[test]
    fn non_cubist_stream_filter_is_passthrough() {
        let mut sf = CubistStreamFilter::new("dark");
        let mut out = String::new();
        sf.write("hello <toolcall>raw</toolcall> world", &mut out);
        sf.flush(&mut out);
        assert_eq!(out, "hello <toolcall>raw</toolcall> world");
    }

    #[test]
    fn safe_print_prefix_keeps_partial_tag_buffered() {
        // The string ends with "<too" which is a prefix of "<toolcall>"
        // Safe prefix should stop before "<"
        let s = "hello <too";
        let n = safe_print_prefix_bytes(s, "<toolcall>");
        assert_eq!(&s[..n], "hello ");
    }

    #[test]
    fn safe_print_prefix_full_when_no_partial_tag() {
        let s = "hello world";
        let n = safe_print_prefix_bytes(s, "<toolcall>");
        assert_eq!(n, s.len());
    }
}
