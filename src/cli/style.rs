use console::{StyledObject, style};

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
    style(value).green().bold()
}

pub fn rewrite(value: &'static str) -> StyledObject<&'static str> {
    style(value).blue().bold()
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
    style(format!("L{line:>4}")).blue().bold()
}

pub fn line_tag_err(line: usize) -> StyledObject<String> {
    style(format!("L{line:>4}")).for_stderr().blue().bold()
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
