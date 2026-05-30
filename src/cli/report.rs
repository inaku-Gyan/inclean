use console::StyledObject;

use crate::pipeline::run::{DiffAspect, SkippedFile, Summary, UnfixableKind};

use super::style as cli_style;

pub fn print_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("{}", cli_style::warning_line(warning));
    }
}

pub fn print_skipped_parse_failures(skipped: &[SkippedFile]) {
    if skipped.is_empty() {
        return;
    }
    eprintln!(
        "{} skipped {} file(s) that could not be parsed:",
        cli_style::warning("warning:"),
        skipped.len()
    );
    for skipped_file in skipped {
        eprintln!(
            "  - {}: {}",
            cli_style::path_err(skipped_file.relpath.display()),
            skipped_file.reason
        );
    }
}

pub fn render_unfixable_report(summary: &Summary) -> String {
    if summary.unfixable.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str(&format!(
        "{} {}\n",
        cli_style::error(summary.unfixable.len()),
        cli_style::error("unfixable violation(s):")
    ));
    for unfixable in &summary.unfixable {
        out.push_str(&format!(
            "  {}:{}: {}\n",
            cli_style::path_err(unfixable.file_relpath.display()),
            cli_style::error(unfixable.line),
            unfixable_kind(unfixable.kind)
        ));
        out.push_str(&format!(
            "    {} {}\n",
            cli_style::label_err("original:"),
            cli_style::include_err(&unfixable.original_line)
        ));
        if let Some(message) = &unfixable.message {
            out.push_str(&format!(
                "    {} {message}\n",
                cli_style::label_err("message:")
            ));
        }
        for (rule, final_text) in &unfixable.rules {
            match final_text {
                Some(text) => {
                    out.push_str(&format!(
                        "    {} `{}`: #include {}\n",
                        cli_style::label_err("rule"),
                        cli_style::rule_err(rule),
                        cli_style::include_err(text)
                    ));
                }
                None => {
                    out.push_str(&format!(
                        "    {} `{}`\n",
                        cli_style::label_err("rule"),
                        cli_style::rule_err(rule)
                    ));
                }
            }
        }
        if !unfixable.differing_aspects.is_empty() {
            let parts = unfixable
                .differing_aspects
                .iter()
                .map(diff_aspect)
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "    {} {}\n",
                cli_style::label_err("differs in:"),
                cli_style::warning(parts)
            ));
        }
    }
    out
}

fn unfixable_kind(kind: UnfixableKind) -> StyledObject<&'static str> {
    match kind {
        UnfixableKind::Error => cli_style::error("error"),
        UnfixableKind::EvaluationFailure => cli_style::failure("evaluation_failure"),
        UnfixableKind::Conflict => cli_style::conflict("conflict"),
        UnfixableKind::TrailingCommentError => cli_style::error("trailing_comment_error"),
    }
}

fn diff_aspect(aspect: &DiffAspect) -> &'static str {
    match aspect {
        DiffAspect::IncludePath => "include path",
        DiffAspect::OutputForm => "output_form",
        DiffAspect::TrailingComment => "trailing_comment",
    }
}
