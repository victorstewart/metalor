// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use metalor::parser::{
    interpolate_braced_variables, parse_exec_array, significant_lines, valid_identifier,
};
use std::collections::BTreeMap;

#[test]
fn filters_significant_lines() {
    let lines: Vec<_> = significant_lines(
        r#"
      # comment

      FIRST thing
        SECOND thing
      "#,
    )
    .collect();

    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].number, 4);
    assert_eq!(lines[0].text, "FIRST thing");
    assert_eq!(lines[1].number, 5);
    assert_eq!(lines[1].text, "SECOND thing");
}

#[test]
fn parses_exec_form_json_arrays() {
    let argv = parse_exec_array(r#"["clang", "-c", "main.c"]"#).unwrap();
    assert_eq!(argv, vec!["clang", "-c", "main.c"]);
}

#[test]
fn rejects_non_array_exec_forms() {
    let error = parse_exec_array(r#""clang -c main.c""#).unwrap_err();
    assert!(format!("{error:#}").contains("exec-form arrays must use JSON array syntax"));
}

#[test]
fn rejects_empty_exec_form_arrays() {
    let error = parse_exec_array("[]").unwrap_err();
    assert!(format!("{error:#}").contains("exec-form arrays must not be empty"));
}

#[test]
fn interpolates_braced_variables() {
    let mut variables = BTreeMap::new();
    variables.insert("CHANNEL".to_string(), "stable".to_string());
    let value = interpolate_braced_variables("mode=${CHANNEL}", &variables, "ARG").unwrap();
    assert_eq!(value, "mode=stable");
}

#[test]
fn interpolates_multiple_braced_variables_in_one_string() {
    let mut variables = BTreeMap::new();
    variables.insert("CHANNEL".to_string(), "stable".to_string());
    variables.insert("PROFILE".to_string(), "release".to_string());
    let value = interpolate_braced_variables("${CHANNEL}/${PROFILE}", &variables, "ARG").unwrap();
    assert_eq!(value, "stable/release");
}

#[test]
fn allows_explicit_mappings_for_non_identifier_variable_names() {
    let mut variables = BTreeMap::new();
    variables.insert("bad-name".to_string(), "mapped".to_string());
    let value = interpolate_braced_variables("mode=${bad-name}", &variables, "ARG").unwrap();
    assert_eq!(value, "mode=mapped");
}

#[test]
fn rejects_missing_braced_variables() {
    let error =
        interpolate_braced_variables("mode=${CHANNEL}", &BTreeMap::new(), "ARG").unwrap_err();
    assert!(format!("{error:#}").contains("undefined ARG CHANNEL"));
}

#[test]
fn rejects_unterminated_braced_variables() {
    let mut variables = BTreeMap::new();
    variables.insert("CHANNEL".to_string(), "stable".to_string());
    let error = interpolate_braced_variables("mode=${CHANNEL", &variables, "ARG").unwrap_err();
    assert!(format!("{error:#}").contains("unterminated ${NAME} expansion"));
}

#[test]
fn rejects_invalid_braced_variable_names_without_explicit_mappings() {
    let error =
        interpolate_braced_variables("mode=${bad-name}", &BTreeMap::new(), "ARG").unwrap_err();
    assert!(format!("{error:#}").contains("invalid ARG reference bad-name"));
}

#[test]
fn validates_identifiers() {
    assert!(valid_identifier("CHANNEL_2"));
    assert!(!valid_identifier("2CHANNEL"));
}
