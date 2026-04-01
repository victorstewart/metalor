// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignificantLine<'a> {
    pub number: usize,
    pub text: &'a str,
}

pub fn significant_lines(source: &str) -> impl Iterator<Item = SignificantLine<'_>> {
    source.lines().enumerate().filter_map(|(index, raw_line)| {
        let text = raw_line.trim();
        if text.is_empty() || text.starts_with('#') {
            None
        } else {
            Some(SignificantLine {
                number: index + 1,
                text,
            })
        }
    })
}

pub fn valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first == '_' || first.is_ascii_alphabetic() => {}
        _ => return false,
    }

    chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

pub fn parse_exec_array(raw: &str) -> Result<Vec<String>> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('[') {
        bail!("exec-form arrays must use JSON array syntax");
    }
    let argv: Vec<String> = serde_json::from_str(trimmed).context("invalid exec-form array")?;
    if argv.is_empty() {
        bail!("exec-form arrays must not be empty");
    }
    Ok(argv)
}

pub fn interpolate_braced_variables(
    value: &str,
    variables: &BTreeMap<String, String>,
    variable_kind: &str,
) -> Result<String> {
    let mut output = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'$' && index + 1 < bytes.len() && bytes[index + 1] == b'{' {
            let rest = &value[index + 2..];
            let end = rest.find('}').context("unterminated ${NAME} expansion")?;
            let name = &rest[..end];
            if !valid_identifier(name) && !variables.contains_key(name) {
                bail!("invalid {variable_kind} reference {name}");
            }
            let replacement = variables
                .get(name)
                .with_context(|| format!("undefined {variable_kind} {name}"))?;
            output.push_str(replacement);
            index += end + 3;
            continue;
        }

        output.push(bytes[index] as char);
        index += 1;
    }

    Ok(output)
}
