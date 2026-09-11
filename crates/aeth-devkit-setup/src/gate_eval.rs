//! The evaluator seam: one function, Monty behind it. Nothing outside this module names a
//! Monty type, so `rustpython-vm` could replace it by rewriting this file alone (spec 2.5).

use anyhow::{Result, anyhow, bail};
use monty::{MontyRun, RunProgress};
use monty_types::{CompileOptions, ExtFunctionResult, MontyObject, PrintWriter, ResourceTracker};

/// A `pyproject.toml` value as a gate sees it. The seam's own type: `gate` builds it from
/// `toml_edit`, this module turns it into whatever the evaluator wants.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
  None,
  Bool(bool),
  Int(i64),
  Float(f64),
  Str(String),
  List(Vec<Value>),
  Dict(Vec<(String, Value)>),
}

/// What an expression can ask about: bare boolean names, and the two functions.
pub struct World<'a> {
  pub flags: &'a [(&'static str, bool)],
  pub keys: &'a dyn Fn(&str) -> Value,
  pub dep: &'a dyn Fn(&str) -> bool,
}

/// The truth value of `expr`, a Python expression, in `world`. `keys` and `dep` are host
/// functions: the sandbox suspends on the call, the closure answers, the run resumes.
/// Any failure (syntax, an unknown name, a bad argument, a runtime exception) is an error
/// naming the expression: an unknown name is never silently false (spec 2.4).
pub fn evaluate(expr: &str, world: &World) -> Result<bool> {
  let code = format!("bool({expr})");
  let mut names: Vec<String> = world.flags.iter().map(|(n, _)| (*n).to_string()).collect();
  let mut values: Vec<MontyObject> = world.flags.iter().map(|(_, v)| MontyObject::Bool(*v)).collect();
  for f in ["keys", "dep"] {
    names.push(f.to_string());
    values.push(MontyObject::Function {
      name: f.to_string(),
      docstring: None,
    });
  }
  let fail = |e: &dyn std::fmt::Display| anyhow!("gate `{expr}`: {e}");
  let run = MontyRun::new(code, "gate", names, CompileOptions::default()).map_err(|e| fail(&e))?;
  let mut progress = run
    .start(values, ResourceTracker::default(), PrintWriter::Disabled)
    .map_err(|e| fail(&e))?;
  loop {
    progress = match progress {
      RunProgress::Complete(MontyObject::Bool(b)) => return Ok(b),
      RunProgress::Complete(other) => bail!("gate `{expr}`: bool() returned {other:?}"),
      RunProgress::FunctionCall(call) => {
        let arg = match call.args.as_slice() {
          [MontyObject::String(s)] if call.kwargs.is_empty() => s.clone(),
          _ => bail!("gate `{expr}`: {}() takes one string argument", call.function_name),
        };
        let result = match call.function_name.as_str() {
          "keys" => to_monty(&(world.keys)(&arg)),
          "dep" => MontyObject::Bool((world.dep)(&arg)),
          other => bail!("gate `{expr}`: unknown function {other}"),
        };
        call
          .resume(ExtFunctionResult::Return(result), PrintWriter::Disabled)
          .map_err(|e| fail(&e))?
      }
      RunProgress::NameLookup(lookup) => bail!("gate `{expr}`: unknown name `{}`", lookup.name),
      RunProgress::OsCall(_) | RunProgress::ResolveFutures(_) => bail!("gate `{expr}`: gates cannot do I/O or async"),
    };
  }
}

fn to_monty(v: &Value) -> MontyObject {
  match v {
    Value::None => MontyObject::None,
    Value::Bool(b) => MontyObject::Bool(*b),
    Value::Int(i) => MontyObject::Int(*i),
    Value::Float(f) => MontyObject::Float(*f),
    Value::Str(s) => MontyObject::String(s.clone()),
    Value::List(items) => MontyObject::List(items.iter().map(to_monty).collect()),
    Value::Dict(pairs) => MontyObject::dict(
      pairs
        .iter()
        .map(|(k, v)| (MontyObject::String(k.clone()), to_monty(v)))
        .collect::<Vec<_>>(),
    ),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn world<'a>(flags: &'a [(&'static str, bool)], keys: &'a dyn Fn(&str) -> Value, dep: &'a dyn Fn(&str) -> bool) -> World<'a> {
    World { flags, keys, dep }
  }

  fn keys(path: &str) -> Value {
    match path {
      "tool.docker.wireguard" => Value::Bool(true),
      "tool.docker.services" => Value::List(vec![Value::Str("smoke".into())]),
      "project.name" => Value::Str("demo-app".into()),
      "tool.ruff.lint.per-file-ignores" => Value::Dict(vec![("tests/**".into(), Value::List(vec![Value::Str("D1".into())]))]),
      "project.version" => Value::Float(1.5),
      "tool.pytest.count" => Value::Int(3),
      _ => Value::None,
    }
  }

  fn dep(name: &str) -> bool {
    name == "aeth-ext"
  }

  #[test]
  fn python_semantics_over_the_three_kinds_of_name() {
    let flags = [("rust", true), ("publish_index", false)];
    let w = world(&flags, &keys, &dep);
    for (expr, want) in [
      ("keys(\"tool.docker.wireguard\")", true),
      ("keys(\"tool.docker.missing\")", false),
      ("not keys(\"tool.docker.missing\")", true),
      ("\"smoke\" in (keys(\"tool.docker.services\") or ())", true),
      ("\"other\" in (keys(\"tool.docker.services\") or ())", false),
      ("keys(\"project.name\") != \"aeth-ext\" and dep(\"aeth-ext\")", true),
      ("dep(\"mypy\")", false),
      ("rust and not publish_index", true),
      ("keys(\"tool.pytest.count\") > 2", true),
      ("keys(\"project.version\") == 1.5", true),
      ("\"tests/**\" in keys(\"tool.ruff.lint.per-file-ignores\")", true),
      ("keys(\"project.name\").startswith(\"demo\")", true),
      ("any(s.startswith(\"sm\") for s in keys(\"tool.docker.services\"))", true),
      ("keys(\"tool.\" + \"docker.wireguard\")", true),
    ] {
      assert_eq!(evaluate(expr, &w).unwrap(), want, "{expr}");
    }
  }

  #[test]
  fn errors_name_the_expression_and_the_cause() {
    let flags = [("rust", true)];
    let w = world(&flags, &keys, &dep);
    let err = evaluate("nope", &w).unwrap_err().to_string();
    assert!(err.contains("nope") && err.contains("unknown name"), "{err}");
    let err = evaluate("keys(1)", &w).unwrap_err().to_string();
    assert!(err.contains("one string argument"), "{err}");
    let err = evaluate("rust and", &w).unwrap_err().to_string();
    assert!(err.contains("rust and"), "{err}");
    let err = evaluate("keys(\"x\").bogus()", &w).unwrap_err().to_string();
    assert!(err.contains("bogus"), "{err}");
  }
}
