use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use std::fmt;
use tracing::Subscriber;
use tracing_subscriber::{
    fmt::{
        format::{self, FormatEvent, Writer},
        time::SystemTime,
        FmtContext,
    },
    registry::LookupSpan,
};

const PIPELINE_TARGET: &str = "hyprwhspr::text_pipeline";
const MAX_DIFF_CHARS: usize = 2048;
const PREVIEW_CHAR_LIMIT: usize = 160;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextPipelineRecord {
    pub input: String,
    pub output: String,
    pub steps: Vec<PipelineStepRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStepRecord {
    pub name: String,
    pub before: String,
    pub after: String,
    pub applied: bool,
    pub change_count: Option<usize>,
}

impl TextPipelineRecord {
    pub fn new(input: String, output: String, steps: Vec<PipelineStepRecord>) -> Self {
        Self {
            input,
            output,
            steps,
        }
    }

    pub fn changed_steps(&self) -> usize {
        self.steps.iter().filter(|step| step.applied).count()
    }

    pub fn render_pretty(&self, use_color: bool) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "┌─ Text Pipeline (steps: {}, changed: {})",
            self.steps.len(),
            self.changed_steps()
        ));
        push_body_line(
            &mut lines,
            format!("IN  : {}", preview_value(&self.input, use_color)),
        );

        for step in &self.steps {
            for line in step.render_lines(use_color) {
                push_body_line(&mut lines, line);
            }
        }

        push_body_line(
            &mut lines,
            format!("OUT : {}", preview_value(&self.output, use_color)),
        );
        lines.push("└─".to_string());

        lines.join("\n")
    }
}

impl PipelineStepRecord {
    pub fn new(
        name: impl Into<String>,
        before: String,
        after: String,
        change_count: Option<usize>,
    ) -> Self {
        let before_owned = before;
        let applied = before_owned != after;
        Self {
            name: name.into(),
            before: before_owned,
            after,
            applied,
            change_count,
        }
    }

    fn render_lines(&self, use_color: bool) -> Vec<String> {
        let mut lines = Vec::new();
        let summary = if self.applied {
            match self.change_count {
                Some(count) if count > 0 => format!("• {} (applied ×{})", self.name, count),
                _ => format!("• {} (applied)", self.name),
            }
        } else {
            format!("• {} (no change)", self.name)
        };
        lines.push(summary);

        if let Some(diff_lines) = self.inline_diff(use_color) {
            for diff in diff_lines {
                lines.push(format!("  {}", diff));
            }
        } else if self.applied {
            lines.push(format!("  - {}", preview_value(&self.before, use_color)));
            lines.push(format!("  + {}", preview_value(&self.after, use_color)));
        }

        lines
    }

    fn inline_diff(&self, use_color: bool) -> Option<Vec<String>> {
        let before_len = self.before.len();
        let after_len = self.after.len();
        if !self.applied || before_len + after_len > MAX_DIFF_CHARS {
            return None;
        }

        let diff = TextDiff::from_words(&self.before, &self.after);
        let mut removed = String::new();
        let mut added = String::new();
        let mut has_delete = false;
        let mut has_insert = false;

        for change in diff.iter_all_changes() {
            let escaped = escape_fragment(change.value());
            match change.tag() {
                ChangeTag::Delete => {
                    has_delete = true;
                    removed.push_str(&stylize(escaped.clone(), use_color, DiffStyle::Delete));
                }
                ChangeTag::Insert => {
                    has_insert = true;
                    added.push_str(&stylize(escaped.clone(), use_color, DiffStyle::Insert));
                }
                ChangeTag::Equal => {
                    removed.push_str(&stylize(escaped.clone(), use_color, DiffStyle::Context));
                    added.push_str(&stylize(escaped, use_color, DiffStyle::Context));
                }
            }
        }

        if !has_delete && !has_insert {
            return None;
        }

        let mut lines = Vec::new();
        if has_delete {
            lines.push(format!("- {}", removed));
        }
        if has_insert {
            lines.push(format!("+ {}", added));
        }

        Some(lines)
    }
}

#[derive(Debug, Clone, Copy)]
enum DiffStyle {
    Delete,
    Insert,
    Context,
}

fn stylize(fragment: String, use_color: bool, style: DiffStyle) -> String {
    if !use_color {
        return fragment;
    }

    match style {
        DiffStyle::Delete => fragment.red().to_string(),
        DiffStyle::Insert => fragment.green().to_string(),
        DiffStyle::Context => fragment.dimmed().to_string(),
    }
}

fn escape_fragment(value: &str) -> String {
    value.escape_debug().to_string()
}

fn push_body_line(lines: &mut Vec<String>, content: String) {
    lines.push(format!("│ {}", content));
}

fn preview_value(value: &str, use_color: bool) -> String {
    let mut preview: String = value.chars().take(PREVIEW_CHAR_LIMIT).collect();
    if value.chars().count() > PREVIEW_CHAR_LIMIT {
        preview.push_str("...");
    }
    let escaped = escape_fragment(&preview);
    if use_color {
        escaped.cyan().to_string()
    } else {
        escaped
    }
}

#[derive(Debug, Default)]
struct PipelineEventVisitor {
    pipeline_json: Option<String>,
}

impl tracing::field::Visit for PipelineEventVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "pipeline_json" {
            self.pipeline_json = Some(value.to_string());
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        if field.name() == "pipeline_json" && self.pipeline_json.is_none() {
            self.pipeline_json = Some(format!("{value:?}"));
        }
    }
}

pub struct TextPipelineFormatter {
    inner: format::Format<format::Full, SystemTime>,
}

impl Default for TextPipelineFormatter {
    fn default() -> Self {
        Self {
            inner: format::Format::default(),
        }
    }
}

impl TextPipelineFormatter {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<S, N> FormatEvent<S, N> for TextPipelineFormatter
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> format::FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> fmt::Result {
        if event.metadata().target() == PIPELINE_TARGET {
            let mut visitor = PipelineEventVisitor::default();
            event.record(&mut visitor);
            if let Some(json) = visitor.pipeline_json {
                match serde_json::from_str::<TextPipelineRecord>(&json) {
                    Ok(record) => {
                        let mut header_buf = String::new();
                        let header_writer = Writer::new(&mut header_buf);
                        self.inner.format_event(ctx, header_writer, event)?;

                        if let Some(idx) = header_buf.find(" pipeline_json=") {
                            header_buf.truncate(idx);
                        }

                        writer.write_str(&header_buf)?;
                        writer.write_char('\n')?;
                        writer.write_str(&record.render_pretty(writer.has_ansi_escapes()))?;
                        writer.write_char('\n')?;
                        return Ok(());
                    }
                    Err(e) => {
                        let mut header_buf = String::new();
                        let header_writer = Writer::new(&mut header_buf);
                        self.inner.format_event(ctx, header_writer, event)?;
                        writer.write_str(&header_buf)?;
                        writer.write_char('\n')?;
                        writer.write_str(&format!("│ Failed to render text pipeline: {e}"))?;
                        writer.write_char('\n')?;
                        return Ok(());
                    }
                }
            }
        }

        self.inner.format_event(ctx, writer, event)
    }
}

pub fn record_text_pipeline(record: TextPipelineRecord) {
    if !tracing::level_enabled!(tracing::Level::DEBUG) {
        return;
    }
    if let Ok(json) = serde_json::to_string(&record) {
        tracing::event!(
            target: PIPELINE_TARGET,
            tracing::Level::DEBUG,
            pipeline_json = json.as_str(),
            steps = record.steps.len(),
            applied_steps = record.changed_steps(),
            "text transformation pipeline"
        );
    } else {
        tracing::event!(
            target: PIPELINE_TARGET,
            tracing::Level::DEBUG,
            "text transformation pipeline (serialization failure)"
        );
    }
}
