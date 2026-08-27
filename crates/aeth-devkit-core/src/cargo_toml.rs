//! Reading and rewriting the crate/workspace version in `Cargo.toml`.
//!
//! A project that ships a Rust binary alongside its Python package (via maturin) keeps two
//! version numbers: `[project].version` in `pyproject.toml` and either
//! `[workspace.package].version` or `[package].version` in `Cargo.toml`. The release command
//! must keep them equal. The old shell script used `sed 's/^version = …/'`, which silently
//! did nothing on an indented `Cargo.toml` — exactly the layout this repo uses. Parsing with
//! `toml_edit` instead means indentation, comments, and key order are all preserved *and*
//! the edit cannot miss.

// `DocumentMut` is toml_edit's mutable, format-preserving document. `Item` is any node
// (table, value, array…); `Value` is a leaf (string, integer…).
use toml_edit::{DocumentMut, Item, Value};

/// The two places a version can live, most specific first. A `&[&str]` is a slice of string
/// slices — one dotted path each. `const` arrays of references are fine because `&'static
/// str` literals live for the whole program.
const PATHS: [&[&str]; 2] = [&["workspace", "package", "version"], &["package", "version"]];

/// Walk `path` key by key from the document root, returning the node at the end.
///
/// The lifetime `'a` says the returned reference borrows from `doc`, not from `path`; the
/// compiler needs that spelled out because there are two reference inputs.
fn get_path<'a>(doc: &'a DocumentMut, path: &[&str]) -> Option<&'a Item> {
  // `as_item()` views the whole document as a table `Item`.
  let mut item = doc.as_item();
  for key in path {
    // `get` returns `Option<&Item>`; `?` turns `None` into an early `return None`.
    item = item.get(key)?;
  }
  Some(item)
}

/// Same walk, but yielding a mutable reference so the caller can overwrite the leaf.
fn get_path_mut<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> Option<&'a mut Item> {
  let mut item = doc.as_item_mut();
  for key in path {
    item = item.get_mut(key)?;
  }
  Some(item)
}

/// The version string at the first of [`PATHS`] that exists, if any.
pub fn read_version(doc: &DocumentMut) -> Option<String> {
  // `find_map` runs the closure on each path and stops at the first `Some`. Inside,
  // `and_then` chains `Option`s: node → its `&str` (if it is a string) → an owned `String`.
  PATHS
    .iter()
    .find_map(|p| get_path(doc, p).and_then(Item::as_str).map(str::to_string))
}

/// Overwrite the version at the first of [`PATHS`] that exists, keeping the surrounding
/// whitespace/comments. Returns `false` when neither table is present.
pub fn set_version(doc: &mut DocumentMut, version: &str) -> bool {
  for path in PATHS {
    // Probe read-only first. `Item::get_mut` *creates* intermediate tables for keys that do
    // not exist (an empty `workspace = { package = {} }` would appear in a `[package]`-only
    // file), so we must never walk mutably along a path we have not confirmed is there.
    if get_path(doc, path).and_then(Item::as_str).is_none() {
      continue;
    }
    // "if-let chains": both patterns must match for the block to run. Re-walk the (now
    // known-good) path mutably, then insist the leaf is a plain value so we can replace it.
    if let Some(item) = get_path_mut(doc, path)
      && let Some(cur) = item.as_value_mut()
    {
      // Build the new string value, then copy the old "decor" (the whitespace and comments
      // around the value) onto it so the file's formatting survives the edit.
      let mut v = Value::from(version);
      *v.decor_mut() = cur.decor().clone();
      // `*cur = v` writes through the mutable reference, replacing the old value.
      *cur = v;
      return true;
    }
  }
  false
}

#[cfg(test)]
mod tests {
  use super::*;

  const WORKSPACE: &str =
    "[workspace]\n  members = [\"crates/*\"]\n\n[workspace.package]\n  version = \"7.0.0\"\n  edition = \"2024\"\n";
  const PACKAGE: &str = "[package]\nname = \"x\"\nversion = \"1.2.3\"\n\n[dependencies]\nfoo = { version = \"9\" }\n";

  #[test]
  fn reads_workspace_or_package_version() {
    assert_eq!(read_version(&WORKSPACE.parse().unwrap()).as_deref(), Some("7.0.0"));
    assert_eq!(read_version(&PACKAGE.parse().unwrap()).as_deref(), Some("1.2.3"));
    assert_eq!(read_version(&"[dependencies]\n".parse().unwrap()), None);
  }

  #[test]
  fn sets_version_preserving_layout() {
    let mut d: DocumentMut = WORKSPACE.parse().unwrap();
    assert!(set_version(&mut d, "7.0.2"));
    // The only difference must be the version text itself — indentation and all else intact.
    assert_eq!(d.to_string(), WORKSPACE.replace("7.0.0", "7.0.2"));
    let mut p: DocumentMut = PACKAGE.parse().unwrap();
    assert!(set_version(&mut p, "2.0.0"));
    // Note the `[dependencies] foo = { version = "9" }` is untouched: we never match it.
    assert_eq!(p.to_string(), PACKAGE.replace("1.2.3", "2.0.0"));
    let mut none: DocumentMut = "[dependencies]\n".parse().unwrap();
    assert!(!set_version(&mut none, "1"));
  }
}
