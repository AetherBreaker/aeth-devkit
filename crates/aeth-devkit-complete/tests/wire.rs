//! The text format the shims parse.
//!
//! Both shims read this with primitive tools (`read -r` splitting on tabs, `-split "\t"`),
//! so a stray tab or newline in a task's help would silently forge a column or a row. These
//! tests pin that down.

use aeth_devkit_complete::engine::{Directive, Item, ItemKind};
use aeth_devkit_complete::wire::render;

fn item(value: &str, display: &str, tooltip: &str, kind: ItemKind) -> Item {
  Item {
    value: value.to_string(),
    display: display.to_string(),
    tooltip: tooltip.to_string(),
    kind,
  }
}

#[test]
fn renders_items_with_a_header_and_four_columns() {
  let d = Directive::Items(vec![item("build", "build", "Compile", ItemKind::Command)]);
  assert_eq!(render(&d), "items\nbuild\tbuild\tCompile\tcommand\n");
}

#[test]
fn renders_the_sentinels_as_a_bare_header() {
  assert_eq!(render(&Directive::Dirs), "dirs\n");
  assert_eq!(render(&Directive::Files), "files\n");
}

#[test]
fn renders_an_empty_item_list_as_the_header_alone() {
  // Distinct from a sentinel: "items, of which there are none".
  assert_eq!(render(&Directive::Items(Vec::new())), "items\n");
}

#[test]
fn each_kind_has_a_stable_wire_token() {
  let d = Directive::Items(vec![
    item("a", "a", "t", ItemKind::Command),
    item("b", "b", "t", ItemKind::Param),
    item("c", "c", "t", ItemKind::Value),
  ]);
  let out = render(&d);
  assert!(out.contains("a\ta\tt\tcommand\n"));
  assert!(out.contains("b\tb\tt\tparam\n"));
  assert!(out.contains("c\tc\tt\tvalue\n"));
}

#[test]
fn a_tab_or_newline_in_a_field_cannot_break_the_columns() {
  // A task's help text is arbitrary user input from pyproject.toml.
  let d = Directive::Items(vec![item("x", "x", "two\tcols\nand a line", ItemKind::Value)]);
  assert_eq!(render(&d), "items\nx\tx\ttwo cols and a line\tvalue\n");
}

#[test]
fn a_carriage_return_is_neutralised_too() {
  // Windows line endings would otherwise split a record when the shim splits on \r?\n.
  // Each control character becomes one space; runs are not collapsed: preserving the
  // column count is simpler than special-casing a CR/LF pair, and the invariant is the same.
  let d = Directive::Items(vec![item("x", "x", "a\r\nb", ItemKind::Value)]);
  assert_eq!(render(&d), "items\nx\tx\ta  b\tvalue\n");
}

#[test]
fn the_inserted_value_is_sanitised_as_well_as_the_help() {
  // Choices come from pyproject too, so the first column is no more trusted than the third.
  let d = Directive::Items(vec![item("a\tb", "a\tb", "t", ItemKind::Value)]);
  assert_eq!(render(&d), "items\na b\ta b\tt\tvalue\n");
}
