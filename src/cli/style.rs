use std::fmt::Display;

use console::{StyledObject, style};
use itertools::Itertools;

pub const HELP_STYLES: clap::builder::styling::Styles = clap::builder::styling::Styles::styled()
    .header(clap::builder::styling::AnsiColor::Cyan.on_default().bold())
    .usage(
        clap::builder::styling::AnsiColor::Cyan
            .on_default()
            .bold()
            .underline(),
    )
    .literal(clap::builder::styling::AnsiColor::Green.on_default().bold())
    .placeholder(clap::builder::styling::AnsiColor::Yellow.on_default())
    .error(clap::builder::styling::AnsiColor::Red.on_default().bold())
    .valid(clap::builder::styling::AnsiColor::Green.on_default())
    .invalid(
        clap::builder::styling::AnsiColor::Magenta
            .on_default()
            .bold(),
    );

pub fn success<D>(value: D) -> StyledObject<D> {
    style(value).green().bold()
}

pub fn success_err<D>(value: D) -> StyledObject<D> {
    style(value).for_stderr().green().bold()
}

pub fn status<D>(value: D) -> StyledObject<D> {
    style(value).cyan().bold()
}

pub fn path<D>(value: D) -> StyledObject<D> {
    style(value).cyan()
}

pub fn path_err<D>(value: D) -> StyledObject<D> {
    style(value).for_stderr().cyan()
}

pub fn rule<D>(value: D) -> StyledObject<D> {
    style(value).magenta()
}

pub fn rules<D: Display>(values: &[D]) -> String {
    values.iter().map(rule).join(&label(", ").to_string())
}

pub fn rule_err<D>(value: D) -> StyledObject<D> {
    style(value).for_stderr().magenta()
}

pub fn include<D>(value: D) -> StyledObject<D> {
    style(value).yellow()
}

pub fn include_err<D>(value: D) -> StyledObject<D> {
    style(value).for_stderr().yellow()
}

pub fn label<D>(value: D) -> StyledObject<D> {
    style(value).dim()
}

pub fn label_err<D>(value: D) -> StyledObject<D> {
    style(value).for_stderr().dim()
}

pub fn keep(value: &'static str) -> StyledObject<&'static str> {
    style(value).cyan().bold()
}

pub fn rewrite(value: &'static str) -> StyledObject<&'static str> {
    style(value).green().bold()
}

pub fn warning<D>(value: D) -> StyledObject<D> {
    style(value).for_stderr().yellow().bold()
}

pub fn warning_out<D>(value: D) -> StyledObject<D> {
    style(value).yellow().bold()
}

pub fn error<D>(value: D) -> StyledObject<D> {
    style(value).for_stderr().red().bold()
}

pub fn failure<D>(value: D) -> StyledObject<D> {
    style(value).for_stderr().red().bold()
}

pub fn conflict<D>(value: D) -> StyledObject<D> {
    style(value).for_stderr().magenta().bold()
}

pub fn command<D>(value: D) -> StyledObject<D> {
    style(value).cyan()
}

pub fn command_err<D>(value: D) -> StyledObject<D> {
    style(value).for_stderr().cyan()
}

pub fn line_tag(line: usize) -> StyledObject<String> {
    style(format!("Ln{line:>3} ")).dim()
}

pub fn line_tag_err(line: usize) -> StyledObject<String> {
    style(format!("Ln{line:>3} ")).for_stderr().dim()
}

pub fn warning_line(message: &str) -> String {
    if let Some(rest) = message.strip_prefix("warning:") {
        format!("{}{}", warning("warning:"), rest)
    } else {
        format!("{} {message}", warning("warning:"))
    }
}

pub fn error_line(message: &str) -> String {
    if let Some(rest) = message.strip_prefix("error:") {
        format!("{}{}", error("error:"), rest)
    } else {
        format!("{} {message}", error("error:"))
    }
}
